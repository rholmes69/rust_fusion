//! Menus, toolbar, browser tree, timeline, inspector and dialogs.

use crate::app::{fmt_num, App, Mode, SketchTool};
use crate::camera::StdView;
use cad_kernel::expr;
use cad_model::constraints::{Constraint, PointRef, SegRef};
use cad_model::sketch::{Entity, PlaneKind};
use cad_kernel::profile::EdgeKind;
use cad_model::{Axis, EdgeTreatment, ExtentKind, FeatureKind, Operation, Parameter, RevolveAxis};
use eframe::egui::{self, Color32, RichText};
use std::collections::HashMap;

const ACCENT: Color32 = Color32::from_rgb(0x1f, 0x6f, 0xd1);
const ERROR: Color32 = Color32::from_rgb(0xc6, 0x28, 0x28);

fn feature_color(k: &FeatureKind) -> Color32 {
    match k {
        FeatureKind::Sketch(_) => Color32::from_rgb(0x2f, 0x7d, 0xe1),
        FeatureKind::Extrude { .. } => Color32::from_rgb(0xe0, 0x8a, 0x1e),
        FeatureKind::Revolve { .. } => Color32::from_rgb(0x8e, 0x5c, 0xd9),
        _ => Color32::from_rgb(0x3a, 0xa6, 0x5a),
    }
}

/// A single-line expression editor that shows the evaluated value (or error).
/// Returns (changed, gained_focus).
fn expr_edit(ui: &mut egui::Ui, label: &str, text: &mut String, vars: &HashMap<String, f64>, unit: &str) -> (bool, bool) {
    let mut changed = false;
    let mut focus = false;
    ui.label(label);
    ui.horizontal(|ui| {
        let r = ui.add(egui::TextEdit::singleline(text).desired_width(120.0).font(egui::TextStyle::Monospace));
        changed = r.changed();
        focus = r.gained_focus();
        match expr::eval(text, vars) {
            Ok(v) => ui.label(RichText::new(format!("= {} {unit}", fmt_num(v))).weak()),
            Err(e) => ui.label(RichText::new("⚠").color(ERROR)).on_hover_text(e.0),
        };
    });
    ui.end_row();
    (changed, focus)
}

fn op_combo(ui: &mut egui::Ui, op: &mut Operation) -> bool {
    let mut changed = false;
    ui.label("Operation");
    egui::ComboBox::from_id_salt("op").selected_text(op.label()).show_ui(ui, |ui| {
        for o in Operation::ALL {
            changed |= ui.selectable_value(op, o, o.label()).changed();
        }
    });
    ui.end_row();
    changed
}

// ---------------------------------------------------------------------- menu

pub fn menu_bar(app: &mut App, ui: &mut egui::Ui) {
    egui::menu::bar(ui, |ui| {
        ui.menu_button("File", |ui| {
            if ui.button("New            Ctrl+N").clicked() {
                app.new_document();
                ui.close_menu();
            }
            if ui.button("Open…          Ctrl+O").clicked() {
                ui.close_menu();
                app.open_dialog();
            }
            if ui.button("Image to Model…").clicked() {
                app.image.open = true;
                ui.close_menu();
            }
            if ui.button("Open Sample").clicked() {
                app.open_sample();
                ui.close_menu();
            }
            ui.separator();
            if ui.button("Save           Ctrl+S").clicked() {
                ui.close_menu();
                app.save(false);
            }
            if ui.button("Save As…  Ctrl+Shift+S").clicked() {
                ui.close_menu();
                app.save(true);
            }
            ui.separator();
            if ui.button("Export STL…").clicked() {
                ui.close_menu();
                app.export_stl();
            }
            if ui.button("Export glTF (.glb)…").clicked() {
                ui.close_menu();
                let _ = app.export_gltf_to(None);
            }
            if ui.button("Export STEP…").clicked() {
                ui.close_menu();
                let _ = app.export_step_to(None);
            }
            if ui.button("Send to Rust_Blender").on_hover_text("Export as glTF and import it into the running Rust_Blender (its MCP server)").clicked() {
                ui.close_menu();
                let r = crate::export::send_to_blender(app);
                app.status = r.unwrap_or_else(|e| e);
            }
            ui.separator();
            if ui.button("Quit").clicked() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
        ui.menu_button("Edit", |ui| {
            if ui.add_enabled(app.can_undo(), egui::Button::new("Undo   Ctrl+Z")).clicked() {
                app.undo();
                ui.close_menu();
            }
            if ui.add_enabled(!app.redo.is_empty(), egui::Button::new("Redo   Ctrl+Y")).clicked() {
                app.redo();
                ui.close_menu();
            }
        });
        ui.menu_button("View", |ui| {
            for (label, v) in [
                ("Home", StdView::Home),
                ("Front", StdView::Front),
                ("Top", StdView::Top),
                ("Right", StdView::Right),
            ] {
                if ui.button(label).clicked() {
                    app.camera.set_view(v);
                    ui.close_menu();
                }
            }
            if ui.button("Fit All   F").clicked() {
                app.fit_all();
                ui.close_menu();
            }
            ui.separator();
            ui.checkbox(&mut app.camera.perspective, "Perspective");
            ui.checkbox(&mut app.show_grid, "Grid");
            ui.checkbox(&mut app.show_origin, "Origin axes");
        });
        ui.menu_button("Modify", |ui| {
            if ui.button("Parameters…").clicked() {
                app.show_params = true;
                ui.close_menu();
            }
        });
        ui.menu_button("Help", |ui| {
            if ui.button("Controls & shortcuts").clicked() {
                app.show_help = true;
                ui.close_menu();
            }
        });
    });
}

// ---------------------------------------------------------------------- toolbar

fn tool_button(ui: &mut egui::Ui, label: &str, hint: &str, active: bool) -> bool {
    let text = RichText::new(label).size(14.0);
    let b = egui::Button::new(if active { text.color(Color32::WHITE) } else { text })
        .fill(if active { ACCENT } else { ui.visuals().widgets.inactive.weak_bg_fill })
        .min_size(egui::vec2(0.0, 30.0));
    ui.add(b).on_hover_text(hint).clicked()
}

pub fn toolbar(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        if let Mode::Sketch(s) = &app.mode {
            let current = s.tool;
            ui.label(RichText::new("SKETCH").strong().color(ACCENT));
            ui.separator();
            for (t, key) in [
                (SketchTool::Select, "Esc"),
                (SketchTool::Line, "L"),
                (SketchTool::Rectangle, "R"),
                (SketchTool::Circle, "C"),
            ] {
                if tool_button(ui, t.label(), &format!("{} ({key})", t.label()), current == t) {
                    if let Mode::Sketch(s) = &mut app.mode {
                        s.tool = t;
                        s.pending.clear();
                    }
                }
            }
            ui.separator();
            if tool_button(ui, "✔ Finish Sketch", "Finish sketch (Enter)", false) {
                app.finish_sketch();
            }
            return;
        }
        ui.label(RichText::new("SOLID").strong().color(ACCENT));
        ui.separator();
        let choosing = matches!(app.mode, Mode::ChoosePlane { .. });
        if tool_button(ui, "✏ Create Sketch", "Create a sketch on a plane (S)", choosing) {
            app.start_sketch_command();
        }
        ui.separator();
        for (name, hint) in [
            ("Extrude", "Extrude sketch profiles (E)"),
            ("Revolve", "Revolve sketch profiles around a sketch axis"),
        ] {
            if tool_button(ui, name, hint, false) {
                app.start_feature_command(name);
            }
        }
        ui.separator();
        for name in ["Fillet", "Chamfer", "Shell"] {
            if tool_button(ui, name, &format!("{name} the selected (or last) extrude"), false) {
                app.start_edge_command(name);
            }
        }
        ui.separator();
        for name in ["Box", "Cylinder", "Sphere"] {
            if tool_button(ui, name, &format!("Create a {}", name.to_lowercase()), false) {
                app.start_feature_command(name);
            }
        }
        ui.separator();
        if tool_button(ui, "Building", "Generate a building: storeys, windows, door, roof", app.gen.show_building) {
            app.gen.show_building = !app.gen.show_building;
        }
        if tool_button(ui, "Parts", "Parametric part library: bolts, nuts, gears, flanges…", app.gen.show_parts) {
            app.gen.show_parts = !app.gen.show_parts;
        }
        if tool_button(ui, "Image to Model", "Build a model from a sheet of views (front, top, side…)", app.image.open) {
            app.image.open = !app.image.open;
        }
        if tool_button(ui, "AI Copilot", "Show the AI copilot (Ctrl+L)", app.settings.show_copilot) {
            app.settings.show_copilot = !app.settings.show_copilot;
            app.settings.save();
        }
        ui.separator();
        if tool_button(ui, "ƒx Parameters", "Edit user parameters", app.show_params) {
            app.show_params = !app.show_params;
        }
    });
}

pub fn status_bar(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.label(&app.status);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let n_err = app.model.errors.len() + app.model.param_errors.len();
            if n_err > 0 {
                ui.label(RichText::new(format!("⚠ {n_err} error(s)")).color(ERROR));
            }
            ui.label(format!("{} bodies · {} features", app.model.bodies.len(), app.doc.features.len()));
            if let Mode::Sketch(s) = &app.mode {
                if let Some(c) = s.cursor {
                    ui.monospace(format!("{}, {} mm", fmt_num(c.x), fmt_num(c.y)));
                }
            }
        });
    });
}

// ---------------------------------------------------------------------- timeline

pub fn timeline(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("TIMELINE").small().strong());
        let n = app.doc.features.len();
        let active = app.doc.active_len();
        let busy = !matches!(app.mode, Mode::Idle);
        let mut set_marker: Option<usize> = None;
        ui.add_enabled_ui(!busy, |ui| {
            if ui.small_button("|◀").on_hover_text("Roll back to start").clicked() {
                set_marker = Some(0);
            }
            if ui.small_button("◀").on_hover_text("Step back").clicked() && active > 0 {
                set_marker = Some(active - 1);
            }
            if ui.small_button("▶").on_hover_text("Step forward").clicked() && active < n {
                set_marker = Some(active + 1);
            }
            if ui.small_button("▶|").on_hover_text("Roll forward to end").clicked() {
                set_marker = Some(n);
            }
        });
        ui.separator();

        let mut edit = None;
        let mut delete = None;
        let mut toggle_suppress = None;
        let mut rename_commit = false;
        egui::ScrollArea::horizontal().id_salt("tl").show(ui, |ui| {
            ui.horizontal(|ui| {
                for i in 0..=n {
                    if i == active {
                        let (r, _) = ui.allocate_exact_size(egui::vec2(6.0, 26.0), egui::Sense::hover());
                        ui.painter().rect_filled(r.shrink2(egui::vec2(1.0, 0.0)), 1.0, Color32::from_rgb(0xe0, 0x6c, 0x00));
                    }
                    if i == n {
                        break;
                    }
                    let f = &app.doc.features[i];
                    let id = f.id;
                    let rolled = i >= active;
                    let err = app.model.errors.get(&id);
                    let mut text = RichText::new(&f.name);
                    if f.suppressed {
                        text = text.strikethrough();
                    }
                    if rolled || f.suppressed {
                        text = text.weak();
                    }
                    if err.is_some() {
                        text = text.color(ERROR);
                    }
                    let selected = app.selected_feature == Some(id);
                    let resp = ui
                        .horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(egui::vec2(6.0, 20.0), egui::Sense::hover());
                            ui.painter().rect_filled(r, 2.0, feature_color(&f.kind));
                            ui.add(egui::SelectableLabel::new(selected, text))
                        })
                        .inner;
                    let tip = match err {
                        Some(e) => format!("{} — error: {e}\nDouble-click to edit, right-click for more", f.kind.type_name()),
                        None => format!("{}\nDouble-click to edit, right-click for more", f.kind.type_name()),
                    };
                    let resp = resp.on_hover_text(tip);
                    if resp.clicked() {
                        app.selected_feature = Some(id);
                    }
                    if resp.double_clicked() {
                        edit = Some(id);
                    }
                    resp.context_menu(|ui| {
                        if ui.button("Edit feature").clicked() {
                            edit = Some(id);
                            ui.close_menu();
                        }
                        ui.horizontal(|ui| {
                            ui.label("Name");
                            let f = &mut app.doc.features[i];
                            let r = ui.text_edit_singleline(&mut f.name);
                            if r.lost_focus() {
                                rename_commit = true;
                            }
                        });
                        let sup = app.doc.features[i].suppressed;
                        if ui.button(if sup { "Unsuppress" } else { "Suppress" }).clicked() {
                            toggle_suppress = Some(i);
                            ui.close_menu();
                        }
                        if ui.button("Roll timeline to here").clicked() {
                            set_marker = Some(i + 1);
                            ui.close_menu();
                        }
                        if ui.button(RichText::new("Delete").color(ERROR)).clicked() {
                            delete = Some(id);
                            ui.close_menu();
                        }
                    });
                }
            });
        });
        if rename_commit {
            app.dirty = true;
        }
        if let Some(m) = set_marker {
            if !busy {
                app.checkpoint();
                app.doc.rollback = if m >= n { None } else { Some(m) };
                app.rebuild();
            }
        }
        if let Some(i) = toggle_suppress {
            app.checkpoint();
            app.doc.features[i].suppressed ^= true;
            app.rebuild();
        }
        if let Some(id) = delete {
            app.delete_feature(id);
        }
        if let Some(id) = edit {
            app.edit_feature(id);
        }
    });
}

// ---------------------------------------------------------------------- browser

pub fn browser(app: &mut App, ui: &mut egui::Ui) {
    ui.add_space(4.0);
    ui.label(RichText::new("BROWSER").small().strong());
    ui.horizontal(|ui| {
        ui.label("Design");
        if ui.text_edit_singleline(&mut app.doc.name).changed() {
            app.dirty = true;
        }
    });
    ui.separator();
    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::CollapsingHeader::new("Document Settings").default_open(false).show(ui, |ui| {
            ui.label("Units: millimetres, degrees");
            if ui.link(format!("Parameters ({})", app.doc.parameters.len())).clicked() {
                app.show_params = true;
            }
        });
        egui::CollapsingHeader::new("Origin").default_open(false).show(ui, |ui| {
            ui.checkbox(&mut app.show_origin, "Show axes");
            ui.checkbox(&mut app.show_grid, "Show grid");
        });

        let mut changed_vis = false;
        egui::CollapsingHeader::new(format!("Bodies ({})", app.model.bodies.len()))
            .default_open(true)
            .show(ui, |ui| {
                let mut select = None;
                for (i, b) in app.model.bodies.iter().enumerate() {
                    ui.horizontal(|ui| {
                        let mut vis = !app.hidden_bodies.contains(&b.name);
                        if ui.checkbox(&mut vis, "").on_hover_text("Visibility").changed() {
                            if vis {
                                app.hidden_bodies.remove(&b.name);
                            } else {
                                app.hidden_bodies.insert(b.name.clone());
                            }
                            changed_vis = true;
                        }
                        if ui.selectable_label(app.selected_body == Some(i), &b.name).clicked() {
                            select = Some(i);
                        }
                    });
                }
                if let Some(i) = select {
                    app.selected_body = if app.selected_body == Some(i) { None } else { Some(i) };
                    changed_vis = true;
                }
            });
        if changed_vis {
            app.rebuild();
        }

        let sketches: Vec<_> = app.doc.features[..app.doc.active_len()]
            .iter()
            .filter(|f| matches!(f.kind, FeatureKind::Sketch(_)))
            .map(|f| (f.id, f.name.clone()))
            .collect();
        let mut edit = None;
        egui::CollapsingHeader::new(format!("Sketches ({})", sketches.len()))
            .default_open(true)
            .show(ui, |ui| {
                for (id, name) in sketches {
                    ui.horizontal(|ui| {
                        let mut vis = !app.hidden_sketches.contains(&id);
                        if ui.checkbox(&mut vis, "").on_hover_text("Visibility").changed() {
                            if vis {
                                app.hidden_sketches.remove(&id);
                            } else {
                                app.hidden_sketches.insert(id);
                            }
                        }
                        let r = ui.selectable_label(app.selected_feature == Some(id), &name);
                        if r.clicked() {
                            app.selected_feature = Some(id);
                        }
                        if r.double_clicked() {
                            edit = Some(id);
                        }
                        r.on_hover_text("Double-click to edit");
                    });
                }
            });
        if let Some(id) = edit {
            app.edit_feature(id);
        }
    });
}

// ---------------------------------------------------------------------- inspector

pub fn inspector(app: &mut App, ui: &mut egui::Ui) {
    ui.add_space(4.0);
    match &app.mode {
        Mode::ChoosePlane { .. } => plane_panel(app, ui),
        Mode::Sketch(_) => sketch_panel(app, ui),
        Mode::Command(_) => command_panel(app, ui),
        Mode::Idle => body_panel(app, ui),
    }
}

fn plane_panel(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Create Sketch");
    ui.label("Click an origin plane in the viewport, or pick one here.");
    ui.add_space(6.0);
    let vars = app.model.params.clone();
    let mut chosen = None;
    if let Mode::ChoosePlane { offset } = &mut app.mode {
        egui::Grid::new("plane").num_columns(2).show(ui, |ui| {
            expr_edit(ui, "Offset", offset, &vars, "mm");
        });
        ui.add_space(6.0);
        for k in PlaneKind::ALL {
            if ui.button(k.label()).clicked() {
                chosen = Some((k, offset.clone()));
            }
        }
    }
    ui.add_space(10.0);
    if ui.button("Cancel").clicked() {
        app.mode = Mode::Idle;
    }
    if let Some((k, off)) = chosen {
        app.create_sketch(k, off);
    }
}

fn sketch_panel(app: &mut App, ui: &mut egui::Ui) {
    let Mode::Sketch(s) = &app.mode else { return };
    let id = s.feature;
    let selected = s.selected;
    let name = app.doc.feature(id).map(|f| f.name.clone()).unwrap_or_default();
    ui.heading(format!("Editing {name}"));
    let vars = app.model.params.clone();
    let mut changed = false;
    let mut focus = false;
    let mut select = None;
    let mut delete = None;
    let mut region_count = 0;
    if let Some(g) = app.model.sketches.get(&id) {
        region_count = g.regions.len();
    }
    let errors: Vec<(usize, String)> =
        app.model.sketches.get(&id).map(|g| g.entity_errors.clone()).unwrap_or_default();
    let constraint_err = app.model.sketches.get(&id).and_then(|g| g.constraint_error.clone());
    let mut add_c: Option<Constraint> = None;
    let mut auto_c = false;
    let mut remove_c: Option<usize> = None;

    if let Some(cad_model::Feature { kind: FeatureKind::Sketch(sk), .. }) = app.doc.feature_mut(id) {
        egui::Grid::new("sk_plane").num_columns(2).show(ui, |ui| {
            ui.label("Plane");
            ui.label(sk.plane.label());
            ui.end_row();
            let (c, f) = expr_edit(ui, "Offset", &mut sk.offset, &vars, "mm");
            changed |= c;
            focus |= f;
        });
        ui.label(RichText::new(format!("{} closed profile(s)", region_count)).weak());
        ui.separator();
        ui.label(RichText::new("ENTITIES").small().strong());
        egui::ScrollArea::vertical().max_height(200.0).id_salt("ents").show(ui, |ui| {
            for (i, e) in sk.entities.iter().enumerate() {
                let err = errors.iter().any(|(k, _)| *k == i);
                let mut t = RichText::new(format!("{}. {}", i + 1, e.kind_name()));
                if err {
                    t = t.color(ERROR);
                }
                if ui.selectable_label(selected == Some(i), t).clicked() {
                    select = Some(i);
                }
            }
            if sk.entities.is_empty() {
                ui.label(RichText::new("Nothing drawn yet.").weak());
            }
        });
        ui.separator();
        if let Some(i) = selected.filter(|&i| i < sk.entities.len()) {
            ui.label(RichText::new(format!("ENTITY {}", i + 1)).small().strong());
            egui::Grid::new("ent").num_columns(2).show(ui, |ui| match &mut sk.entities[i] {
                Entity::Rectangle { x, y, width, height } => {
                    for (l, v) in [("X", x), ("Y", y), ("Width", width), ("Height", height)] {
                        let (c, f) = expr_edit(ui, l, v, &vars, "mm");
                        changed |= c;
                        focus |= f;
                    }
                }
                Entity::Circle { cx, cy, radius } => {
                    for (l, v) in [("Center X", cx), ("Center Y", cy), ("Radius", radius)] {
                        let (c, f) = expr_edit(ui, l, v, &vars, "mm");
                        changed |= c;
                        focus |= f;
                    }
                }
                Entity::Polyline { points, closed } => {
                    ui.label("Closed");
                    changed |= ui.checkbox(closed, "").changed();
                    ui.end_row();
                    for (k, p) in points.iter_mut().enumerate() {
                        ui.label(format!("P{}", k + 1));
                        ui.horizontal(|ui| {
                            for c in p.iter_mut() {
                                let r = ui.add(egui::DragValue::new(c).speed(0.5).suffix(" mm"));
                                changed |= r.changed();
                                focus |= r.drag_started() || r.gained_focus();
                            }
                        });
                        ui.end_row();
                    }
                }
            });
            if let Some((_, e)) = errors.iter().find(|(k, _)| *k == i) {
                ui.colored_label(ERROR, e);
            }
            if let Entity::Polyline { points, closed } = &sk.entities[i] {
                let n = points.len();
                let segs = if *closed { n } else { n.saturating_sub(1) };
                ui.label(RichText::new("CONSTRAIN SEGMENTS").small().strong());
                egui::ScrollArea::vertical().max_height(140.0).id_salt("segs").show(ui, |ui| {
                    for sgi in 0..segs {
                        ui.horizontal(|ui| {
                            let (a, b) = (points[sgi], points[(sgi + 1) % n]);
                            let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
                            let seg = SegRef { entity: i, segment: sgi };
                            ui.monospace(format!("s{:<2}", sgi + 1));
                            if ui.small_button("H").on_hover_text("Horizontal").clicked() {
                                add_c = Some(Constraint::Horizontal { seg });
                            }
                            if ui.small_button("V").on_hover_text("Vertical").clicked() {
                                add_c = Some(Constraint::Vertical { seg });
                            }
                            if ui.small_button(format!("L={}", fmt_num(len))).on_hover_text("Lock this length (edit it below)").clicked() {
                                add_c = Some(Constraint::Length { seg, value: fmt_num(len) });
                            }
                            if (sgi + 1 < segs || *closed) && ui.small_button("Perp").on_hover_text("Perpendicular to the next segment").clicked() {
                                add_c = Some(Constraint::Perpendicular { a: seg, b: SegRef { entity: i, segment: (sgi + 1) % n } });
                            }
                        });
                    }
                });
                if ui.small_button("Pin first point here").clicked() {
                    add_c = Some(Constraint::Position { p: PointRef { entity: i, index: 0 }, x: fmt_num(points[0][0]), y: fmt_num(points[0][1]) });
                }
            }
            if ui.button(RichText::new("Delete entity (Del)").color(ERROR)).clicked() {
                delete = Some(i);
            }
        } else {
            ui.label(RichText::new("Use Select (Esc) and click a curve to edit its dimensions. Values accept expressions like  width/2  or  1 in.").weak());
        }
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("CONSTRAINTS ({})", sk.constraints.len())).small().strong());
            if ui.small_button("Auto-constrain").on_hover_text("Make nearly horizontal/vertical lines exact and join touching ends").clicked() {
                auto_c = true;
            }
        });
        egui::ScrollArea::vertical().max_height(150.0).id_salt("cons").show(ui, |ui| {
            for (ci, c) in sk.constraints.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    let label = c.label();
                    match c {
                        Constraint::Length { value, .. } | Constraint::Angle { value, .. } => {
                            ui.label(label.split(" = ").next().unwrap_or("").to_string());
                            let r = ui.add(egui::TextEdit::singleline(value).desired_width(70.0).font(egui::TextStyle::Monospace));
                            changed |= r.changed();
                            focus |= r.gained_focus();
                        }
                        Constraint::Position { x, y, .. } => {
                            ui.label(label.split(" = ").next().unwrap_or("").to_string());
                            for v in [x, y] {
                                let r = ui.add(egui::TextEdit::singleline(v).desired_width(50.0).font(egui::TextStyle::Monospace));
                                changed |= r.changed();
                                focus |= r.gained_focus();
                            }
                        }
                        _ => {
                            ui.label(label);
                        }
                    }
                    if ui.small_button("x").on_hover_text("Remove constraint").clicked() {
                        remove_c = Some(ci);
                    }
                });
            }
        });
        if let Some(e) = &constraint_err {
            ui.colored_label(ERROR, e);
        }
    }
    if add_c.is_some() || auto_c || remove_c.is_some() {
        app.checkpoint();
        if let Some(cad_model::Feature { kind: FeatureKind::Sketch(sk), .. }) = app.doc.feature_mut(id) {
            if let Some(c) = add_c {
                if !sk.constraints.contains(&c) {
                    sk.constraints.push(c);
                }
            }
            if auto_c {
                let add = cad_model::constraints::auto_constraints(&sk.polylines(), &sk.constraints, 8.0);
                sk.constraints.extend(add);
            }
            if let Some(ci) = remove_c {
                sk.constraints.remove(ci);
            }
        }
        changed = true;
    }
    if focus {
        // record an undo step when an edit begins
        let snapshot = app.doc.clone();
        app.undo.push(snapshot);
        app.redo.clear();
    }
    if changed {
        app.dirty = true;
        app.rebuild();
    }
    if let Some(i) = select {
        if let Mode::Sketch(s) = &mut app.mode {
            s.selected = Some(i);
            s.tool = SketchTool::Select;
        }
    }
    if let Some(i) = delete {
        if let Mode::Sketch(s) = &mut app.mode {
            s.selected = Some(i);
        }
        crate::viewport::delete_selected_entity(app);
    }
    ui.add_space(10.0);
    if ui.button("✔ Finish Sketch").clicked() {
        app.finish_sketch();
    }
}

fn command_panel(app: &mut App, ui: &mut egui::Ui) {
    let Mode::Command(c) = &app.mode else { return };
    let id = c.feature;
    let is_new = c.is_new;
    let vars = app.model.params.clone();
    let err = app.model.errors.get(&id).cloned();
    let sketch_names: HashMap<_, _> = app.doc.features.iter().map(|f| (f.id, f.name.clone())).collect();
    let mut changed = false;
    let Some(f) = app.doc.feature_mut(id) else { return };
    ui.heading(format!("{} {}", if is_new { "New" } else { "Edit" }, f.kind.type_name()));
    ui.horizontal(|ui| {
        ui.label("Name");
        ui.text_edit_singleline(&mut f.name);
    });
    ui.separator();
    egui::Grid::new("cmd").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
        let ex = |ui: &mut egui::Ui, l: &str, v: &mut String, unit: &str| expr_edit(ui, l, v, &vars, unit).0;
        match &mut f.kind {
            FeatureKind::Extrude { sketch, profiles, distance, extent, flip, op, edges } => {
                ui.label("Profiles");
                ui.label(format!(
                    "{} selected in {}",
                    profiles.len(),
                    sketch_names.get(sketch).cloned().unwrap_or_default()
                ));
                ui.end_row();
                ui.label("Extent");
                ui.horizontal(|ui| {
                    changed |= ui.selectable_value(extent, ExtentKind::OneSide, "One side").changed();
                    changed |= ui.selectable_value(extent, ExtentKind::Symmetric, "Symmetric").changed();
                });
                ui.end_row();
                changed |= ex(ui, "Distance", distance, "mm");
                ui.label("Flip direction");
                changed |= ui.checkbox(flip, "").changed();
                ui.end_row();
                changed |= op_combo(ui, op);
                ui.label("Edges");
                ui.horizontal(|ui| {
                    let cur = edges.as_ref().map(|e| e.kind);
                    for (label, k) in [("Sharp", None), ("Fillet", Some(EdgeKind::Fillet)), ("Chamfer", Some(EdgeKind::Chamfer))] {
                        if ui.selectable_label(cur == k, label).clicked() && cur != k {
                            *edges = k.map(|kind| EdgeTreatment {
                                kind,
                                size: edges.as_ref().map(|e| e.size.clone()).unwrap_or_else(|| "1 mm".into()),
                                start: edges.as_ref().map_or(false, |e| e.start),
                                end: edges.as_ref().map_or(true, |e| e.end),
                            });
                            changed = true;
                        }
                    }
                });
                ui.end_row();
                if let Some(e) = edges {
                    changed |= ex(ui, "Edge size", &mut e.size, "mm");
                    ui.label("Apply at");
                    ui.horizontal(|ui| {
                        changed |= ui.checkbox(&mut e.start, "start cap").changed();
                        changed |= ui.checkbox(&mut e.end, "end cap").changed();
                    });
                    ui.end_row();
                }
            }
            FeatureKind::Shell { target, thickness, open } => {
                ui.label("Body from");
                ui.label(sketch_names.get(target).cloned().unwrap_or_default());
                ui.end_row();
                changed |= ex(ui, "Thickness", thickness, "mm");
                ui.label("Open face");
                ui.horizontal(|ui| {
                    for o in ["end", "start", "none"] {
                        changed |= ui.selectable_value(open, o.to_string(), o).changed();
                    }
                });
                ui.end_row();
            }
            FeatureKind::Revolve { sketch, profiles, axis, angle, op } => {
                ui.label("Profiles");
                ui.label(format!(
                    "{} selected in {}",
                    profiles.len(),
                    sketch_names.get(sketch).cloned().unwrap_or_default()
                ));
                ui.end_row();
                ui.label("Axis");
                ui.horizontal(|ui| {
                    changed |= ui.selectable_value(axis, RevolveAxis::SketchU, "Sketch horizontal").changed();
                    changed |= ui.selectable_value(axis, RevolveAxis::SketchV, "Sketch vertical").changed();
                });
                ui.end_row();
                changed |= ex(ui, "Angle", angle, "°");
                changed |= op_combo(ui, op);
            }
            FeatureKind::Box { corner, size, op } => {
                for (l, v) in ["Corner X", "Corner Y", "Corner Z"].iter().zip(corner.iter_mut()) {
                    changed |= ex(ui, l, v, "mm");
                }
                for (l, v) in ["Length (X)", "Height (Y)", "Width (Z)"].iter().zip(size.iter_mut()) {
                    changed |= ex(ui, l, v, "mm");
                }
                changed |= op_combo(ui, op);
            }
            FeatureKind::Cylinder { base, axis, radius, height, op } => {
                for (l, v) in ["Base X", "Base Y", "Base Z"].iter().zip(base.iter_mut()) {
                    changed |= ex(ui, l, v, "mm");
                }
                ui.label("Axis");
                ui.horizontal(|ui| {
                    for (a, n) in [(Axis::X, "X"), (Axis::Y, "Y"), (Axis::Z, "Z")] {
                        changed |= ui.selectable_value(axis, a, n).changed();
                    }
                });
                ui.end_row();
                changed |= ex(ui, "Radius", radius, "mm");
                changed |= ex(ui, "Height", height, "mm");
                changed |= op_combo(ui, op);
            }
            FeatureKind::Sphere { center, radius, op } => {
                for (l, v) in ["Center X", "Center Y", "Center Z"].iter().zip(center.iter_mut()) {
                    changed |= ex(ui, l, v, "mm");
                }
                changed |= ex(ui, "Radius", radius, "mm");
                changed |= op_combo(ui, op);
            }
            FeatureKind::Sketch(_) => {}
        }
    });
    if matches!(f.kind, FeatureKind::Extrude { .. } | FeatureKind::Revolve { .. }) {
        ui.label(RichText::new("Click profiles in the viewport to add or remove them.").weak());
    }
    if let Some(e) = err {
        ui.add_space(4.0);
        ui.colored_label(ERROR, format!("⚠ {e}"));
    }
    ui.add_space(10.0);
    let mut ok = false;
    let mut cancel = false;
    ui.horizontal(|ui| {
        ok = ui.add(egui::Button::new(RichText::new("OK").color(Color32::WHITE)).fill(ACCENT)).clicked();
        cancel = ui.button("Cancel").clicked();
    });
    if changed {
        app.rebuild();
    }
    if ok {
        app.commit_command();
    } else if cancel {
        app.cancel_mode();
    }
}

fn body_panel(app: &mut App, ui: &mut egui::Ui) {
    let Some(i) = app.selected_body else { return };
    let Some(b) = app.model.bodies.get(i) else { return };
    ui.heading(&b.name);
    let creator = app.doc.feature(b.created_by).map(|f| f.name.clone()).unwrap_or_default();
    egui::Grid::new("props").num_columns(2).show(ui, |ui| {
        ui.label("Created by");
        ui.label(creator);
        ui.end_row();
        ui.label("Volume");
        ui.label(format!("{} cm³", fmt_num(b.solid.volume() / 1000.0)));
        ui.end_row();
        ui.label("Surface area");
        ui.label(format!("{} cm²", fmt_num(b.solid.area() / 100.0)));
        ui.end_row();
        if let Some((lo, hi)) = b.solid.bounds() {
            let s = hi - lo;
            ui.label("Bounding box");
            ui.label(format!("{} × {} × {} mm", fmt_num(s.x), fmt_num(s.y), fmt_num(s.z)));
            ui.end_row();
        }
        ui.label("Facets");
        ui.label(format!("{}", b.solid.polygons.len()));
        ui.end_row();
    });
    ui.add_space(6.0);
    ui.label(RichText::new("Mass estimate (steel, 7.85 g/cm³)").weak());
    ui.label(format!("{} g", fmt_num(b.solid.volume() / 1000.0 * 7.85)));
}

// ---------------------------------------------------------------------- windows

pub fn parameters_window(app: &mut App, ctx: &egui::Context) {
    if !app.show_params {
        return;
    }
    let mut open = true;
    let mut changed = false;
    let mut focus = false;
    let mut remove = None;
    egui::Window::new("Parameters").open(&mut open).default_width(560.0).show(ctx, |ui| {
        ui.label(RichText::new("Use parameter names in any value field, e.g.  width / 2 + 3 mm").weak());
        ui.separator();
        egui::Grid::new("params").num_columns(5).striped(true).spacing([10.0, 6.0]).show(ui, |ui| {
            for h in ["Name", "Expression", "Value", "Comment", ""] {
                ui.label(RichText::new(h).strong());
            }
            ui.end_row();
            for (i, p) in app.doc.parameters.iter_mut().enumerate() {
                let r = ui.add(egui::TextEdit::singleline(&mut p.name).desired_width(110.0));
                changed |= r.changed();
                focus |= r.gained_focus();
                let r = ui.add(egui::TextEdit::singleline(&mut p.expr).desired_width(140.0).font(egui::TextStyle::Monospace));
                changed |= r.changed();
                focus |= r.gained_focus();
                match (app.model.params.get(&p.name), app.model.param_errors.get(&p.name)) {
                    (_, Some(e)) => ui.label(RichText::new("⚠ error").color(ERROR)).on_hover_text(e),
                    (Some(v), _) => ui.label(fmt_num(*v)),
                    _ => ui.label("—"),
                };
                let r = ui.add(egui::TextEdit::singleline(&mut p.comment).desired_width(140.0));
                changed |= r.changed();
                focus |= r.gained_focus();
                if ui.small_button("🗑").on_hover_text("Delete").clicked() {
                    remove = Some(i);
                }
                ui.end_row();
            }
        });
        ui.add_space(6.0);
        if ui.button("+ Add parameter").clicked() {
            app.checkpoint();
            let n = (1..).map(|i| format!("p{i}")).find(|n| !app.doc.parameters.iter().any(|p| &p.name == n)).unwrap();
            app.doc.parameters.push(Parameter { name: n, expr: "10 mm".into(), comment: String::new() });
            changed = true;
        }
    });
    if focus {
        app.checkpoint();
    }
    if let Some(i) = remove {
        app.checkpoint();
        app.doc.parameters.remove(i);
        changed = true;
    }
    if changed {
        app.dirty = true;
        app.rebuild();
    }
    app.show_params = open;
}

pub fn help_window(app: &mut App, ctx: &egui::Context) {
    let mut open = app.show_help;
    egui::Window::new("Controls & shortcuts").open(&mut open).show(ctx, |ui| {
        egui::Grid::new("help").num_columns(2).striped(true).show(ui, |ui| {
            for (k, v) in [
                ("Left drag / Right drag", "Orbit"),
                ("Middle drag / Shift+Left drag", "Pan"),
                ("Scroll wheel", "Zoom at cursor"),
                ("F", "Fit all"),
                ("S", "Create sketch"),
                ("E", "Extrude"),
                ("L / R / C", "Line / Rectangle / Circle (in a sketch)"),
                ("Double-click / Right-click", "End polyline"),
                ("Click start point", "Close polyline into a profile"),
                ("Esc", "Cancel / Select tool / Finish sketch"),
                ("Enter", "OK / Finish sketch"),
                ("Delete", "Delete selected entity or feature"),
                ("Ctrl+Z / Ctrl+Y", "Undo / Redo"),
                ("Ctrl+S / Ctrl+O", "Save / Open"),
            ] {
                ui.monospace(k);
                ui.label(v);
                ui.end_row();
            }
        });
    });
    app.show_help = open;
}
