//! Converting model geometry into GPU vertex lists.

use crate::render::{StaticScene, Vertex};
use cad_kernel::edges::feature_edges;
use cad_kernel::profile::{Frame, Region};
use cad_model::Model;
use glam::{DVec2, DVec3};
use std::collections::HashSet;

pub const BODY: [f32; 4] = [0.70, 0.73, 0.77, 1.0];
pub const BODY_SELECTED: [f32; 4] = [0.42, 0.64, 0.96, 1.0];
pub const EDGE: [f32; 4] = [0.12, 0.13, 0.16, 1.0];
pub const SKETCH: [f32; 4] = [0.10, 0.36, 0.86, 1.0];
pub const SKETCH_INACTIVE: [f32; 4] = [0.35, 0.45, 0.62, 0.9];
pub const SKETCH_SELECTED: [f32; 4] = [0.95, 0.45, 0.05, 1.0];
pub const PREVIEW: [f32; 4] = [0.05, 0.55, 0.85, 1.0];
pub const PROFILE_FILL: [f32; 4] = [0.98, 0.70, 0.35, 0.28];
pub const PROFILE_HOVER: [f32; 4] = [0.98, 0.60, 0.20, 0.45];
pub const PROFILE_SELECTED: [f32; 4] = [0.20, 0.50, 0.95, 0.45];
pub const GRID_MINOR: [f32; 4] = [0.55, 0.58, 0.64, 0.18];
pub const GRID_MAJOR: [f32; 4] = [0.45, 0.48, 0.55, 0.38];

pub fn build_static(
    model: &Model,
    hidden: &HashSet<String>,
    selected: Option<usize>,
) -> (StaticScene, Vec<(usize, [DVec3; 3])>) {
    let mut s = StaticScene::default();
    let mut picks = Vec::new();
    for (bi, body) in model.bodies.iter().enumerate() {
        if hidden.contains(&body.name) {
            continue;
        }
        let color = if selected == Some(bi) { BODY_SELECTED } else { BODY };
        for p in &body.solid.polygons {
            let n = p.plane.normal;
            for i in 1..p.vertices.len() - 1 {
                let tri = [p.vertices[0], p.vertices[i], p.vertices[i + 1]];
                for v in tri {
                    s.tris.push(Vertex::new(v, n, color));
                }
                picks.push((bi, tri));
            }
        }
        for [a, b] in feature_edges(&body.solid, 25.0) {
            s.edges.push(Vertex::line(a, EDGE));
            s.edges.push(Vertex::line(b, EDGE));
        }
    }
    (s, picks)
}

pub fn push_line(out: &mut Vec<Vertex>, a: DVec3, b: DVec3, c: [f32; 4]) {
    out.push(Vertex::line(a, c));
    out.push(Vertex::line(b, c));
}

pub fn push_polyline(out: &mut Vec<Vertex>, frame: &Frame, pts: &[DVec2], closed: bool, c: [f32; 4]) {
    if pts.len() < 2 {
        return;
    }
    let n = pts.len();
    let segs = if closed { n } else { n - 1 };
    for i in 0..segs {
        push_line(out, frame.to_3d(pts[i]), frame.to_3d(pts[(i + 1) % n]), c);
    }
}

pub fn push_region_fill(out: &mut Vec<Vertex>, frame: &Frame, r: &Region, c: [f32; 4]) {
    let n = frame.normal();
    for t in r.triangulate() {
        for p in t {
            out.push(Vertex::new(frame.to_3d(p), n, c));
        }
    }
}

pub fn push_quad_fill(out: &mut Vec<Vertex>, q: [DVec3; 4], c: [f32; 4]) {
    for i in [0, 1, 2, 0, 2, 3] {
        out.push(Vertex::new(q[i], DVec3::Z, c));
    }
}

/// A square grid on `frame` centred near `center` (2D), sized for the view.
pub fn push_grid(out: &mut Vec<Vertex>, frame: &Frame, center: DVec2, step: f64, half_count: i32) {
    let major = step * 10.0;
    let cx = (center.x / major).round() * major;
    let cy = (center.y / major).round() * major;
    let ext = step * half_count as f64;
    let is_major = |v: f64| ((v / major) - (v / major).round()).abs() < 1e-6;
    for i in -half_count..=half_count {
        let off = i as f64 * step;
        let c = if is_major(cx + off) { GRID_MAJOR } else { GRID_MINOR };
        push_line(
            out,
            frame.to_3d(DVec2::new(cx + off, cy - ext)),
            frame.to_3d(DVec2::new(cx + off, cy + ext)),
            c,
        );
        let c = if is_major(cy + off) { GRID_MAJOR } else { GRID_MINOR };
        push_line(
            out,
            frame.to_3d(DVec2::new(cx - ext, cy + off)),
            frame.to_3d(DVec2::new(cx + ext, cy + off)),
            c,
        );
    }
}

/// Grid spacing appropriate for a camera distance (1-2-5 not needed; powers of ten).
pub fn grid_step(distance: f64) -> f64 {
    10f64.powf((distance / 40.0).max(1e-6).log10().floor())
}
