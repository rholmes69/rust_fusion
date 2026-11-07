//! Multi-view drawing analysis (no AI): split a sheet into view cells, extract each view's
//! silhouette as a polygon, scale views to shared dimensions, build the visual hull
//! (intersection of extruded silhouettes) as editing commands, and score a model against
//! the views with a silhouette IoU.

use cad_kernel::csg::Solid;
use cad_model::commands::{Command, Ex};
use glam::{DVec2, DVec3};
use image::RgbaImage;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ViewKind {
    Front,
    Back,
    Left,
    Right,
    Top,
    Bottom,
    Iso,
    Ignore,
}

impl ViewKind {
    pub const ALL: [ViewKind; 8] = [
        ViewKind::Front,
        ViewKind::Back,
        ViewKind::Left,
        ViewKind::Right,
        ViewKind::Top,
        ViewKind::Bottom,
        ViewKind::Iso,
        ViewKind::Ignore,
    ];
    pub fn label(self) -> &'static str {
        match self {
            ViewKind::Front => "Front",
            ViewKind::Back => "Back",
            ViewKind::Left => "Left",
            ViewKind::Right => "Right",
            ViewKind::Top => "Top",
            ViewKind::Bottom => "Bottom",
            ViewKind::Iso => "Iso / other",
            ViewKind::Ignore => "Ignore",
        }
    }
    pub fn parse(s: &str) -> ViewKind {
        match s.to_ascii_lowercase().trim() {
            "front" | "front view" | "elevation" | "front elevation" | "south" => ViewKind::Front,
            "back" | "rear" | "back view" | "north" => ViewKind::Back,
            "left" | "left side" | "west" => ViewKind::Left,
            "right" | "right side" | "side" | "east" => ViewKind::Right,
            "top" | "plan" | "top view" | "roof" => ViewKind::Top,
            "bottom" | "underside" => ViewKind::Bottom,
            "ignore" | "none" | "text" | "title" | "blank" => ViewKind::Ignore,
            _ => ViewKind::Iso,
        }
    }
    /// Orthographic views usable for the visual hull.
    pub fn is_ortho(self) -> bool {
        !matches!(self, ViewKind::Iso | ViewKind::Ignore)
    }
    /// Camera view name in the app.
    pub fn camera(self) -> &'static str {
        match self {
            ViewKind::Front => "front",
            ViewKind::Back => "back",
            ViewKind::Left => "left",
            ViewKind::Right => "right",
            ViewKind::Top => "top",
            ViewKind::Bottom => "bottom",
            _ => "home",
        }
    }
    /// The canonical view of the same axis pair (back→front, ...), and whether to mirror.
    fn canonical(self) -> (ViewKind, bool) {
        match self {
            ViewKind::Back => (ViewKind::Front, true),
            ViewKind::Left => (ViewKind::Right, true),
            ViewKind::Bottom => (ViewKind::Top, true),
            v => (v, false),
        }
    }
}

/// Default view labels for a grid (third-angle conventions).
pub fn default_labels(rows: usize, cols: usize) -> Vec<ViewKind> {
    use ViewKind::*;
    let mut v = vec![Iso; rows * cols];
    let mut set = |r: usize, c: usize, k: ViewKind| {
        if r < rows && c < cols {
            v[r * cols + c] = k;
        }
    };
    match (rows, cols) {
        (1, 1) => set(0, 0, Front),
        (1, 2) => {
            set(0, 0, Front);
            set(0, 1, Right)
        }
        (1, 3) => {
            set(0, 0, Front);
            set(0, 1, Right);
            set(0, 2, Top)
        }
        (2, 2) => {
            set(0, 0, Top);
            set(1, 0, Front);
            set(1, 1, Right)
        }
        (3, 3) | (3, 4) => {
            set(0, 1, Top);
            set(1, 0, Left);
            set(1, 1, Front);
            set(1, 2, Right);
            set(2, 1, Bottom);
            if cols == 4 {
                set(1, 3, Back);
            }
        }
        _ => {
            let order = [Front, Right, Top, Back, Left, Bottom];
            for (i, k) in order.iter().enumerate() {
                set(i / cols, i % cols, *k);
            }
        }
    }
    v
}

// ---------------------------------------------------------------------- layout

fn background(img: &RgbaImage) -> [f32; 3] {
    let (w, h) = img.dimensions();
    let mut px: Vec<[f32; 3]> = Vec::new();
    let step = ((w + h) / 400).max(1);
    for x in (0..w).step_by(step as usize) {
        for y in [0, h - 1] {
            let p = img.get_pixel(x, y);
            px.push([p[0] as f32, p[1] as f32, p[2] as f32]);
        }
    }
    for y in (0..h).step_by(step as usize) {
        for x in [0, w - 1] {
            let p = img.get_pixel(x, y);
            px.push([p[0] as f32, p[1] as f32, p[2] as f32]);
        }
    }
    let mut out = [0.0; 3];
    for (c, o) in out.iter_mut().enumerate() {
        let mut v: Vec<f32> = px.iter().map(|p| p[c]).collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        *o = v[v.len() / 2];
    }
    out
}

fn ink_mask(img: &RgbaImage, threshold: f32) -> Vec<bool> {
    let bg = background(img);
    img.pixels()
        .map(|p| {
            let d = ((p[0] as f32 - bg[0]).powi(2) + (p[1] as f32 - bg[1]).powi(2) + (p[2] as f32 - bg[2]).powi(2)).sqrt();
            d > threshold && p[3] > 32
        })
        .collect()
}

/// Guess the grid of views from blank gaps (and ruled lines) in the sheet.
pub fn detect_grid(img: &RgbaImage, threshold: f32) -> (usize, usize) {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let ink = ink_mask(img, threshold);
    let bands = |len: usize, other: usize, f: &dyn Fn(usize, usize) -> bool| -> usize {
        let counts: Vec<f32> = (0..len).map(|i| (0..other).filter(|&j| f(i, j)).count() as f32 / other as f32).collect();
        // a separator is (almost) empty or a ruled line spanning most of the sheet
        let sep: Vec<bool> = counts.iter().map(|&c| c < 0.002 || c > 0.85).collect();
        let min_gap = (len / 60).max(2);
        let mut n = 0;
        let mut i = 0;
        while i < len {
            if !sep[i] {
                let s = i;
                while i < len && !sep[i] {
                    i += 1;
                }
                if i - s > min_gap * 2 {
                    n += 1;
                }
            } else {
                i += 1;
            }
        }
        n.clamp(1, 4)
    };
    let rows = bands(h, w, &|y, x| ink[y * w + x]);
    let cols = bands(w, h, &|x, y| ink[y * w + x]);
    (rows, cols)
}

pub fn cell(img: &RgbaImage, rows: usize, cols: usize, r: usize, c: usize) -> RgbaImage {
    let (w, h) = img.dimensions();
    let cw = w as f32 / cols as f32;
    let ch = h as f32 / rows as f32;
    // inset to skip ruled cell borders
    let ix = (cw * 0.02).ceil() as u32;
    let iy = (ch * 0.02).ceil() as u32;
    let x0 = (c as f32 * cw) as u32 + ix;
    let y0 = (r as f32 * ch) as u32 + iy;
    let x1 = (((c + 1) as f32 * cw) as u32).min(w).saturating_sub(ix);
    let y1 = (((r + 1) as f32 * ch) as u32).min(h).saturating_sub(iy);
    image::imageops::crop_imm(img, x0, y0, x1.saturating_sub(x0).max(1), y1.saturating_sub(y0).max(1)).to_image()
}

// ---------------------------------------------------------------------- silhouette

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Silhouette {
    /// Outline in pixels (x right, y DOWN), counter-clockwise in y-up terms.
    pub outline: Vec<DVec2>,
    /// Pixel bounding box (min, max).
    pub min: DVec2,
    pub max: DVec2,
    /// Mask of the solid area (for previews), cell-sized.
    #[serde(skip)]
    #[allow(dead_code)]
    pub mask: Vec<bool>,
    pub w: usize,
    pub h: usize,
}

impl Silhouette {
    pub fn size(&self) -> DVec2 {
        self.max - self.min
    }
}

fn dilate(m: &[bool], w: usize, h: usize, r: i32) -> Vec<bool> {
    let mut out = m.to_vec();
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            if m[(y as usize) * w + x as usize] {
                for dy in -r..=r {
                    for dx in -r..=r {
                        let (nx, ny) = (x + dx, y + dy);
                        if nx >= 0 && ny >= 0 && nx < w as i32 && ny < h as i32 {
                            out[ny as usize * w + nx as usize] = true;
                        }
                    }
                }
            }
        }
    }
    out
}

fn erode(m: &[bool], w: usize, h: usize, r: i32) -> Vec<bool> {
    let inv: Vec<bool> = m.iter().map(|b| !b).collect();
    dilate(&inv, w, h, r).into_iter().map(|b| !b).collect()
}

/// Extract the main solid silhouette of a view cell.
pub fn silhouette(cell: &RgbaImage, threshold: f32) -> Option<Silhouette> {
    let (w, h) = (cell.width() as usize, cell.height() as usize);
    if w < 8 || h < 8 {
        return None;
    }
    let r = ((w.max(h) / 200) as i32).clamp(1, 4);
    let ink = dilate(&ink_mask(cell, threshold), w, h, r);
    // flood fill the outside from the border
    let mut outside = vec![false; w * h];
    let mut stack: Vec<usize> = Vec::new();
    for x in 0..w {
        stack.push(x);
        stack.push((h - 1) * w + x);
    }
    for y in 0..h {
        stack.push(y * w);
        stack.push(y * w + w - 1);
    }
    while let Some(i) = stack.pop() {
        if outside[i] || ink[i] {
            continue;
        }
        outside[i] = true;
        let (x, y) = (i % w, i / w);
        if x > 0 {
            stack.push(i - 1);
        }
        if x + 1 < w {
            stack.push(i + 1);
        }
        if y > 0 {
            stack.push(i - w);
        }
        if y + 1 < h {
            stack.push(i + w);
        }
    }
    let solid: Vec<bool> = outside.iter().map(|o| !o).collect();
    // keep the largest connected component (drops labels and dimension text)
    let mut comp = vec![0u32; w * h];
    let mut best = (0u32, 0usize);
    let mut next = 1u32;
    for start in 0..w * h {
        if !solid[start] || comp[start] != 0 {
            continue;
        }
        let mut n = 0;
        stack.push(start);
        comp[start] = next;
        while let Some(i) = stack.pop() {
            n += 1;
            let (x, y) = (i % w, i / w);
            for (ok, j) in [(x > 0, i.wrapping_sub(1)), (x + 1 < w, i + 1), (y > 0, i.wrapping_sub(w)), (y + 1 < h, i + w)] {
                if ok && solid[j] && comp[j] == 0 {
                    comp[j] = next;
                    stack.push(j);
                }
            }
        }
        if n > best.1 {
            best = (next, n);
        }
        next += 1;
    }
    if best.1 < (w * h) / 400 {
        return None;
    }
    let mask: Vec<bool> = comp.iter().map(|&c| c == best.0).collect();
    let mask = erode(&mask, w, h, r);
    let outline = trace_outline(&mask, w, h)?;
    let tol = (w.max(h) as f64 * 0.004).max(0.75);
    let outline = simplify_closed(&outline, tol);
    if outline.len() < 3 {
        return None;
    }
    let (mut mn, mut mx) = (DVec2::splat(f64::MAX), DVec2::splat(f64::MIN));
    for p in &outline {
        mn = mn.min(*p);
        mx = mx.max(*p);
    }
    Some(Silhouette { outline, min: mn, max: mx, mask, w, h })
}

/// Marching squares on a binary mask; returns the longest closed contour (pixel units, y down).
fn trace_outline(mask: &[bool], w: usize, h: usize) -> Option<Vec<DVec2>> {
    use std::collections::HashMap;
    let at = |x: i64, y: i64| x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h && mask[y as usize * w + x as usize];
    // points are stored doubled so edge midpoints are integers
    let mut next: HashMap<(i64, i64), (i64, i64)> = HashMap::new();
    for y in -1..h as i64 {
        for x in -1..w as i64 {
            let (tl, tr, br, bl) = (at(x, y), at(x + 1, y), at(x + 1, y + 1), at(x, y + 1));
            let code = (tl as u8) << 3 | (tr as u8) << 2 | (br as u8) << 1 | bl as u8;
            // edge midpoints (doubled coords, pixel centres at odd values)
            let top = (2 * x + 2, 2 * y + 1);
            let right = (2 * x + 3, 2 * y + 2);
            let bottom = (2 * x + 2, 2 * y + 3);
            let left = (2 * x + 1, 2 * y + 2);
            // segments oriented with the solid on the left (y down)
            let segs: &[((i64, i64), (i64, i64))] = &match code {
                0 | 15 => [].to_vec(),
                1 => vec![(left, bottom)],
                2 => vec![(bottom, right)],
                3 => vec![(left, right)],
                4 => vec![(right, top)],
                5 => vec![(left, top), (right, bottom)],
                6 => vec![(bottom, top)],
                7 => vec![(left, top)],
                8 => vec![(top, left)],
                9 => vec![(top, bottom)],
                10 => vec![(top, right), (bottom, left)],
                11 => vec![(top, right)],
                12 => vec![(right, left)],
                13 => vec![(right, bottom)],
                14 => vec![(bottom, left)],
                _ => unreachable!(),
            };
            for &(a, b) in segs {
                next.insert(a, b);
            }
        }
    }
    let mut best: Option<Vec<DVec2>> = None;
    while let Some((&start, _)) = next.iter().next() {
        let mut lp = Vec::new();
        let mut cur = start;
        while let Some(n) = next.remove(&cur) {
            lp.push(DVec2::new(cur.0 as f64 / 2.0 - 0.5, cur.1 as f64 / 2.0 - 0.5));
            cur = n;
            if cur == start {
                break;
            }
        }
        if best.as_ref().map_or(true, |b| lp.len() > b.len()) {
            best = Some(lp);
        }
    }
    best
}

fn perp_dist(p: DVec2, a: DVec2, b: DVec2) -> f64 {
    let ab = b - a;
    let l = ab.length();
    if l < 1e-12 {
        return p.distance(a);
    }
    (ab.perp_dot(p - a) / l).abs()
}

fn dp(pts: &[DVec2], tol: f64, out: &mut Vec<DVec2>) {
    if pts.len() < 3 {
        out.extend_from_slice(&pts[..pts.len().saturating_sub(1)]);
        return;
    }
    let (a, b) = (pts[0], *pts.last().unwrap());
    let (mut idx, mut dmax) = (0, 0.0);
    for (i, p) in pts.iter().enumerate().skip(1).take(pts.len() - 2) {
        let d = perp_dist(*p, a, b);
        if d > dmax {
            dmax = d;
            idx = i;
        }
    }
    if dmax > tol {
        dp(&pts[..=idx], tol, out);
        dp(&pts[idx..], tol, out);
    } else {
        out.push(a);
    }
}

/// Douglas–Peucker for a closed loop.
pub fn simplify_closed(pts: &[DVec2], tol: f64) -> Vec<DVec2> {
    if pts.len() < 4 {
        return pts.to_vec();
    }
    // split at the point farthest from the first one
    let far = (0..pts.len()).max_by(|&i, &j| pts[0].distance(pts[i]).partial_cmp(&pts[0].distance(pts[j])).unwrap()).unwrap();
    let mut a: Vec<DVec2> = pts[..=far].to_vec();
    let mut b: Vec<DVec2> = pts[far..].to_vec();
    b.push(pts[0]);
    let mut out = Vec::new();
    dp(&a, tol, &mut out);
    dp(&b, tol, &mut out);
    a.clear();
    out
}

// ---------------------------------------------------------------------- scaling + hull

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Dims {
    /// Along X (front view width).
    pub width: f64,
    /// Along Z (top view height / right view width).
    pub depth: f64,
    /// Along Y.
    pub height: f64,
}

/// A view with its silhouette, ready for the hull.
#[derive(Debug, Clone)]
pub struct ViewSil {
    pub kind: ViewKind,
    pub sil: Silhouette,
}

/// Fill in missing dimensions from silhouette aspect ratios. `known` entries <= 0 are unknown.
pub fn resolve_dims(views: &[ViewSil], known: Dims) -> Result<Dims, String> {
    let get = |k: ViewKind| views.iter().find(|v| v.kind.canonical().0 == k).map(|v| v.sil.size());
    let (f, t, r) = (get(ViewKind::Front), get(ViewKind::Top), get(ViewKind::Right));
    let mut d = known;
    for _ in 0..3 {
        if let Some(s) = f {
            if d.width > 0.0 && d.height <= 0.0 {
                d.height = d.width * s.y / s.x;
            }
            if d.height > 0.0 && d.width <= 0.0 {
                d.width = d.height * s.x / s.y;
            }
        }
        if let Some(s) = t {
            if d.width > 0.0 && d.depth <= 0.0 {
                d.depth = d.width * s.y / s.x;
            }
            if d.depth > 0.0 && d.width <= 0.0 {
                d.width = d.depth * s.x / s.y;
            }
        }
        if let Some(s) = r {
            if d.height > 0.0 && d.depth <= 0.0 {
                d.depth = d.height * s.x / s.y;
            }
            if d.depth > 0.0 && d.height <= 0.0 {
                d.height = d.depth * s.y / s.x;
            }
        }
    }
    if d.width <= 0.0 && d.height <= 0.0 && d.depth <= 0.0 {
        return Err("give at least one overall dimension (width, depth or height)".into());
    }
    // an axis not seen in any view: assume a cube-ish extent
    let m = d.width.max(d.height).max(d.depth);
    for v in [&mut d.width, &mut d.height, &mut d.depth] {
        if *v <= 0.0 {
            *v = m;
        }
    }
    Ok(d)
}

/// Silhouette outline converted to sketch coordinates (mm) on its plane.
/// Front→XY (x=X, y=Y), Top→XZ (x=X, y=-Z), Right→YZ (x=-Z, y=Y). Back/Left/Bottom are mirrored.
pub fn sketch_outline(v: &ViewSil, d: &Dims) -> (&'static str, Vec<[f64; 2]>) {
    let (canon, mirror) = v.kind.canonical();
    let s = v.sil.size();
    let (plane, ext_x, ext_y, y_base) = match canon {
        ViewKind::Front => ("XY", d.width, d.height, 0.0),
        ViewKind::Top => ("XZ", d.width, d.depth, -d.depth / 2.0),
        _ => ("YZ", d.depth, d.height, 0.0),
    };
    let sx = ext_x / s.x.max(1e-9);
    let sy = ext_y / s.y.max(1e-9);
    let mut pts: Vec<[f64; 2]> = v
        .sil
        .outline
        .iter()
        .map(|p| {
            let u = (p.x - v.sil.min.x) * sx - ext_x / 2.0;
            let w = (v.sil.max.y - p.y) * sy + y_base; // image y is down
            [if mirror { -u } else { u }, w]
        })
        .collect();
    if mirror {
        pts.reverse();
    }
    (plane, pts)
}

/// Commands for the visual hull: each canonical view's silhouette extruded through the part,
/// intersected with the others.
pub fn hull_commands(views: &[ViewSil], d: &Dims) -> Result<Vec<Command>, String> {
    let mut chosen: Vec<&ViewSil> = Vec::new();
    for canon in [ViewKind::Front, ViewKind::Top, ViewKind::Right] {
        let pick = views
            .iter()
            .filter(|v| v.kind.canonical().0 == canon)
            .min_by_key(|v| v.kind.canonical().1); // prefer the un-mirrored view
        chosen.extend(pick);
    }
    if chosen.is_empty() {
        return Err("no orthographic views (front/back/left/right/top/bottom) are labelled".into());
    }
    let reach = 2.0 * (d.width.max(d.depth).max(d.height)) + 10.0;
    let mut cmds = vec![
        Command::SetParameter { name: "overall_width".into(), value: Ex(format!("{:.3} mm", d.width)), comment: Some("from image (X)".into()) },
        Command::SetParameter { name: "overall_depth".into(), value: Ex(format!("{:.3} mm", d.depth)), comment: Some("from image (Z)".into()) },
        Command::SetParameter { name: "overall_height".into(), value: Ex(format!("{:.3} mm", d.height)), comment: Some("from image (Y)".into()) },
    ];
    for (i, v) in chosen.iter().enumerate() {
        let (plane, pts) = sketch_outline(v, d);
        let name = format!("{} silhouette", v.kind.label());
        cmds.push(Command::NewSketch { name: Some(name.clone()), plane: plane.into(), offset: Ex("0".into()) });
        cmds.push(Command::AddPolygon { sketch: None, points: pts, closed: true });
        cmds.push(Command::Extrude {
            sketch: None,
            profiles: None,
            distance: Ex(format!("{reach:.1}")),
            symmetric: true,
            flip: false,
            operation: Some(if i == 0 { "new_body" } else { "intersect" }.into()),
            name: Some(format!("{} hull", v.kind.label())),
        });
    }
    Ok(cmds)
}

// ---------------------------------------------------------------------- scoring

/// Project a solid into a view's sketch coordinates (see [`sketch_outline`]).
fn project(kind: ViewKind, p: DVec3) -> DVec2 {
    let (canon, mirror) = kind.canonical();
    let q = match canon {
        ViewKind::Front => DVec2::new(p.x, p.y),
        ViewKind::Top => DVec2::new(p.x, -p.z),
        _ => DVec2::new(-p.z, p.y),
    };
    if mirror { DVec2::new(-q.x, q.y) } else { q }
}

fn point_in_poly(p: DVec2, poly: &[[f64; 2]]) -> bool {
    let mut inside = false;
    let n = poly.len();
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (poly[i], poly[j]);
        if (a[1] > p.y) != (b[1] > p.y) && p.x < (b[0] - a[0]) * (p.y - a[1]) / (b[1] - a[1]) + a[0] {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Intersection-over-union of the model's projection and the view silhouette (0..1).
pub fn iou(solids: &[&Solid], v: &ViewSil, d: &Dims) -> f64 {
    let (_, poly) = sketch_outline(v, d);
    if poly.len() < 3 {
        return 0.0;
    }
    let tris: Vec<[DVec2; 3]> = solids
        .iter()
        .flat_map(|s| s.triangles())
        .map(|t| [project(v.kind, t[0]), project(v.kind, t[1]), project(v.kind, t[2])])
        .collect();
    let (mut lo, mut hi) = (DVec2::splat(f64::MAX), DVec2::splat(f64::MIN));
    for p in &poly {
        lo = lo.min(DVec2::new(p[0], p[1]));
        hi = hi.max(DVec2::new(p[0], p[1]));
    }
    for t in &tris {
        for p in t {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
    }
    const N: usize = 140;
    let size = hi - lo;
    let step = size.x.max(size.y) / N as f64;
    if step <= 0.0 {
        return 0.0;
    }
    let (nx, ny) = ((size.x / step).ceil() as usize + 1, (size.y / step).ceil() as usize + 1);
    let mut model = vec![false; nx * ny];
    for t in &tris {
        let area = (t[1] - t[0]).perp_dot(t[2] - t[0]);
        if area.abs() < 1e-12 {
            continue;
        }
        let (mut a, mut b) = (t[0].min(t[1]).min(t[2]), t[0].max(t[1]).max(t[2]));
        a = (a - lo) / step;
        b = (b - lo) / step;
        for iy in (a.y.floor().max(0.0) as usize)..=(b.y.ceil() as usize).min(ny - 1) {
            for ix in (a.x.floor().max(0.0) as usize)..=(b.x.ceil() as usize).min(nx - 1) {
                let p = lo + DVec2::new(ix as f64 + 0.5, iy as f64 + 0.5) * step;
                let w0 = (t[1] - t[0]).perp_dot(p - t[0]) / area;
                let w1 = (t[2] - t[1]).perp_dot(p - t[1]) / area;
                let w2 = (t[0] - t[2]).perp_dot(p - t[2]) / area;
                if w0 >= -1e-9 && w1 >= -1e-9 && w2 >= -1e-9 {
                    model[iy * nx + ix] = true;
                }
            }
        }
    }
    let (mut inter, mut uni) = (0usize, 0usize);
    for iy in 0..ny {
        for ix in 0..nx {
            let p = lo + DVec2::new(ix as f64 + 0.5, iy as f64 + 0.5) * step;
            let r = point_in_poly(p, &poly);
            let m = model[iy * nx + ix];
            inter += (r && m) as usize;
            uni += (r || m) as usize;
        }
    }
    if uni == 0 { 0.0 } else { inter as f64 / uni as f64 }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use cad_model::commands::Executor;
    use cad_model::Document;

    /// A synthetic third-angle sheet (2×2: top / iso, front / right) of an L-shaped block
    /// 80 wide (X), 40 deep (Z), 50 tall (Y); the upright is 20 thick at the left.
    pub fn l_block_sheet() -> RgbaImage {
        let (cw, ch) = (300u32, 260u32);
        let mut img = RgbaImage::from_pixel(cw * 2, ch * 2, image::Rgba([255, 255, 255, 255]));
        let s = 2.5; // px per mm
        let fill = |img: &mut RgbaImage, ox: u32, oy: u32, pts: &[(f64, f64)]| {
            // fill polygon given in mm (x right, y up) with origin at the cell's lower-left area
            for y in 0..ch {
                for x in 0..cw {
                    let p = DVec2::new((x as f64 - 50.0) / s, (ch as f64 - 40.0 - y as f64) / s);
                    let poly: Vec<[f64; 2]> = pts.iter().map(|q| [q.0, q.1]).collect();
                    if point_in_poly(p, &poly) {
                        img.put_pixel(ox + x, oy + y, image::Rgba([90, 100, 120, 255]));
                    }
                }
            }
        };
        // front (X right, Y up): L profile
        fill(&mut img, 0, ch, &[(0.0, 0.0), (80.0, 0.0), (80.0, 10.0), (20.0, 10.0), (20.0, 50.0), (0.0, 50.0)]);
        // top (X right, image-up = -Z): full 80 x 40 rectangle
        fill(&mut img, 0, 0, &[(0.0, 0.0), (80.0, 0.0), (80.0, 40.0), (0.0, 40.0)]);
        // right (image-right = -Z, Y up): 40 x 50 rectangle
        fill(&mut img, cw, ch, &[(0.0, 0.0), (40.0, 0.0), (40.0, 50.0), (0.0, 50.0)]);
        // a label that must be ignored
        for y in 10..20 {
            for x in 10..60 {
                img.put_pixel(cw + x, y, image::Rgba([0, 0, 0, 255]));
            }
        }
        img
    }

    #[test]
    fn sheet_to_visual_hull() {
        let img = l_block_sheet();
        assert_eq!(detect_grid(&img, 40.0), (2, 2));
        let labels = default_labels(2, 2);
        let mut views = Vec::new();
        for r in 0..2 {
            for c in 0..2 {
                let k = labels[r * 2 + c];
                if !k.is_ortho() {
                    continue;
                }
                let sil = silhouette(&cell(&img, 2, 2, r, c), 40.0).expect("silhouette");
                views.push(ViewSil { kind: k, sil });
            }
        }
        assert_eq!(views.len(), 3);
        let d = resolve_dims(&views, Dims { width: 80.0, depth: 0.0, height: 0.0 }).unwrap();
        assert!((d.height - 50.0).abs() < 1.5 && (d.depth - 40.0).abs() < 1.5, "{d:?}");
        let cmds = hull_commands(&views, &d).unwrap();
        let mut doc = Document::default();
        let mut ex = Executor::default();
        for c in &cmds {
            ex.apply(&mut doc, c).unwrap();
        }
        let m = doc.regenerate();
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert_eq!(m.bodies.len(), 1);
        let vol = m.bodies[0].solid.volume();
        let exact = (80.0 * 10.0 + 20.0 * 40.0) * 40.0;
        assert!((vol - exact).abs() / exact < 0.06, "volume {vol} vs {exact}");
        let solids: Vec<&Solid> = m.bodies.iter().map(|b| &b.solid).collect();
        for v in &views {
            let s = iou(&solids, v, &d);
            assert!(s > 0.93, "{:?} iou {s}", v.kind);
        }
        // a wrong model scores lower
        let cube = cad_kernel::primitives::cuboid(DVec3::new(-40.0, 0.0, -20.0), DVec3::new(40.0, 50.0, 20.0));
        let front = views.iter().find(|v| v.kind == ViewKind::Front).unwrap();
        assert!(iou(&[&cube], front, &d) < 0.8);
    }
}
