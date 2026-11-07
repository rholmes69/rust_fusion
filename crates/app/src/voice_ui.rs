//! Voice workflow: push-to-talk / hands-free mic, wake word ("Fusion, …"), voice intents
//! (stop, undo, repeat, that's all), CAD-specific speech fixes, spoken confirmations and
//! conversation mode. Speech recognition runs locally (see `voice.rs`).

use crate::app::App;
use crate::voice::{VoiceEngine, VoiceEvent, WHISPER_MODELS};
use eframe::egui::{self, Color32, RichText};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoiceIntent {
    Stop,
    Undo,
    Goodbye,
    Repeat,
}

fn norm(text: &str) -> String {
    let t: String = text
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '\'' || c == '.' || c.is_whitespace() { c } else { ' ' })
        .collect();
    t.split_whitespace().collect::<Vec<_>>().join(" ").trim_end_matches('.').to_string()
}

/// Short utterances that control the voice session rather than the design.
pub fn voice_intent(text: &str) -> Option<VoiceIntent> {
    let t = norm(text).replace('.', "");
    let t = t.trim_start_matches("okay ").trim_start_matches("ok ").trim_start_matches("no ");
    match t {
        "stop" | "cancel" | "never mind" | "nevermind" | "stop that" | "cancel that" | "be quiet" | "quiet" | "hold on" | "wait" => {
            Some(VoiceIntent::Stop)
        }
        "undo" | "undo that" | "undo it" | "scratch that" | "take that back" | "go back" | "that's wrong" | "wrong" | "no" => {
            Some(VoiceIntent::Undo)
        }
        "goodbye" | "bye" | "stop listening" | "that's all" | "that is all" | "i'm done" | "we're done" | "go to sleep" => {
            Some(VoiceIntent::Goodbye)
        }
        "repeat" | "repeat that" | "say that again" | "what did you do" | "what did you just do" | "what happened" => {
            Some(VoiceIntent::Repeat)
        }
        _ => None,
    }
}

/// If `text` starts with the wake phrase, return what follows it.
pub fn strip_wake(text: &str, wake: &str) -> Option<String> {
    let wake = norm(wake);
    if wake.is_empty() {
        return None;
    }
    let mut variants = vec![wake.clone()];
    if wake == "fusion" {
        variants.extend(["rust fusion", "fusions", "fushion", "fuson", "fusion's", "the fusion", "few shun"].map(String::from));
    }
    let words: Vec<&str> = text.split_whitespace().collect();
    let t = norm(text);
    for prefix in ["hey ", "ok ", "okay ", "hi ", ""] {
        for v in &variants {
            let p = format!("{prefix}{v}");
            if t == p || t.starts_with(&format!("{p} ")) {
                let skip = p.split_whitespace().count();
                let rest = words.get(skip..).unwrap_or(&[]).join(" ");
                return Some(rest.trim_start_matches([',', '.', '!', '?', ':', ' ']).trim().to_string());
            }
        }
    }
    None
}

/// Fix frequent speech-recognition mistakes for CAD vocabulary.
pub fn fix_mishearings(text: &str) -> String {
    const FIXES: &[(&str, &str)] = &[
        ("extra it", "extrude it"),
        ("extreme it", "extrude it"),
        ("ex trude", "extrude"),
        ("extrude it by", "extrude it by"),
        ("fill it the", "fillet the"),
        ("fill it", "fillet"),
        ("filet", "fillet"),
        ("champ for", "chamfer"),
        ("champer", "chamfer"),
        ("chamber the", "chamfer the"),
        ("re volve", "revolve"),
        ("revolt", "revolve"),
        ("spear", "sphere"),
        ("silinder", "cylinder"),
        ("a whole", "a hole"),
        ("whole through", "hole through"),
        ("radios", "radius"),
        ("mill imeters", "millimeters"),
        ("milimeters", "millimeters"),
        ("mils", "mm"),
        ("sketch one", "sketch1"),
        ("extrude one", "extrude1"),
        ("extrude two", "extrude2"),
        ("by by", "by"),
    ];
    let mut out = format!(" {} ", text.to_lowercase());
    for (a, b) in FIXES {
        out = out.replace(&format!(" {a} "), &format!(" {b} "));
        out = out.replace(&format!(" {a},"), &format!(" {b},"));
        out = out.replace(&format!(" {a}."), &format!(" {b}."));
    }
    out.trim().to_string()
}

// ---------------------------------------------------------------------- TTS

/// Speaks one utterance at a time with the OS voice; can be interrupted.
#[derive(Default)]
pub struct Speaker {
    child: Option<std::process::Child>,
    pub last: String,
}

impl Speaker {
    pub fn speak(&mut self, text: &str) {
        self.stop();
        let text: String = text.replace(['"', '\'', '`', '$', '\n', ';', '&', '|'], " ").chars().take(400).collect();
        if text.trim().is_empty() {
            return;
        }
        self.last = text.clone();
        self.child = spawn_tts(&text);
    }

    pub fn stop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    pub fn busy(&mut self) -> bool {
        match &mut self.child {
            Some(c) => match c.try_wait() {
                Ok(None) => true,
                _ => {
                    self.child = None;
                    false
                }
            },
            None => false,
        }
    }
}

impl Drop for Speaker {
    fn drop(&mut self) {
        self.stop();
    }
}

fn spawn_tts(text: &str) -> Option<std::process::Child> {
    use std::process::{Command as P, Stdio};
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let script = format!(
            "Add-Type -AssemblyName System.Speech; $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; $s.Rate = 1; $s.Speak(\"{text}\")"
        );
        P::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .creation_flags(0x08000000)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()
    }
    #[cfg(target_os = "macos")]
    {
        P::new("say").arg(text).stdout(Stdio::null()).stderr(Stdio::null()).spawn().ok()
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        P::new("espeak-ng")
            .arg(text)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .or_else(|_| P::new("espeak").arg(text).stdout(Stdio::null()).stderr(Stdio::null()).spawn())
            .ok()
    }
}

// ---------------------------------------------------------------------- state

pub struct VoiceState {
    pub engine: Option<VoiceEngine>,
    ctx: egui::Context,
    pub loaded: Option<String>,
    pub loading: bool,
    pub status: String,
    pub download: Option<(String, f32)>,
    pub speaker: Speaker,
    /// Transcribe as soon as the model is ready (mic stopped while loading).
    pending_pcm: Option<Vec<f32>>,
    pub transcribing: bool,
    /// The copilot request in flight came from voice: speak its result.
    pub expect_reply: bool,
    pub conversation_active: bool,
    wake_clip: bool,
    reopen_at: Option<Instant>,
}

impl VoiceState {
    pub fn new(ctx: &egui::Context) -> VoiceState {
        VoiceState {
            engine: None,
            ctx: ctx.clone(),
            loaded: None,
            loading: false,
            status: String::new(),
            download: None,
            speaker: Speaker::default(),
            pending_pcm: None,
            transcribing: false,
            expect_reply: false,
            conversation_active: false,
            wake_clip: false,
            reopen_at: None,
        }
    }

    fn engine(&mut self) -> &mut VoiceEngine {
        let ctx = self.ctx.clone();
        self.engine.get_or_insert_with(|| VoiceEngine::new(move || ctx.request_repaint()))
    }

    fn ensure_model(&mut self, name: &str) {
        if self.loaded.as_deref() != Some(name) && !self.loading {
            self.loading = true;
            self.status = format!("Loading Whisper {name}…");
            self.engine().load(name);
        }
    }

    pub fn recording(&self) -> bool {
        self.engine.as_ref().is_some_and(|e| e.recorder.recording)
    }

    fn start_recording(&mut self, model: &str, wake_clip: bool) -> Result<(), String> {
        self.ensure_model(model);
        self.speaker.stop();
        self.wake_clip = wake_clip;
        self.engine().recorder.start()?;
        if !wake_clip {
            self.status = "Listening…".into();
        }
        Ok(())
    }

    fn stop_and_transcribe(&mut self) {
        let pcm = self.engine().recorder.stop();
        if pcm.len() < 16000 / 4 {
            self.status = if self.wake_clip { String::new() } else { "Didn't hear anything.".into() };
            return;
        }
        if self.loaded.is_some() {
            self.transcribing = true;
            if !self.wake_clip {
                self.status = "Transcribing…".into();
            }
            self.engine().transcribe(pcm);
        } else {
            self.pending_pcm = Some(pcm);
            self.status = "Waiting for the Whisper model…".into();
        }
    }
}

/// Push-to-talk toggle (mic button, Ctrl+M, backtick).
pub fn toggle_mic(app: &mut App) {
    let model = app.settings.voice_model.clone();
    let v = &mut app.voice;
    if v.recording() {
        v.stop_and_transcribe();
    } else if let Err(e) = v.start_recording(&model, false) {
        v.status = e;
    }
}

fn handle_transcript(app: &mut App, raw: &str, was_wake_clip: bool) {
    let text = fix_mishearings(raw.trim());
    if text.is_empty() || text == "[blank_audio]" || text.starts_with('(') && text.ends_with(')') {
        if !was_wake_clip {
            app.voice.status = "Didn't catch that.".into();
        }
        return;
    }
    let text = if was_wake_clip && !app.voice.conversation_active {
        match strip_wake(&text, &app.settings.wake_phrase) {
            Some(rest) if !rest.is_empty() => rest,
            Some(_) => {
                // wake word alone: open the mic for the request
                app.voice.speaker.speak("Yes?");
                app.voice.reopen_at = Some(Instant::now() + Duration::from_millis(700));
                app.voice.conversation_active = true;
                return;
            }
            None => return, // not for us
        }
    } else {
        strip_wake(&text, &app.settings.wake_phrase).filter(|r| !r.is_empty()).unwrap_or(text)
    };
    app.voice.status = format!("Heard: {text}");
    match voice_intent(&text) {
        Some(VoiceIntent::Stop) => {
            app.copilot.busy = None;
            app.voice.speaker.stop();
            app.copilot.push(crate::copilot::Role::Info, "(voice) Stopped.");
        }
        Some(VoiceIntent::Undo) => {
            let r = app.app_action(cad_model::commands::AppAction::Undo);
            let msg = r.unwrap_or_else(|e| e);
            app.copilot.push(crate::copilot::Role::Action, format!("(voice) {msg}"));
            if app.settings.speak_replies {
                app.voice.speaker.speak(&format!("Okay, {msg}."));
            }
        }
        Some(VoiceIntent::Repeat) => {
            let last = app.voice.speaker.last.clone();
            app.voice.speaker.speak(if last.is_empty() { "I haven't done anything yet." } else { &last });
        }
        Some(VoiceIntent::Goodbye) => {
            app.voice.conversation_active = false;
            app.voice.speaker.speak("Okay.");
        }
        None => {
            app.voice.expect_reply = true;
            if app.settings.conversation {
                app.voice.conversation_active = true;
            }
            crate::copilot::submit(app, &text, true);
        }
    }
}

/// Per-frame voice work.
pub fn poll(app: &mut App) {
    // spoken confirmations
    let to_speak = std::mem::take(&mut app.copilot.to_speak);
    if app.voice.expect_reply && app.copilot.busy.is_none() {
        if let Some(last) = to_speak.last() {
            app.voice.expect_reply = false;
            if app.settings.speak_replies {
                app.voice.speaker.speak(last);
            }
            if app.voice.conversation_active {
                app.voice.reopen_at = Some(Instant::now() + Duration::from_millis(400));
            }
        }
    }
    let Some(engine) = app.voice.engine.as_ref() else {
        if app.settings.wake_word {
            // first use of wake mode creates the engine
            let model = app.settings.voice_model.clone();
            let _ = app.voice.start_recording(&model, true);
        }
        return;
    };
    let events: Vec<VoiceEvent> = engine.rx.try_iter().collect();
    for ev in events {
        match ev {
            VoiceEvent::Status(s) => app.voice.status = s,
            VoiceEvent::Download(file, p) => app.voice.download = Some((file, p)),
            VoiceEvent::Ready(name) => {
                app.voice.loading = false;
                app.voice.download = None;
                app.voice.status = format!("Whisper {name} ready");
                app.voice.loaded = Some(name);
                if let Some(pcm) = app.voice.pending_pcm.take() {
                    app.voice.transcribing = true;
                    app.voice.engine().transcribe(pcm);
                }
            }
            VoiceEvent::Transcript { text, secs } => {
                app.voice.transcribing = false;
                let wake = app.voice.wake_clip;
                log::info!("transcript ({secs:.1}s): {text}");
                handle_transcript(app, &text, wake);
            }
            VoiceEvent::Error(e) => {
                app.voice.loading = false;
                app.voice.transcribing = false;
                app.voice.download = None;
                app.voice.status = e.clone();
                app.copilot.push(crate::copilot::Role::Error, format!("Voice: {e}"));
                app.settings.wake_word = false;
                app.voice.conversation_active = false;
            }
        }
    }

    let speaking = app.voice.speaker.busy();
    let recording = app.voice.recording();
    if recording {
        app.voice.ctx.request_repaint_after(Duration::from_millis(50));
        let (th, sil) = (app.settings.vad_threshold, app.settings.silence_secs);
        let wake_clip = app.voice.wake_clip;
        let rec = &mut app.voice.engine().recorder;
        let finished = (app.settings.hands_free || wake_clip) && rec.update_vad(th, sil);
        // nobody spoke for a while in conversation / wake mode
        let idle = !rec.heard_speech && rec.started.is_some_and(|s| s.elapsed() > Duration::from_secs(if wake_clip { 4 } else { 7 }));
        if finished {
            app.voice.stop_and_transcribe();
        } else if idle && (wake_clip || app.voice.conversation_active) {
            app.voice.engine().recorder.stop();
            if !wake_clip {
                app.voice.conversation_active = false;
                app.voice.status = "Conversation ended.".into();
            }
        }
    } else if !speaking && !app.voice.transcribing && app.copilot.busy.is_none() {
        let reopen = app.voice.reopen_at.is_some_and(|t| Instant::now() >= t);
        let model = app.settings.voice_model.clone();
        if reopen {
            app.voice.reopen_at = None;
            let _ = app.voice.start_recording(&model, false);
        } else if app.voice.reopen_at.is_none() && app.settings.wake_word && app.voice.loaded.is_some() {
            let _ = app.voice.start_recording(&model, true);
        } else if app.voice.reopen_at.is_some() {
            app.voice.ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
    if speaking {
        app.voice.ctx.request_repaint_after(Duration::from_millis(150));
    }
}

/// Mic button, level meter and status (above the copilot input).
pub fn mic_row(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        let recording = app.voice.recording() && !app.voice.wake_clip;
        let label = if recording { RichText::new("Stop").color(Color32::WHITE) } else { RichText::new("Talk") };
        let b = egui::Button::new(label).fill(if recording { Color32::from_rgb(0xc6, 0x28, 0x28) } else { ui.visuals().widgets.inactive.weak_bg_fill });
        if ui.add(b).on_hover_text("Speak a request (Ctrl+M or `)").clicked() {
            if app.voice.wake_clip && app.voice.recording() {
                app.voice.engine().recorder.stop();
            }
            toggle_mic(app);
        }
        if app.voice.recording() {
            let lvl = app.voice.engine.as_ref().map_or(0.0, |e| e.recorder.level());
            let (r, _) = ui.allocate_exact_size(egui::vec2(60.0, 8.0), egui::Sense::hover());
            ui.painter().rect_filled(r, 2.0, Color32::from_gray(220));
            let w = (lvl / (app.settings.vad_threshold * 5.0)).clamp(0.0, 1.0) * r.width();
            ui.painter().rect_filled(egui::Rect::from_min_size(r.min, egui::vec2(w, r.height())), 2.0, Color32::from_rgb(0x2e, 0x7d, 0x32));
        }
        if app.voice.transcribing || app.voice.loading {
            ui.spinner();
        }
        if let Some((f, p)) = &app.voice.download {
            ui.label(RichText::new(if *p >= 0.0 { format!("{f} {:.0}%", p * 100.0) } else { f.clone() }).small());
        } else if app.settings.wake_word && app.voice.wake_clip && app.voice.recording() {
            ui.label(RichText::new(format!("Say \"{}, …\"", app.settings.wake_phrase)).small().weak());
        } else if !app.voice.status.is_empty() {
            ui.label(RichText::new(&app.voice.status).small().weak());
        }
    });
}

pub fn settings_ui(app: &mut App, ui: &mut egui::Ui) -> bool {
    let mut save = false;
    if !cfg!(feature = "voice") {
        ui.label("This build has no voice support (built with --no-default-features).");
    }
    ui.horizontal(|ui| {
        ui.label("Whisper model");
        egui::ComboBox::from_id_salt("whisper").selected_text(&app.settings.voice_model).show_ui(ui, |ui| {
            for (name, _) in WHISPER_MODELS {
                save |= ui.selectable_value(&mut app.settings.voice_model, name.to_string(), *name).changed();
            }
        });
        if ui.small_button("Load").clicked() {
            let m = app.settings.voice_model.clone();
            app.voice.ensure_model(&m);
        }
    });
    ui.label(RichText::new("Downloaded once from Hugging Face; shared with Rust_Blender's cache.").small().weak());
    save |= ui.checkbox(&mut app.settings.hands_free, "Hands-free: stop recording when I stop talking").changed();
    save |= ui.add(egui::Slider::new(&mut app.settings.silence_secs, 0.5..=3.0).text("silence (s)")).changed();
    save |= ui.add(egui::Slider::new(&mut app.settings.vad_threshold, 0.002..=0.1).logarithmic(true).text("mic threshold")).changed();
    save |= ui.checkbox(&mut app.settings.speak_replies, "Speak confirmations").changed();
    save |= ui.checkbox(&mut app.settings.conversation, "Conversation mode (listen again after each reply)").changed();
    ui.horizontal(|ui| {
        save |= ui.checkbox(&mut app.settings.wake_word, "Wake word").changed();
        save |= ui.add(egui::TextEdit::singleline(&mut app.settings.wake_phrase).desired_width(90.0)).lost_focus();
    });
    if !app.settings.wake_word && app.voice.wake_clip && app.voice.recording() {
        app.voice.engine().recorder.stop();
    }
    save
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intents_and_wake() {
        assert_eq!(voice_intent("Stop."), Some(VoiceIntent::Stop));
        assert_eq!(voice_intent("No, undo that!"), Some(VoiceIntent::Undo));
        assert_eq!(voice_intent("Okay, that's all."), Some(VoiceIntent::Goodbye));
        assert_eq!(voice_intent("extrude it by ten"), None);
        assert_eq!(strip_wake("Hey Fusion, add a box.", "fusion").as_deref(), Some("add a box."));
        assert_eq!(strip_wake("Rust Fusion top view", "fusion").as_deref(), Some("top view"));
        assert_eq!(strip_wake("add a box", "fusion"), None);
    }

    #[test]
    fn cad_mishearings() {
        assert_eq!(fix_mishearings("Extra it by 20 mils"), "extrude it by 20 mm");
        assert_eq!(fix_mishearings("fill it the top edges"), "fillet the top edges");
        assert_eq!(fix_mishearings("drill a whole through the plate"), "drill a hole through the plate");
    }
}
