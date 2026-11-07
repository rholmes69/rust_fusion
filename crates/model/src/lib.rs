//! cad-model: the parametric document.
//!
//! A [`Document`] is an ordered *timeline* of [`Feature`]s plus a table of user
//! [`Parameter`]s. [`Document::regenerate`] replays the timeline (up to the rollback
//! marker) to produce bodies and evaluated sketches. Every numeric input is an
//! expression string, so changing a parameter and regenerating updates the model.

pub mod commands;
pub mod constraints;
pub mod macros;
pub mod sketch;

use cad_kernel::csg::{BoolOp, Solid};
use cad_kernel::expr::{self, ExprError};
use cad_kernel::primitives;
use cad_kernel::profile;
use glam::{DVec2, DVec3};
use serde::{Deserialize, Serialize};
use sketch::{Sketch, SketchGeometry};
use std::collections::HashMap;

pub type FeatureId = u64;

pub const FILE_FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Parameter {
    pub name: String,
    pub expr: String,
    #[serde(default)]
    pub comment: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Operation {
    NewBody,
    Join,
    Cut,
    Intersect,
}

impl Operation {
    pub const ALL: [Operation; 4] = [
        Operation::NewBody,
        Operation::Join,
        Operation::Cut,
        Operation::Intersect,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Operation::NewBody => "New Body",
            Operation::Join => "Join",
            Operation::Cut => "Cut",
            Operation::Intersect => "Intersect",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExtentKind {
    OneSide,
    Symmetric,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RevolveAxis {
    /// The sketch's horizontal axis through its origin.
    SketchU,
    /// The sketch's vertical axis through its origin.
    SketchV,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    pub fn vec(self) -> DVec3 {
        match self {
            Axis::X => DVec3::X,
            Axis::Y => DVec3::Y,
            Axis::Z => DVec3::Z,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FeatureKind {
    Sketch(Sketch),
    /// Hollow out the solid made by an extrude, leaving walls of `thickness`.
    Shell {
        target: FeatureId,
        thickness: String,
        /// "end" (open at the extrude's far cap), "start", or "none" (closed hollow).
        open: String,
    },
    Extrude {
        sketch: FeatureId,
        profiles: Vec<usize>,
        distance: String,
        extent: ExtentKind,
        flip: bool,
        op: Operation,
        /// Optional fillet / chamfer of the cap edges.
        #[serde(default)]
        edges: Option<EdgeTreatment>,
    },
    Revolve {
        sketch: FeatureId,
        profiles: Vec<usize>,
        axis: RevolveAxis,
        angle: String,
        op: Operation,
    },
    Box {
        corner: [String; 3],
        size: [String; 3],
        op: Operation,
    },
    Cylinder {
        base: [String; 3],
        axis: Axis,
        radius: String,
        height: String,
        op: Operation,
    },
    Sphere {
        center: [String; 3],
        radius: String,
        op: Operation,
    },
}

impl FeatureKind {
    pub fn type_name(&self) -> &'static str {
        match self {
            FeatureKind::Sketch(_) => "Sketch",
            FeatureKind::Extrude { .. } => "Extrude",
            FeatureKind::Shell { .. } => "Shell",
            FeatureKind::Revolve { .. } => "Revolve",
            FeatureKind::Box { .. } => "Box",
            FeatureKind::Cylinder { .. } => "Cylinder",
            FeatureKind::Sphere { .. } => "Sphere",
        }
    }

    pub fn operation(&self) -> Option<Operation> {
        match self {
            FeatureKind::Sketch(_) | FeatureKind::Shell { .. } => None,
            FeatureKind::Extrude { op, .. }
            | FeatureKind::Revolve { op, .. }
            | FeatureKind::Box { op, .. }
            | FeatureKind::Cylinder { op, .. }
            | FeatureKind::Sphere { op, .. } => Some(*op),
        }
    }

    /// The sketch this feature consumes, if any.
    pub fn parent_sketch(&self) -> Option<FeatureId> {
        match self {
            FeatureKind::Extrude { sketch, .. } | FeatureKind::Revolve { sketch, .. } => {
                Some(*sketch)
            }
            _ => None,
        }
    }

    /// The feature this one is built from (sketch or target), for dependency tracking.
    pub fn depends_on(&self) -> Option<FeatureId> {
        match self {
            FeatureKind::Shell { target, .. } => Some(*target),
            other => other.parent_sketch(),
        }
    }
}

/// Fillet or chamfer of an extrude's cap edges.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EdgeTreatment {
    pub kind: cad_kernel::profile::EdgeKind,
    pub size: String,
    /// Apply at the start cap (sketch plane side).
    #[serde(default)]
    pub start: bool,
    /// Apply at the end cap.
    #[serde(default = "yes")]
    pub end: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Feature {
    pub id: FeatureId,
    pub name: String,
    #[serde(default)]
    pub suppressed: bool,
    pub kind: FeatureKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    #[serde(default = "default_version")]
    pub version: u32,
    pub name: String,
    pub parameters: Vec<Parameter>,
    pub features: Vec<Feature>,
    /// Number of timeline features that are active. `None` = all (marker at end).
    #[serde(default)]
    pub rollback: Option<usize>,
    pub next_id: FeatureId,
}

fn default_version() -> u32 {
    FILE_FORMAT_VERSION
}

impl Default for Document {
    fn default() -> Self {
        Document {
            version: FILE_FORMAT_VERSION,
            name: "Untitled".into(),
            parameters: Vec::new(),
            features: Vec::new(),
            rollback: None,
            next_id: 1,
        }
    }
}

/// A solid body produced by regeneration.
#[derive(Debug, Clone)]
pub struct Body {
    pub name: String,
    pub solid: Solid,
    /// Feature that created this body.
    pub created_by: FeatureId,
}

/// Everything produced by replaying the timeline.
#[derive(Debug, Clone, Default)]
pub struct Model {
    pub params: HashMap<String, f64>,
    pub param_errors: HashMap<String, String>,
    pub bodies: Vec<Body>,
    pub sketches: HashMap<FeatureId, SketchGeometry>,
    pub errors: HashMap<FeatureId, String>,
}

impl Document {
    pub fn alloc_id(&mut self) -> FeatureId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Unique default name like "Sketch3".
    pub fn unique_name(&self, base: &str) -> String {
        (1..)
            .map(|i| format!("{base}{i}"))
            .find(|n| !self.features.iter().any(|f| &f.name == n))
            .unwrap()
    }

    pub fn active_len(&self) -> usize {
        self.rollback
            .unwrap_or(self.features.len())
            .min(self.features.len())
    }

    /// Insert a feature at the rollback marker and advance the marker past it.
    pub fn insert_feature(&mut self, name: String, kind: FeatureKind) -> FeatureId {
        let id = self.alloc_id();
        let at = self.active_len();
        self.features.insert(
            at,
            Feature {
                id,
                name,
                suppressed: false,
                kind,
            },
        );
        if self.rollback.is_some() {
            self.rollback = Some(at + 1);
        }
        if self.rollback == Some(self.features.len()) {
            self.rollback = None;
        }
        id
    }

    pub fn index_of(&self, id: FeatureId) -> Option<usize> {
        self.features.iter().position(|f| f.id == id)
    }

    pub fn feature(&self, id: FeatureId) -> Option<&Feature> {
        self.features.iter().find(|f| f.id == id)
    }

    pub fn feature_mut(&mut self, id: FeatureId) -> Option<&mut Feature> {
        self.features.iter_mut().find(|f| f.id == id)
    }

    /// Features that depend on `id` (directly).
    pub fn dependents(&self, id: FeatureId) -> Vec<FeatureId> {
        self.features
            .iter()
            .filter(|f| f.kind.depends_on() == Some(id))
            .map(|f| f.id)
            .collect()
    }

    /// Delete a feature and everything that consumes it.
    pub fn delete_feature(&mut self, id: FeatureId) {
        let mut doomed = vec![id];
        let mut i = 0;
        while i < doomed.len() {
            doomed.extend(self.dependents(doomed[i]));
            i += 1;
        }
        let active = self.active_len();
        let removed_before_marker = self.features[..active]
            .iter()
            .filter(|f| doomed.contains(&f.id))
            .count();
        self.features.retain(|f| !doomed.contains(&f.id));
        if let Some(r) = self.rollback {
            let r = r - removed_before_marker;
            self.rollback = if r >= self.features.len() {
                None
            } else {
                Some(r)
            };
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("document serializes")
    }

    pub fn from_json(s: &str) -> Result<Document, String> {
        let d: Document = serde_json::from_str(s).map_err(|e| e.to_string())?;
        if d.version > FILE_FORMAT_VERSION {
            return Err(format!(
                "file format version {} is newer than supported ({})",
                d.version, FILE_FORMAT_VERSION
            ));
        }
        Ok(d)
    }

    /// Replay the timeline.
    pub fn regenerate(&self) -> Model {
        let pairs: Vec<(String, String)> = self
            .parameters
            .iter()
            .map(|p| (p.name.clone(), p.expr.clone()))
            .collect();
        let (params, param_errors) = expr::resolve_parameters(&pairs);
        let mut m = Model {
            params,
            param_errors,
            ..Default::default()
        };
        let mut body_counter = 0usize;

        for f in &self.features[..self.active_len()] {
            if f.suppressed {
                continue;
            }
            if let Err(e) = regen_feature(f, &self.features, &mut m, &mut body_counter) {
                m.errors.insert(f.id, e);
            }
        }
        m
    }
}

fn v3(e: &[String; 3], vars: &HashMap<String, f64>) -> Result<DVec3, ExprError> {
    Ok(DVec3::new(
        expr::eval(&e[0], vars)?,
        expr::eval(&e[1], vars)?,
        expr::eval(&e[2], vars)?,
    ))
}

fn regen_feature(f: &Feature, doc_features: &[Feature], m: &mut Model, counter: &mut usize) -> Result<(), String> {
    let vars = m.params.clone();
    let ev = |s: &str| expr::eval(s, &vars).map_err(|e| e.0);
    let (tool, op) = match &f.kind {
        FeatureKind::Sketch(sk) => {
            let g = sk.evaluate(&vars).map_err(|e| e.0)?;
            let err = g
                .entity_errors
                .first()
                .map(|(i, e)| format!("entity {}: {e}", i + 1))
                .or_else(|| g.constraint_error.clone());
            m.sketches.insert(f.id, g);
            return match err {
                Some(e) => Err(e),
                None => Ok(()),
            };
        }
        FeatureKind::Extrude {
            sketch,
            profiles,
            distance,
            extent,
            flip,
            op,
            edges,
        } => {
            let g = m
                .sketches
                .get(sketch)
                .ok_or("sketch is missing or failed")?;
            let frame = g.frame.ok_or("sketch has no plane")?;
            let d = ev(distance)?;
            let d = if *flip { -d } else { d };
            let (d0, d1) = match extent {
                ExtentKind::OneSide => (0.0, d),
                ExtentKind::Symmetric => (-d / 2.0, d / 2.0),
            };
            if d.abs() < 1e-9 {
                return Err("distance is zero".into());
            }
            let tool = match edges {
                None => sweep_profiles(g, profiles, |r| Ok(profile::extrude(&frame, r, d0, d1)))?,
                Some(e) => {
                    let size = ev(&e.size)?;
                    if size <= 0.0 {
                        return Err("fillet/chamfer size must be positive".into());
                    }
                    // start/end refer to the sketch-plane cap and the far cap
                    let (s0, s1) = (if e.start { size } else { 0.0 }, if e.end { size } else { 0.0 });
                    let (a, b) = if d1 >= d0 { (s0, s1) } else { (s1, s0) };
                    sweep_profiles(g, profiles, |r| profile::extrude_treated(&frame, r, d0, d1, e.kind, a, b))?
                }
            };
            (tool, *op)
        }
        FeatureKind::Shell { target, thickness, open } => {
            let t = ev(thickness)?;
            if t <= 0.0 {
                return Err("shell thickness must be positive".into());
            }
            let Some(FeatureKind::Extrude { sketch, profiles, distance, extent, flip, .. }) =
                doc_features.iter().find(|x| x.id == *target).map(|x| &x.kind)
            else {
                return Err("the shell target must be an extrude".into());
            };
            let g = m.sketches.get(sketch).ok_or("the extrude's sketch is missing")?;
            let frame = g.frame.ok_or("sketch has no plane")?;
            let d = ev(distance)?;
            let d = if *flip { -d } else { d };
            let (a, b) = match extent {
                ExtentKind::OneSide => (0.0, d),
                ExtentKind::Symmetric => (-d / 2.0, d / 2.0),
            };
            // "start"/"end" follow the extrude direction
            let (lo, hi) = (a.min(b), a.max(b));
            let start_is_lo = b >= a;
            let open_lo = (open == "start" && start_is_lo) || (open == "end" && !start_is_lo);
            let open_hi = (open == "end" && start_is_lo) || (open == "start" && !start_is_lo);
            let z0 = if open_lo { lo - 1.0 } else { lo + t };
            let z1 = if open_hi { hi + 1.0 } else { hi - t };
            if z1 <= z0 {
                return Err("shell is thicker than the part".into());
            }
            let tool = sweep_profiles(g, profiles, |r| {
                let inner = profile::offset_region(r, t).ok_or("shell is thicker than the profile allows")?;
                Ok(profile::extrude(&frame, &inner, z0, z1))
            })?;
            (tool, Operation::Cut)
        }
        FeatureKind::Revolve {
            sketch,
            profiles,
            axis,
            angle,
            op,
        } => {
            let g = m
                .sketches
                .get(sketch)
                .ok_or("sketch is missing or failed")?;
            let frame = g.frame.ok_or("sketch has no plane")?;
            let a = ev(angle)?;
            let dir = match axis {
                RevolveAxis::SketchU => DVec2::X,
                RevolveAxis::SketchV => DVec2::Y,
            };
            let tool = sweep_profiles(g, profiles, |r| {
                profile::revolve(&frame, r, DVec2::ZERO, dir, a)
            })?;
            (tool, *op)
        }
        FeatureKind::Box { corner, size, op } => {
            let c = v3(corner, &vars).map_err(|e| e.0)?;
            let s = v3(size, &vars).map_err(|e| e.0)?;
            if s.x.abs() < 1e-9 || s.y.abs() < 1e-9 || s.z.abs() < 1e-9 {
                return Err("box size must be non-zero".into());
            }
            (primitives::cuboid(c, c + s), *op)
        }
        FeatureKind::Cylinder {
            base,
            axis,
            radius,
            height,
            op,
        } => {
            let b = v3(base, &vars).map_err(|e| e.0)?;
            let (r, h) = (ev(radius)?, ev(height)?);
            if r <= 0.0 || h.abs() < 1e-9 {
                return Err("radius must be positive and height non-zero".into());
            }
            let (b, dir) = if h < 0.0 {
                (b + axis.vec() * h, axis.vec())
            } else {
                (b, axis.vec())
            };
            (
                primitives::cylinder(b, dir, r, h.abs(), sketch::CIRCLE_SEGMENTS),
                *op,
            )
        }
        FeatureKind::Sphere { center, radius, op } => {
            let c = v3(center, &vars).map_err(|e| e.0)?;
            let r = ev(radius)?;
            if r <= 0.0 {
                return Err("radius must be positive".into());
            }
            (primitives::sphere(c, r, 48, 24), *op)
        }
    };
    apply_operation(m, f, tool, op, counter)
}

fn sweep_profiles(
    g: &SketchGeometry,
    profiles: &[usize],
    mut sweep: impl FnMut(&profile::Region) -> Result<Solid, String>,
) -> Result<Solid, String> {
    if profiles.is_empty() {
        return Err("no profile selected".into());
    }
    let mut tool = Solid::default();
    for &p in profiles {
        let r = g
            .regions
            .get(p)
            .ok_or_else(|| format!("profile {} no longer exists", p + 1))?;
        tool = tool.union(&sweep(r)?);
    }
    Ok(tool)
}

fn overlaps(a: &Solid, b: &Solid) -> bool {
    match (a.bounds(), b.bounds()) {
        (Some((alo, ahi)), Some((blo, bhi))) => {
            alo.cmple(bhi + 1e-6).all() && blo.cmple(ahi + 1e-6).all()
        }
        _ => false,
    }
}

fn apply_operation(
    m: &mut Model,
    f: &Feature,
    tool: Solid,
    op: Operation,
    counter: &mut usize,
) -> Result<(), String> {
    if tool.is_empty() {
        return Err("operation produced no geometry".into());
    }
    let mut new_body = |m: &mut Model, solid: Solid| {
        *counter += 1;
        m.bodies.push(Body {
            name: format!("Body{counter}"),
            solid,
            created_by: f.id,
        });
    };
    let targets: Vec<usize> = (0..m.bodies.len())
        .filter(|&i| overlaps(&m.bodies[i].solid, &tool))
        .collect();
    match op {
        Operation::NewBody => new_body(m, tool),
        Operation::Join => {
            if targets.is_empty() {
                new_body(m, tool);
            } else {
                let first = targets[0];
                let mut acc = m.bodies[first].solid.union(&tool);
                for &i in targets[1..].iter() {
                    acc = acc.union(&m.bodies[i].solid);
                }
                m.bodies[first].solid = acc;
                for &i in targets[1..].iter().rev() {
                    m.bodies.remove(i);
                }
            }
        }
        Operation::Cut | Operation::Intersect => {
            if targets.is_empty() {
                return Err("no bodies intersect the tool".into());
            }
            let bop = if op == Operation::Cut {
                BoolOp::Subtract
            } else {
                BoolOp::Intersect
            };
            for &i in &targets {
                let r = m.bodies[i].solid.boolean(&tool, bop);
                m.bodies[i].solid = r;
            }
            m.bodies
                .retain(|b| !b.solid.is_empty() && b.solid.volume().abs() > 1e-9);
        }
    }
    Ok(())
}

/// A small demo part used for the "Sample" document.
pub fn sample_document() -> Document {
    use sketch::{Entity, PlaneKind};
    let mut d = Document {
        name: "Bracket sample".into(),
        ..Default::default()
    };
    d.parameters = vec![
        Parameter {
            name: "width".into(),
            expr: "80 mm".into(),
            comment: "Plate width".into(),
        },
        Parameter {
            name: "depth".into(),
            expr: "50 mm".into(),
            comment: "Plate depth".into(),
        },
        Parameter {
            name: "thickness".into(),
            expr: "8 mm".into(),
            comment: String::new(),
        },
        Parameter {
            name: "hole_d".into(),
            expr: "12 mm".into(),
            comment: "Hole diameter".into(),
        },
    ];
    let mut sk = Sketch::new(PlaneKind::XZ);
    sk.entities.push(Entity::Rectangle {
        x: "-width/2".into(),
        y: "-depth/2".into(),
        width: "width".into(),
        height: "depth".into(),
    });
    for sx in ["-1", "1"] {
        sk.entities.push(Entity::Circle {
            cx: format!("{sx} * (width/2 - 12)"),
            cy: "0".into(),
            radius: "hole_d / 2".into(),
        });
    }
    let s1 = d.insert_feature("Sketch1".into(), FeatureKind::Sketch(sk));
    d.insert_feature(
        "Extrude1".into(),
        FeatureKind::Extrude {
            sketch: s1,
            profiles: vec![0],
            distance: "thickness".into(),
            extent: ExtentKind::OneSide,
            flip: false,
            op: Operation::NewBody,
            edges: None,
        },
    );
    d.insert_feature(
        "Cylinder1".into(),
        FeatureKind::Cylinder {
            base: ["0".into(), "thickness".into(), "0".into()],
            axis: Axis::Y,
            radius: "14".into(),
            height: "20".into(),
            op: Operation::Join,
        },
    );
    d.insert_feature(
        "Cylinder2".into(),
        FeatureKind::Cylinder {
            base: ["0".into(), "-1".into(), "0".into()],
            axis: Axis::Y,
            radius: "8".into(),
            height: "thickness + 22".into(),
            op: Operation::Cut,
        },
    );
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use sketch::{Entity, PlaneKind};

    #[test]
    fn sample_regenerates_and_is_parametric() {
        let mut d = sample_document();
        let m = d.regenerate();
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        assert_eq!(m.bodies.len(), 1);
        let v1 = m.bodies[0].solid.volume();
        d.parameters[2].expr = "16 mm".into(); // thickness
        let v2 = d.regenerate().bodies[0].solid.volume();
        assert!(v2 > v1);
    }

    #[test]
    fn rollback_suppress_and_delete() {
        let mut d = sample_document();
        d.rollback = Some(2);
        let m = d.regenerate();
        assert_eq!(m.bodies.len(), 1);
        let plate = m.bodies[0].solid.volume();
        let expected = 80.0 * 50.0 * 8.0 - 2.0 * std::f64::consts::PI * 36.0 * 8.0;
        assert!((plate - expected).abs() / expected < 0.01);
        // inserting while rolled back goes at the marker
        let id = d.insert_feature(
            "Box1".into(),
            FeatureKind::Box {
                corner: ["100".into(), "0".into(), "0".into()],
                size: ["5".into(), "5".into(), "5".into()],
                op: Operation::NewBody,
            },
        );
        assert_eq!(d.index_of(id), Some(2));
        assert_eq!(d.rollback, Some(3));
        d.rollback = None;
        assert_eq!(d.regenerate().bodies.len(), 2);
        // deleting the sketch deletes its extrude too
        let sk = d.features[0].id;
        d.delete_feature(sk);
        assert!(d.features.iter().all(|f| f.kind.parent_sketch().is_none()));
    }

    #[test]
    fn fillet_chamfer_and_shell() {
        use crate::commands::{Command, Executor};
        let mut d = Document::default();
        let mut ex = Executor::default();
        let cmds: Vec<Command> = serde_json::from_str(r#"[
            {"action":"new_sketch","plane":"XZ"},
            {"action":"add_center_rectangle","width":60,"height":40},
            {"action":"extrude","distance":30},
            {"action":"fillet","feature":"Extrude1","size":3},
            {"action":"shell","feature":"Extrude1","thickness":2,"open":"end"}
        ]"#).unwrap();
        for c in &cmds {
            ex.apply(&mut d, c).unwrap();
        }
        let m = d.regenerate();
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        let v = m.bodies[0].solid.volume();
        let outer = 60.0 * 40.0 * 30.0;
        let cavity = 56.0 * 36.0 * 28.0;
        assert!(v < outer - cavity && v > (outer - cavity) * 0.8, "{v}");
        // chamfer instead, both caps, and shell closed
        let c: Command = serde_json::from_str(r#"{"action":"chamfer","feature":"Extrude1","size":2,"start":true,"end":true}"#).unwrap();
        ex.apply(&mut d, &c).unwrap();
        let c: Command = serde_json::from_str(r#"{"action":"set_feature_value","feature":"Shell1","field":"thickness","value":25}"#).unwrap();
        ex.apply(&mut d, &c).unwrap();
        assert!(!d.regenerate().errors.is_empty(), "too-thick shell must error");
        // deleting the extrude removes the shell too
        let id = d.features[1].id;
        d.delete_feature(id);
        assert_eq!(d.features.len(), 1);
    }

    #[test]
    fn json_round_trip() {
        let d = sample_document();
        let back = Document::from_json(&d.to_json()).unwrap();
        assert_eq!(d, back);
    }

    #[test]
    fn revolve_feature() {
        let mut d = Document::default();
        let mut sk = Sketch::new(PlaneKind::XY);
        sk.entities.push(Entity::Rectangle {
            x: "10".into(),
            y: "0".into(),
            width: "5".into(),
            height: "20".into(),
        });
        let s = d.insert_feature("Sketch1".into(), FeatureKind::Sketch(sk));
        d.insert_feature(
            "Revolve1".into(),
            FeatureKind::Revolve {
                sketch: s,
                profiles: vec![0],
                axis: RevolveAxis::SketchV,
                angle: "360 deg".into(),
                op: Operation::NewBody,
            },
        );
        let m = d.regenerate();
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        let exact = std::f64::consts::PI * (225.0 - 100.0) * 20.0;
        assert!((m.bodies[0].solid.volume() - exact).abs() / exact < 0.01);
    }

    #[test]
    fn errors_are_reported_not_fatal() {
        let mut d = sample_document();
        d.parameters[0].expr = "oops +".into();
        let m = d.regenerate();
        assert!(m.param_errors.contains_key("width"));
        assert!(!m.errors.is_empty());
    }
}
