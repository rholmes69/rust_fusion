//! Image → Model: turn a sheet with several views of a part or building into a solid.
//!
//! 1. Load the image; the grid of views is detected and each cell gets a view label
//!    (or a vision model reads the sheet: layout, labels, overall size, description).
//! 2. Each orthographic view's silhouette is traced locally and the views are intersected
//!    into a "visual hull" — accurate outlines that don't depend on the AI guessing numbers.
//! 3. A vision model adds what silhouettes can't show (holes, pockets, windows…).
//! 4. Self-check: the model is projected into every view and compared with the drawing
//!    (silhouette IoU); renders of the model plus the scores go back to the vision model,
//!    which corrects the design. Changes that make the score worse are undone.

use crate::app::App;
use crate::copilot::{AiEvent, Role};
use crate::llm::{self, Msg};
use crate::views::{self, Dims, Silhouette, ViewKind, ViewSil};
use cad_model::commands::{self, extract_json, parse_ai_answer};
use eframe::egui::{self, Color32, RichText};
use image::RgbaImage;
use std::path::PathBuf;

struct CheckRun {
    /// indices into `ImageState::views`
    order: Vec<usize>,
    pos: usize,
    pngs: Vec<(ViewKind, Vec<u8>)>,
    saved: (bool, bool, bool, crate::camera::Camera),
    refine: bool,
}

pub struct ImageState {
    pub open: bool,
    pub path: Option<PathBuf>,
    img: Option<RgbaImage>,
    /// Downscaled PNG sent to vision models.
    png: Option<Vec<u8>>,
    tex: Option<egui::TextureHandle>,
    pub rows: usize,
    pub cols: usize,
    pub labels: Vec<ViewKind>,
    pub threshold: f32,
    pub known: Dims,
    pub description: String,
    sils: Vec<Option<Silhouette>>,
    views: Vec<ViewSil>,
    dims: Option<Dims>,
    pub log: Vec<(bool, String)>,
    busy: Option<(u64, &'static str)>,
    iteration: u32,
    last_score: Option<f64>,
    check: Option<CheckRun>,
}

impl Default for ImageState {
    fn default() -> Self {
        ImageState {
            open: false,
            path: None,
            img: None,
            png: None,
            tex: None,
            rows: 1,
            cols: 1,
            labels: vec![ViewKind::Front],
            threshold: 40.0,
            known: Dims::default(),
            description: String::new(),
            sils: Vec::new(),
            views: Vec::new(),
            dims: None,
            log: Vec::new(),
            busy: None,
            iteration: 0,
            last_score: None,
            check: None,
        }
    }
}

impl ImageState {
    fn say(&mut self, ok: bool, s: impl Into<String>) {
        self.log.push((ok, s.into()));
    }

    pub fn load(&mut self, path: PathBuf) -> Result<(), String> {
        let img = image::open(&path).map_err(|e| format!("can't open image: {e}"))?.to_rgba8();
        // a copy for vision models, max 1600 px on the long side
        let (w, h) = img.dimensions();
        let scale = (1600.0 / w.max(h) as f32).min(1.0);
        let small = if scale < 1.0 {
            image::imageops::resize(&img, (w as f32 * scale) as u32, (h as f32 * scale) as u32, image::imageops::FilterType::Triangle)
        } else {
            img.clone()
        };
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(small).write_to(&mut buf, image::ImageFormat::Png).map_err(|e| e.to_string())?;
        self.png = Some(buf.into_inner());
        let (r, c) = views::detect_grid(&img, self.threshold);
        self.rows = r;
        self.cols = c;
        self.labels = views::default_labels(r, c);
        self.img = Some(img);
        self.tex = None;
        self.path = Some(path.clone());
        self.description.clear();
        self.iteration = 0;
        self.last_score = None;
        self.log.clear();
        self.say(true, format!("Loaded {} — detected a {r}×{c} grid of views.", path.display()));
        self.extract();
        Ok(())
    }

    /// Trace silhouettes for every cell.
    pub fn extract(&mut self) {
        let Some(img) = &self.img else { return };
        self.labels.resize(self.rows * self.cols, ViewKind::Iso);
        self.sils = (0..self.rows * self.cols)
            .map(|i| views::silhouette(&views::cell(img, self.rows, self.cols, i / self.cols, i % self.cols), self.threshold))
            .collect();
        self.collect_views();
    }

    fn collect_views(&mut self) {
        self.views = (0..self.labels.len())
            .filter(|&i| self.labels[i].is_ortho())
            .filter_map(|i| self.sils.get(i).cloned().flatten().map(|sil| ViewSil { kind: self.labels[i], sil }))
            .collect();
    }

    fn sheet_png(&self) -> Vec<Vec<u8>> {
        self.png.iter().cloned().collect()
    }
}

// ---------------------------------------------------------------------- actions

fn vision_request(app: &mut App, tag: &'static str, msgs: Vec<Msg>) {
    let model = if app.settings.vision_model.is_empty() { app.settings.model.clone() } else { app.settings.vision_model.clone() };
    if model.is_empty() {
        app.image.say(false, "Choose a vision model first (Copilot ⚙ ▸ Vision model).");
        return;
    }
    let id = app.copilot.alloc_id();
    app.image.busy = Some((id, tag));
    let cfg = crate::copilot::llm_config(app);
    let schema = if tag == "read" { None } else { Some(commands::answer_schema()) };
    app.copilot.spawn(move || AiEvent::Custom { id, tag, result: llm::chat(&cfg, &model, &msgs, schema.as_ref()) });
}

const READ_PROMPT: &str = "This image is a sheet showing several views of ONE object (a mechanical part or a building). \
Identify the grid of views and what each view shows. Answer only with JSON: \
{\"rows\": R, \"cols\": C, \"views\": [{\"row\": 0, \"col\": 0, \"view\": \"front|back|left|right|top|bottom|iso|ignore\"}], \
\"width_mm\": number or null, \"depth_mm\": number or null, \"height_mm\": number or null, \"description\": \"what the object is and its visible features\"}. \
width = overall size left-to-right in the front view, height = bottom-to-top in the front view, depth = front-to-back. \
Use dimensions written on the drawing when present; otherwise estimate realistic sizes (for buildings, doors are about 2100 mm tall and storeys about 3000 mm). \
Use \"iso\" for perspective/isometric pictures and \"ignore\" for title blocks or empty cells.";

pub fn ai_read(app: &mut App) {
    let imgs = app.image.sheet_png();
    if imgs.is_empty() {
        return;
    }
    app.image.say(true, "Asking the vision model to read the sheet…");
    vision_request(app, "read", vec![Msg::user(READ_PROMPT).with_images(imgs)]);
}

fn frame_text(d: &Dims) -> String {
    format!(
        "Overall size: width {:.1} mm (X), depth {:.1} mm (Z), height {:.1} mm (Y). The object is centred on X and Z with its base at Y=0, \
so X runs {:.1}..{:.1}, Z runs {:.1}..{:.1}, Y runs 0..{:.1}. The front view looks along -Z (you see X and Y), the top view looks down (you see X and Z, \
the top of the image is -Z), the right view looks along -X (you see -Z to the right and Y up).",
        d.width, d.depth, d.height, -d.width / 2.0, d.width / 2.0, -d.depth / 2.0, d.depth / 2.0, d.height
    )
}

/// Build the visual hull from the labelled silhouettes.
pub fn build_hull(app: &mut App) {
    app.image.collect_views();
    if app.image.views.is_empty() {
        app.image.say(false, "No silhouettes found in the labelled orthographic views. Check the labels and the ink threshold.");
        return;
    }
    let d = match views::resolve_dims(&app.image.views, app.image.known) {
        Ok(d) => d,
        Err(e) => {
            app.image.say(false, e);
            return;
        }
    };
    let cmds = match views::hull_commands(&app.image.views, &d) {
        Ok(c) => c,
        Err(e) => {
            app.image.say(false, e);
            return;
        }
    };
    if !app.doc.features.is_empty() {
        let name = app.doc.name.clone();
        app.checkpoint();
        app.doc = cad_model::Document { name, ..Default::default() };
        app.executor = Default::default();
        app.rebuild();
    }
    let rep = app.run_commands(&cmds, "image");
    for e in &rep.errors {
        app.image.say(false, e.clone());
    }
    app.image.dims = Some(d);
    app.image.iteration = 0;
    app.fit_pending = true;
    let used: Vec<&str> = app.image.views.iter().map(|v| v.kind.label()).collect();
    app.image.say(
        rep.errors.is_empty(),
        format!("Built the silhouette hull from {} — {:.0} × {:.0} × {:.0} mm.", used.join(", "), d.width, d.depth, d.height),
    );
    score(app);
}

/// Silhouette IoU of the current model in every labelled view.
fn score(app: &mut App) -> Option<f64> {
    let d = app.image.dims?;
    let solids = app.visible_solids();
    if solids.is_empty() {
        app.image.say(false, "No bodies to compare.");
        return None;
    }
    let scores: Vec<(ViewKind, f64)> = app.image.views.iter().map(|v| (v.kind, views::iou(&solids, v, &d))).collect();
    let mean = scores.iter().map(|s| s.1).sum::<f64>() / scores.len().max(1) as f64;
    let text = scores.iter().map(|(k, s)| format!("{} {:.0}%", k.label(), s * 100.0)).collect::<Vec<_>>().join(", ");
    app.image.say(true, format!("Match with the drawing: {text} (mean {:.0}%).", mean * 100.0));
    app.image.last_score = Some(mean);
    Some(mean)
}

fn score_text(app: &App) -> String {
    let Some(d) = app.image.dims else { return String::new() };
    let solids = app.visible_solids();
    app.image
        .views
        .iter()
        .map(|v| format!("{} {:.0}%", v.kind.label(), views::iou(&solids, v, &d) * 100.0))
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn ai_details(app: &mut App) {
    let Some(d) = app.image.dims else {
        app.image.say(false, "Build the silhouette model first.");
        return;
    };
    let mut text = format!(
        "{}\n\nThe image shows several views of the object{}. A model was built as the intersection of the extruded silhouettes. \
Add the details visible in the views that silhouettes cannot capture: through-holes, pockets, slots, recesses, steps, and for buildings \
windows, doors and roof features. Use new sketches on XY / XZ / YZ (with offsets) and extrudes with operation \"cut\" or \"join\". \
Do not rebuild the outer shape.\n{}\n\nCurrent design:\n{}",
        system_text(),
        if app.image.description.is_empty() { String::new() } else { format!(" ({})", app.image.description) },
        frame_text(&d),
        commands::describe(&app.doc, &app.model)
    );
    text += "\nAnswer only with the JSON object.";
    let imgs = app.image.sheet_png();
    app.image.say(true, "Asking the vision model for details (holes, pockets, openings)…");
    vision_request(app, "details", vec![Msg::user(text).with_images(imgs)]);
}

fn system_text() -> String {
    format!(
        "You are the modelling assistant inside Rust Fusion, a parametric CAD app (mm, degrees, Y up). Reply ONLY with a JSON object \
{{\"reply\": \"one short sentence\", \"commands\": [...]}} using these commands:\n{}",
        commands::COMMAND_REFERENCE
    )
}

/// Render every labelled view (screenshots) then score; with `refine`, ask the AI to fix differences.
pub fn check(app: &mut App, refine: bool) {
    if app.image.dims.is_none() || app.image.views.is_empty() {
        app.image.say(false, "Build the silhouette model first.");
        return;
    }
    let saved = (app.show_grid, app.show_origin, app.settings.show_copilot, app.camera);
    app.show_grid = false;
    app.show_origin = false;
    app.camera.perspective = false;
    app.image.check = Some(CheckRun { order: (0..app.image.views.len()).collect(), pos: 0, pngs: Vec::new(), saved, refine });
    shoot_next(app);
}

fn shoot_next(app: &mut App) {
    let Some(run) = &app.image.check else { return };
    let i = run.order[run.pos];
    let view = app.image.views[i].kind.camera().to_string();
    let _ = app.app_action(cad_model::commands::AppAction::View(view));
    app.fit_all();
    crate::shots::request(app, crate::shots::ShotTarget::ImageCheck(i));
}

pub fn on_screenshot(app: &mut App, view: usize, png: Option<Vec<u8>>) {
    let Some(run) = app.image.check.as_mut() else { return };
    if let Some(p) = png {
        let kind = app.image.views[view].kind;
        run.pngs.push((kind, p));
    }
    run.pos += 1;
    if run.pos < run.order.len() {
        shoot_next(app);
        return;
    }
    let run = app.image.check.take().unwrap();
    let (grid, origin, _, cam) = run.saved;
    app.show_grid = grid;
    app.show_origin = origin;
    app.camera = cam;
    let before = app.image.last_score;
    let Some(mean) = score(app) else { return };
    if !run.refine {
        return;
    }
    if mean > 0.985 {
        app.image.say(true, "The model already matches the drawing closely.");
        return;
    }
    let _ = before;
    let d = app.image.dims.unwrap();
    let order: Vec<&str> = run.pngs.iter().map(|(k, _)| k.label()).collect();
    let text = format!(
        "{}\n\nImage 1 is the reference sheet{}. Images 2..{} are the current model rendered (orthographic) from these views, in order: {}.\n\
Silhouette agreement with the drawing (intersection over union): {}.\n{}\n\nCurrent design:\n{}\n\
Compare the model with the reference and reply with commands that make it match better: change parameters or feature values, add or cut features. \
If it already matches, return an empty command list. Answer only with the JSON object.",
        system_text(),
        if app.image.description.is_empty() { String::new() } else { format!(" ({})", app.image.description) },
        run.pngs.len() + 1,
        order.join(", "),
        score_text(app),
        frame_text(&d),
        commands::describe(&app.doc, &app.model)
    );
    let mut imgs = app.image.sheet_png();
    imgs.extend(run.pngs.into_iter().map(|p| p.1));
    app.image.say(true, format!("Refinement round {}: sending renders to the vision model…", app.image.iteration + 1));
    vision_request(app, "refine", vec![Msg::user(text).with_images(imgs)]);
}

/// Worker results for this module.
pub fn on_event(app: &mut App, id: u64, tag: &'static str, result: Result<String, String>) {
    if app.image.busy.map(|b| b.0) != Some(id) {
        return;
    }
    app.image.busy = None;
    let text = match result {
        Ok(t) => t,
        Err(e) => {
            app.image.say(false, e);
            return;
        }
    };
    match tag {
        "read" => {
            let Some(v) = extract_json(&text) else {
                app.image.say(false, "The vision model did not return JSON.");
                return;
            };
            let st = &mut app.image;
            let (r, c) = (v["rows"].as_u64().unwrap_or(st.rows as u64) as usize, v["cols"].as_u64().unwrap_or(st.cols as u64) as usize);
            if (1..=4).contains(&r) && (1..=4).contains(&c) {
                st.rows = r;
                st.cols = c;
                st.labels = vec![ViewKind::Iso; r * c];
            }
            if let Some(list) = v["views"].as_array() {
                for e in list {
                    let (rr, cc) = (e["row"].as_u64().unwrap_or(99) as usize, e["col"].as_u64().unwrap_or(99) as usize);
                    if rr < st.rows && cc < st.cols {
                        st.labels[rr * st.cols + cc] = ViewKind::parse(e["view"].as_str().unwrap_or("iso"));
                    }
                }
            }
            let num = |k: &str| v[k].as_f64().filter(|x| *x > 0.0).unwrap_or(0.0);
            if st.known.width <= 0.0 && st.known.depth <= 0.0 && st.known.height <= 0.0 {
                st.known = Dims { width: num("width_mm"), depth: num("depth_mm"), height: num("height_mm") };
            }
            st.description = v["description"].as_str().unwrap_or("").to_string();
            let msg = format!(
                "Read the sheet: {}×{} grid; size {:.0} × {:.0} × {:.0} mm. {}",
                st.rows, st.cols, st.known.width, st.known.depth, st.known.height, st.description
            );
            st.say(true, msg);
            st.extract();
        }
        "details" | "refine" => {
            let (reply, cmds, errs) = parse_ai_answer(&text);
            if !reply.is_empty() {
                app.image.say(true, format!("AI: {reply}"));
            }
            for e in errs {
                app.image.say(false, format!("could not parse {e}"));
            }
            if cmds.is_empty() {
                app.image.say(true, "No changes suggested.");
                return;
            }
            let before = app.image.last_score;
            let rep = app.run_commands(&cmds, "image AI");
            for l in &rep.lines {
                app.copilot.push(Role::Action, format!("✔ {l}"));
            }
            for e in &rep.errors {
                app.image.say(false, e.clone());
            }
            app.image.say(true, format!("Applied {} change(s).", rep.lines.len()));
            let after = score(app);
            if let (Some(b), Some(a)) = (before, after) {
                if a + 0.01 < b && tag == "refine" {
                    let _ = app.app_action(cad_model::commands::AppAction::Undo);
                    app.image.last_score = Some(b);
                    app.image.say(false, format!("That made the match worse ({:.0}% → {:.0}%), so it was undone.", b * 100.0, a * 100.0));
                    return;
                }
            }
            if tag == "refine" {
                app.image.iteration += 1;
                if app.image.iteration < app.settings.image_iterations {
                    check(app, true);
                } else {
                    app.image.say(true, "Finished the refinement rounds.");
                }
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------- UI

pub fn window(app: &mut App, ctx: &egui::Context) {
    if !app.image.open {
        return;
    }
    let mut open = true;
    egui::Window::new("Image to Model").open(&mut open).default_size([860.0, 620.0]).show(ctx, |ui| {
        ui.horizontal(|ui| {
            if ui.button("Open image…").clicked() {
                if let Some(p) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg"]).pick_file() {
                    if let Err(e) = app.image.load(p) {
                        app.image.say(false, e);
                    }
                }
            }
            if let Some(p) = &app.image.path {
                ui.label(RichText::new(p.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default()).weak());
            } else {
                ui.label(RichText::new("A sheet with 1–9 views (front, top, side, …) of one part or building.").weak());
            }
            if app.image.busy.is_some() || app.image.check.is_some() {
                ui.spinner();
            }
        });
        ui.separator();
        ui.columns(2, |cols| {
            preview(app, &mut cols[0]);
            controls(app, &mut cols[1]);
        });
    });
    app.image.open = open;
}

fn preview(app: &mut App, ui: &mut egui::Ui) {
    let Some(img) = &app.image.img else {
        ui.label("No image loaded.");
        return;
    };
    let (w, h) = img.dimensions();
    if app.image.tex.is_none() {
        let ci = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], img.as_raw());
        app.image.tex = Some(ui.ctx().load_texture("sheet", ci, Default::default()));
    }
    let tex = app.image.tex.as_ref().unwrap();
    let avail = ui.available_width();
    let scale = (avail / w as f32).min(460.0 / h as f32);
    let size = egui::vec2(w as f32 * scale, h as f32 * scale);
    let r = ui.add(egui::Image::new((tex.id(), size)));
    let rect = r.rect;
    let p = ui.painter_at(rect);
    let (rows, cols) = (app.image.rows, app.image.cols);
    for i in 1..cols {
        let x = rect.left() + rect.width() * i as f32 / cols as f32;
        p.line_segment([egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())], egui::Stroke::new(1.0_f32, Color32::from_rgb(30, 110, 220)));
    }
    for i in 1..rows {
        let y = rect.top() + rect.height() * i as f32 / rows as f32;
        p.line_segment([egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)], egui::Stroke::new(1.0_f32, Color32::from_rgb(30, 110, 220)));
    }
    let (cw, ch) = (w as f32 / cols as f32, h as f32 / rows as f32);
    for idx in 0..rows * cols {
        let (rr, cc) = (idx / cols, idx % cols);
        let ox = cc as f32 * cw + (cw * 0.02).ceil();
        let oy = rr as f32 * ch + (ch * 0.02).ceil();
        let k = app.image.labels.get(idx).copied().unwrap_or(ViewKind::Iso);
        if let Some(Some(s)) = app.image.sils.get(idx) {
            if k.is_ortho() {
                let pts: Vec<egui::Pos2> = s
                    .outline
                    .iter()
                    .map(|q| rect.min + egui::vec2((ox + q.x as f32) * scale, (oy + q.y as f32) * scale))
                    .collect();
                p.add(egui::Shape::closed_line(pts, egui::Stroke::new(2.0_f32, Color32::from_rgb(240, 120, 0))));
            }
        }
        let pos = rect.min + egui::vec2(cc as f32 * cw * scale + 4.0, rr as f32 * ch * scale + 4.0);
        let g = p.layout_no_wrap(k.label().to_string(), egui::FontId::proportional(12.0), Color32::WHITE);
        p.rect_filled(egui::Rect::from_min_size(pos, g.size()).expand(2.0), 3.0, Color32::from_rgba_unmultiplied(20, 70, 160, 210));
        p.galley(pos, g, Color32::WHITE);
    }
    ui.label(RichText::new("Orange = traced silhouette used for the model.").small().weak());
}

fn controls(app: &mut App, ui: &mut egui::Ui) {
    let loaded = app.image.img.is_some();
    let busy = app.image.busy.is_some() || app.image.check.is_some();
    let mut re_extract = false;
    egui::Grid::new("img_grid").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        ui.label("Grid");
        ui.horizontal(|ui| {
            re_extract |= ui.add(egui::DragValue::new(&mut app.image.rows).range(1..=4).suffix(" rows")).changed();
            re_extract |= ui.add(egui::DragValue::new(&mut app.image.cols).range(1..=4).suffix(" cols")).changed();
            if ui.small_button("Auto").clicked() {
                if let Some(img) = &app.image.img {
                    let (r, c) = views::detect_grid(img, app.image.threshold);
                    app.image.rows = r;
                    app.image.cols = c;
                    app.image.labels = views::default_labels(r, c);
                    re_extract = true;
                }
            }
        });
        ui.end_row();
        ui.label("Ink threshold");
        re_extract |= ui.add(egui::Slider::new(&mut app.image.threshold, 10.0..=150.0)).drag_stopped();
        ui.end_row();
        ui.label("Width (X)");
        ui.add(egui::DragValue::new(&mut app.image.known.width).range(0.0..=1e6).suffix(" mm"));
        ui.end_row();
        ui.label("Depth (Z)");
        ui.add(egui::DragValue::new(&mut app.image.known.depth).range(0.0..=1e6).suffix(" mm"));
        ui.end_row();
        ui.label("Height (Y)");
        ui.add(egui::DragValue::new(&mut app.image.known.height).range(0.0..=1e6).suffix(" mm"));
        ui.end_row();
    });
    ui.label(RichText::new("Give at least one size (0 = work it out from the views).").small().weak());
    if re_extract {
        let n = app.image.rows * app.image.cols;
        if app.image.labels.len() != n {
            app.image.labels = views::default_labels(app.image.rows, app.image.cols);
        }
        app.image.extract();
    }
    // per-cell labels
    let mut relabel = false;
    egui::Grid::new("labels").spacing([4.0, 4.0]).show(ui, |ui| {
        for r in 0..app.image.rows {
            for c in 0..app.image.cols {
                let i = r * app.image.cols + c;
                if i >= app.image.labels.len() {
                    continue;
                }
                let found = matches!(app.image.sils.get(i), Some(Some(_)));
                egui::ComboBox::from_id_salt(("lbl", i))
                    .width(92.0)
                    .selected_text(format!("{}{}", app.image.labels[i].label(), if found { "" } else { " ·" }))
                    .show_ui(ui, |ui| {
                        for k in ViewKind::ALL {
                            relabel |= ui.selectable_value(&mut app.image.labels[i], k, k.label()).changed();
                        }
                    });
            }
            ui.end_row();
        }
    });
    if relabel {
        app.image.collect_views();
    }
    ui.horizontal(|ui| {
        ui.label("Vision model");
        ui.label(RichText::new(if app.settings.vision_model.is_empty() { "(not set — Copilot ⚙)".into() } else { llm::display_name(&app.settings.vision_model) }).strong());
    });
    if !app.image.description.is_empty() {
        ui.label(RichText::new(&app.image.description).small().italics());
    }
    ui.add_space(4.0);
    ui.add_enabled_ui(loaded && !busy, |ui| {
        if ui.button("AI: read the sheet (labels, size)").clicked() {
            ai_read(app);
        }
        if ui.button("1. Build from silhouettes").clicked() {
            build_hull(app);
        }
        let has = app.image.dims.is_some();
        ui.add_enabled_ui(has, |ui| {
            if ui.button("2. AI: add holes, pockets, openings").clicked() {
                ai_details(app);
            }
            ui.horizontal(|ui| {
                if ui.button("3. Check & refine with AI").clicked() {
                    app.image.iteration = 0;
                    check(app, true);
                }
                ui.add(egui::DragValue::new(&mut app.settings.image_iterations).range(1..=8).suffix(" rounds"));
                if ui.button("Check only").clicked() {
                    check(app, false);
                }
            });
        });
    });
    ui.separator();
    egui::ScrollArea::vertical().max_height(180.0).stick_to_bottom(true).show(ui, |ui| {
        for (ok, l) in &app.image.log {
            ui.label(RichText::new(l).small().color(if *ok { Color32::from_gray(60) } else { Color32::from_rgb(0xc6, 0x28, 0x28) }));
        }
    });
}

/// MCP / copilot helper: load a sheet and build its hull in one go.
pub fn image_to_model(app: &mut App, path: &str, known: Dims) -> Result<String, String> {
    app.image.load(PathBuf::from(path))?;
    app.image.open = true;
    if known.width > 0.0 || known.depth > 0.0 || known.height > 0.0 {
        app.image.known = known;
    }
    build_hull(app);
    let log: Vec<String> = app.image.log.iter().map(|(ok, l)| format!("{}{l}", if *ok { "" } else { "error: " })).collect();
    if app.image.dims.is_some() { Ok(log.join("\n")) } else { Err(log.join("\n")) }
}
