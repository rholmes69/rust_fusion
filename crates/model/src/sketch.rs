//! Sketches: 2D geometry on a construction plane, with parametric dimensions.

use cad_kernel::expr::{self, ExprError};
use cad_kernel::profile::{find_regions, Frame, Loop2, Region};
use glam::{DVec2, DVec3};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Segments used to approximate a full circle.
pub const CIRCLE_SEGMENTS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlaneKind {
    /// Front plane, normal +Z
    XY,
    /// Ground / top plane, normal +Y
    XZ,
    /// Right plane, normal +X
    YZ,
}

impl PlaneKind {
    pub const ALL: [PlaneKind; 3] = [PlaneKind::XY, PlaneKind::XZ, PlaneKind::YZ];

    pub fn label(self) -> &'static str {
        match self {
            PlaneKind::XY => "XY plane",
            PlaneKind::XZ => "XZ plane",
            PlaneKind::YZ => "YZ plane",
        }
    }

    /// Frame for this plane offset along its normal. The application is Y-up.
    pub fn frame(self, offset: f64) -> Frame {
        let (u, v) = match self {
            PlaneKind::XY => (DVec3::X, DVec3::Y),
            PlaneKind::XZ => (DVec3::X, DVec3::NEG_Z),
            PlaneKind::YZ => (DVec3::NEG_Z, DVec3::Y),
        };
        Frame {
            origin: u.cross(v) * offset,
            u,
            v,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Entity {
    /// Connected line segments. Closed polylines form profiles.
    Polyline { points: Vec<[f64; 2]>, closed: bool },
    /// Corner rectangle: corner (x, y) plus signed width/height expressions.
    Rectangle {
        x: String,
        y: String,
        width: String,
        height: String,
    },
    /// Center + radius circle.
    Circle {
        cx: String,
        cy: String,
        radius: String,
    },
}

impl Entity {
    pub fn kind_name(&self) -> &'static str {
        match self {
            Entity::Polyline { closed: true, .. } => "Closed polyline",
            Entity::Polyline { .. } => "Polyline",
            Entity::Rectangle { .. } => "Rectangle",
            Entity::Circle { .. } => "Circle",
        }
    }

    /// Evaluate to a 2D polyline (and whether it is closed).
    pub fn evaluate(&self, vars: &HashMap<String, f64>) -> Result<(Vec<DVec2>, bool), ExprError> {
        let e = |s: &str| expr::eval(s, vars);
        Ok(match self {
            Entity::Polyline { points, closed } => (
                points.iter().map(|p| DVec2::new(p[0], p[1])).collect(),
                *closed,
            ),
            Entity::Rectangle {
                x,
                y,
                width,
                height,
            } => {
                let (x, y, w, h) = (e(x)?, e(y)?, e(width)?, e(height)?);
                (
                    vec![
                        DVec2::new(x, y),
                        DVec2::new(x + w, y),
                        DVec2::new(x + w, y + h),
                        DVec2::new(x, y + h),
                    ],
                    true,
                )
            }
            Entity::Circle { cx, cy, radius } => {
                let r = e(radius)?;
                if r <= 0.0 {
                    return Err(ExprError("circle radius must be positive".into()));
                }
                (
                    Loop2::circle(DVec2::new(e(cx)?, e(cy)?), r, CIRCLE_SEGMENTS).pts,
                    true,
                )
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sketch {
    pub plane: PlaneKind,
    /// Offset of the sketch plane along its normal (expression).
    pub offset: String,
    pub entities: Vec<Entity>,
    /// Geometric / dimensional constraints on polylines.
    #[serde(default)]
    pub constraints: Vec<crate::constraints::Constraint>,
}

impl Sketch {
    pub fn new(plane: PlaneKind) -> Sketch {
        Sketch {
            plane,
            offset: "0".into(),
            entities: Vec::new(),
            constraints: Vec::new(),
        }
    }

    /// Delete an entity and the constraints that use it (re-indexing the rest).
    pub fn remove_entity(&mut self, i: usize) {
        if i < self.entities.len() {
            self.entities.remove(i);
            self.constraints.retain_mut(|c| c.reindex_after_delete(i));
        }
    }

    /// Current polyline points (unsolved) keyed by entity index.
    pub fn polylines(&self) -> crate::constraints::Polys {
        self.entities
            .iter()
            .enumerate()
            .filter_map(|(i, e)| match e {
                Entity::Polyline { points, closed } => Some((
                    i,
                    (
                        points.iter().map(|p| DVec2::new(p[0], p[1])).collect(),
                        *closed,
                    ),
                )),
                _ => None,
            })
            .collect()
    }
}

/// The evaluated geometry of a sketch.
#[derive(Debug, Clone, Default)]
pub struct SketchGeometry {
    pub frame: Option<Frame>,
    /// One evaluated curve per entity (empty if the entity failed to evaluate).
    pub curves: Vec<(Vec<DVec2>, bool)>,
    /// Closed areas that can be extruded / revolved.
    pub regions: Vec<Region>,
    pub entity_errors: Vec<(usize, String)>,
    pub constraint_error: Option<String>,
}

impl Sketch {
    pub fn evaluate(&self, vars: &HashMap<String, f64>) -> Result<SketchGeometry, ExprError> {
        let offset = expr::eval(&self.offset, vars)?;
        let mut g = SketchGeometry {
            frame: Some(self.plane.frame(offset)),
            ..Default::default()
        };
        let mut loops = Vec::new();
        let mut solved = self.polylines();
        if !self.constraints.is_empty() {
            if let Err(e) = crate::constraints::solve(&mut solved, &self.constraints, vars) {
                g.constraint_error = Some(e);
            }
        }
        for (i, ent) in self.entities.iter().enumerate() {
            let res = match solved.remove(&i) {
                Some(p) => Ok(p),
                None => ent.evaluate(vars),
            };
            match res {
                Ok((pts, closed)) => {
                    if closed && pts.len() >= 3 {
                        loops.push(Loop2::new(pts.clone()));
                    }
                    g.curves.push((pts, closed));
                }
                Err(err) => {
                    g.entity_errors.push((i, err.0));
                    g.curves.push((Vec::new(), false));
                }
            }
        }
        g.regions = find_regions(&loops);
        Ok(g)
    }
}
