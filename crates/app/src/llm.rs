//! Language-model clients: local Ollama and cloud OpenRouter (OpenAI-compatible).
//! A model name written `openrouter:<vendor/model>` goes to OpenRouter; anything else to Ollama.
//! All calls are blocking and meant to run on worker threads.

use serde_json::{json, Value};
use std::time::Duration;

pub const OPENROUTER_PREFIX: &str = "openrouter:";
pub const OPENROUTER_KEYS_URL: &str = "https://openrouter.ai/keys";

#[derive(Debug, Clone)]
pub struct LlmConfig {
    pub ollama_host: String,
    pub openrouter_key: String,
    pub temperature: f32,
}

/// One chat message; `images` are PNG/JPEG bytes (base64-encoded on the wire).
#[derive(Debug, Clone)]
pub struct Msg {
    pub role: &'static str,
    pub text: String,
    pub images: Vec<Vec<u8>>,
}

impl Msg {
    pub fn system(t: impl Into<String>) -> Msg {
        Msg { role: "system", text: t.into(), images: vec![] }
    }
    pub fn user(t: impl Into<String>) -> Msg {
        Msg { role: "user", text: t.into(), images: vec![] }
    }
    pub fn assistant(t: impl Into<String>) -> Msg {
        Msg { role: "assistant", text: t.into(), images: vec![] }
    }
    pub fn with_images(mut self, imgs: Vec<Vec<u8>>) -> Msg {
        self.images = imgs;
        self
    }
}

#[derive(Debug, Clone)]
pub struct RemoteModel {
    pub id: String,
    pub name: String,
    pub context: u64,
    pub price_in: f64,
    pub price_out: f64,
    pub vision: bool,
}

#[allow(dead_code)]
pub fn is_remote(model: &str) -> bool {
    model.starts_with(OPENROUTER_PREFIX)
}

pub fn display_name(model: &str) -> String {
    match model.strip_prefix(OPENROUTER_PREFIX) {
        Some(id) => format!("☁ {id}"),
        None => model.to_string(),
    }
}

fn agent(timeout_s: u64) -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(timeout_s)))
        .build()
        .into()
}

fn b64(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

fn mime(data: &[u8]) -> &'static str {
    if data.starts_with(&[0xFF, 0xD8]) { "image/jpeg" } else { "image/png" }
}

/// Send a chat and return the assistant text. `schema` requests structured JSON from Ollama.
pub fn chat(cfg: &LlmConfig, model: &str, msgs: &[Msg], schema: Option<&Value>) -> Result<String, String> {
    if model.trim().is_empty() {
        return Err("no AI model selected — pick one in the Copilot panel".into());
    }
    match model.strip_prefix(OPENROUTER_PREFIX) {
        Some(id) => openrouter_chat(cfg, id, msgs),
        None => ollama_chat(cfg, model, msgs, schema),
    }
}

fn ollama_chat(cfg: &LlmConfig, model: &str, msgs: &[Msg], schema: Option<&Value>) -> Result<String, String> {
    let host = cfg.ollama_host.trim_end_matches('/');
    let messages: Vec<Value> = msgs
        .iter()
        .map(|m| {
            let mut v = json!({"role": m.role, "content": m.text});
            if !m.images.is_empty() {
                v["images"] = Value::Array(m.images.iter().map(|i| Value::String(b64(i))).collect());
            }
            v
        })
        .collect();
    let mut body = json!({
        "model": model,
        "messages": messages,
        "stream": false,
        "think": false,
        "options": {"temperature": cfg.temperature, "num_ctx": 16384},
    });
    if let Some(s) = schema {
        body["format"] = s.clone();
    }
    let a = agent(600);
    for attempt in 0..2 {
        let mut resp = a
            .post(&format!("{host}/api/chat"))
            .send_json(&body)
            .map_err(|e| format!("can't reach Ollama at {host} ({e}). Is Ollama running?"))?;
        let status = resp.status().as_u16();
        let v: Value = resp.body_mut().read_json().unwrap_or_default();
        if status >= 400 {
            let err = v["error"].as_str().unwrap_or("unknown error").to_string();
            // Older Ollama versions reject "think"; some models reject schemas.
            if attempt == 0 && (err.contains("think") || err.contains("format")) {
                body.as_object_mut().unwrap().remove("think");
                if err.contains("format") {
                    body["format"] = json!("json");
                }
                continue;
            }
            if err.contains("not found") {
                return Err(format!("Ollama model '{model}' is not installed — run: ollama pull {model}"));
            }
            return Err(format!("Ollama error: {err}"));
        }
        return Ok(v["message"]["content"].as_str().unwrap_or_default().to_string());
    }
    Err("Ollama request failed".into())
}

fn openrouter_chat(cfg: &LlmConfig, id: &str, msgs: &[Msg]) -> Result<String, String> {
    if cfg.openrouter_key.is_empty() {
        return Err(format!("no OpenRouter API key — add one in Copilot ⚙ (get one at {OPENROUTER_KEYS_URL})"));
    }
    let messages: Vec<Value> = msgs
        .iter()
        .map(|m| {
            if m.images.is_empty() {
                json!({"role": m.role, "content": m.text})
            } else {
                let mut parts = vec![json!({"type": "text", "text": m.text})];
                for i in &m.images {
                    parts.push(json!({"type": "image_url", "image_url": {"url": format!("data:{};base64,{}", mime(i), b64(i))}}));
                }
                json!({"role": m.role, "content": parts})
            }
        })
        .collect();
    let body = json!({"model": id, "messages": messages, "temperature": cfg.temperature});
    let mut resp = agent(600)
        .post("https://openrouter.ai/api/v1/chat/completions")
        .header("Authorization", &format!("Bearer {}", cfg.openrouter_key))
        .header("X-Title", "Rust Fusion")
        .send_json(&body)
        .map_err(|e| format!("can't reach OpenRouter: {e}"))?;
    let status = resp.status().as_u16();
    let v: Value = resp.body_mut().read_json().unwrap_or_default();
    if status >= 400 || v.get("error").is_some() {
        let msg = v["error"]["message"].as_str().unwrap_or("request failed");
        return Err(format!("OpenRouter error {status}: {msg}"));
    }
    Ok(v["choices"][0]["message"]["content"].as_str().unwrap_or_default().to_string())
}

/// Installed Ollama models.
pub fn ollama_models(host: &str) -> Result<Vec<String>, String> {
    let host = host.trim_end_matches('/');
    let mut resp = agent(5)
        .get(&format!("{host}/api/tags"))
        .call()
        .map_err(|_| format!("Ollama is not reachable at {host}"))?;
    let v: Value = resp.body_mut().read_json().map_err(|e| e.to_string())?;
    let mut names: Vec<String> = v["models"]
        .as_array()
        .map(|a| a.iter().filter_map(|m| m["name"].as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    names.retain(|n| !n.contains("embed"));
    names.sort();
    Ok(names)
}

/// Pick a sensible default among installed models (bigger instruction models first).
pub fn best_ollama_model(models: &[String]) -> Option<String> {
    const PREF: &[&str] = &["qwen3:14b", "qwen3:8b", "qwen2.5:14b", "qwen2.5-coder:14b", "qwen3", "qwen2.5", "llama3.1", "gemma3", "mistral"];
    PREF.iter()
        .find_map(|p| models.iter().find(|m| m.starts_with(p)))
        .or_else(|| models.first())
        .cloned()
}

/// Pick a vision-capable model among installed ones.
pub fn best_vision_model(models: &[String]) -> Option<String> {
    const PREF: &[&str] = &["qwen2.5vl", "qwen3-vl", "gemma3", "llama3.2-vision", "llava", "minicpm-v", "moondream"];
    PREF.iter().find_map(|p| models.iter().find(|m| m.starts_with(p))).cloned()
}

/// OpenRouter model catalogue (public; no key needed).
pub fn openrouter_models() -> Result<Vec<RemoteModel>, String> {
    let mut resp = agent(30)
        .get("https://openrouter.ai/api/v1/models")
        .call()
        .map_err(|e| format!("can't reach OpenRouter: {e}"))?;
    let v: Value = resp.body_mut().read_json().map_err(|e| e.to_string())?;
    let per_m = |x: &Value| x.as_str().and_then(|s| s.parse::<f64>().ok()).or_else(|| x.as_f64()).unwrap_or(0.0) * 1e6;
    let mut out: Vec<RemoteModel> = v["data"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|m| {
                    let id = m["id"].as_str()?.to_string();
                    let inputs = m["architecture"]["input_modalities"].as_array().cloned().unwrap_or_default();
                    Some(RemoteModel {
                        name: m["name"].as_str().unwrap_or(&id).to_string(),
                        context: m["context_length"].as_u64().unwrap_or(0),
                        price_in: per_m(&m["pricing"]["prompt"]),
                        price_out: per_m(&m["pricing"]["completion"]),
                        vision: inputs.iter().any(|i| i.as_str() == Some("image")),
                        id,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

/// Validate an OpenRouter key; returns a short account summary.
pub fn check_openrouter_key(key: &str) -> Result<String, String> {
    if key.is_empty() {
        return Err("no key entered".into());
    }
    let mut resp = agent(15)
        .get("https://openrouter.ai/api/v1/key")
        .header("Authorization", &format!("Bearer {key}"))
        .call()
        .map_err(|e| format!("can't reach OpenRouter: {e}"))?;
    let status = resp.status().as_u16();
    let v: Value = resp.body_mut().read_json().unwrap_or_default();
    if status >= 400 {
        return Err(format!("key rejected ({status})"));
    }
    let used = v["data"]["usage"].as_f64().unwrap_or(0.0);
    Ok(match v["data"]["limit"].as_f64() {
        Some(l) => format!("Key OK — ${used:.2} used of ${l:.2}"),
        None => format!("Key OK — ${used:.2} used"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-shot fake Ollama server returning `reply` as the assistant message.
    pub fn fake_ollama(reply: &str) -> (String, std::thread::JoinHandle<String>) {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let body = json!({"message": {"role": "assistant", "content": reply}}).to_string();
        let h = std::thread::spawn(move || {
            let mut req = server.recv().unwrap();
            let mut got = String::new();
            req.as_reader().read_to_string(&mut got).unwrap();
            req.respond(tiny_http::Response::from_string(body)).unwrap();
            got
        });
        (format!("http://127.0.0.1:{port}"), h)
    }

    #[test]
    fn ollama_round_trip_with_image() {
        let (host, h) = fake_ollama(r#"{"reply":"ok","commands":[]}"#);
        let cfg = LlmConfig { ollama_host: host, openrouter_key: String::new(), temperature: 0.1 };
        let msgs = [Msg::system("sys"), Msg::user("hi").with_images(vec![vec![0x89, b'P', b'N', b'G']])];
        let out = chat(&cfg, "qwen3:8b", &msgs, Some(&json!({"type": "object"}))).unwrap();
        assert!(out.contains("\"reply\""));
        let sent: Value = serde_json::from_str(&h.join().unwrap()).unwrap();
        assert_eq!(sent["model"], "qwen3:8b");
        assert_eq!(sent["messages"][1]["images"].as_array().unwrap().len(), 1);
        assert_eq!(sent["format"]["type"], "object");
    }

    #[test]
    fn model_choice() {
        let m = vec!["llava:7b".to_string(), "qwen3:8b".to_string(), "nomic-embed".to_string()];
        assert_eq!(best_ollama_model(&m).as_deref(), Some("qwen3:8b"));
        assert_eq!(best_vision_model(&m).as_deref(), Some("llava:7b"));
        assert!(is_remote("openrouter:anthropic/claude-sonnet-4.5"));
    }
}
