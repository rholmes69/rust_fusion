//! High-level generators that expand into ordinary editing commands, so everything they
//! create stays in the timeline and is driven by named parameters:
//! buildings (walls, slabs, windows, doors, flat or gable roof), single walls and openings,
//! and a library of parametric parts (bolt, nut, washer, spur gear, flange, tube,
//! L-bracket, enclosure, stairs).

use crate::commands::{Command, Ex};
use crate::constraints::{Constraint, PointRef};
use serde_json::{Map, Value};
use std::collections::HashMap;

fn ex(s: impl Into<String>) -> Ex {
    Ex(s.into())
}

fn param(name: &str, value: impl Into<String>, comment: &str) -> Command {
    Command::SetParameter { name: name.into(), value: ex(value), comment: Some(comment.into()) }
}

fn num(v: f64) -> String {
    let s = format!("{v:.4}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// First unused prefix like "bolt1".
pub fn unique_prefix(base: &str, used: &[String]) -> String {
    (1..).map(|i| format!("{base}{i}")).find(|p| !used.iter().any(|u| u.starts_with(&format!("{p}_")))).unwrap()
}

/// A closed polygon whose vertices are pinned by expression constraints (fully parametric).
fn param_polygon(sketch_entities_before: usize, pts: &[(String, String)], initial: &[[f64; 2]]) -> Vec<Command> {
    let mut out = vec![Command::AddPolygon { sketch: None, points: initial.to_vec(), closed: true }];
    for (i, (x, y)) in pts.iter().enumerate() {
        out.push(Command::AddConstraint {
            sketch: None,
            constraint: Constraint::Position { p: PointRef { entity: sketch_entities_before, index: i }, x: x.clone(), y: y.clone() },
        });
    }
    out
}

// ---------------------------------------------------------------------- building

#[derive(Debug, Clone)]
pub struct BuildingSpec {
    pub width: String,
    pub depth: String,
    pub floors: u32,
    pub floor_height: String,
    pub wall: String,
    pub slab: String,
    pub roof: String,
    pub roof_height: String,
    pub window_width: String,
    pub window_height: String,
    pub sill: String,
    pub door_width: String,
    pub door_height: String,
    pub windows_front: u32,
    pub windows_side: u32,
}

impl Default for BuildingSpec {
    fn default() -> Self {
        BuildingSpec {
            width: "12000".into(),
            depth: "8000".into(),
            floors: 2,
            floor_height: "3000".into(),
            wall: "300".into(),
            slab: "250".into(),
            roof: "gable".into(),
            roof_height: "2500".into(),
            window_width: "1200".into(),
            window_height: "1400".into(),
            sill: "900".into(),
            door_width: "1000".into(),
            door_height: "2200".into(),
            windows_front: 4,
            windows_side: 2,
        }
    }
}

/// Expand a building into parameters + features. `p` is the parameter prefix (e.g. "bldg1").
pub fn building(spec: &BuildingSpec, p: &str, vars: &HashMap<String, f64>) -> Vec<Command> {
    let n = |s: &str| format!("{p}_{s}");
    let f = spec.floors.max(1);
    let mut c = vec![
        param(&n("width"), spec.width.clone(), "building width (X)"),
        param(&n("depth"), spec.depth.clone(), "building depth (Z)"),
        param(&n("floors"), f.to_string(), "number of storeys"),
        param(&n("floor_h"), spec.floor_height.clone(), "storey height"),
        param(&n("wall"), spec.wall.clone(), "wall thickness"),
        param(&n("slab"), spec.slab.clone(), "floor slab thickness"),
        param(&n("roof_h"), spec.roof_height.clone(), "roof rise"),
        param(&n("win_w"), spec.window_width.clone(), "window width"),
        param(&n("win_h"), spec.window_height.clone(), "window height"),
        param(&n("sill"), spec.sill.clone(), "window sill height"),
        param(&n("door_w"), spec.door_width.clone(), "door width"),
        param(&n("door_h"), spec.door_height.clone(), "door height"),
    ];
    let (w, d, h, t, s) = (n("width"), n("depth"), n("floor_h"), n("wall"), n("slab"));
    // shell
    c.push(Command::Box {
        corner: [ex(format!("-{w}/2")), ex("0"), ex(format!("-{d}/2"))],
        size: [ex(w.clone()), ex(format!("{} * {h}", n("floors"))), ex(d.clone())],
        operation: Some("new_body".into()),
        name: Some(format!("{p} shell")),
    });
    for i in 0..f {
        c.push(Command::Box {
            corner: [ex(format!("-{w}/2 + {t}")), ex(format!("{i} * {h} + {s}")), ex(format!("-{d}/2 + {t}"))],
            size: [ex(format!("{w} - 2*{t}")), ex(format!("{h} - {s}")), ex(format!("{d} - 2*{t}"))],
            operation: Some("cut".into()),
            name: Some(format!("{p} storey {}", i + 1)),
        });
    }
    // facades: windows on every storey, a door in the middle of the front
    let facades = [
        ("front", "XY", format!("{d}/2"), w.clone(), spec.windows_front, true),
        ("back", "XY", format!("-{d}/2"), w.clone(), spec.windows_front, false),
        ("right", "YZ", format!("{w}/2"), d.clone(), spec.windows_side, false),
        ("left", "YZ", format!("-{w}/2"), d.clone(), spec.windows_side, false),
    ];
    let door_w = vars.get("__door_w").copied().unwrap_or(1000.0);
    let _ = door_w;
    for (side, plane, offset, len, count, door) in facades {
        if count == 0 && !door {
            continue;
        }
        c.push(Command::NewSketch { name: Some(format!("{p} {side} openings")), plane: plane.into(), offset: ex(offset) });
        for i in 0..f {
            for k in 0..count {
                let frac = (k as f64 + 0.5) / count as f64;
                // on the ground floor of the front, skip windows that would hit the door
                if door && i == 0 && count % 2 == 1 && k == count / 2 {
                    continue;
                }
                c.push(Command::AddRectangle {
                    sketch: None,
                    x: ex(format!("-{len}/2 + {len} * {} - {}/2", num(frac), n("win_w"))),
                    y: ex(format!("{i} * {h} + {s} + {}", n("sill"))),
                    width: ex(n("win_w")),
                    height: ex(n("win_h")),
                });
            }
        }
        if door {
            c.push(Command::AddRectangle {
                sketch: None,
                x: ex(format!("-{}/2", n("door_w"))),
                y: ex(s.clone()),
                width: ex(n("door_w")),
                height: ex(n("door_h")),
            });
        }
        c.push(Command::Extrude {
            sketch: None,
            profiles: None,
            distance: ex(format!("2 * {t} + 200")),
            symmetric: true,
            flip: false,
            operation: Some("cut".into()),
            name: Some(format!("{p} {side} openings cut")),
        });
    }
    // roof
    let top = format!("{} * {h}", n("floors"));
    if spec.roof.eq_ignore_ascii_case("gable") {
        let rh = n("roof_h");
        // YZ sketch: x = -Z, y = Y. Eaves overhang 300 mm.
        let pts = vec![
            (format!("-{d}/2 - 300"), top.clone()),
            (format!("{d}/2 + 300"), top.clone()),
            ("0".to_string(), format!("{top} + {rh}")),
        ];
        let ev = |e: &str| crate::commands::eval_num(e, vars).unwrap_or(0.0);
        let init: Vec<[f64; 2]> = pts.iter().map(|(x, y)| [ev(x), ev(y)]).collect();
        c.push(Command::NewSketch { name: Some(format!("{p} roof profile")), plane: "YZ".into(), offset: ex("0") });
        c.extend(param_polygon(0, &pts, &init));
        c.push(Command::Extrude {
            sketch: None,
            profiles: None,
            distance: ex(format!("{w} + 600")),
            symmetric: true,
            flip: false,
            operation: Some("join".into()),
            name: Some(format!("{p} gable roof")),
        });
    } else {
        c.push(Command::Box {
            corner: [ex(format!("-{w}/2 - 200")), ex(top.clone()), ex(format!("-{d}/2 - 200"))],
            size: [ex(format!("{w} + 400")), ex(s.clone()), ex(format!("{d} + 400"))],
            operation: Some("join".into()),
            name: Some(format!("{p} flat roof")),
        });
    }
    c
}

/// A straight wall in plan from (x1, z1) to (x2, z2).
pub fn wall(from: [f64; 2], to: [f64; 2], height: &str, thickness: f64, base: &str, op: &str, name: &str) -> Result<Vec<Command>, String> {
    let (a, b) = (glam::DVec2::new(from[0], from[1]), glam::DVec2::new(to[0], to[1]));
    let dir = (b - a).normalize_or_zero();
    if dir == glam::DVec2::ZERO {
        return Err("wall start and end are the same point".into());
    }
    let nrm = glam::DVec2::new(-dir.y, dir.x) * thickness / 2.0;
    // plan (x, z) → XZ sketch (x, -z)
    let pts = [a + nrm, b + nrm, b - nrm, a - nrm].map(|p| [p.x, -p.y]);
    Ok(vec![
        Command::NewSketch { name: Some(format!("{name} plan")), plane: "XZ".into(), offset: ex(base) },
        Command::AddPolygon { sketch: None, points: pts.to_vec(), closed: true },
        Command::Extrude {
            sketch: None,
            profiles: None,
            distance: ex(height),
            symmetric: false,
            flip: false,
            operation: Some(op.into()),
            name: Some(name.into()),
        },
    ])
}

/// A rectangular opening (window/door) cut through walls: centred at plan (x, z), `width`
/// along the wall direction `angle_deg` (0 = along X), from `sill` up `height`.
pub fn opening(x: f64, z: f64, width: f64, depth: f64, sill: &str, height: &str, angle_deg: f64, name: &str) -> Vec<Command> {
    let a = angle_deg.to_radians();
    let u = glam::DVec2::new(a.cos(), a.sin()) * width / 2.0;
    let v = glam::DVec2::new(-a.sin(), a.cos()) * depth / 2.0;
    let c = glam::DVec2::new(x, z);
    let pts = [c - u - v, c + u - v, c + u + v, c - u + v].map(|p| [p.x, -p.y]);
    vec![
        Command::NewSketch { name: Some(format!("{name} plan")), plane: "XZ".into(), offset: ex(sill) },
        Command::AddPolygon { sketch: None, points: pts.to_vec(), closed: true },
        Command::Extrude {
            sketch: None,
            profiles: None,
            distance: ex(height),
            symmetric: false,
            flip: false,
            operation: Some("cut".into()),
            name: Some(name.into()),
        },
    ]
}

// ---------------------------------------------------------------------- part library

pub const PART_KINDS: &[(&str, &str)] = &[
    ("bolt", "Hex bolt — size (M3…M24), length"),
    ("nut", "Hex nut — size (M3…M24)"),
    ("washer", "Flat washer — size (M3…M24)"),
    ("spur_gear", "Spur gear — module, teeth, thickness, bore"),
    ("flange", "Flange — outer_d, thickness, pcd, holes, hole_d, bore"),
    ("tube", "Tube — outer_d, inner_d, length"),
    ("l_bracket", "L-bracket — width, height, depth, thickness, hole_d"),
    ("enclosure", "Open box — length, width, height, wall"),
    ("stairs", "Stairs — steps, rise, run, width"),
];

/// Metric coarse thread data: (d, across-flats s, head height k, nut height m, washer OD, washer t).
fn metric(size: &str) -> Option<(f64, f64, f64, f64, f64, f64)> {
    Some(match size.to_ascii_uppercase().trim() {
        "M3" => (3.0, 5.5, 2.0, 2.4, 7.0, 0.5),
        "M4" => (4.0, 7.0, 2.8, 3.2, 9.0, 0.8),
        "M5" => (5.0, 8.0, 3.5, 4.7, 10.0, 1.0),
        "M6" => (6.0, 10.0, 4.0, 5.2, 12.0, 1.6),
        "M8" => (8.0, 13.0, 5.3, 6.8, 16.0, 1.6),
        "M10" => (10.0, 16.0, 6.4, 8.4, 20.0, 2.0),
        "M12" => (12.0, 18.0, 7.5, 10.8, 24.0, 2.5),
        "M16" => (16.0, 24.0, 10.0, 14.8, 30.0, 3.0),
        "M20" => (20.0, 30.0, 12.5, 18.0, 37.0, 3.0),
        "M24" => (24.0, 36.0, 15.0, 21.5, 44.0, 4.0),
        _ => return None,
    })
}

/// Involute spur gear outline (sketch coordinates).
pub fn gear_outline(module: f64, teeth: u32, pressure_deg: f64) -> Vec<[f64; 2]> {
    let z = teeth as f64;
    let rp = module * z / 2.0;
    let rb = rp * pressure_deg.to_radians().cos();
    let ra = rp + module;
    let rf = (rp - 1.25 * module).max(rb * 0.9);
    let inv = |a: f64| a.tan() - a;
    let alpha = pressure_deg.to_radians();
    // half tooth thickness angle at the pitch circle
    let half = std::f64::consts::PI / (2.0 * z) + inv(alpha);
    let samples = 6;
    let mut pts = Vec::new();
    for k in 0..teeth {
        let c = k as f64 * std::f64::consts::TAU / z;
        let flank = |sign: f64| -> Vec<[f64; 2]> {
            (0..=samples)
                .map(|i| {
                    let r = rb.max(rf) + (ra - rb.max(rf)) * i as f64 / samples as f64;
                    let a = (rb / r).min(1.0).acos();
                    let th = c + sign * (half - inv(a));
                    [r * th.cos(), r * th.sin()]
                })
                .collect()
        };
        let mut right = flank(-1.0);
        let left = flank(1.0);
        // root point before the tooth
        let gap = c - std::f64::consts::PI / z;
        pts.push([rf * gap.cos(), rf * gap.sin()]);
        pts.append(&mut right);
        pts.extend(left.into_iter().rev());
    }
    pts
}

fn get_num(m: &Map<String, Value>, k: &str, d: f64) -> f64 {
    m.get(k).and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim_end_matches("mm").trim().parse().ok()))).unwrap_or(d)
}

fn get_str(m: &Map<String, Value>, k: &str, d: &str) -> String {
    m.get(k).and_then(|v| v.as_str().map(str::to_string).or_else(|| v.as_f64().map(num))).unwrap_or_else(|| d.to_string())
}

/// Expand a library part. Position = [x, y, z] of the part's base centre.
pub fn part(kind: &str, m: &Map<String, Value>, pos: [f64; 3], p: &str) -> Result<Vec<Command>, String> {
    let n = |s: &str| format!("{p}_{s}");
    let (px, py, pz) = (num(pos[0]), num(pos[1]), num(pos[2]));
    let at_x = |e: &str| format!("{px} + ({e})");
    let at_y = |e: &str| format!("{py} + ({e})");
    let at_z = |e: &str| format!("{pz} + ({e})");
    // XZ sketch coords: (x, -z); offset = y
    let sk_x = |e: &str| at_x(e);
    let sk_cy = num(-pos[2]);
    let mut c = Vec::new();
    let kind = kind.to_ascii_lowercase().replace([' ', '-'], "_");
    match kind.as_str() {
        "bolt" | "screw" => {
            let size = get_str(m, "size", "M8");
            let (d, s, k, ..) = metric(&size).ok_or(format!("unknown bolt size {size} (M3…M24)"))?;
            let len = get_num(m, "length", 4.0 * d);
            c.push(param(&n("d"), format!("{} mm", num(d)), &format!("{size} nominal diameter")));
            c.push(param(&n("length"), format!("{} mm", num(len)), "shank length"));
            c.push(param(&n("head_k"), format!("{} mm", num(k)), "head height"));
            c.push(Command::Cylinder {
                base: [ex(px.clone()), ex(py.clone()), ex(pz.clone())],
                axis: Some("y".into()),
                radius: ex(format!("{}/2", n("d"))),
                height: ex(n("length")),
                operation: Some("new_body".into()),
                name: Some(format!("{p} shank")),
            });
            c.push(Command::NewSketch { name: Some(format!("{p} head")), plane: "XZ".into(), offset: ex(at_y(&n("length"))) });
            c.push(Command::AddRegularPolygon { sketch: None, cx: ex(px.clone()), cy: ex(sk_cy.clone()), radius: ex(num(s / 3f64.sqrt())), sides: 6 });
            c.push(Command::Extrude { sketch: None, profiles: None, distance: ex(n("head_k")), symmetric: false, flip: false, operation: Some("join".into()), name: Some(format!("{p} head")) });
        }
        "nut" => {
            let size = get_str(m, "size", "M8");
            let (d, s, _, mh, ..) = metric(&size).ok_or(format!("unknown nut size {size}"))?;
            c.push(param(&n("d"), format!("{} mm", num(d)), &format!("{size} thread")));
            c.push(param(&n("m"), format!("{} mm", num(mh)), "nut height"));
            c.push(Command::NewSketch { name: Some(format!("{p} nut")), plane: "XZ".into(), offset: ex(py.clone()) });
            c.push(Command::AddRegularPolygon { sketch: None, cx: ex(px.clone()), cy: ex(sk_cy.clone()), radius: ex(num(s / 3f64.sqrt())), sides: 6 });
            c.push(Command::AddCircle { sketch: None, cx: ex(px.clone()), cy: ex(sk_cy.clone()), radius: ex(format!("{}/2", n("d"))) });
            c.push(Command::Extrude { sketch: None, profiles: None, distance: ex(n("m")), symmetric: false, flip: false, operation: Some("new_body".into()), name: Some(format!("{p} nut")) });
        }
        "washer" => {
            let size = get_str(m, "size", "M8");
            let (d, .., od, t) = metric(&size).ok_or(format!("unknown washer size {size}"))?;
            c.push(param(&n("id"), format!("{} mm", num(d + 0.4)), "inner diameter"));
            c.push(param(&n("od"), format!("{} mm", num(od)), "outer diameter"));
            c.push(param(&n("t"), format!("{} mm", num(t)), "thickness"));
            c.push(Command::NewSketch { name: Some(format!("{p} washer")), plane: "XZ".into(), offset: ex(py.clone()) });
            c.push(Command::AddCircle { sketch: None, cx: ex(px.clone()), cy: ex(sk_cy.clone()), radius: ex(format!("{}/2", n("od"))) });
            c.push(Command::AddCircle { sketch: None, cx: ex(px.clone()), cy: ex(sk_cy.clone()), radius: ex(format!("{}/2", n("id"))) });
            c.push(Command::Extrude { sketch: None, profiles: None, distance: ex(n("t")), symmetric: false, flip: false, operation: Some("new_body".into()), name: Some(format!("{p} washer")) });
        }
        "spur_gear" | "gear" => {
            let module = get_num(m, "module", 2.0);
            let teeth = get_num(m, "teeth", 20.0).round().clamp(6.0, 200.0) as u32;
            let thick = get_num(m, "thickness", 10.0);
            let bore = get_num(m, "bore", (module * teeth as f64 * 0.25).max(3.0));
            c.push(param(&n("thickness"), format!("{} mm", num(thick)), &format!("gear m{} z{}", num(module), teeth)));
            c.push(param(&n("bore"), format!("{} mm", num(bore)), "bore diameter"));
            let pts: Vec<[f64; 2]> = gear_outline(module, teeth, 20.0).into_iter().map(|q| [q[0] + pos[0], q[1] - pos[2]]).collect();
            c.push(Command::NewSketch { name: Some(format!("{p} gear")), plane: "XZ".into(), offset: ex(py.clone()) });
            c.push(Command::AddPolygon { sketch: None, points: pts, closed: true });
            c.push(Command::AddCircle { sketch: None, cx: ex(px.clone()), cy: ex(sk_cy.clone()), radius: ex(format!("{}/2", n("bore"))) });
            c.push(Command::Extrude { sketch: None, profiles: None, distance: ex(n("thickness")), symmetric: false, flip: false, operation: Some("new_body".into()), name: Some(format!("{p} gear")) });
        }
        "flange" => {
            let od = get_num(m, "outer_d", 120.0);
            let holes = get_num(m, "holes", 6.0).round().clamp(2.0, 36.0) as u32;
            c.push(param(&n("od"), format!("{} mm", num(od)), "outer diameter"));
            c.push(param(&n("t"), format!("{} mm", num(get_num(m, "thickness", 12.0))), "thickness"));
            c.push(param(&n("pcd"), format!("{} mm", num(get_num(m, "pcd", od * 0.75))), "bolt circle diameter"));
            c.push(param(&n("hole_d"), format!("{} mm", num(get_num(m, "hole_d", 9.0))), "bolt hole diameter"));
            c.push(param(&n("bore"), format!("{} mm", num(get_num(m, "bore", od * 0.4))), "bore diameter"));
            c.push(Command::NewSketch { name: Some(format!("{p} flange")), plane: "XZ".into(), offset: ex(py.clone()) });
            c.push(Command::AddCircle { sketch: None, cx: ex(px.clone()), cy: ex(sk_cy.clone()), radius: ex(format!("{}/2", n("od"))) });
            c.push(Command::AddCircle { sketch: None, cx: ex(px.clone()), cy: ex(sk_cy.clone()), radius: ex(format!("{}/2", n("bore"))) });
            for i in 0..holes {
                let ang = 360.0 * i as f64 / holes as f64;
                c.push(Command::AddCircle {
                    sketch: None,
                    cx: ex(format!("{px} + {}/2 * cos({})", n("pcd"), num(ang))),
                    cy: ex(format!("{sk_cy} + {}/2 * sin({})", n("pcd"), num(ang))),
                    radius: ex(format!("{}/2", n("hole_d"))),
                });
            }
            c.push(Command::Extrude { sketch: None, profiles: None, distance: ex(n("t")), symmetric: false, flip: false, operation: Some("new_body".into()), name: Some(format!("{p} flange")) });
        }
        "tube" | "pipe" => {
            c.push(param(&n("od"), format!("{} mm", num(get_num(m, "outer_d", 40.0))), "outer diameter"));
            c.push(param(&n("id"), format!("{} mm", num(get_num(m, "inner_d", 32.0))), "inner diameter"));
            c.push(param(&n("length"), format!("{} mm", num(get_num(m, "length", 100.0))), "length"));
            c.push(Command::NewSketch { name: Some(format!("{p} tube")), plane: "XZ".into(), offset: ex(py.clone()) });
            c.push(Command::AddCircle { sketch: None, cx: ex(px.clone()), cy: ex(sk_cy.clone()), radius: ex(format!("{}/2", n("od"))) });
            c.push(Command::AddCircle { sketch: None, cx: ex(px.clone()), cy: ex(sk_cy.clone()), radius: ex(format!("{}/2", n("id"))) });
            c.push(Command::Extrude { sketch: None, profiles: None, distance: ex(n("length")), symmetric: false, flip: false, operation: Some("new_body".into()), name: Some(format!("{p} tube")) });
        }
        "l_bracket" | "bracket" | "angle" => {
            c.push(param(&n("w"), format!("{} mm", num(get_num(m, "width", 60.0))), "base length (X)"));
            c.push(param(&n("h"), format!("{} mm", num(get_num(m, "height", 50.0))), "upright height (Y)"));
            c.push(param(&n("depth"), format!("{} mm", num(get_num(m, "depth", 40.0))), "depth (Z)"));
            c.push(param(&n("t"), format!("{} mm", num(get_num(m, "thickness", 6.0))), "thickness"));
            c.push(param(&n("hole_d"), format!("{} mm", num(get_num(m, "hole_d", 8.5))), "hole diameter"));
            let (w, h, dp, t, hd) = (n("w"), n("h"), n("depth"), n("t"), n("hole_d"));
            c.push(Command::Box { corner: [ex(at_x(&format!("-{w}/2"))), ex(py.clone()), ex(at_z(&format!("-{dp}/2")))], size: [ex(w.clone()), ex(t.clone()), ex(dp.clone())], operation: Some("new_body".into()), name: Some(format!("{p} base")) });
            c.push(Command::Box { corner: [ex(at_x(&format!("-{w}/2"))), ex(py.clone()), ex(at_z(&format!("-{dp}/2")))], size: [ex(t.clone()), ex(h.clone()), ex(dp.clone())], operation: Some("join".into()), name: Some(format!("{p} upright")) });
            c.push(Command::Cylinder { base: [ex(at_x(&format!("({w})/4"))), ex(at_y("-1")), ex(pz.clone())], axis: Some("y".into()), radius: ex(format!("{hd}/2")), height: ex(format!("{t} + 2")), operation: Some("cut".into()), name: Some(format!("{p} base hole")) });
            c.push(Command::Cylinder { base: [ex(at_x(&format!("-{w}/2 - 1"))), ex(at_y(&format!("{h} * 0.6"))), ex(pz.clone())], axis: Some("x".into()), radius: ex(format!("{hd}/2")), height: ex(format!("{t} + 2")), operation: Some("cut".into()), name: Some(format!("{p} upright hole")) });
        }
        "enclosure" | "box" | "case" => {
            c.push(param(&n("l"), format!("{} mm", num(get_num(m, "length", 120.0))), "length (X)"));
            c.push(param(&n("w"), format!("{} mm", num(get_num(m, "width", 80.0))), "width (Z)"));
            c.push(param(&n("h"), format!("{} mm", num(get_num(m, "height", 50.0))), "height (Y)"));
            c.push(param(&n("wall"), format!("{} mm", num(get_num(m, "wall", 2.5))), "wall thickness"));
            let (l, w, h, t) = (n("l"), n("w"), n("h"), n("wall"));
            c.push(Command::Box { corner: [ex(at_x(&format!("-{l}/2"))), ex(py.clone()), ex(at_z(&format!("-{w}/2")))], size: [ex(l.clone()), ex(h.clone()), ex(w.clone())], operation: Some("new_body".into()), name: Some(format!("{p} body")) });
            c.push(Command::Box { corner: [ex(at_x(&format!("-{l}/2 + {t}"))), ex(at_y(&t)), ex(at_z(&format!("-{w}/2 + {t}")))], size: [ex(format!("{l} - 2*{t}")), ex(h.clone()), ex(format!("{w} - 2*{t}"))], operation: Some("cut".into()), name: Some(format!("{p} cavity")) });
        }
        "stairs" | "staircase" => {
            let steps = get_num(m, "steps", 12.0).round().clamp(1.0, 60.0) as u32;
            c.push(param(&n("rise"), format!("{} mm", num(get_num(m, "rise", 175.0))), "step rise"));
            c.push(param(&n("run"), format!("{} mm", num(get_num(m, "run", 280.0))), "step run (going)"));
            c.push(param(&n("width"), format!("{} mm", num(get_num(m, "width", 1000.0))), "stair width"));
            let (r, g, w) = (n("rise"), n("run"), n("width"));
            for i in 0..steps {
                c.push(Command::Box {
                    corner: [ex(at_x(&format!("-{w}/2"))), ex(py.clone()), ex(at_z(&format!("-{i} * {g}")))],
                    size: [ex(w.clone()), ex(format!("{} * {r}", i + 1)), ex(format!("-{g}"))],
                    operation: Some(if i == 0 { "new_body" } else { "join" }.into()),
                    name: Some(format!("{p} step {}", i + 1)),
                });
            }
        }
        other => {
            let kinds: Vec<&str> = PART_KINDS.iter().map(|k| k.0).collect();
            return Err(format!("unknown part '{other}' (available: {})", kinds.join(", ")));
        }
    }
    let _ = sk_x;
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::Executor;
    use crate::Document;

    fn run(doc: &mut Document, cmds: &[Command]) {
        let mut ex = Executor::default();
        for c in cmds {
            ex.apply(doc, c).unwrap_or_else(|e| panic!("{c:?}: {e}"));
        }
    }

    #[test]
    fn building_is_hollow_with_openings_and_roof() {
        let mut d = Document::default();
        let spec = BuildingSpec { floors: 2, windows_front: 3, windows_side: 2, ..Default::default() };
        run(&mut d, &building(&spec, "bldg1", &HashMap::new()));
        let m = d.regenerate();
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert_eq!(m.bodies.len(), 1);
        let (lo, hi) = m.bodies[0].solid.bounds().unwrap();
        assert!((hi.y - (6000.0 + 2500.0)).abs() < 1.0, "{hi:?}");
        assert!(lo.x < -6000.0 && hi.x > 6000.0);
        let full = 12000.0 * 8000.0 * 6000.0;
        assert!(m.bodies[0].solid.volume() < full * 0.5, "should be hollow");
        // parametric: a wider building
        let i = d.parameters.iter().position(|p| p.name == "bldg1_width").unwrap();
        d.parameters[i].expr = "15000".into();
        let m2 = d.regenerate();
        assert!(m2.errors.is_empty(), "{:?}", m2.errors);
        assert!(m2.bodies[0].solid.bounds().unwrap().1.x > 7500.0);
    }

    #[test]
    fn library_parts_build() {
        for (kind, _) in PART_KINDS {
            let mut d = Document::default();
            let cmds = part(kind, &Map::new(), [10.0, 0.0, 5.0], "p1").unwrap();
            run(&mut d, &cmds);
            let m = d.regenerate();
            assert!(m.errors.is_empty(), "{kind}: {:?}", m.errors);
            assert!(!m.bodies.is_empty() && m.bodies.iter().all(|b| b.solid.volume() > 0.0), "{kind}");
        }
        assert!(part("teleporter", &Map::new(), [0.0; 3], "p").is_err());
    }

    #[test]
    fn gear_outline_is_sane() {
        let g = gear_outline(2.0, 20, 20.0);
        let r_max = g.iter().map(|p| (p[0] * p[0] + p[1] * p[1]).sqrt()).fold(0.0, f64::max);
        assert!((r_max - 22.0).abs() < 0.1, "{r_max}");
    }

    #[test]
    fn walls_and_openings() {
        let mut d = Document::default();
        let mut cmds = wall([0.0, 0.0], [5000.0, 0.0], "2700", 200.0, "0", "new_body", "Wall1").unwrap();
        cmds.extend(opening(2500.0, 0.0, 1000.0, 400.0, "900", "1200", 0.0, "Window1"));
        run(&mut d, &cmds);
        let m = d.regenerate();
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        let v = m.bodies[0].solid.volume();
        let exact = 5000.0 * 200.0 * 2700.0 - 1000.0 * 200.0 * 1200.0;
        assert!((v - exact).abs() / exact < 1e-6, "{v}");
    }
}
