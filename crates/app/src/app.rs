//! Application state, document lifecycle (undo/redo, files) and the top-level layout.

use crate::camera::{Camera, StdView};
use crate::render::{GpuResources, StaticScene};
use crate::scene;
use cad_model::commands::{AppAction, Command, Executor, Outcome};
use cad_model::sketch::{PlaneKind, Sketch};
use cad_model::{
    sample_document, Axis, Document, ExtentKind, FeatureId, FeatureKind, Model, Operation, RevolveAxis,
};
use eframe::egui;
use glam::{DVec2, DVec3};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SketchTool {
    Select,
    Line,
    Rectangle,
    Circle,
}

impl SketchTool {
    pub fn label(self) -> &'static str {
        match self {
            SketchTool::Select => "Select",
            SketchTool::Line => "Line",
            SketchTool::Rectangle => "Rectangle",
            SketchTool::Circle => "Circle",
        }
    }
}

pub struct SketchSession {
    pub feature: FeatureId,
    pub tool: SketchTool,
    pub pending: Vec<DVec2>,
    pub selected: Option<usize>,
    pub saved_rollback: Option<usize>,
    /// Snapped cursor position on the sketch plane.
    pub cursor: Option<DVec2>,
    pub snapped_to_point: bool,
}

pub struct CommandSession {
    pub feature: FeatureId,
    pub before: Document,
    pub is_new: bool,
}

pub enum Mode {
    Idle,
    ChoosePlane { offset: String },
    Sketch(SketchSession),
    Command(CommandSession),
}

pub struct App {
    pub doc: Document,
    pub model: Model,
    pub static_gen: u64,
    pub static_scene: Arc<StaticScene>,
    /// (body index, triangle) for ray picking.
    pub pick_tris: Vec<(usize, [DVec3; 3])>,
    pub undo: Vec<Document>,
    pub redo: Vec<Document>,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    pub camera: Camera,
    pub mode: Mode,
    pub selected_body: Option<usize>,
    pub selected_feature: Option<FeatureId>,
    pub hidden_bodies: HashSet<String>,
    pub hidden_sketches: HashSet<FeatureId>,
    pub show_origin: bool,
    pub show_grid: bool,
    pub show_params: bool,
    pub show_help: bool,
    pub status: String,
    pub hover_region: Option<(FeatureId, usize)>,
    pub hover_plane: Option<PlaneKind>,
    pub hover_world: Option<DVec3>,
    pub viewport_rect: egui::Rect,
    pub fit_pending: bool,
    pub title: String,
    pub settings: crate::settings::Settings,
    pub copilot: crate::copilot::Copilot,
    pub executor: Executor,
    pub mcp: Option<crate::mcp::McpServer>,
    pub mcp_status: String,
    pub voice: crate::voice_ui::VoiceState,
    pub image: crate::image_model::ImageState,
    pub shots: Vec<crate::shots::ShotTarget>,
    pub gen: crate::gen_ui::GenState,
}

/// Result of running a batch of commands.
#[derive(Debug, Default, Clone)]
pub struct RunReport {
    pub lines: Vec<String>,
    pub errors: Vec<String>,
}

pub fn fmt_num(v: f64) -> String {
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    if s == "-0" {
        "0".into()
    } else {
        s
    }
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, path: Option<PathBuf>) -> App {
        if let Some(rs) = cc.wgpu_render_state.as_ref() {
            GpuResources::install(rs);
        }
        style(&cc.egui_ctx);
        let settings = crate::settings::Settings::load();
        let mut copilot = crate::copilot::Copilot::new();
        copilot.ctx = Some(cc.egui_ctx.clone());
        let mut app = App {
            doc: Document::default(),
            model: Model::default(),
            static_gen: 0,
            static_scene: Arc::new(StaticScene::default()),
            pick_tris: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            path: None,
            dirty: false,
            camera: Camera::default(),
            mode: Mode::Idle,
            selected_body: None,
            selected_feature: None,
            hidden_bodies: HashSet::new(),
            hidden_sketches: HashSet::new(),
            show_origin: true,
            show_grid: true,
            show_params: false,
            show_help: false,
            status: "Welcome. Create a sketch (S), add a primitive, or open File > Open Sample.".into(),
            hover_region: None,
            hover_plane: None,
            hover_world: None,
            viewport_rect: egui::Rect::NOTHING,
            fit_pending: false,
            title: String::new(),
            settings,
            copilot,
            executor: Executor::default(),
            mcp: None,
            mcp_status: String::new(),
            voice: crate::voice_ui::VoiceState::new(&cc.egui_ctx),
            image: crate::image_model::ImageState::default(),
            shots: Vec::new(),
            gen: Default::default(),
        };
        let host = app.settings.ollama_host.clone();
        app.copilot.refresh_ollama(host);
        crate::mcp::restart(&mut app, &cc.egui_ctx);
        match path {
            Some(p) if p.as_os_str() == "--sample" => app.open_sample(),
            Some(p) => app.open_path(p),
            None => {}
        }
        app.rebuild();
        app
    }

    // ------------------------------------------------------------------ model

    /// Regenerate the model and rebuild GPU geometry.
    pub fn rebuild(&mut self) {
        self.model = self.doc.regenerate();
        if let Some(i) = self.selected_body {
            if i >= self.model.bodies.len() {
                self.selected_body = None;
            }
        }
        let (s, picks) = scene::build_static(&self.model, &self.hidden_bodies, self.selected_body);
        self.static_scene = Arc::new(s);
        self.pick_tris = picks;
        self.static_gen += 1;
    }

    /// Record an undo step before mutating the document.
    pub fn checkpoint(&mut self) {
        self.undo.push(self.doc.clone());
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.dirty = true;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty() && !matches!(self.mode, Mode::Command(_))
    }

    pub fn undo(&mut self) {
        if !self.can_undo() {
            return;
        }
        if let Some(d) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.doc, d));
            self.after_history_change();
        }
    }

    pub fn redo(&mut self) {
        if matches!(self.mode, Mode::Command(_)) {
            return;
        }
        if let Some(d) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.doc, d));
            self.after_history_change();
        }
    }

    fn after_history_change(&mut self) {
        self.dirty = true;
        if let Mode::Sketch(s) = &mut self.mode {
            if self.doc.feature(s.feature).is_none() {
                self.mode = Mode::Idle;
            } else {
                s.pending.clear();
                s.selected = None;
            }
        }
        self.rebuild();
    }

    // ------------------------------------------------------------------ files

    pub fn new_document(&mut self) {
        self.set_document(Document::default(), None);
        self.status = "New design.".into();
    }

    pub fn set_document(&mut self, doc: Document, path: Option<PathBuf>) {
        self.doc = doc;
        self.path = path;
        self.undo.clear();
        self.redo.clear();
        self.dirty = false;
        self.mode = Mode::Idle;
        self.selected_body = None;
        self.selected_feature = None;
        self.hidden_bodies.clear();
        self.hidden_sketches = self
            .doc
            .features
            .iter()
            .filter(|f| matches!(f.kind, FeatureKind::Sketch(_)) && !self.doc.dependents(f.id).is_empty())
            .map(|f| f.id)
            .collect();
        self.rebuild();
        self.fit_pending = true;
    }

    pub fn open_sample(&mut self) {
        self.set_document(sample_document(), None);
        self.status = "Sample loaded. Open Modify > Parameters and change 'thickness' or 'width'.".into();
    }

    pub fn open_dialog(&mut self) {
        if let Some(p) = rfd::FileDialog::new()
            .add_filter("Rust Fusion design", &["rfz", "json"])
            .pick_file()
        {
            self.open_path(p);
        }
    }

    pub fn open_path(&mut self, p: PathBuf) {
        match std::fs::read_to_string(&p).map_err(|e| e.to_string()).and_then(|s| Document::from_json(&s)) {
            Ok(d) => {
                self.set_document(d, Some(p.clone()));
                self.status = format!("Opened {}", p.display());
            }
            Err(e) => self.status = format!("Could not open {}: {e}", p.display()),
        }
    }

    pub fn save(&mut self, save_as: bool) {
        let path = match (&self.path, save_as) {
            (Some(p), false) => Some(p.clone()),
            _ => rfd::FileDialog::new()
                .add_filter("Rust Fusion design", &["rfz"])
                .set_file_name(format!("{}.rfz", self.doc.name))
                .save_file(),
        };
        let Some(p) = path else { return };
        match std::fs::write(&p, self.doc.to_json()) {
            Ok(()) => {
                self.status = format!("Saved {}", p.display());
                self.path = Some(p);
                self.dirty = false;
            }
            Err(e) => self.status = format!("Save failed: {e}"),
        }
    }

    pub fn export_stl(&mut self) {
        let _ = self.export_stl_to(None);
    }

    /// Visible bodies' solids.
    pub fn visible_solids(&self) -> Vec<&cad_kernel::Solid> {
        self.model
            .bodies
            .iter()
            .filter(|b| !self.hidden_bodies.contains(&b.name))
            .map(|b| &b.solid)
            .collect()
    }

    fn export_path(&self, path: Option<String>, ext: &str, label: &str) -> Option<PathBuf> {
        match path {
            Some(p) => Some(PathBuf::from(p)),
            None => rfd::FileDialog::new()
                .add_filter(label, &[ext])
                .set_file_name(format!("{}.{ext}", self.doc.name))
                .save_file(),
        }
    }

    /// Export visible bodies; `path` None opens a dialog.
    pub fn export_stl_to(&mut self, path: Option<String>) -> Result<String, String> {
        let r = self.export_stl_inner(path);
        self.status = match &r {
            Ok(m) | Err(m) => m.clone(),
        };
        r
    }

    fn export_stl_inner(&mut self, path: Option<String>) -> Result<String, String> {
        let solids = self.visible_solids();
        if solids.is_empty() {
            return Err("Nothing to export: no visible bodies.".into());
        }
        let p = self.export_path(path, "stl", "STL mesh").ok_or("Export cancelled.")?;
        let res = std::fs::File::create(&p).and_then(|f| {
            let mut w = std::io::BufWriter::new(f);
            cad_kernel::stl::write_binary(&mut w, &solids)
        });
        res.map(|_| format!("Exported {} body(ies) to {}", solids.len(), p.display()))
            .map_err(|e| format!("Export failed: {e}"))
    }

    pub fn export_gltf_to(&mut self, path: Option<String>) -> Result<String, String> {
        let p = self.export_path(path, "glb", "glTF binary").ok_or("Export cancelled.")?;
        let r = crate::export::write_glb(self, &p);
        self.status = match &r {
            Ok(m) | Err(m) => m.clone(),
        };
        r
    }

    pub fn export_step_to(&mut self, path: Option<String>) -> Result<String, String> {
        let p = self.export_path(path, "step", "STEP").ok_or("Export cancelled.")?;
        let r = crate::export::write_step(self, &p);
        self.status = match &r {
            Ok(m) | Err(m) => m.clone(),
        };
        r
    }

    /// Save to `path` (or the current file / a dialog).
    pub fn save_to(&mut self, path: Option<String>) -> Result<String, String> {
        if let Some(p) = path {
            self.path = Some(PathBuf::from(p));
        }
        self.save(false);
        if self.dirty { Err("Not saved.".into()) } else { Ok(self.status.clone()) }
    }

    /// Run validated commands (AI, voice, MCP). Document edits form one undo step.
    pub fn run_commands(&mut self, cmds: &[Command], source: &str) -> RunReport {
        if !matches!(self.mode, Mode::Idle) {
            self.cancel_mode();
        }
        let mut report = RunReport::default();
        let errors_before: HashSet<FeatureId> = self.model.errors.keys().copied().collect();
        let bodies_before = self.model.bodies.len();
        let consumed_before: HashSet<FeatureId> =
            self.doc.features.iter().filter(|f| !self.doc.dependents(f.id).is_empty()).map(|f| f.id).collect();
        let mut before = self.doc.clone();
        let mut changed = false;
        let flush = |app: &mut App, before: &mut Document, changed: &mut bool| {
            if *changed {
                app.undo.push(std::mem::replace(before, app.doc.clone()));
                app.redo.clear();
                app.dirty = true;
                app.rebuild();
                *changed = false;
            }
        };
        for c in cmds {
            match self.executor.apply(&mut self.doc, c) {
                Ok(Outcome::Changed(m)) => {
                    changed = true;
                    report.lines.push(m);
                }
                Ok(Outcome::App(a)) => {
                    flush(self, &mut before, &mut changed);
                    match self.app_action(a) {
                        Ok(m) => report.lines.push(m),
                        Err(e) => report.errors.push(e),
                    }
                    before = self.doc.clone();
                }
                Err(e) => {
                    let name = serde_json::to_value(c)
                        .ok()
                        .and_then(|v| v.get("action").and_then(|a| a.as_str()).map(str::to_string))
                        .unwrap_or_default();
                    report.errors.push(format!("{name}: {e}"));
                }
            }
        }
        flush(self, &mut before, &mut changed);
        // hide sketches that just got consumed by a feature (like finishing a command does)
        for f in &self.doc.features {
            if matches!(f.kind, FeatureKind::Sketch(_)) && !self.doc.dependents(f.id).is_empty() && !consumed_before.contains(&f.id) {
                self.hidden_sketches.insert(f.id);
            }
        }
        for (id, e) in &self.model.errors {
            if !errors_before.contains(id) {
                let n = self.doc.feature(*id).map(|f| f.name.clone()).unwrap_or_default();
                report.errors.push(format!("{n} failed to build: {e}"));
            }
        }
        for (n, e) in &self.model.param_errors {
            report.errors.push(format!("parameter {n}: {e}"));
        }
        // frame the result when the batch created the first solids
        if bodies_before == 0 && !self.model.bodies.is_empty() && source != "instant" {
            self.fit_pending = true;
        }
        if !report.lines.is_empty() {
            self.status = format!("{source}: {}", report.lines.last().unwrap());
        }
        report
    }

    pub fn app_action(&mut self, a: AppAction) -> Result<String, String> {
        match a {
            AppAction::Undo => {
                if !self.can_undo() {
                    return Err("nothing to undo".into());
                }
                self.undo();
                Ok("undid the last change".into())
            }
            AppAction::Redo => {
                if self.redo.is_empty() {
                    return Err("nothing to redo".into());
                }
                self.redo();
                Ok("redid the change".into())
            }
            AppAction::View(v) => {
                let view = match v.to_ascii_lowercase().as_str() {
                    "top" => StdView::Top,
                    "front" => StdView::Front,
                    "right" => StdView::Right,
                    "left" => StdView::Left,
                    "back" => StdView::Back,
                    "bottom" => StdView::Bottom,
                    "home" | "iso" | "isometric" => StdView::Home,
                    o => return Err(format!("unknown view '{o}'")),
                };
                self.camera.set_view(view);
                Ok(format!("{v} view"))
            }
            AppAction::FitAll => {
                self.fit_all();
                Ok("fit the view".into())
            }
            AppAction::ExportStl(p) => self.export_stl_to(p),
            AppAction::ExportGltf(p) => self.export_gltf_to(p),
            AppAction::ExportStep(p) => self.export_step_to(p),
            AppAction::Save(p) => self.save_to(p),
            AppAction::NewDesign => {
                self.new_document();
                Ok("started a new design".into())
            }
        }
    }

    // ------------------------------------------------------------------ commands

    pub fn start_sketch_command(&mut self) {
        self.cancel_mode();
        self.mode = Mode::ChoosePlane { offset: "0".into() };
        self.status = "Select a plane for the new sketch (click an origin plane or use the panel).".into();
    }

    pub fn create_sketch(&mut self, plane: PlaneKind, offset: String) {
        self.checkpoint();
        let name = self.doc.unique_name("Sketch");
        let mut sk = Sketch::new(plane);
        sk.offset = offset;
        let id = self.doc.insert_feature(name, FeatureKind::Sketch(sk));
        self.rebuild();
        self.enter_sketch(id);
    }

    pub fn enter_sketch(&mut self, id: FeatureId) {
        let Some(idx) = self.doc.index_of(id) else { return };
        let plane = match &self.doc.features[idx].kind {
            FeatureKind::Sketch(s) => s.plane,
            _ => return,
        };
        let saved = self.doc.rollback;
        // Roll the timeline back so the sketch is edited in its own context.
        if idx + 1 < self.doc.features.len() {
            self.doc.rollback = Some(idx + 1);
        }
        self.hidden_sketches.remove(&id);
        self.camera.set_view(match plane {
            PlaneKind::XY => StdView::Front,
            PlaneKind::XZ => StdView::Top,
            PlaneKind::YZ => StdView::Right,
        });
        self.mode = Mode::Sketch(SketchSession {
            feature: id,
            tool: SketchTool::Line,
            pending: Vec::new(),
            selected: None,
            saved_rollback: saved,
            cursor: None,
            snapped_to_point: false,
        });
        self.selected_feature = Some(id);
        self.rebuild();
        self.status = "Sketching: L line, R rectangle, C circle, Esc select, Enter finish sketch.".into();
    }

    pub fn finish_sketch(&mut self) {
        if let Mode::Sketch(s) = std::mem::replace(&mut self.mode, Mode::Idle) {
            self.doc.rollback = s.saved_rollback;
            if self.doc.rollback.map_or(false, |r| r >= self.doc.features.len()) {
                self.doc.rollback = None;
            }
            self.camera.set_view(StdView::Home);
            self.rebuild();
            self.status = "Sketch finished. Use Extrude (E) or Revolve to make a solid.".into();
        }
    }

    /// The sketch a new extrude/revolve should start with.
    fn default_sketch(&self) -> Option<FeatureId> {
        let active = &self.doc.features[..self.doc.active_len()];
        let is_usable = |id: FeatureId| {
            self.model.sketches.get(&id).map_or(false, |g| !g.regions.is_empty())
        };
        if let Some(sel) = self.selected_feature {
            if active.iter().any(|f| f.id == sel && matches!(f.kind, FeatureKind::Sketch(_))) && is_usable(sel) {
                return Some(sel);
            }
        }
        active
            .iter()
            .rev()
            .find(|f| matches!(f.kind, FeatureKind::Sketch(_)) && is_usable(f.id))
            .map(|f| f.id)
    }

    fn default_op(&self) -> Operation {
        if self.model.bodies.is_empty() {
            Operation::NewBody
        } else {
            Operation::Join
        }
    }

    pub fn start_feature_command(&mut self, which: &str) {
        self.cancel_mode();
        let before = self.doc.clone();
        let op = self.default_op();
        let s3 = |a: &str, b: &str, c: &str| [a.to_string(), b.to_string(), c.to_string()];
        let kind = match which {
            "Extrude" | "Revolve" => {
                let Some(sk) = self.default_sketch() else {
                    self.status = "Draw a sketch with a closed profile first.".into();
                    return;
                };
                let n = self.model.sketches.get(&sk).map_or(0, |g| g.regions.len());
                let profiles = if n == 1 { vec![0] } else { vec![] };
                if which == "Extrude" {
                    FeatureKind::Extrude {
                        sketch: sk,
                        profiles,
                        distance: "10 mm".into(),
                        extent: ExtentKind::OneSide,
                        flip: false,
                        op,
                        edges: None,
                    }
                } else {
                    FeatureKind::Revolve {
                        sketch: sk,
                        profiles,
                        axis: RevolveAxis::SketchV,
                        angle: "360 deg".into(),
                        op,
                    }
                }
            }
            "Box" => FeatureKind::Box { corner: s3("0", "0", "0"), size: s3("40", "20", "30"), op },
            "Cylinder" => FeatureKind::Cylinder {
                base: s3("0", "0", "0"),
                axis: Axis::Y,
                radius: "15".into(),
                height: "30".into(),
                op,
            },
            "Sphere" => FeatureKind::Sphere { center: s3("0", "0", "0"), radius: "20".into(), op },
            _ => return,
        };
        let name = self.doc.unique_name(which);
        let id = self.doc.insert_feature(name, kind);
        self.mode = Mode::Command(CommandSession { feature: id, before, is_new: true });
        self.selected_feature = Some(id);
        self.rebuild();
        self.status = match which {
            "Extrude" | "Revolve" => "Click profiles in the viewport to add/remove them, set values, then OK.".into(),
            _ => "Set the values, then OK.".into(),
        };
    }

    /// The extrude a fillet/chamfer/shell should apply to: the selected one, or the last one.
    fn target_extrude(&self) -> Option<FeatureId> {
        let active = &self.doc.features[..self.doc.active_len()];
        let is_ex = |id: FeatureId| active.iter().any(|f| f.id == id && matches!(f.kind, FeatureKind::Extrude { .. }));
        if let Some(sel) = self.selected_feature.filter(|s| is_ex(*s)) {
            return Some(sel);
        }
        if let Some(b) = self.selected_body.and_then(|i| self.model.bodies.get(i)) {
            if is_ex(b.created_by) {
                return Some(b.created_by);
            }
        }
        active.iter().rev().find(|f| matches!(f.kind, FeatureKind::Extrude { .. })).map(|f| f.id)
    }

    /// Fillet / Chamfer (edits an extrude) or Shell (new feature).
    pub fn start_edge_command(&mut self, which: &str) {
        self.cancel_mode();
        let Some(target) = self.target_extrude() else {
            self.status = "Fillet, chamfer and shell work on extrudes — make one first (or select one).".into();
            return;
        };
        let before = self.doc.clone();
        if which == "Shell" {
            let name = self.doc.unique_name("Shell");
            // put the shell right after the timeline marker (after its target)
            let id = self.doc.insert_feature(
                name,
                FeatureKind::Shell { target, thickness: "2 mm".into(), open: "end".into() },
            );
            self.mode = Mode::Command(CommandSession { feature: id, before, is_new: true });
            self.selected_feature = Some(id);
        } else {
            let kind = if which == "Fillet" { cad_kernel::profile::EdgeKind::Fillet } else { cad_kernel::profile::EdgeKind::Chamfer };
            if let Some(f) = self.doc.feature_mut(target) {
                if let FeatureKind::Extrude { edges, .. } = &mut f.kind {
                    let size = edges.as_ref().map(|e| e.size.clone()).unwrap_or_else(|| "2 mm".into());
                    *edges = Some(cad_model::EdgeTreatment { kind, size, start: false, end: true });
                }
            }
            self.mode = Mode::Command(CommandSession { feature: target, before, is_new: false });
            self.selected_feature = Some(target);
        }
        self.rebuild();
        self.status = format!("{which}: set the size, then OK.");
    }

    pub fn edit_feature(&mut self, id: FeatureId) {
        let Some(f) = self.doc.feature(id) else { return };
        if matches!(f.kind, FeatureKind::Sketch(_)) {
            self.cancel_mode();
            self.enter_sketch(id);
            return;
        }
        self.cancel_mode();
        self.mode = Mode::Command(CommandSession { feature: id, before: self.doc.clone(), is_new: false });
        self.selected_feature = Some(id);
        self.status = "Editing feature. OK to apply, Cancel to revert.".into();
    }

    pub fn commit_command(&mut self) {
        if let Mode::Command(c) = std::mem::replace(&mut self.mode, Mode::Idle) {
            if c.before != self.doc {
                self.undo.push(c.before);
                self.redo.clear();
                self.dirty = true;
            }
            if let Some(sk) = self.doc.feature(c.feature).and_then(|f| f.kind.parent_sketch()) {
                self.hidden_sketches.insert(sk);
            }
            self.status = match self.model.errors.get(&c.feature) {
                Some(e) => format!("Feature has an error: {e}"),
                None => "Done.".into(),
            };
            self.rebuild();
        }
    }

    /// Leave the current mode, reverting an uncommitted command.
    pub fn cancel_mode(&mut self) {
        match std::mem::replace(&mut self.mode, Mode::Idle) {
            Mode::Command(c) => {
                self.doc = c.before;
                self.rebuild();
                self.status = "Cancelled.".into();
            }
            Mode::Sketch(s) => {
                self.mode = Mode::Sketch(s);
                self.finish_sketch();
            }
            _ => {}
        }
    }

    pub fn delete_feature(&mut self, id: FeatureId) {
        if matches!(self.mode, Mode::Command(_) | Mode::Sketch(_)) {
            self.cancel_mode();
        }
        self.checkpoint();
        self.doc.delete_feature(id);
        if self.selected_feature == Some(id) {
            self.selected_feature = None;
        }
        self.rebuild();
    }

    pub fn fit_all(&mut self) {
        let mut lo = DVec3::splat(f64::MAX);
        let mut hi = DVec3::splat(f64::MIN);
        for b in &self.model.bodies {
            if let Some((a, c)) = b.solid.bounds() {
                lo = lo.min(a);
                hi = hi.max(c);
            }
        }
        for g in self.model.sketches.values() {
            if let Some(fr) = g.frame {
                for (pts, _) in &g.curves {
                    for p in pts {
                        let w = fr.to_3d(*p);
                        lo = lo.min(w);
                        hi = hi.max(w);
                    }
                }
            }
        }
        if lo.x <= hi.x {
            self.camera.fit(lo, hi);
        } else {
            self.camera.target = DVec3::ZERO;
            self.camera.distance = 250.0;
        }
    }

    // ------------------------------------------------------------------ shortcuts

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() {
            return;
        }
        use egui::Key;
        use egui::Modifiers as M;
        // Key events carry their own modifiers, so fast chords are still recognised.
        let chord = |m: M, k: Key| ctx.input_mut(|i| i.consume_key(m, k));
        if chord(M::COMMAND | M::SHIFT, Key::Z) || chord(M::COMMAND, Key::Y) {
            self.redo();
            return;
        }
        if chord(M::COMMAND, Key::Z) {
            self.undo();
            return;
        }
        if chord(M::COMMAND | M::SHIFT, Key::S) {
            self.save(true);
            return;
        }
        if chord(M::COMMAND, Key::S) {
            self.save(false);
            return;
        }
        if chord(M::COMMAND, Key::O) {
            self.open_dialog();
            return;
        }
        if chord(M::COMMAND, Key::N) {
            self.new_document();
            return;
        }
        if chord(M::COMMAND, Key::L) || chord(M::NONE, Key::Slash) {
            self.status = "Copilot: type a request and press Enter.".into();
            self.settings.show_copilot = true;
            self.copilot.focus_input = true;
            self.settings.save();
            return;
        }
        if chord(M::COMMAND, Key::M) || chord(M::NONE, Key::Backtick) {
            crate::voice_ui::toggle_mic(self);
            return;
        }
        if ctx.input(|i| i.modifiers.command) {
            return;
        }
        let pressed = |k: Key| chord(M::NONE, k);
        if pressed(Key::F) {
            self.fit_all();
        }
        match &mut self.mode {
            Mode::Sketch(s) => {
                if pressed(Key::L) {
                    s.tool = SketchTool::Line;
                    s.pending.clear();
                } else if pressed(Key::R) {
                    s.tool = SketchTool::Rectangle;
                    s.pending.clear();
                } else if pressed(Key::C) {
                    s.tool = SketchTool::Circle;
                    s.pending.clear();
                } else if pressed(Key::Escape) {
                    if s.tool == SketchTool::Line && s.pending.len() >= 2 {
                        crate::viewport::commit_polyline(self, false);
                    } else if !s.pending.is_empty() {
                        s.pending.clear();
                    } else if s.tool != SketchTool::Select {
                        s.tool = SketchTool::Select;
                    } else {
                        self.finish_sketch();
                    }
                } else if pressed(Key::Enter) {
                    if s.tool == SketchTool::Line && s.pending.len() >= 2 {
                        crate::viewport::commit_polyline(self, false);
                    } else {
                        self.finish_sketch();
                    }
                } else if pressed(Key::Delete) || pressed(Key::Backspace) {
                    crate::viewport::delete_selected_entity(self);
                }
            }
            Mode::Command(_) => {
                if pressed(Key::Escape) {
                    self.cancel_mode();
                } else if pressed(Key::Enter) {
                    self.commit_command();
                }
            }
            Mode::ChoosePlane { .. } => {
                if pressed(Key::Escape) {
                    self.mode = Mode::Idle;
                    self.status.clear();
                }
            }
            Mode::Idle => {
                if pressed(Key::S) {
                    self.start_sketch_command();
                } else if pressed(Key::E) {
                    self.start_feature_command("Extrude");
                } else if pressed(Key::Escape) {
                    self.selected_body = None;
                    self.selected_feature = None;
                    self.rebuild();
                } else if pressed(Key::Delete) {
                    if let Some(id) = self.selected_feature {
                        self.delete_feature(id);
                    }
                }
            }
        }
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let t = format!(
            "{}{} — Rust Fusion",
            self.doc.name,
            if self.dirty { " *" } else { "" }
        );
        if t != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(t.clone()));
            self.title = t;
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if frame.wgpu_render_state().is_none() {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.heading("Rust Fusion needs a GPU (wgpu) renderer, which could not be created.");
            });
            return;
        }
        self.shortcuts(ctx);
        self.update_title(ctx);
        crate::copilot::poll(self);
        crate::mcp::poll(self);
        crate::voice_ui::poll(self);
        crate::shots::poll(self);

        egui::TopBottomPanel::top("menu").show(ctx, |ui| crate::panels::menu_bar(self, ui));
        egui::TopBottomPanel::top("toolbar")
            .frame(egui::Frame::side_top_panel(&ctx.style()).inner_margin(egui::Margin::symmetric(8.0, 6.0)))
            .show(ctx, |ui| crate::panels::toolbar(self, ui));
        egui::TopBottomPanel::bottom("status")
            .frame(egui::Frame::side_top_panel(&ctx.style()).inner_margin(egui::Margin::symmetric(8.0, 3.0)))
            .show(ctx, |ui| crate::panels::status_bar(self, ui));
        egui::TopBottomPanel::bottom("timeline")
            .resizable(false)
            .frame(egui::Frame::side_top_panel(&ctx.style()).inner_margin(egui::Margin::symmetric(8.0, 6.0)))
            .show(ctx, |ui| crate::panels::timeline(self, ui));
        egui::SidePanel::left("browser")
            .resizable(true)
            .default_width(230.0)
            .show(ctx, |ui| crate::panels::browser(self, ui));
        if self.settings.show_copilot {
            egui::SidePanel::right("copilot")
                .resizable(true)
                .default_width(340.0)
                .min_width(260.0)
                .show(ctx, |ui| crate::copilot::panel(self, ui));
        }
        let show_right = !matches!(self.mode, Mode::Idle) || self.selected_body.is_some();
        if show_right {
            egui::SidePanel::right("inspector")
                .resizable(true)
                .default_width(290.0)
                .show(ctx, |ui| crate::panels::inspector(self, ui));
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| crate::viewport::viewport(self, ui));

        crate::panels::parameters_window(self, ctx);
        crate::panels::help_window(self, ctx);
        crate::copilot::browser_window(self, ctx);
        crate::image_model::window(self, ctx);
        crate::gen_ui::windows(self, ctx);

        if self.fit_pending {
            self.fit_pending = false;
            self.fit_all();
        }
    }
}

fn style(ctx: &egui::Context) {
    ctx.set_visuals(egui::Visuals::light());
    ctx.style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(8.0, 4.0);
        s.visuals.selection.bg_fill = egui::Color32::from_rgb(0x1f, 0x6f, 0xd1);
        s.visuals.widgets.noninteractive.bg_stroke.color = egui::Color32::from_gray(210);
    });
}
