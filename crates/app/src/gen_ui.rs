//! Windows for the building generator and the parametric part library.

use crate::app::App;
use cad_model::commands::{Command, Ex};
use cad_model::macros::PART_KINDS;
use eframe::egui::{self, RichText};
use serde_json::{Map, Value};

pub struct GenState {
    pub show_building: bool,
    pub show_parts: bool,
    // building
    pub width: f64,
    pub depth: f64,
    pub floors: u32,
    pub floor_h: f64,
    pub wall: f64,
    pub gable: bool,
    pub roof_h: f64,
    pub win_front: u32,
    pub win_side: u32,
    pub win_w: f64,
    pub win_h: f64,
    pub replace: bool,
    // parts
    pub kind: usize,
    pub size: String,
    pub values: Vec<(String, f64)>,
    pub pos: [f64; 3],
    pub log: String,
}

impl Default for GenState {
    fn default() -> Self {
        let mut g = GenState {
            show_building: false,
            show_parts: false,
            width: 12000.0,
            depth: 8000.0,
            floors: 2,
            floor_h: 3000.0,
            wall: 300.0,
            gable: true,
            roof_h: 2500.0,
            win_front: 4,
            win_side: 2,
            win_w: 1200.0,
            win_h: 1400.0,
            replace: true,
            kind: 0,
            size: "M8".into(),
            values: Vec::new(),
            pos: [0.0; 3],
            log: String::new(),
        };
        g.values = defaults(PART_KINDS[0].0);
        g
    }
}

fn defaults(kind: &str) -> Vec<(String, f64)> {
    let v = |pairs: &[(&str, f64)]| pairs.iter().map(|(k, x)| (k.to_string(), *x)).collect();
    match kind {
        "bolt" => v(&[("length", 40.0)]),
        "nut" | "washer" => v(&[]),
        "spur_gear" => v(&[("module", 2.0), ("teeth", 20.0), ("thickness", 10.0), ("bore", 10.0)]),
        "flange" => v(&[("outer_d", 120.0), ("thickness", 12.0), ("pcd", 90.0), ("holes", 6.0), ("hole_d", 9.0), ("bore", 48.0)]),
        "tube" => v(&[("outer_d", 40.0), ("inner_d", 32.0), ("length", 100.0)]),
        "l_bracket" => v(&[("width", 60.0), ("height", 50.0), ("depth", 40.0), ("thickness", 6.0), ("hole_d", 8.5)]),
        "enclosure" => v(&[("length", 120.0), ("width", 80.0), ("height", 50.0), ("wall", 2.5)]),
        "stairs" => v(&[("steps", 12.0), ("rise", 175.0), ("run", 280.0), ("width", 1000.0)]),
        _ => Vec::new(),
    }
}

fn report(app: &mut App, rep: crate::app::RunReport) -> String {
    for l in &rep.lines {
        app.copilot.push(crate::copilot::Role::Action, format!("✔ {l}"));
    }
    if rep.errors.is_empty() {
        app.fit_pending = true;
        rep.lines.last().cloned().unwrap_or_default()
    } else {
        rep.errors.join("; ")
    }
}

pub fn windows(app: &mut App, ctx: &egui::Context) {
    building_window(app, ctx);
    parts_window(app, ctx);
}

fn building_window(app: &mut App, ctx: &egui::Context) {
    if !app.gen.show_building {
        return;
    }
    let mut open = true;
    let mut create = false;
    egui::Window::new("Building").open(&mut open).resizable(false).show(ctx, |ui| {
        let g = &mut app.gen;
        egui::Grid::new("bldg").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            let row = |ui: &mut egui::Ui, label: &str, v: &mut f64, lo: f64, hi: f64| {
                ui.label(label);
                ui.add(egui::DragValue::new(v).range(lo..=hi).speed(50.0).suffix(" mm"));
                ui.end_row();
            };
            row(ui, "Width (X)", &mut g.width, 2000.0, 200000.0);
            row(ui, "Depth (Z)", &mut g.depth, 2000.0, 200000.0);
            ui.label("Storeys");
            ui.add(egui::DragValue::new(&mut g.floors).range(1..=60));
            ui.end_row();
            row(ui, "Storey height", &mut g.floor_h, 2000.0, 10000.0);
            row(ui, "Wall thickness", &mut g.wall, 50.0, 1000.0);
            ui.label("Roof");
            ui.horizontal(|ui| {
                ui.selectable_value(&mut g.gable, true, "Gable");
                ui.selectable_value(&mut g.gable, false, "Flat");
            });
            ui.end_row();
            if g.gable {
                row(ui, "Roof rise", &mut g.roof_h, 300.0, 20000.0);
            }
            ui.label("Windows front / side");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut g.win_front).range(0..=30));
                ui.add(egui::DragValue::new(&mut g.win_side).range(0..=30));
            });
            ui.end_row();
            row(ui, "Window width", &mut g.win_w, 300.0, 6000.0);
            row(ui, "Window height", &mut g.win_h, 300.0, 4000.0);
        });
        ui.checkbox(&mut g.replace, "Start a new design");
        ui.label(RichText::new("Every size becomes a parameter (bldg1_width, bldg1_floor_h, …) you can change later.").small().weak());
        create = ui.button("Create building").clicked();
        if !g.log.is_empty() {
            ui.label(RichText::new(&g.log).small());
        }
    });
    if create {
        let g = &app.gen;
        let mut cmds = Vec::new();
        if g.replace {
            cmds.push(Command::ClearDesign);
        }
        let mm = |v: f64| Some(Ex(format!("{v} mm")));
        cmds.push(Command::Building {
            width: mm(g.width),
            depth: mm(g.depth),
            floors: Some(g.floors),
            floor_height: mm(g.floor_h),
            wall_thickness: mm(g.wall),
            roof: Some(if g.gable { "gable" } else { "flat" }.into()),
            roof_height: mm(g.roof_h),
            windows_front: Some(g.win_front),
            windows_side: Some(g.win_side),
            window_width: mm(g.win_w),
            window_height: mm(g.win_h),
        });
        let rep = app.run_commands(&cmds, "building");
        app.gen.log = report(app, rep);
    }
    app.gen.show_building = open;
}

fn parts_window(app: &mut App, ctx: &egui::Context) {
    if !app.gen.show_parts {
        return;
    }
    let mut open = true;
    let mut insert = false;
    egui::Window::new("Part Library").open(&mut open).resizable(false).show(ctx, |ui| {
        let g = &mut app.gen;
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.set_width(150.0);
                for (i, (k, _)) in PART_KINDS.iter().enumerate() {
                    if ui.selectable_label(g.kind == i, k.replace('_', " ")).clicked() && g.kind != i {
                        g.kind = i;
                        g.values = defaults(k);
                    }
                }
            });
            ui.separator();
            ui.vertical(|ui| {
                let (kind, desc) = PART_KINDS[g.kind];
                ui.label(RichText::new(desc).strong());
                egui::Grid::new("part").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                    if matches!(kind, "bolt" | "nut" | "washer") {
                        ui.label("Size");
                        egui::ComboBox::from_id_salt("msize").selected_text(&g.size).show_ui(ui, |ui| {
                            for s in ["M3", "M4", "M5", "M6", "M8", "M10", "M12", "M16", "M20", "M24"] {
                                ui.selectable_value(&mut g.size, s.to_string(), s);
                            }
                        });
                        ui.end_row();
                    }
                    for (k, v) in g.values.iter_mut() {
                        ui.label(k.replace('_', " "));
                        let counts = matches!(k.as_str(), "teeth" | "holes" | "steps");
                        ui.add(egui::DragValue::new(v).speed(if counts { 0.2 } else { 0.5 }).suffix(if counts { "" } else { " mm" }));
                        ui.end_row();
                    }
                    ui.label("Position X Y Z");
                    ui.horizontal(|ui| {
                        for c in g.pos.iter_mut() {
                            ui.add(egui::DragValue::new(c).speed(1.0));
                        }
                    });
                    ui.end_row();
                });
                insert = ui.button("Insert part").clicked();
                if !g.log.is_empty() {
                    ui.label(RichText::new(&g.log).small());
                }
            });
        });
    });
    if insert {
        let g = &app.gen;
        let kind = PART_KINDS[g.kind].0.to_string();
        let mut params = Map::new();
        if matches!(kind.as_str(), "bolt" | "nut" | "washer") {
            params.insert("size".into(), Value::String(g.size.clone()));
        }
        for (k, v) in &g.values {
            params.insert(k.clone(), serde_json::json!(v));
        }
        let rep = app.run_commands(&[Command::Part { kind, params, position: Some(g.pos) }], "parts");
        app.gen.log = report(app, rep);
    }
    app.gen.show_parts = open;
}
