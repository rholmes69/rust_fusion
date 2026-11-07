//! AI Copilot: plain-English (typed or spoken) modelling through Ollama or OpenRouter.
//!
//! Requests go through an instant parser first ("undo", "top view", "set width to 80").
//! Everything else is sent to the selected model with the command reference and a
//! description of the design; the model answers with JSON commands that are validated
//! and applied as one undo step. Failed commands are sent back for repair.

use crate::app::App;
use crate::llm::{self, LlmConfig, Msg, RemoteModel};
use cad_model::commands::{self, parse_ai_answer, Command, Ex};
use cad_model::Document;
use crossbeam_channel::{unbounded, Receiver, Sender};
use eframe::egui::{self, Color32, RichText};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    Action,
    Error,
    Info,
}

#[derive(Debug, Clone)]
pub struct ChatLine {
    pub role: Role,
    pub text: String,
}

pub enum AiEvent {
    Answer { id: u64, text: String },
    Failed { id: u64, err: String },
    OllamaModels(Result<Vec<String>, String>),
    RemoteModels(Result<Vec<RemoteModel>, String>),
    KeyCheck(Result<String, String>),
    /// Generic result for other modules (image-to-model) keyed by id.
    Custom { id: u64, tag: &'static str, result: Result<String, String> },
}

pub struct Copilot {
    pub log: Vec<ChatLine>,
    pub input: String,
    pub busy: Option<u64>,
    next_id: u64,
    pub tx: Sender<AiEvent>,
    rx: Receiver<AiEvent>,
    pub ctx: Option<egui::Context>,
    pub ollama_models: Vec<String>,
    pub ollama_status: String,
    pub remote_models: Vec<RemoteModel>,
    pub remote_loading: bool,
    pub remote_filter: String,
    pub show_settings: bool,
    pub show_browser: bool,
    pub key_status: String,
    /// (user request, assistant reply) pairs kept as conversation context.
    history: Vec<(String, String)>,
    /// Messages of the request in flight (for repair rounds).
    pending_msgs: Vec<Msg>,
    pending_source: String,
    repair_round: u32,
    /// Replies waiting to be spoken by the voice module.
    pub to_speak: Vec<String>,
    pub focus_input: bool,
}

impl Copilot {
    pub fn new() -> Copilot {
        let (tx, rx) = unbounded();
        Copilot {
            log: vec![ChatLine {
                role: Role::Info,
                text: "Describe what to build or change, e.g. \"an 80×50 plate 8 mm thick with two 12 mm holes\", \"make it 10 mm thicker\", \"top view\".".into(),
            }],
            input: String::new(),
            busy: None,
            next_id: 1,
            tx,
            rx,
            ctx: None,
            ollama_models: Vec::new(),
            ollama_status: String::new(),
            remote_models: Vec::new(),
            remote_loading: false,
            remote_filter: String::new(),
            show_settings: false,
            show_browser: false,
            key_status: String::new(),
            history: Vec::new(),
            pending_msgs: Vec::new(),
            pending_source: String::new(),
            repair_round: 0,
            to_speak: Vec::new(),
            focus_input: false,
        }
    }

    pub fn alloc_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    pub fn push(&mut self, role: Role, text: impl Into<String>) {
        self.log.push(ChatLine { role, text: text.into() });
        if self.log.len() > 400 {
            self.log.drain(0..100);
        }
    }

    pub fn spawn(&self, f: impl FnOnce() -> AiEvent + Send + 'static) {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let ev = f();
            let _ = tx.send(ev);
            if let Some(c) = ctx {
                c.request_repaint();
            }
        });
    }

    pub fn refresh_ollama(&mut self, host: String) {
        self.ollama_status = "checking Ollama…".into();
        self.spawn(move || AiEvent::OllamaModels(llm::ollama_models(&host)));
    }
}

pub fn llm_config(app: &App) -> LlmConfig {
    LlmConfig {
        ollama_host: app.settings.ollama_host.clone(),
        openrouter_key: app.settings.openrouter_key(),
        temperature: app.settings.temperature,
    }
}

// ---------------------------------------------------------------------- prompt

const SYSTEM: &str = "You are the modelling copilot inside Rust Fusion, a parametric 3D CAD app (millimetres, degrees, Y up). \
Turn the user's request into editing commands. Answer ONLY with a JSON object: {\"reply\": \"one short sentence about what you did\", \"commands\": [ ... ]}. \
Rules: use only the actions listed below; create named parameters for the main dimensions of new parts so they stay editable; \
draw profiles centred on the origin unless told otherwise; to make holes, draw circles inside the outer profile in the same sketch \
(or cut later with operation \"cut\"); never invent feature names — use names from the design description; \
for a question that needs no change, return an empty command list and answer in reply.";

fn system_prompt() -> String {
    format!("{SYSTEM}\n\n{}\n\nExample — user: \"a 60 x 40 x 10 block with a 15 mm hole through the middle\" → \
{{\"reply\":\"Added a 60×40×10 block with a Ø15 through hole.\",\"commands\":[\
{{\"action\":\"set_parameter\",\"name\":\"length\",\"value\":\"60 mm\"}},{{\"action\":\"set_parameter\",\"name\":\"width\",\"value\":\"40 mm\"}},\
{{\"action\":\"set_parameter\",\"name\":\"thickness\",\"value\":\"10 mm\"}},{{\"action\":\"new_sketch\",\"plane\":\"XZ\"}},\
{{\"action\":\"add_center_rectangle\",\"width\":\"length\",\"height\":\"width\"}},{{\"action\":\"add_circle\",\"radius\":7.5}},\
{{\"action\":\"extrude\",\"distance\":\"thickness\"}}]}}", commands::COMMAND_REFERENCE)
}

fn user_message(doc: &Document, model: &cad_model::Model, request: &str, from_voice: bool) -> String {
    format!(
        "Current design:\n{}\n{}Request: {}",
        commands::describe(doc, model),
        if from_voice { "(The request was spoken; it may contain speech-recognition mistakes.)\n" } else { "" },
        request
    )
}

// ---------------------------------------------------------------------- instant parser

fn word_number(w: &str) -> Option<f64> {
    const SMALL: [&str; 20] = [
        "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven", "twelve",
        "thirteen", "fourteen", "fifteen", "sixteen", "seventeen", "eighteen", "nineteen",
    ];
    const TENS: [&str; 8] = ["twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety"];
    if let Ok(v) = w.parse::<f64>() {
        return Some(v);
    }
    if let Some(i) = SMALL.iter().position(|s| *s == w) {
        return Some(i as f64);
    }
    if let Some(i) = TENS.iter().position(|s| *s == w) {
        return Some((i as f64 + 2.0) * 10.0);
    }
    if let Some((a, b)) = w.split_once('-') {
        return Some(word_number(a)? + word_number(b)?);
    }
    None
}

/// Turn spoken number words into digits: "eighty five millimeters" → "85 mm".
pub fn normalize_numbers(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut acc: Option<f64> = None;
    for w in text.split_whitespace() {
        let lw = w.to_ascii_lowercase();
        let unit = match lw.trim_end_matches([',', '.']) {
            "millimeters" | "millimetres" | "millimeter" | "millimetre" => Some("mm"),
            "centimeters" | "centimetres" | "centimeter" | "centimetre" => Some("cm"),
            "inches" | "inch" => Some("in"),
            "degrees" | "degree" => Some("deg"),
            "meters" | "metres" | "meter" | "metre" => Some("m"),
            _ => None,
        };
        match (word_number(lw.trim_end_matches([',', '.'])), lw.as_str()) {
            (Some(v), _) if !lw.chars().all(|c| c.is_ascii_digit() || c == '.') || acc.is_some() => {
                acc = Some(match acc {
                    Some(a) if v < 10.0 && a >= 20.0 && a % 10.0 == 0.0 => a + v,
                    Some(a) => {
                        out.push(fmt(a));
                        v
                    }
                    None => v,
                });
            }
            (_, "hundred") if acc.is_some() => acc = acc.map(|a| a * 100.0),
            (_, "point") if acc.is_some() => out.push(format!("{}.", fmt(acc.take().unwrap()))),
            _ => {
                if let Some(a) = acc.take() {
                    out.push(fmt(a));
                }
                out.push(unit.map(str::to_string).unwrap_or_else(|| w.to_string()));
            }
        }
    }
    if let Some(a) = acc {
        out.push(fmt(a));
    }
    out.join(" ").replace(". ", ".")
}

fn fmt(v: f64) -> String {
    crate::app::fmt_num(v)
}

/// Commands that don't need a language model.
pub fn instant(text: &str, doc: &Document) -> Option<Vec<Command>> {
    let t = normalize_numbers(text).to_ascii_lowercase();
    let t = t.trim().trim_end_matches(['.', '!', '?']).trim();
    let t = t.strip_prefix("please ").unwrap_or(t);
    let t = t.strip_prefix("no ").unwrap_or(t).trim();
    let one = |c: Command| Some(vec![c]);
    match t {
        "undo" | "undo that" | "undo it" | "go back" => return one(Command::Undo),
        "redo" | "redo that" => return one(Command::Redo),
        "fit" | "fit all" | "zoom to fit" | "zoom extents" | "fit view" | "zoom fit" | "show all" => return one(Command::FitAll),
        "export stl" | "export to stl" | "export an stl" => return one(Command::ExportStl { path: None }),
        "export gltf" | "export to gltf" | "export glb" => return one(Command::ExportGltf { path: None }),
        "export step" | "export to step" => return one(Command::ExportStep { path: None }),
        "save" | "save it" | "save the design" => return one(Command::Save { path: None }),
        _ => {}
    }
    for v in ["top", "front", "right", "left", "back", "bottom", "home", "iso", "isometric"] {
        let forms = [format!("{v} view"), format!("view {v}"), format!("show {v} view"), format!("go to {v} view"), format!("{v}")];
        if forms.iter().any(|f| f == t) && (t != v || ["top", "front", "home", "isometric"].contains(&v)) {
            return one(Command::View { view: v.into() });
        }
    }
    let find_feature = |name: &str| doc.features.iter().find(|f| f.name.to_ascii_lowercase() == name.trim()).map(|f| f.name.clone());
    for verb in ["delete ", "remove "] {
        if let Some(rest) = t.strip_prefix(verb) {
            if let Some(n) = find_feature(rest.trim_start_matches("the ")) {
                return one(Command::DeleteFeature { feature: n });
            }
        }
    }
    if let Some(rest) = t.strip_prefix("unsuppress ") {
        if let Some(n) = find_feature(rest) {
            return one(Command::SuppressFeature { feature: n, suppressed: false });
        }
    }
    if let Some(rest) = t.strip_prefix("suppress ") {
        if let Some(n) = find_feature(rest) {
            return one(Command::SuppressFeature { feature: n, suppressed: true });
        }
    }
    // "set width to 80 mm", "width = 80", "make width 80", "change the width to 80"
    let body = ["set ", "make ", "change "].iter().find_map(|p| t.strip_prefix(p)).unwrap_or(t);
    let body = body.strip_prefix("the ").unwrap_or(body);
    let (lhs, rhs) = body
        .split_once(" to ")
        .or_else(|| body.split_once('='))
        .or_else(|| body.split_once(" equal "))
        .or_else(|| {
            // "make width 80": name followed by a value
            let (a, b) = body.split_once(' ')?;
            b.chars().next().filter(|c| c.is_ascii_digit() || *c == '-' || *c == '(').map(|_| (a, b))
        })?;
    let (lhs, rhs) = (lhs.trim(), rhs.trim());
    if rhs.is_empty() || cad_kernel::expr::eval_with(rhs, &|_| Some(1.0)).is_err() {
        return None;
    }
    if let Some(p) = doc.parameters.iter().find(|p| p.name.to_ascii_lowercase() == lhs) {
        return one(Command::SetParameter { name: p.name.clone(), value: Ex(rhs.to_string()), comment: None });
    }
    // "<feature> <field>" e.g. "extrude1 distance"
    let (fname, field) = lhs.rsplit_once(' ')?;
    let fname = fname.trim_end_matches("'s");
    let feat = find_feature(fname)?;
    one(Command::SetFeatureValue { feature: feat, field: field.to_string(), value: Ex(rhs.to_string()) })
}

// ---------------------------------------------------------------------- flow

/// Submit a request (typed, spoken or from MCP `ask_copilot`).
pub fn submit(app: &mut App, text: &str, from_voice: bool) {
    let text = text.trim().to_string();
    if text.is_empty() {
        return;
    }
    app.copilot.push(Role::User, if from_voice { format!("(voice) {text}") } else { text.clone() });
    if app.settings.instant_commands {
        if let Some(cmds) = instant(&text, &app.doc) {
            let report = app.run_commands(&cmds, "instant");
            let msg = if report.errors.is_empty() {
                report.lines.join("; ")
            } else {
                report.errors.join("; ")
            };
            app.copilot.push(if report.errors.is_empty() { Role::Action } else { Role::Error }, format!("⚡ {msg}"));
            app.copilot.to_speak.push(if report.errors.is_empty() { short(&msg) } else { "That didn't work.".into() });
            return;
        }
    }
    if app.copilot.busy.is_some() {
        app.copilot.push(Role::Error, "Still working on the previous request — press Stop to cancel it.");
        return;
    }
    let mut msgs = vec![Msg::system(system_prompt())];
    for (u, a) in app.copilot.history.iter().rev().take(4).rev() {
        msgs.push(Msg::user(format!("Request: {u}")));
        msgs.push(Msg::assistant(a.clone()));
    }
    msgs.push(Msg::user(user_message(&app.doc, &app.model, &text, from_voice)));
    app.copilot.history.push((text.clone(), String::new()));
    app.copilot.repair_round = 0;
    app.copilot.pending_source = if from_voice { "voice".into() } else { "copilot".into() };
    send(app, msgs);
}

fn send(app: &mut App, msgs: Vec<Msg>) {
    let id = app.copilot.alloc_id();
    app.copilot.busy = Some(id);
    app.copilot.pending_msgs = msgs.clone();
    let cfg = llm_config(app);
    let model = app.settings.model.clone();
    let schema = commands::answer_schema();
    app.copilot.spawn(move || match llm::chat(&cfg, &model, &msgs, Some(&schema)) {
        Ok(text) => AiEvent::Answer { id, text },
        Err(err) => AiEvent::Failed { id, err },
    });
}

fn short(s: &str) -> String {
    let s = s.split(['\n']).next().unwrap_or(s);
    if s.len() > 140 { format!("{}…", &s[..s.char_indices().nth(137).map(|c| c.0).unwrap_or(s.len())]) } else { s.to_string() }
}

/// Drain worker events; call once per frame.
pub fn poll(app: &mut App) {
    while let Ok(ev) = app.copilot.rx.try_recv() {
        match ev {
            AiEvent::Answer { id, text } if app.copilot.busy == Some(id) => {
                app.copilot.busy = None;
                handle_answer(app, &text);
            }
            AiEvent::Failed { id, err } if app.copilot.busy == Some(id) => {
                app.copilot.busy = None;
                app.copilot.push(Role::Error, err);
                app.copilot.to_speak.push("Sorry, the AI request failed.".into());
            }
            AiEvent::Answer { .. } | AiEvent::Failed { .. } => {} // cancelled
            AiEvent::OllamaModels(r) => match r {
                Ok(m) => {
                    app.copilot.ollama_status = format!("Ollama: {} model(s)", m.len());
                    if app.settings.model.is_empty() {
                        if let Some(b) = llm::best_ollama_model(&m) {
                            app.settings.model = b;
                            app.settings.save();
                        }
                    }
                    if app.settings.vision_model.is_empty() {
                        if let Some(b) = llm::best_vision_model(&m) {
                            app.settings.vision_model = b;
                            app.settings.save();
                        }
                    }
                    app.copilot.ollama_models = m;
                }
                Err(e) => app.copilot.ollama_status = e,
            },
            AiEvent::RemoteModels(r) => {
                app.copilot.remote_loading = false;
                match r {
                    Ok(m) => app.copilot.remote_models = m,
                    Err(e) => app.copilot.key_status = e,
                }
            }
            AiEvent::KeyCheck(r) => app.copilot.key_status = r.unwrap_or_else(|e| e),
            AiEvent::Custom { id, tag, result } => crate::image_model::on_event(app, id, tag, result),
        }
    }
}

fn handle_answer(app: &mut App, text: &str) {
    let (reply, cmds, parse_errs) = parse_ai_answer(text);
    if !reply.is_empty() {
        app.copilot.push(Role::Assistant, reply.clone());
    }
    if let Some(h) = app.copilot.history.last_mut() {
        h.1 = text.trim().chars().take(1500).collect();
    }
    let mut errors: Vec<String> = parse_errs.iter().map(|e| format!("could not parse {e}")).collect();
    if !cmds.is_empty() {
        let report = app.run_commands(&cmds, &app.copilot.pending_source.clone());
        for l in &report.lines {
            app.copilot.push(Role::Action, format!("✔ {l}"));
        }
        errors.extend(report.errors);
    } else if reply.is_empty() && parse_errs.is_empty() {
        app.copilot.push(Role::Error, format!("The model did not return commands:\n{}", short(text)));
    }
    for e in &errors {
        app.copilot.push(Role::Error, e.clone());
    }
    if errors.is_empty() {
        if !reply.is_empty() {
            app.copilot.to_speak.push(short(&reply));
        }
        return;
    }
    if app.settings.auto_repair && app.copilot.repair_round < 2 {
        app.copilot.repair_round += 1;
        let mut msgs = app.copilot.pending_msgs.clone();
        msgs.push(Msg::assistant(text.to_string()));
        msgs.push(Msg::user(format!(
            "Some of that failed:\n- {}\n\nThe commands that worked have already been applied. Current design:\n{}\nReply with JSON commands that fix the problem (do not repeat the commands that worked).",
            errors.join("\n- "),
            commands::describe(&app.doc, &app.model)
        )));
        app.copilot.push(Role::Info, format!("Asking the model to fix {} problem(s) (round {})…", errors.len(), app.copilot.repair_round));
        send(app, msgs);
    } else {
        app.copilot.to_speak.push("Done, but some steps failed.".into());
    }
}

// ---------------------------------------------------------------------- UI

fn model_menu(ui: &mut egui::Ui, id: &str, value: &mut String, local: &[String], remote: &[String]) -> bool {
    let mut changed = false;
    let label = if value.is_empty() { "(choose a model)".to_string() } else { llm::display_name(value) };
    egui::ComboBox::from_id_salt(id).selected_text(label).width(210.0).show_ui(ui, |ui| {
        if local.is_empty() {
            ui.label(RichText::new("No Ollama models found").weak());
        }
        for m in local {
            changed |= ui.selectable_value(value, m.clone(), m).changed();
        }
        if !remote.is_empty() {
            ui.separator();
        }
        for m in remote {
            changed |= ui.selectable_value(value, m.clone(), llm::display_name(m)).changed();
        }
    });
    changed
}

pub fn panel(app: &mut App, ui: &mut egui::Ui) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("AI COPILOT").small().strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("✖").on_hover_text("Hide (Ctrl+L to show)").clicked() {
                app.settings.show_copilot = false;
                app.settings.save();
            }
            if ui.selectable_label(app.copilot.show_settings, "⚙").on_hover_text("Settings").clicked() {
                app.copilot.show_settings ^= true;
            }
        });
    });
    let mut save = false;
    ui.horizontal(|ui| {
        ui.label("Model");
        let (local, remote) = (app.copilot.ollama_models.clone(), app.settings.remote_models.clone());
        save |= model_menu(ui, "builder", &mut app.settings.model, &local, &remote);
        if ui.small_button("⟳").on_hover_text("Refresh Ollama models").clicked() {
            let h = app.settings.ollama_host.clone();
            app.copilot.refresh_ollama(h);
        }
    });
    if !app.copilot.ollama_status.is_empty() {
        ui.label(RichText::new(&app.copilot.ollama_status).small().weak());
    }
    if app.copilot.show_settings {
        save |= settings_ui(app, ui);
    }
    if save {
        app.settings.save();
    }
    ui.separator();

    // input at the bottom
    egui::TopBottomPanel::bottom("copilot_input").show_inside(ui, |ui| {
        ui.add_space(4.0);
        crate::voice_ui::mic_row(app, ui);
        let id = egui::Id::new("copilot_text");
        let r = ui.add(
            egui::TextEdit::multiline(&mut app.copilot.input)
                .id(id)
                .desired_rows(2)
                .desired_width(f32::INFINITY)
                .hint_text("Ask for a change… (Enter to send, Shift+Enter for a new line)"),
        );
        if app.copilot.focus_input {
            r.request_focus();
            app.copilot.focus_input = false;
        }
        let enter = r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
        ui.horizontal(|ui| {
            let busy = app.copilot.busy.is_some();
            if busy {
                ui.spinner();
                if ui.button("Stop").clicked() {
                    app.copilot.busy = None;
                    app.copilot.push(Role::Info, "Stopped.");
                }
            }
            let send = ui.add_enabled(!busy, egui::Button::new("Send")).clicked();
            if send || (enter && !busy) {
                let t = std::mem::take(&mut app.copilot.input);
                submit(app, t.trim_end_matches('\n'), false);
            }
            if ui.small_button("Clear chat").clicked() {
                app.copilot.log.truncate(1);
                app.copilot.history.clear();
            }
        });
    });

    egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
        for line in &app.copilot.log {
            let (color, prefix) = match line.role {
                Role::User => (Color32::from_rgb(0x1f, 0x4f, 0xa8), "You: "),
                Role::Assistant => (Color32::from_rgb(0x20, 0x26, 0x30), "AI: "),
                Role::Action => (Color32::from_rgb(0x2e, 0x7d, 0x32), ""),
                Role::Error => (Color32::from_rgb(0xc6, 0x28, 0x28), "⚠ "),
                Role::Info => (Color32::from_gray(110), ""),
            };
            let text = RichText::new(format!("{prefix}{}", line.text)).color(color);
            ui.add(egui::Label::new(if line.role == Role::Action { text.small() } else { text }).wrap());
        }
    });
}

fn settings_ui(app: &mut App, ui: &mut egui::Ui) -> bool {
    let mut save = false;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        egui::Grid::new("cp_set").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
            ui.label("Vision model");
            let (local, remote) = (app.copilot.ollama_models.clone(), app.settings.remote_models.clone());
            save |= model_menu(ui, "vision", &mut app.settings.vision_model, &local, &remote);
            ui.end_row();
            ui.label("Ollama host");
            save |= ui.add(egui::TextEdit::singleline(&mut app.settings.ollama_host).desired_width(200.0)).lost_focus();
            ui.end_row();
            ui.label("OpenRouter key");
            ui.horizontal(|ui| {
                let hint = if std::env::var("OPENROUTER_API_KEY").is_ok() { "using $OPENROUTER_API_KEY" } else { "sk-or-…" };
                save |= ui
                    .add(egui::TextEdit::singleline(&mut app.settings.openrouter_key).password(true).hint_text(hint).desired_width(140.0))
                    .lost_focus();
                if ui.small_button("Test").clicked() {
                    let k = app.settings.openrouter_key();
                    app.copilot.key_status = "checking…".into();
                    app.copilot.spawn(move || AiEvent::KeyCheck(llm::check_openrouter_key(&k)));
                }
            });
            ui.end_row();
            ui.label("Temperature");
            save |= ui.add(egui::Slider::new(&mut app.settings.temperature, 0.0..=1.0)).changed();
            ui.end_row();
        });
        if !app.copilot.key_status.is_empty() {
            ui.label(RichText::new(&app.copilot.key_status).small());
        }
        save |= ui.checkbox(&mut app.settings.instant_commands, "Instant commands (skip the AI for simple requests)").changed();
        save |= ui.checkbox(&mut app.settings.auto_repair, "Let the AI fix its own errors").changed();
        ui.horizontal(|ui| {
            if ui.button("Browse OpenRouter models…").clicked() {
                app.copilot.show_browser = true;
                if app.copilot.remote_models.is_empty() && !app.copilot.remote_loading {
                    app.copilot.remote_loading = true;
                    app.copilot.spawn(|| AiEvent::RemoteModels(llm::openrouter_models()));
                }
            }
            if ui.button("Import Rust_Blender settings").on_hover_text("Copy Ollama host, models, OpenRouter key and Whisper model").clicked() {
                let ok = app.settings.import_rust_blender();
                app.copilot.key_status = if ok { "Imported from Rust_Blender.".into() } else { "No Rust_Blender settings found.".into() };
                save = true;
            }
        });
        ui.collapsing("MCP server", |ui| crate::mcp::settings_ui(app, ui));
        ui.collapsing("Voice", |ui| save |= crate::voice_ui::settings_ui(app, ui));
    });
    save
}

/// Floating OpenRouter catalogue browser.
pub fn browser_window(app: &mut App, ctx: &egui::Context) {
    if !app.copilot.show_browser {
        return;
    }
    let mut open = true;
    let mut save = false;
    egui::Window::new("OpenRouter models").open(&mut open).default_size([620.0, 480.0]).show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.label("Search");
            ui.text_edit_singleline(&mut app.copilot.remote_filter);
            if app.copilot.remote_loading {
                ui.spinner();
            }
        });
        ui.label(RichText::new("Prices are $ per million tokens (in / out). 👁 = vision.").small().weak());
        let f = app.copilot.remote_filter.to_ascii_lowercase();
        let models: Vec<RemoteModel> = app
            .copilot
            .remote_models
            .iter()
            .filter(|m| f.is_empty() || m.id.to_ascii_lowercase().contains(&f) || m.name.to_ascii_lowercase().contains(&f))
            .take(300)
            .cloned()
            .collect();
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("or").striped(true).num_columns(4).show(ui, |ui| {
                for m in models {
                    let key = format!("{}{}", llm::OPENROUTER_PREFIX, m.id);
                    ui.label(format!("{}{}", if m.vision { "👁 " } else { "" }, m.id)).on_hover_text(&m.name);
                    ui.label(format!("${:.2} / ${:.2}", m.price_in, m.price_out));
                    ui.label(format!("{}k", m.context / 1000));
                    ui.horizontal(|ui| {
                        if ui.small_button("Builder").clicked() {
                            app.settings.model = key.clone();
                            save = true;
                        }
                        if m.vision && ui.small_button("Vision").clicked() {
                            app.settings.vision_model = key.clone();
                            save = true;
                        }
                    });
                    ui.end_row();
                    if save && !app.settings.remote_models.contains(&key) {
                        app.settings.remote_models.push(key);
                    }
                }
            });
        });
    });
    if save {
        app.settings.save();
    }
    app.copilot.show_browser = open;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_words() {
        assert_eq!(normalize_numbers("set width to eighty five millimeters"), "set width to 85 mm");
        assert_eq!(normalize_numbers("make it twelve point five"), "make it 12.5");
        assert_eq!(normalize_numbers("width 80 mm"), "width 80 mm");
    }

    #[test]
    fn instant_commands() {
        let d = cad_model::sample_document();
        assert_eq!(instant("Undo.", &d), Some(vec![Command::Undo]));
        assert_eq!(instant("top view", &d), Some(vec![Command::View { view: "top".into() }]));
        match instant("set thickness to twelve millimeters", &d).unwrap().remove(0) {
            Command::SetParameter { name, value, .. } => assert_eq!((name.as_str(), value.0.as_str()), ("thickness", "12 mm")),
            other => panic!("{other:?}"),
        }
        assert!(matches!(instant("width = 90", &d).unwrap()[0], Command::SetParameter { .. }));
        assert!(matches!(instant("set extrude1 distance to 20", &d).unwrap()[0], Command::SetFeatureValue { .. }));
        assert!(matches!(instant("delete cylinder2", &d).unwrap()[0], Command::DeleteFeature { .. }));
        assert_eq!(instant("add a bracket with three ribs", &d), None);
        assert_eq!(instant("make it bigger", &d), None);
    }
}
