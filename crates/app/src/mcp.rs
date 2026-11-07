//! MCP (Model Context Protocol) server so external agents — Claude Desktop, Claude Code,
//! Cursor, VS Code — can model in the running app.
//!
//! * Streamable-HTTP endpoint: `POST http://127.0.0.1:<port>/mcp` (JSON responses), localhost only.
//! * stdio shim for clients that launch a process: `rust-fusion --mcp-stdio` forwards JSON-RPC
//!   lines to the running app (starting it if needed).
//!
//! Protocol messages are answered on the server thread; tool calls are forwarded to the UI
//! thread (which owns the document) and answered through a one-shot channel.

use crate::app::App;
use cad_model::commands::{self, Command};
use crossbeam_channel::{bounded, unbounded, Receiver, Sender};
use eframe::egui::{self, RichText};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::sync::Arc;
use std::time::Duration;

const PROTOCOL_VERSION: &str = "2025-06-18";

pub struct ToolCall {
    pub name: String,
    pub args: Value,
    pub reply: Sender<ToolResult>,
}

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub text: String,
    pub is_error: bool,
    pub image_png: Option<Vec<u8>>,
}

impl ToolResult {
    pub fn ok(t: impl Into<String>) -> Self {
        ToolResult { text: t.into(), is_error: false, image_png: None }
    }
    pub fn err(t: impl Into<String>) -> Self {
        ToolResult { text: t.into(), is_error: true, image_png: None }
    }
}

pub struct McpServer {
    rx: Receiver<ToolCall>,
    pub port: u16,
    server: Arc<tiny_http::Server>,
}

impl Drop for McpServer {
    fn drop(&mut self) {
        self.server.unblock();
    }
}

pub fn tools() -> Value {
    json!([
        {"name": "get_design", "description": "Describe the current design: parameters, timeline features (with errors), bodies with bounding boxes and volumes. Units mm, Y up.",
         "inputSchema": {"type": "object", "properties": {"include_json": {"type": "boolean", "description": "Also return the raw document JSON"}}}},
        {"name": "get_command_reference", "description": "The full list of editing commands accepted by run_commands, with coordinate conventions.",
         "inputSchema": {"type": "object", "properties": {}}},
        {"name": "run_commands", "description": "Apply a batch of editing commands (one undo step). Call get_command_reference first. Returns a log and any errors.",
         "inputSchema": {"type": "object", "properties": {"commands": {"type": "array", "items": {"type": "object", "properties": {"action": {"type": "string"}}, "required": ["action"]}}}, "required": ["commands"]}},
        {"name": "set_parameter", "description": "Create or change a user parameter (value is an expression such as \"80 mm\" or \"width/2\").",
         "inputSchema": {"type": "object", "properties": {"name": {"type": "string"}, "value": {"type": "string"}}, "required": ["name", "value"]}},
        {"name": "ask_copilot", "description": "Hand a plain-English request to the app's own AI copilot (runs asynchronously; check get_design afterwards).",
         "inputSchema": {"type": "object", "properties": {"request": {"type": "string"}}, "required": ["request"]}},
        {"name": "screenshot", "description": "PNG of the 3D viewport. Optionally set a standard view first.",
         "inputSchema": {"type": "object", "properties": {"view": {"type": "string", "enum": ["home", "top", "front", "right", "left", "back", "bottom"]}}}},
        {"name": "export", "description": "Export visible bodies to a file.",
         "inputSchema": {"type": "object", "properties": {"format": {"type": "string", "enum": ["stl", "gltf", "step"]}, "path": {"type": "string", "description": "Absolute output path"}}, "required": ["format", "path"]}},
        {"name": "undo", "description": "Undo the last change.", "inputSchema": {"type": "object", "properties": {}}},
        {"name": "image_to_model", "description": "Build a solid from an image file that shows several orthographic views (front/top/side…) of one object: views are detected, silhouettes traced and intersected. Replaces the current design. Give at least one overall size if known.",
         "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}, "width_mm": {"type": "number"}, "depth_mm": {"type": "number"}, "height_mm": {"type": "number"}}, "required": ["path"]}}
    ])
}

fn rpc_result(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn rpc_error(id: &Value, code: i64, msg: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": msg}})
}

/// Handle one JSON-RPC message; `None` for notifications.
pub fn handle_message(msg: &Value, calls: &Sender<ToolCall>, wake: &dyn Fn()) -> Option<Value> {
    let id = msg.get("id")?.clone();
    let method = msg["method"].as_str().unwrap_or("");
    Some(match method {
        "initialize" => rpc_result(
            &id,
            json!({
                "protocolVersion": msg["params"]["protocolVersion"].as_str().unwrap_or(PROTOCOL_VERSION),
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": "rust-fusion", "version": env!("CARGO_PKG_VERSION")},
                "instructions": "Rust Fusion is a parametric CAD app. Call get_command_reference, then get_design, then run_commands. Units are mm, Y is up."
            }),
        ),
        "ping" => rpc_result(&id, json!({})),
        "tools/list" => rpc_result(&id, json!({"tools": tools()})),
        "tools/call" => {
            let name = msg["params"]["name"].as_str().unwrap_or("").to_string();
            let args = msg["params"]["arguments"].clone();
            let (tx, rx) = bounded(1);
            if calls.send(ToolCall { name, args, reply: tx }).is_err() {
                return Some(rpc_error(&id, -32603, "app is shutting down"));
            }
            wake();
            let r = rx.recv_timeout(Duration::from_secs(180)).unwrap_or_else(|_| ToolResult::err("timed out waiting for the app"));
            let mut content = vec![json!({"type": "text", "text": r.text})];
            if let Some(png) = r.image_png {
                use base64::Engine;
                content.push(json!({"type": "image", "mimeType": "image/png", "data": base64::engine::general_purpose::STANDARD.encode(png)}));
            }
            rpc_result(&id, json!({"content": content, "isError": r.is_error}))
        }
        _ => rpc_error(&id, -32601, &format!("method not found: {method}")),
    })
}

fn origin_ok(origin: Option<&str>) -> bool {
    match origin {
        None => true,
        Some(o) => {
            let host = o.split("://").nth(1).unwrap_or(o);
            let host = host.split([':', '/']).next().unwrap_or("");
            matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "null")
        }
    }
}

impl McpServer {
    pub fn start(port: u16, wake: impl Fn() + Send + 'static) -> Result<McpServer, String> {
        let server = Arc::new(
            tiny_http::Server::http(("127.0.0.1", port)).map_err(|e| format!("can't listen on 127.0.0.1:{port}: {e}"))?,
        );
        let port = server.server_addr().to_ip().map(|a| a.port()).unwrap_or(port);
        let (tx, rx) = unbounded::<ToolCall>();
        let srv = server.clone();
        std::thread::Builder::new()
            .name("mcp".into())
            .spawn(move || {
                for mut req in srv.incoming_requests() {
                    let origin = req.headers().iter().find(|h| h.field.equiv("Origin")).map(|h| h.value.to_string());
                    let json_header = tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap();
                    if !origin_ok(origin.as_deref()) {
                        let _ = req.respond(tiny_http::Response::from_string("forbidden origin").with_status_code(403));
                        continue;
                    }
                    if *req.method() != tiny_http::Method::Post {
                        let _ = req.respond(tiny_http::Response::from_string("POST JSON-RPC to /mcp").with_status_code(405));
                        continue;
                    }
                    let mut body = String::new();
                    let _ = req.as_reader().read_to_string(&mut body);
                    let reply = match serde_json::from_str::<Value>(&body) {
                        Err(e) => Some(rpc_error(&Value::Null, -32700, &format!("parse error: {e}"))),
                        Ok(Value::Array(batch)) => {
                            let out: Vec<Value> = batch.iter().filter_map(|m| handle_message(m, &tx, &wake)).collect();
                            (!out.is_empty()).then(|| Value::Array(out))
                        }
                        Ok(m) => handle_message(&m, &tx, &wake),
                    };
                    let _ = match reply {
                        Some(v) => req.respond(tiny_http::Response::from_string(v.to_string()).with_header(json_header)),
                        None => req.respond(tiny_http::Response::empty(202)),
                    };
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(McpServer { rx, port, server })
    }
}

/// (Re)start the server according to settings.
pub fn restart(app: &mut App, ctx: &egui::Context) {
    app.mcp = None;
    if !app.settings.mcp_enabled {
        app.mcp_status = "MCP server off".into();
        return;
    }
    let c = ctx.clone();
    match McpServer::start(app.settings.mcp_port, move || c.request_repaint()) {
        Ok(s) => {
            app.mcp_status = format!("MCP listening on http://127.0.0.1:{}/mcp", s.port);
            app.mcp = Some(s);
        }
        Err(e) => app.mcp_status = format!("MCP: {e}"),
    }
}

/// Serve pending tool calls on the UI thread.
pub fn poll(app: &mut App) {
    let calls: Vec<ToolCall> = match &app.mcp {
        Some(s) => s.rx.try_iter().collect(),
        None => return,
    };
    for c in calls {
        app.copilot.push(crate::copilot::Role::Info, format!("MCP ▸ {}", c.name));
        if c.name == "screenshot" {
            if let Some(v) = c.args["view"].as_str() {
                let _ = app.app_action(cad_model::commands::AppAction::View(v.to_string()));
            }
            crate::shots::request(app, crate::shots::ShotTarget::Mcp(c.reply));
            continue;
        }
        let r = call_tool(app, &c.name, &c.args);
        let _ = c.reply.send(r);
    }
}

fn call_tool(app: &mut App, name: &str, args: &Value) -> ToolResult {
    match name {
        "get_design" => {
            let mut t = commands::describe(&app.doc, &app.model);
            if args["include_json"].as_bool() == Some(true) {
                t += "\nDocument JSON:\n";
                t += &app.doc.to_json();
            }
            ToolResult::ok(t)
        }
        "get_command_reference" => ToolResult::ok(commands::COMMAND_REFERENCE),
        "run_commands" => {
            let list = args["commands"].as_array().cloned().unwrap_or_default();
            if list.is_empty() {
                return ToolResult::err("no commands given");
            }
            let mut cmds = Vec::new();
            let mut errors = Vec::new();
            for c in list {
                match serde_json::from_value::<Command>(c.clone()) {
                    Ok(x) => cmds.push(x),
                    Err(e) => errors.push(format!("{}: {e}", c["action"].as_str().unwrap_or("?"))),
                }
            }
            let rep = app.run_commands(&cmds, "MCP");
            errors.extend(rep.errors);
            for l in &rep.lines {
                app.copilot.push(crate::copilot::Role::Action, format!("✔ {l}"));
            }
            let mut t = rep.lines.iter().map(|l| format!("ok: {l}")).collect::<Vec<_>>().join("\n");
            for e in &errors {
                t += &format!("\nerror: {e}");
            }
            t += &format!("\n\n{}", commands::describe(&app.doc, &app.model));
            ToolResult { text: t, is_error: !errors.is_empty() && rep.lines.is_empty(), image_png: None }
        }
        "set_parameter" => {
            let c = Command::SetParameter {
                name: args["name"].as_str().unwrap_or("").to_string(),
                value: commands::Ex(match &args["value"] {
                    Value::String(s) => s.clone(),
                    v => v.to_string(),
                }),
                comment: None,
            };
            let rep = app.run_commands(&[c], "MCP");
            if rep.errors.is_empty() {
                ToolResult::ok(rep.lines.join("\n"))
            } else {
                ToolResult::err(rep.errors.join("\n"))
            }
        }
        "ask_copilot" => {
            let req = args["request"].as_str().unwrap_or("").to_string();
            if req.is_empty() {
                return ToolResult::err("empty request");
            }
            crate::copilot::submit(app, &req, false);
            ToolResult::ok("Submitted to the Rust Fusion copilot. Call get_design in a few seconds to see the result.")
        }
        "export" => {
            let path = args["path"].as_str().map(str::to_string);
            if path.is_none() {
                return ToolResult::err("path is required");
            }
            let r = match args["format"].as_str().unwrap_or("stl") {
                "stl" => app.export_stl_to(path),
                "gltf" | "glb" => app.export_gltf_to(path),
                "step" | "stp" => app.export_step_to(path),
                f => Err(format!("unknown format {f}")),
            };
            match r {
                Ok(m) => ToolResult::ok(m),
                Err(e) => ToolResult::err(e),
            }
        }
        "image_to_model" => {
            let n = |k: &str| args[k].as_f64().unwrap_or(0.0);
            let known = crate::views::Dims { width: n("width_mm"), depth: n("depth_mm"), height: n("height_mm") };
            match crate::image_model::image_to_model(app, args["path"].as_str().unwrap_or(""), known) {
                Ok(m) => ToolResult::ok(format!("{m}\n\n{}", commands::describe(&app.doc, &app.model))),
                Err(e) => ToolResult::err(e),
            }
        }
        "undo" => match app.app_action(cad_model::commands::AppAction::Undo) {
            Ok(m) => ToolResult::ok(m),
            Err(e) => ToolResult::err(e),
        },
        _ => ToolResult::err(format!("unknown tool {name}")),
    }
}

pub fn client_config(port: u16) -> String {
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "rust-fusion".into());
    serde_json::to_string_pretty(&json!({"mcpServers": {"rust-fusion": {"command": exe, "args": ["--mcp-stdio", "--port", port.to_string()]}}}))
        .unwrap()
}

pub fn settings_ui(app: &mut App, ui: &mut egui::Ui) {
    let mut restart_now = false;
    restart_now |= ui.checkbox(&mut app.settings.mcp_enabled, "Enable MCP server").changed();
    ui.horizontal(|ui| {
        ui.label("Port");
        restart_now |= ui.add(egui::DragValue::new(&mut app.settings.mcp_port).range(1024..=65535)).lost_focus();
    });
    ui.label(RichText::new(&app.mcp_status).small());
    ui.label(RichText::new("Claude Desktop / Claude Code config:").small());
    let cfg = client_config(app.settings.mcp_port);
    ui.add(egui::TextEdit::multiline(&mut cfg.as_str()).font(egui::TextStyle::Monospace).desired_rows(5));
    if ui.small_button("Copy config").clicked() {
        ui.ctx().copy_text(cfg);
    }
    if restart_now {
        app.settings.save();
        let ctx = ui.ctx().clone();
        restart(app, &ctx);
    }
}

/// `rust-fusion --mcp-stdio [--port N]`: bridge stdin/stdout JSON-RPC to the running app.
pub fn run_stdio_bridge(port: u16) {
    let url = format!("http://127.0.0.1:{port}/mcp");
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(200)))
        .build()
        .into();
    let mut launched = false;
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let mut answer: Option<String> = None;
        for attempt in 0..60 {
            match agent.post(&url).header("Content-Type", "application/json").send(line.as_bytes()) {
                Ok(mut r) => {
                    let s = r.body_mut().read_to_string().unwrap_or_default();
                    answer = Some(s);
                    break;
                }
                Err(_) => {
                    if !launched {
                        launched = true;
                        if let Ok(exe) = std::env::current_exe() {
                            let _ = std::process::Command::new(exe).spawn();
                        }
                    }
                    if attempt == 59 {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(500));
                }
            }
        }
        let out = match answer {
            Some(s) if !s.trim().is_empty() => s,
            Some(_) => continue, // notification accepted
            None => {
                let id = serde_json::from_str::<Value>(&line).ok().and_then(|v| v.get("id").cloned());
                match id {
                    Some(id) => rpc_error(&id, -32603, "Rust Fusion is not running and could not be started").to_string(),
                    None => continue,
                }
            }
        };
        let _ = writeln!(stdout, "{}", out.replace('\n', " "));
        let _ = stdout.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_basics() {
        let (tx, _rx) = unbounded();
        let init = handle_message(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}), &tx, &|| {}).unwrap();
        assert_eq!(init["result"]["serverInfo"]["name"], "rust-fusion");
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        let list = handle_message(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}), &tx, &|| {}).unwrap();
        assert!(list["result"]["tools"].as_array().unwrap().len() >= 9);
        assert!(handle_message(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}), &tx, &|| {}).is_none());
        assert!(origin_ok(Some("http://localhost:3000")) && !origin_ok(Some("https://evil.example")));
    }

    #[test]
    fn http_tool_call_reaches_ui_thread() {
        let srv = McpServer::start(0, || {}).unwrap();
        let port = srv.port;
        let rx = srv.rx.clone();
        std::thread::spawn(move || {
            let c = rx.recv().unwrap();
            assert_eq!(c.name, "get_design");
            c.reply.send(ToolResult::ok("design!")).unwrap();
        });
        let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
        let mut r = agent
            .post(&format!("http://127.0.0.1:{port}/mcp"))
            .send_json(&json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"get_design","arguments":{}}}))
            .unwrap();
        let v: Value = r.body_mut().read_json().unwrap();
        assert_eq!(v["result"]["content"][0]["text"], "design!");
    }
}
