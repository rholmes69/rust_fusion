//! Persistent user settings (AI models, keys, voice, MCP).
//!
//! Stored in `<config dir>/rust_fusion/settings.json`. On first run, the Ollama host,
//! models, OpenRouter key and Whisper model are imported from Rust_Blender's settings
//! (`<config dir>/rust_blender/settings.json`) so both apps share one setup.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub ollama_host: String,
    /// Builder model. `openrouter:<id>` = cloud, otherwise an Ollama model name.
    pub model: String,
    /// Vision model for picture → model and self-checks.
    pub vision_model: String,
    pub openrouter_key: String,
    /// Cloud models the user picked from the OpenRouter catalogue (shown in menus).
    pub remote_models: Vec<String>,
    pub instant_commands: bool,
    /// Ask the model to fix its own errors (up to 2 extra rounds).
    pub auto_repair: bool,
    pub temperature: f32,
    pub show_copilot: bool,

    pub mcp_enabled: bool,
    pub mcp_port: u16,

    pub voice_model: String,
    pub hands_free: bool,
    pub vad_threshold: f32,
    pub silence_secs: f32,
    pub speak_replies: bool,
    pub conversation: bool,
    pub wake_word: bool,
    pub wake_phrase: String,

    pub image_iterations: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            ollama_host: "http://localhost:11434".into(),
            model: String::new(),
            vision_model: String::new(),
            openrouter_key: String::new(),
            remote_models: Vec::new(),
            instant_commands: true,
            auto_repair: true,
            temperature: 0.2,
            show_copilot: true,
            mcp_enabled: true,
            mcp_port: 7979,
            voice_model: "base.en".into(),
            hands_free: true,
            vad_threshold: 0.02,
            silence_secs: 1.2,
            speak_replies: true,
            conversation: false,
            wake_word: false,
            wake_phrase: "fusion".into(),
            image_iterations: 3,
        }
    }
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("rust_fusion")
}

fn path() -> PathBuf {
    config_dir().join("settings.json")
}

impl Settings {
    pub fn load() -> Settings {
        if let Some(s) = std::fs::read_to_string(path()).ok().and_then(|t| serde_json::from_str(&t).ok()) {
            return s;
        }
        let mut s = Settings::default();
        s.import_rust_blender();
        s
    }

    pub fn save(&self) {
        let _ = std::fs::create_dir_all(config_dir());
        if let Ok(t) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path(), t);
        }
    }

    /// Copy shared fields from Rust_Blender's settings when ours are empty.
    pub fn import_rust_blender(&mut self) -> bool {
        let p = dirs::config_dir().unwrap_or_default().join("rust_blender").join("settings.json");
        let Some(v) = std::fs::read_to_string(p).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        else {
            return false;
        };
        let get = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string).filter(|s| !s.is_empty());
        let mut any = false;
        let mut take = |dst: &mut String, k: &str, only_if_empty: bool| {
            if let Some(val) = get(k) {
                if !only_if_empty || dst.is_empty() {
                    *dst = val;
                    any = true;
                }
            }
        };
        take(&mut self.ollama_host, "ollama_host", false);
        take(&mut self.model, "model", true);
        take(&mut self.vision_model, "vision_model", true);
        take(&mut self.openrouter_key, "openrouter_key", true);
        take(&mut self.voice_model, "voice_model", false);
        if let Some(a) = v.get("remote_models").and_then(|a| a.as_array()) {
            for m in a.iter().filter_map(|m| m.as_str()) {
                if !self.remote_models.iter().any(|x| x == m) {
                    self.remote_models.push(m.to_string());
                    any = true;
                }
            }
        }
        any
    }

    pub fn openrouter_key(&self) -> String {
        if self.openrouter_key.trim().is_empty() {
            std::env::var("OPENROUTER_API_KEY").unwrap_or_default().trim().to_string()
        } else {
            self.openrouter_key.trim().to_string()
        }
    }
}
