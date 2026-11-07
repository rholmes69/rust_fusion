//! The 3D viewport: navigation, picking, sketch tools and overlays.

use crate::app::{fmt_num, App, Mode, SketchTool};
use crate::camera::StdView;
use crate::render::{DynamicScene, Uniforms, ViewportCallback};
use crate::scene::{self, *};
use cad_kernel::profile::Frame;
use cad_model::sketch::{Entity, PlaneKind};
use cad_model::{FeatureId, FeatureKind, RevolveAxis};
use eframe::egui::{self, Color32, Pos2, Rect, Stroke};
use eframe::egui_wgpu;
use glam::{DVec2, DVec3, Mat4, Vec4};

const SNAP_PX: f32 = 10.0;

fn project(vp: &Mat4, rect: Rect, p: DVec3) -> Option<Pos2> {
    let c = *vp * Vec4::new(p.x as f32, p.y as f32, p.z as f32, 1.0);
    if c.w <= 1e-6 {
        return None;
    }
    let (x, y) = (c.x / c.w, c.y / c.w);
    Some(Pos2::new(
        rect.left() + (x + 1.0) * 0.5 * rect.width(),
        rect.top() + (1.0 - y) * 0.5 * rect.height(),
    ))
}

fn ray_at(app: &App, rect: Rect, p: Pos2) -> (DVec3, DVec3) {
    let nx = ((p.x - rect.left()) / rect.width() * 2.0 - 1.0) as f64;
    let ny = (1.0 - (p.y - rect.top()) / rect.height() * 2.0) as f64;
    app.camera.ray([nx, ny], aspect(rect))
}

fn aspect(rect: Rect) -> f32 {
    rect.width() / rect.height().max(1.0)
}

fn hit_plane(frame: &Frame, (o, d): (DVec3, DVec3)) -> Option<(f64, DVec2)> {
    let n = frame.normal();
    let denom = d.dot(n);
    if denom.abs() < 1e-9 {
        return None;
    }
    let t = (frame.origin - o).dot(n) / denom;
    if t < 0.0 {
        return None;
    }
    Some((t, frame.to_2d(o + d * t)))
}

fn ray_tri((o, d): (DVec3, DVec3), t: &[DVec3; 3]) -> Option<f64> {
    let e1 = t[1] - t[0];
    let e2 = t[2] - t[0];
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let s = o - t[0];
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = d.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let dist = e2.dot(q) * inv;
    (dist > 0.0).then_some(dist)
}

fn pick_body(app: &App, ray: (DVec3, DVec3)) -> Option<(usize, f64)> {
    app.pick_tris
        .iter()
        .filter_map(|(b, t)| ray_tri(ray, t).map(|d| (*b, d)))
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
}

fn dist_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    (a + ab * t).distance(p)
}

/// Sketches currently displayed: not hidden, plus any in use by the active mode.
fn visible_sketches(app: &App) -> Vec<FeatureId> {
    let mut forced = Vec::new();
    match &app.mode {
        Mode::Sketch(s) => forced.push(s.feature),
        Mode::Command(c) => {
            if let Some(sk) = app.doc.feature(c.feature).and_then(|f| f.kind.parent_sketch()) {
                forced.push(sk);
            }
        }
        _ => {}
    }
    app.doc.features[..app.doc.active_len()]
        .iter()
        .filter(|f| matches!(f.kind, FeatureKind::Sketch(_)) && !f.suppressed)
        .map(|f| f.id)
        .filter(|id| forced.contains(id) || !app.hidden_sketches.contains(id))
        .filter(|id| app.model.sketches.contains_key(id))
        .collect()
}

/// Regions under the cursor across visible sketches: (sketch, region index), nearest plane first.
fn pick_region(app: &App, ray: (DVec3, DVec3)) -> Option<(FeatureId, usize)> {
    let mut best: Option<(f64, f64, FeatureId, usize)> = None;
    for id in visible_sketches(app) {
        let g = &app.model.sketches[&id];
        let Some(fr) = g.frame else { continue };
        let Some((t, p)) = hit_plane(&fr, ray) else { continue };
        for (ri, r) in g.regions.iter().enumerate() {
            if r.contains(p) {
                let area = r.area();
                let better = match best {
                    None => true,
                    Some((bt, ba, _, _)) => t < bt - 1e-6 || ((t - bt).abs() <= 1e-6 && area < ba),
                };
                if better {
                    best = Some((t, area, id, ri));
                }
            }
        }
    }
    best.map(|(_, _, id, r)| (id, r))
}

fn plane_square_size(app: &App) -> f64 {
    app.camera.distance * 0.2
}

fn pick_origin_plane(app: &App, ray: (DVec3, DVec3)) -> Option<PlaneKind> {
    let s = plane_square_size(app);
    PlaneKind::ALL
        .iter()
        .filter_map(|&k| {
            let fr = k.frame(0.0);
            let (t, p) = hit_plane(&fr, ray)?;
            // planes are shown in the quadrant of +u and +v
            (p.x >= 0.0 && p.y >= 0.0 && p.x <= s && p.y <= s).then_some((t, k))
        })
        .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap())
        .map(|(_, k)| k)
}

fn active_sketch_frame(app: &App) -> Option<(FeatureId, Frame)> {
    let Mode::Sketch(s) = &app.mode else { return None };
    let fr = app.model.sketches.get(&s.feature)?.frame?;
    Some((s.feature, fr))
}

fn sketch_entities(app: &App, id: FeatureId) -> Option<&Vec<Entity>> {
    match &app.doc.feature(id)?.kind {
        FeatureKind::Sketch(s) => Some(&s.entities),
        _ => None,
    }
}

fn sketch_entities_mut(app: &mut App, id: FeatureId) -> Option<&mut Vec<Entity>> {
    match &mut app.doc.feature_mut(id)?.kind {
        FeatureKind::Sketch(s) => Some(&mut s.entities),
        _ => None,
    }
}

/// Points the sketch cursor snaps to (endpoints, corners, circle centres, origin).
fn snap_points(app: &App, id: FeatureId) -> Vec<DVec2> {
    let mut pts = vec![DVec2::ZERO];
    if let (Some(g), Some(ents)) = (app.model.sketches.get(&id), sketch_entities(app, id)) {
        for (i, (curve, _)) in g.curves.iter().enumerate() {
            match ents.get(i) {
                Some(Entity::Circle { .. }) if !curve.is_empty() => {
                    pts.push(curve.iter().copied().sum::<DVec2>() / curve.len() as f64);
                }
                _ => pts.extend(curve.iter().copied()),
            }
        }
    }
    if let Mode::Sketch(s) = &app.mode {
        pts.extend(s.pending.iter().copied());
    }
    pts
}

fn add_entity(app: &mut App, id: FeatureId, e: Entity) {
    app.checkpoint();
    if let Some(ents) = sketch_entities_mut(app, id) {
        ents.push(e);
    }
    app.rebuild();
}

pub fn commit_polyline(app: &mut App, closed: bool) {
    let Mode::Sketch(s) = &mut app.mode else { return };
    let pts = std::mem::take(&mut s.pending);
    let id = s.feature;
    if pts.len() < 2 || (closed && pts.len() < 3) {
        return;
    }
    let points = pts.iter().map(|p| [p.x, p.y]).collect();
    add_entity(app, id, Entity::Polyline { points, closed });
}

pub fn delete_selected_entity(app: &mut App) {
    let Mode::Sketch(s) = &mut app.mode else { return };
    let Some(i) = s.selected.take() else { return };
    let id = s.feature;
    app.checkpoint();
    if let Some(cad_model::Feature { kind: FeatureKind::Sketch(s), .. }) = app.doc.feature_mut(id) {
        s.remove_entity(i);
    }
    app.rebuild();
}

fn sketch_click(app: &mut App, rect: Rect, vp: &Mat4) {
    let Some((id, _)) = active_sketch_frame(app) else { return };
    let Mode::Sketch(s) = &mut app.mode else { return };
    let Some(p) = s.cursor else { return };
    let snapped = s.snapped_to_point;
    match s.tool {
        SketchTool::Line => {
            if s.pending.len() >= 3 && snapped && p.distance(s.pending[0]) < 1e-9 {
                commit_polyline(app, true);
            } else if s.pending.last().map_or(false, |l| l.distance(p) < 1e-9) {
                // clicking the last point again (double-click) ends the polyline
                commit_polyline(app, false);
            } else {
                s.pending.push(p);
            }
        }
        SketchTool::Rectangle => {
            if let Some(&a) = s.pending.first() {
                s.pending.clear();
                let d = p - a;
                if d.x.abs() > 1e-9 && d.y.abs() > 1e-9 {
                    add_entity(
                        app,
                        id,
                        Entity::Rectangle { x: fmt_num(a.x), y: fmt_num(a.y), width: fmt_num(d.x), height: fmt_num(d.y) },
                    );
                }
            } else {
                s.pending.push(p);
            }
        }
        SketchTool::Circle => {
            if let Some(&c) = s.pending.first() {
                s.pending.clear();
                let r = c.distance(p);
                if r > 1e-9 {
                    add_entity(app, id, Entity::Circle { cx: fmt_num(c.x), cy: fmt_num(c.y), radius: fmt_num(r) });
                }
            } else {
                s.pending.push(p);
            }
        }
        SketchTool::Select => {
            let hover = app.hover_screen_entity(rect, vp);
            if let Mode::Sketch(s) = &mut app.mode {
                s.selected = hover;
            }
        }
    }
}

impl App {
    /// Index of the active-sketch entity closest to the pointer (within a few pixels).
    fn hover_screen_entity(&self, rect: Rect, vp: &Mat4) -> Option<usize> {
        let (id, fr) = active_sketch_frame(self)?;
        let ptr = self.hover_world.and_then(|w| project(vp, rect, w))?;
        let g = self.model.sketches.get(&id)?;
        let mut best: Option<(f32, usize)> = None;
        for (i, (pts, closed)) in g.curves.iter().enumerate() {
            let n = pts.len();
            if n < 2 {
                continue;
            }
            let segs = if *closed { n } else { n - 1 };
            for k in 0..segs {
                let (Some(a), Some(b)) =
                    (project(vp, rect, fr.to_3d(pts[k])), project(vp, rect, fr.to_3d(pts[(k + 1) % n])))
                else {
                    continue;
                };
                let d = dist_to_segment(ptr, a, b);
                if d < 8.0 && best.map_or(true, |(bd, _)| d < bd) {
                    best = Some((d, i));
                }
            }
        }
        best.map(|(_, i)| i)
    }
}

fn navigate(app: &mut App, ui: &egui::Ui, resp: &egui::Response, rect: Rect) {
    let shift = ui.input(|i| i.modifiers.shift);
    let d = resp.drag_delta();
    let (dx, dy) = (d.x as f64, d.y as f64);
    if resp.dragged_by(egui::PointerButton::Middle) || (shift && resp.dragged_by(egui::PointerButton::Primary)) {
        app.camera.pan(dx, dy, rect.height() as f64);
    } else if resp.dragged_by(egui::PointerButton::Primary) || resp.dragged_by(egui::PointerButton::Secondary) {
        app.camera.orbit(dx, dy);
    }
    if resp.hovered() {
        let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
        let factor = (-(scroll as f64) * 0.0015).exp() / pinch as f64;
        if (factor - 1.0).abs() > 1e-6 {
            let anchor = resp.hover_pos().map(|p| {
                let ray = ray_at(app, rect, p);
                match pick_body(app, ray) {
                    Some((_, t)) => ray.0 + ray.1 * t,
                    None => {
                        // plane through the target facing the camera
                        let n = app.camera.dir();
                        let t = (app.camera.target - ray.0).dot(n) / ray.1.dot(n);
                        ray.0 + ray.1 * t
                    }
                }
            });
            app.camera.zoom_at(factor, anchor);
        }
    }
}

fn paint_background(painter: &egui::Painter, rect: Rect) {
    let top = Color32::from_rgb(0xf4, 0xf6, 0xf9);
    let bottom = Color32::from_rgb(0xc9, 0xd1, 0xdc);
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), top);
    mesh.colored_vertex(rect.right_top(), top);
    mesh.colored_vertex(rect.right_bottom(), bottom);
    mesh.colored_vertex(rect.left_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(mesh));
}

pub fn viewport(app: &mut App, ui: &mut egui::Ui) {
    let rect = ui.available_rect_before_wrap();
    let resp = ui.allocate_rect(rect, egui::Sense::click_and_drag());
    app.viewport_rect = rect;
    let painter = ui.painter_at(rect);
    paint_background(&painter, rect);

    navigate(app, ui, &resp, rect);
    let vp = app.camera.view_proj(aspect(rect));
    let ray = resp.hover_pos().map(|p| ray_at(app, rect, p));

    // ---------------------------------------------------------------- hover
    app.hover_region = None;
    app.hover_plane = None;
    app.hover_world = None;
    let grid = scene::grid_step(app.camera.distance);
    match &app.mode {
        Mode::ChoosePlane { .. } => {
            app.hover_plane = ray.and_then(|r| pick_origin_plane(app, r));
        }
        Mode::Command(c) => {
            let wants_profiles = matches!(
                app.doc.feature(c.feature).map(|f| &f.kind),
                Some(FeatureKind::Extrude { .. } | FeatureKind::Revolve { .. })
            );
            if wants_profiles {
                app.hover_region = ray.and_then(|r| pick_region(app, r));
            }
        }
        Mode::Sketch(_) => {
            if let (Some((id, fr)), Some(r)) = (active_sketch_frame(app), ray) {
                if let Some((_, raw)) = hit_plane(&fr, r) {
                    let ptr = resp.hover_pos().unwrap();
                    let snap = snap_points(app, id)
                        .into_iter()
                        .filter_map(|p| project(&vp, rect, fr.to_3d(p)).map(|s| (s.distance(ptr), p)))
                        .filter(|(d, _)| *d < SNAP_PX)
                        .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
                    let (p, on_point) = match snap {
                        Some((_, p)) => (p, true),
                        None => ((raw / grid).round() * grid, false),
                    };
                    app.hover_world = Some(fr.to_3d(raw));
                    if let Mode::Sketch(s) = &mut app.mode {
                        s.cursor = Some(p);
                        s.snapped_to_point = on_point;
                    }
                }
            }
        }
        Mode::Idle => {}
    }

    // ---------------------------------------------------------------- clicks
    if resp.clicked() {
        match &mut app.mode {
            Mode::ChoosePlane { offset } => {
                if let Some(k) = app.hover_plane {
                    let off = offset.clone();
                    app.create_sketch(k, off);
                }
            }
            Mode::Sketch(_) => sketch_click(app, rect, &vp),
            Mode::Command(c) => {
                if let Some((sk, ri)) = app.hover_region {
                    let fid = c.feature;
                    if let Some(f) = app.doc.feature_mut(fid) {
                        match &mut f.kind {
                            FeatureKind::Extrude { sketch, profiles, .. }
                            | FeatureKind::Revolve { sketch, profiles, .. } => {
                                if *sketch != sk {
                                    *sketch = sk;
                                    profiles.clear();
                                    profiles.push(ri);
                                } else if let Some(pos) = profiles.iter().position(|&p| p == ri) {
                                    profiles.remove(pos);
                                } else {
                                    profiles.push(ri);
                                }
                            }
                            _ => {}
                        }
                    }
                    // the sketch must come before the feature in the timeline
                    let (si, fi) = (app.doc.index_of(sk), app.doc.index_of(fid));
                    if let (Some(si), Some(fi)) = (si, fi) {
                        if si > fi {
                            let f = app.doc.features.remove(fi);
                            app.doc.features.insert(si, f);
                        }
                    }
                    app.rebuild();
                }
            }
            Mode::Idle => {
                let hit = ray.and_then(|r| pick_body(app, r)).map(|(b, _)| b);
                app.selected_body = hit;
                app.selected_feature = hit.map(|b| app.model.bodies[b].created_by);
                app.rebuild();
            }
        }
    }
    if resp.secondary_clicked() {
        if let Mode::Sketch(s) = &app.mode {
            if s.tool == SketchTool::Line && s.pending.len() >= 2 {
                commit_polyline(app, false);
            } else if let Mode::Sketch(s) = &mut app.mode {
                s.pending.clear();
            }
        }
    }

    // ---------------------------------------------------------------- scene
    let dynamic = build_dynamic(app, grid);
    let cam = &app.camera;
    let cb = ViewportCallback {
        uniforms: Uniforms {
            view_proj: vp.to_cols_array_2d(),
            eye: cam.eye().as_vec3().extend(1.0).to_array(),
            light: cam.light_dir().extend(0.0).to_array(),
        },
        static_gen: app.static_gen,
        static_scene: app.static_scene.clone(),
        dynamic,
    };
    painter.add(egui_wgpu::Callback::new_paint_callback(rect, cb));

    overlays(app, ui, &painter, rect, &vp);
}

fn build_dynamic(app: &App, grid: f64) -> DynamicScene {
    let mut d = DynamicScene::default();
    let active = active_sketch_frame(app);

    if app.show_grid {
        let fr = active.map(|(_, f)| f).unwrap_or_else(|| PlaneKind::XZ.frame(0.0));
        let c = fr.to_2d(app.camera.target);
        push_grid(&mut d.lines, &fr, c, grid, 50);
    }
    if app.show_origin {
        let l = app.camera.distance * 0.2;
        push_line(&mut d.lines, DVec3::ZERO, DVec3::X * l, [0.85, 0.15, 0.15, 1.0]);
        push_line(&mut d.lines, DVec3::ZERO, DVec3::Y * l, [0.15, 0.65, 0.20, 1.0]);
        push_line(&mut d.lines, DVec3::ZERO, DVec3::Z * l, [0.15, 0.30, 0.90, 1.0]);
    }

    if let Mode::ChoosePlane { .. } = &app.mode {
        let s = plane_square_size(app);
        for k in PlaneKind::ALL {
            let fr = k.frame(0.0);
            let q = [
                fr.to_3d(DVec2::ZERO),
                fr.to_3d(DVec2::new(s, 0.0)),
                fr.to_3d(DVec2::new(s, s)),
                fr.to_3d(DVec2::new(0.0, s)),
            ];
            let hot = app.hover_plane == Some(k);
            push_quad_fill(&mut d.overlay, q, if hot { [0.98, 0.62, 0.2, 0.45] } else { [0.95, 0.75, 0.45, 0.22] });
            for i in 0..4 {
                push_line(&mut d.lines_top, q[i], q[(i + 1) % 4], [0.85, 0.5, 0.1, 1.0]);
            }
        }
    }

    // profiles involved in the current command
    let (cmd_sketch, cmd_profiles, revolve_axis) = match &app.mode {
        Mode::Command(c) => match app.doc.feature(c.feature).map(|f| &f.kind) {
            Some(FeatureKind::Extrude { sketch, profiles, .. }) => (Some(*sketch), profiles.clone(), None),
            Some(FeatureKind::Revolve { sketch, profiles, axis, .. }) => (Some(*sketch), profiles.clone(), Some(*axis)),
            _ => (None, vec![], None),
        },
        _ => (None, vec![], None),
    };
    let picking = cmd_sketch.is_some();

    for id in visible_sketches(app) {
        let g = &app.model.sketches[&id];
        let Some(fr) = g.frame else { continue };
        let is_active = active.map(|(a, _)| a) == Some(id);
        let selected_entity = match &app.mode {
            Mode::Sketch(s) if is_active => s.selected,
            _ => None,
        };
        for (i, (pts, closed)) in g.curves.iter().enumerate() {
            let c = if selected_entity == Some(i) {
                SKETCH_SELECTED
            } else if is_active {
                SKETCH
            } else {
                SKETCH_INACTIVE
            };
            let out = if is_active || picking { &mut d.lines_top } else { &mut d.lines };
            push_polyline(out, &fr, pts, *closed, c);
        }
        if is_active || picking {
            for (ri, r) in g.regions.iter().enumerate() {
                let c = if cmd_sketch == Some(id) && cmd_profiles.contains(&ri) {
                    PROFILE_SELECTED
                } else if app.hover_region == Some((id, ri)) {
                    PROFILE_HOVER
                } else {
                    PROFILE_FILL
                };
                push_region_fill(&mut d.overlay, &fr, r, c);
            }
        }
        if cmd_sketch == Some(id) {
            if let Some(ax) = revolve_axis {
                let dir = match ax {
                    RevolveAxis::SketchU => DVec2::X,
                    RevolveAxis::SketchV => DVec2::Y,
                };
                let l = app.camera.distance;
                push_line(&mut d.lines_top, fr.to_3d(-dir * l), fr.to_3d(dir * l), [0.9, 0.2, 0.6, 1.0]);
            }
        }
    }

    // rubber-band preview for the active sketch tool
    if let (Mode::Sketch(s), Some((_, fr))) = (&app.mode, active) {
        if let Some(cur) = s.cursor {
            match s.tool {
                SketchTool::Line if !s.pending.is_empty() => {
                    let mut pts = s.pending.clone();
                    pts.push(cur);
                    push_polyline(&mut d.lines_top, &fr, &pts, false, PREVIEW);
                }
                SketchTool::Rectangle if !s.pending.is_empty() => {
                    let a = s.pending[0];
                    let pts = [a, DVec2::new(cur.x, a.y), cur, DVec2::new(a.x, cur.y)];
                    push_polyline(&mut d.lines_top, &fr, &pts, true, PREVIEW);
                }
                SketchTool::Circle if !s.pending.is_empty() => {
                    let c = s.pending[0];
                    let r = c.distance(cur);
                    if r > 1e-9 {
                        let l = cad_kernel::Loop2::circle(c, r, 64);
                        push_polyline(&mut d.lines_top, &fr, &l.pts, true, PREVIEW);
                        push_line(&mut d.lines_top, fr.to_3d(c), fr.to_3d(cur), [0.5, 0.6, 0.7, 0.8]);
                    }
                }
                _ => {}
            }
        }
    }
    d
}

fn overlays(app: &mut App, ui: &mut egui::Ui, painter: &egui::Painter, rect: Rect, vp: &Mat4) {
    let text_color = Color32::from_rgb(0x20, 0x26, 0x30);

    // axis triad (bottom-left)
    let origin = rect.left_bottom() + egui::vec2(48.0, -48.0);
    let (r, u) = (app.camera.right(), app.camera.up());
    let mut axes = [
        (DVec3::X, "X", Color32::from_rgb(210, 40, 40)),
        (DVec3::Y, "Y", Color32::from_rgb(40, 160, 50)),
        (DVec3::Z, "Z", Color32::from_rgb(40, 80, 220)),
    ];
    axes.sort_by(|a, b| a.0.dot(app.camera.dir()).partial_cmp(&b.0.dot(app.camera.dir())).unwrap());
    for (v, name, c) in axes {
        let end = origin + egui::vec2((v.dot(r) * 32.0) as f32, (-v.dot(u) * 32.0) as f32);
        painter.line_segment([origin, end], Stroke::new(2.5_f32, c));
        painter.text(end, egui::Align2::CENTER_CENTER, name, egui::FontId::proportional(13.0), c);
    }

    // sketch cursor marker + live dimension
    if let (Mode::Sketch(s), Some((_, fr))) = (&app.mode, active_sketch_frame(app)) {
        if let Some(cur) = s.cursor {
            if let Some(p) = project(vp, rect, fr.to_3d(cur)) {
                let c = if s.snapped_to_point { Color32::from_rgb(230, 120, 0) } else { Color32::from_rgb(30, 90, 200) };
                if s.snapped_to_point {
                    painter.rect_stroke(Rect::from_center_size(p, egui::vec2(9.0, 9.0)), 0.0, Stroke::new(1.5_f32, c));
                } else if s.tool != SketchTool::Select {
                    painter.line_segment([p - egui::vec2(6.0, 0.0), p + egui::vec2(6.0, 0.0)], Stroke::new(1.0_f32, c));
                    painter.line_segment([p - egui::vec2(0.0, 6.0), p + egui::vec2(0.0, 6.0)], Stroke::new(1.0_f32, c));
                }
                let label = match (s.tool, s.pending.last(), s.pending.first()) {
                    (SketchTool::Line, Some(&a), _) => {
                        let dv = cur - a;
                        format!("{} mm  {}°", fmt_num(dv.length()), fmt_num(dv.y.atan2(dv.x).to_degrees()))
                    }
                    (SketchTool::Rectangle, _, Some(&a)) => {
                        format!("{} × {} mm", fmt_num((cur.x - a.x).abs()), fmt_num((cur.y - a.y).abs()))
                    }
                    (SketchTool::Circle, _, Some(&c)) => format!("Ø {} mm", fmt_num(c.distance(cur) * 2.0)),
                    _ => format!("{}, {}", fmt_num(cur.x), fmt_num(cur.y)),
                };
                let galley_pos = p + egui::vec2(14.0, 14.0);
                let g = painter.layout_no_wrap(label, egui::FontId::monospace(12.0), text_color);
                let bg = Rect::from_min_size(galley_pos, g.size()).expand(3.0);
                painter.rect_filled(bg, 3.0, Color32::from_white_alpha(220));
                painter.galley(galley_pos, g, text_color);
            }
        }
    }

    // hint on an empty design
    if app.doc.features.is_empty() && matches!(app.mode, Mode::Idle) {
        painter.text(
            rect.center() - egui::vec2(0.0, rect.height() * 0.3),
            egui::Align2::CENTER_CENTER,
            "Start with  Create Sketch (S)  or a primitive — or File > Open Sample",
            egui::FontId::proportional(16.0),
            Color32::from_rgb(0x55, 0x60, 0x70),
        );
    }

    // view controls (top-right)
    let mut view: Option<StdView> = None;
    let mut fit = false;
    egui::Area::new(egui::Id::new("view_controls"))
        .fixed_pos(rect.right_top() + egui::vec2(-196.0, 10.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).inner_margin(6.0).show(ui, |ui| {
                ui.set_width(176.0);
                egui::Grid::new("views").spacing([4.0, 4.0]).show(ui, |ui| {
                    for (label, v) in [("Top", StdView::Top), ("Front", StdView::Front), ("Right", StdView::Right)] {
                        if ui.small_button(label).clicked() {
                            view = Some(v);
                        }
                    }
                    ui.end_row();
                    for (label, v) in [("Bottom", StdView::Bottom), ("Back", StdView::Back), ("Left", StdView::Left)] {
                        if ui.small_button(label).clicked() {
                            view = Some(v);
                        }
                    }
                    ui.end_row();
                    if ui.small_button("Home").clicked() {
                        view = Some(StdView::Home);
                    }
                    if ui.small_button("Fit (F)").clicked() {
                        fit = true;
                    }
                    let label = if app.camera.perspective { "Persp" } else { "Ortho" };
                    if ui.small_button(label).on_hover_text("Toggle perspective / orthographic").clicked() {
                        app.camera.perspective = !app.camera.perspective;
                    }
                    ui.end_row();
                });
            });
        });
    if let Some(v) = view {
        app.camera.set_view(v);
    }
    if fit {
        app.fit_all();
    }
    if resp_needs_repaint(app) {
        ui.ctx().request_repaint();
    }
}

fn resp_needs_repaint(app: &App) -> bool {
    matches!(app.mode, Mode::Sketch(_) | Mode::Command(_) | Mode::ChoosePlane { .. })
}
