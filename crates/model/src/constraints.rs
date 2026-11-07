//! 2D geometric constraints for sketch polylines and a damped Gauss–Newton solver.
//!
//! Constraints refer to polyline points (`entity`, `index`) and segments (segment `i` runs
//! from point `i` to `i+1`, wrapping on closed polylines). Dimensional constraints take
//! expressions, so a constrained polyline becomes parametric: change a parameter and the
//! shape is re-solved during regeneration. Solving starts from the stored points and
//! prefers the smallest change, so under-constrained sketches keep their drawn shape.

use cad_kernel::expr;
use glam::DVec2;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PointRef {
    pub entity: usize,
    pub index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SegRef {
    pub entity: usize,
    pub segment: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Constraint {
    Horizontal {
        seg: SegRef,
    },
    Vertical {
        seg: SegRef,
    },
    /// Segment length (expression).
    Length {
        seg: SegRef,
        value: String,
    },
    /// Absolute angle of the segment from the sketch x axis, degrees (expression).
    Angle {
        seg: SegRef,
        value: String,
    },
    Parallel {
        a: SegRef,
        b: SegRef,
    },
    Perpendicular {
        a: SegRef,
        b: SegRef,
    },
    Equal {
        a: SegRef,
        b: SegRef,
    },
    Coincident {
        a: PointRef,
        b: PointRef,
    },
    /// Pin a point to coordinates (expressions).
    Position {
        p: PointRef,
        x: String,
        y: String,
    },
    /// Mirror two points about the sketch's vertical ("y") or horizontal ("x") axis.
    Symmetric {
        a: PointRef,
        b: PointRef,
        axis: String,
    },
}

impl Constraint {
    pub fn label(&self) -> String {
        let s = |r: &SegRef| format!("e{}.s{}", r.entity + 1, r.segment + 1);
        let p = |r: &PointRef| format!("e{}.p{}", r.entity + 1, r.index + 1);
        match self {
            Constraint::Horizontal { seg } => format!("Horizontal {}", s(seg)),
            Constraint::Vertical { seg } => format!("Vertical {}", s(seg)),
            Constraint::Length { seg, value } => format!("Length {} = {value}", s(seg)),
            Constraint::Angle { seg, value } => format!("Angle {} = {value}", s(seg)),
            Constraint::Parallel { a, b } => format!("Parallel {} || {}", s(a), s(b)),
            Constraint::Perpendicular { a, b } => format!("Perpendicular {} to {}", s(a), s(b)),
            Constraint::Equal { a, b } => format!("Equal {} = {}", s(a), s(b)),
            Constraint::Coincident { a, b } => format!("Coincident {} = {}", p(a), p(b)),
            Constraint::Position { p: q, x, y } => format!("Position {} = ({x}, {y})", p(q)),
            Constraint::Symmetric { a, b, axis } => {
                format!("Symmetric {} <-> {} about {axis}", p(a), p(b))
            }
        }
    }

    /// Entities this constraint touches.
    pub fn entities(&self) -> Vec<usize> {
        match self {
            Constraint::Horizontal { seg }
            | Constraint::Vertical { seg }
            | Constraint::Length { seg, .. }
            | Constraint::Angle { seg, .. } => {
                vec![seg.entity]
            }
            Constraint::Parallel { a, b }
            | Constraint::Perpendicular { a, b }
            | Constraint::Equal { a, b } => vec![a.entity, b.entity],
            Constraint::Coincident { a, b } | Constraint::Symmetric { a, b, .. } => {
                vec![a.entity, b.entity]
            }
            Constraint::Position { p, .. } => vec![p.entity],
        }
    }

    /// Fix indices after entity `removed` was deleted; returns false if the constraint must go.
    pub fn reindex_after_delete(&mut self, removed: usize) -> bool {
        if self.entities().contains(&removed) {
            return false;
        }
        let fix = |e: &mut usize| {
            if *e > removed {
                *e -= 1
            }
        };
        match self {
            Constraint::Horizontal { seg }
            | Constraint::Vertical { seg }
            | Constraint::Length { seg, .. }
            | Constraint::Angle { seg, .. } => fix(&mut seg.entity),
            Constraint::Parallel { a, b }
            | Constraint::Perpendicular { a, b }
            | Constraint::Equal { a, b } => {
                fix(&mut a.entity);
                fix(&mut b.entity)
            }
            Constraint::Coincident { a, b } | Constraint::Symmetric { a, b, .. } => {
                fix(&mut a.entity);
                fix(&mut b.entity)
            }
            Constraint::Position { p, .. } => fix(&mut p.entity),
        }
        true
    }
}

/// Polylines being solved: entity index → (points, closed).
pub type Polys = HashMap<usize, (Vec<DVec2>, bool)>;

struct Layout {
    /// (entity, point) → variable offset (x at k, y at k+1)
    index: HashMap<(usize, usize), usize>,
    closed: HashMap<usize, (usize, bool)>,
}

impl Layout {
    fn pt(&self, x: &[f64], r: PointRef) -> Option<DVec2> {
        let k = *self.index.get(&(r.entity, r.index))?;
        Some(DVec2::new(x[k], x[k + 1]))
    }
    fn seg(&self, x: &[f64], s: SegRef) -> Option<(DVec2, DVec2)> {
        let (n, closed) = *self.closed.get(&s.entity)?;
        if n < 2 || (!closed && s.segment + 1 >= n) || s.segment >= n {
            return None;
        }
        let a = self.pt(
            x,
            PointRef {
                entity: s.entity,
                index: s.segment,
            },
        )?;
        let b = self.pt(
            x,
            PointRef {
                entity: s.entity,
                index: (s.segment + 1) % n,
            },
        )?;
        Some((a, b))
    }
}

/// Residuals of all constraints (lengths in mm, angles as unit-vector terms scaled by length).
fn residuals(
    cons: &[(Constraint, Vec<f64>)],
    lay: &Layout,
    x: &[f64],
    out: &mut Vec<f64>,
) -> Result<(), String> {
    out.clear();
    for (c, vals) in cons {
        let bad = || format!("{} refers to geometry that doesn't exist", c.label());
        match c {
            Constraint::Horizontal { seg } => {
                let (a, b) = lay.seg(x, *seg).ok_or_else(bad)?;
                out.push(b.y - a.y);
            }
            Constraint::Vertical { seg } => {
                let (a, b) = lay.seg(x, *seg).ok_or_else(bad)?;
                out.push(b.x - a.x);
            }
            Constraint::Length { seg, .. } => {
                let (a, b) = lay.seg(x, *seg).ok_or_else(bad)?;
                out.push(a.distance(b) - vals[0]);
            }
            Constraint::Angle { seg, .. } => {
                let (a, b) = lay.seg(x, *seg).ok_or_else(bad)?;
                let d = b - a;
                let t = vals[0].to_radians();
                // cross product with the target direction (scaled by length)
                out.push(d.x * t.sin() - d.y * t.cos());
                // keep the direction (not the opposite one)
                out.push((d.length() - (d.x * t.cos() + d.y * t.sin())) * 0.5);
            }
            Constraint::Parallel { a, b } => {
                let (a0, a1) = lay.seg(x, *a).ok_or_else(bad)?;
                let (b0, b1) = lay.seg(x, *b).ok_or_else(bad)?;
                let (u, v) = (a1 - a0, b1 - b0);
                out.push(u.perp_dot(v) / v.length().max(1e-9));
            }
            Constraint::Perpendicular { a, b } => {
                let (a0, a1) = lay.seg(x, *a).ok_or_else(bad)?;
                let (b0, b1) = lay.seg(x, *b).ok_or_else(bad)?;
                let (u, v) = (a1 - a0, b1 - b0);
                out.push(u.dot(v) / v.length().max(1e-9));
            }
            Constraint::Equal { a, b } => {
                let (a0, a1) = lay.seg(x, *a).ok_or_else(bad)?;
                let (b0, b1) = lay.seg(x, *b).ok_or_else(bad)?;
                out.push(a0.distance(a1) - b0.distance(b1));
            }
            Constraint::Coincident { a, b } => {
                let (p, q) = (
                    lay.pt(x, *a).ok_or_else(bad)?,
                    lay.pt(x, *b).ok_or_else(bad)?,
                );
                out.push(p.x - q.x);
                out.push(p.y - q.y);
            }
            Constraint::Position { p, .. } => {
                let q = lay.pt(x, *p).ok_or_else(bad)?;
                out.push(q.x - vals[0]);
                out.push(q.y - vals[1]);
            }
            Constraint::Symmetric { a, b, axis } => {
                let (p, q) = (
                    lay.pt(x, *a).ok_or_else(bad)?,
                    lay.pt(x, *b).ok_or_else(bad)?,
                );
                if axis == "x" || axis == "horizontal" {
                    out.push(p.x - q.x);
                    out.push(p.y + q.y);
                } else {
                    out.push(p.x + q.x);
                    out.push(p.y - q.y);
                }
            }
        }
    }
    Ok(())
}

fn solve_linear(mut a: Vec<f64>, mut b: Vec<f64>, n: usize) -> Option<Vec<f64>> {
    for col in 0..n {
        let piv = (col..n).max_by(|&i, &j| {
            a[i * n + col]
                .abs()
                .partial_cmp(&a[j * n + col].abs())
                .unwrap()
        })?;
        if a[piv * n + col].abs() < 1e-14 {
            return None;
        }
        if piv != col {
            for k in 0..n {
                a.swap(piv * n + k, col * n + k);
            }
            b.swap(piv, col);
        }
        let d = a[col * n + col];
        for r in col + 1..n {
            let f = a[r * n + col] / d;
            if f != 0.0 {
                for k in col..n {
                    a[r * n + k] -= f * a[col * n + k];
                }
                b[r] -= f * b[col];
            }
        }
    }
    let mut x = vec![0.0; n];
    for r in (0..n).rev() {
        let s: f64 = (r + 1..n).map(|k| a[r * n + k] * x[k]).sum();
        x[r] = (b[r] - s) / a[r * n + r];
    }
    Some(x)
}

/// Solve constraints in place. Returns the final residual norm or an error.
pub fn solve(
    polys: &mut Polys,
    constraints: &[Constraint],
    vars: &HashMap<String, f64>,
) -> Result<f64, String> {
    if constraints.is_empty() {
        return Ok(0.0);
    }
    // evaluate dimensional expressions
    let mut cons = Vec::new();
    for c in constraints {
        let ev = |s: &str| expr::eval(s, vars).map_err(|e| format!("{}: {}", c.label(), e.0));
        let vals = match c {
            Constraint::Length { value, .. } | Constraint::Angle { value, .. } => vec![ev(value)?],
            Constraint::Position { x, y, .. } => vec![ev(x)?, ev(y)?],
            _ => vec![],
        };
        if let Constraint::Length { .. } = c {
            if vals[0] <= 0.0 {
                return Err(format!("{}: length must be positive", c.label()));
            }
        }
        cons.push((c.clone(), vals));
    }
    let mut lay = Layout {
        index: HashMap::new(),
        closed: HashMap::new(),
    };
    let mut x0 = Vec::new();
    let mut ents: Vec<usize> = polys.keys().copied().collect();
    ents.sort();
    for e in &ents {
        let (pts, closed) = &polys[e];
        lay.closed.insert(*e, (pts.len(), *closed));
        for (i, p) in pts.iter().enumerate() {
            lay.index.insert((*e, i), x0.len());
            x0.push(p.x);
            x0.push(p.y);
        }
    }
    let n = x0.len();
    let mut x = x0.clone();
    let mut r = Vec::new();
    residuals(&cons, &lay, &x, &mut r)?;
    let cost = |r: &[f64], x: &[f64], reg: f64| {
        r.iter().map(|v| v * v).sum::<f64>()
            + x.iter()
                .zip(&x0)
                .map(|(a, b)| (reg * (a - b)).powi(2))
                .sum::<f64>()
    };
    let mut rt = Vec::new();
    // phase 1 prefers small moves away from the drawn geometry; phase 2 polishes exactly
    for reg in [1e-3, 0.0] {
        let mut lambda = 1e-3;
        for _ in 0..200 {
            let m = r.len();
            let base = cost(&r, &x, reg);
            if r.iter().map(|v| v * v).sum::<f64>().sqrt() < 1e-10 {
                break;
            }
            // numeric Jacobian
            let mut jac = vec![0.0; m * n];
            for j in 0..n {
                let h = 1e-7 * x[j].abs().max(1.0);
                let old = x[j];
                x[j] = old + h;
                residuals(&cons, &lay, &x, &mut rt)?;
                x[j] = old;
                for i in 0..m {
                    jac[i * n + j] = (rt[i] - r[i]) / h;
                }
            }
            // normal equations (J^T J + reg^2 I + lambda diag) dx = -(J^T r + reg^2 (x - x0))
            let mut a = vec![0.0; n * n];
            let mut g = vec![0.0; n];
            for i in 0..m {
                for p in 0..n {
                    let jp = jac[i * n + p];
                    if jp == 0.0 {
                        continue;
                    }
                    g[p] += jp * r[i];
                    for q in 0..n {
                        a[p * n + q] += jp * jac[i * n + q];
                    }
                }
            }
            for p in 0..n {
                a[p * n + p] += reg * reg;
                g[p] += reg * reg * (x[p] - x0[p]);
            }
            let mut improved = false;
            for _ in 0..12 {
                let mut al = a.clone();
                for p in 0..n {
                    al[p * n + p] += lambda * (1.0 + a[p * n + p]);
                }
                let Some(dx) = solve_linear(al, g.iter().map(|v| -v).collect(), n) else {
                    lambda *= 10.0;
                    continue;
                };
                let xn: Vec<f64> = x.iter().zip(&dx).map(|(a, b)| a + b).collect();
                residuals(&cons, &lay, &xn, &mut rt)?;
                if cost(&rt, &xn, reg) < base {
                    x = xn;
                    std::mem::swap(&mut r, &mut rt);
                    lambda = (lambda * 0.3).max(1e-12);
                    improved = true;
                    break;
                }
                lambda *= 10.0;
            }
            if !improved {
                break;
            }
        }
    }
    let norm = r.iter().map(|v| v * v).sum::<f64>().sqrt();
    for e in &ents {
        let (pts, _) = polys.get_mut(e).unwrap();
        for (i, p) in pts.iter_mut().enumerate() {
            let k = lay.index[&(*e, i)];
            *p = DVec2::new(x[k], x[k + 1]);
        }
    }
    if norm > 1e-4 {
        return Err(format!(
            "constraints conflict (residual {norm:.3}) — remove one"
        ));
    }
    Ok(norm)
}

/// Suggest horizontal / vertical constraints for nearly axis-aligned segments,
/// and coincidences between nearly touching endpoints of different polylines.
pub fn auto_constraints(polys: &Polys, existing: &[Constraint], tol_deg: f64) -> Vec<Constraint> {
    let mut out = Vec::new();
    let has = |c: &Constraint| existing.contains(c);
    let mut ents: Vec<usize> = polys.keys().copied().collect();
    ents.sort();
    for &e in &ents {
        let (pts, closed) = &polys[&e];
        let n = pts.len();
        let segs = if *closed { n } else { n.saturating_sub(1) };
        for s in 0..segs {
            let d = pts[(s + 1) % n] - pts[s];
            if d.length() < 1e-9 {
                continue;
            }
            let ang = d.y.atan2(d.x).to_degrees().rem_euclid(180.0);
            let seg = SegRef {
                entity: e,
                segment: s,
            };
            let c = if ang < tol_deg || ang > 180.0 - tol_deg {
                Some(Constraint::Horizontal { seg })
            } else if (ang - 90.0).abs() < tol_deg {
                Some(Constraint::Vertical { seg })
            } else {
                None
            };
            if let Some(c) = c.filter(|c| !has(c)) {
                out.push(c);
            }
        }
    }
    for (i, &e1) in ents.iter().enumerate() {
        for &e2 in &ents[i + 1..] {
            let (p1, c1) = &polys[&e1];
            let (p2, c2) = &polys[&e2];
            if *c1 || *c2 {
                continue;
            }
            for (a, pa) in [(0, p1[0]), (p1.len() - 1, *p1.last().unwrap())] {
                for (b, pb) in [(0, p2[0]), (p2.len() - 1, *p2.last().unwrap())] {
                    if pa.distance(pb) < 1e-3 {
                        let c = Constraint::Coincident {
                            a: PointRef {
                                entity: e1,
                                index: a,
                            },
                            b: PointRef {
                                entity: e2,
                                index: b,
                            },
                        };
                        if !has(&c) {
                            out.push(c);
                        }
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rough_quad_becomes_parametric_rectangle() {
        let mut polys: Polys = HashMap::new();
        polys.insert(
            0,
            (
                vec![
                    DVec2::new(0.0, 0.0),
                    DVec2::new(41.0, 1.5),
                    DVec2::new(39.0, 22.0),
                    DVec2::new(-1.0, 19.0),
                ],
                true,
            ),
        );
        let mut cons = auto_constraints(&polys, &[], 10.0);
        assert_eq!(cons.len(), 4);
        cons.push(Constraint::Length {
            seg: SegRef {
                entity: 0,
                segment: 0,
            },
            value: "w".into(),
        });
        cons.push(Constraint::Length {
            seg: SegRef {
                entity: 0,
                segment: 1,
            },
            value: "w/2".into(),
        });
        cons.push(Constraint::Position {
            p: PointRef {
                entity: 0,
                index: 0,
            },
            x: "0".into(),
            y: "0".into(),
        });
        let mut vars = HashMap::new();
        vars.insert("w".to_string(), 50.0);
        solve(&mut polys, &cons, &vars).unwrap();
        let p = &polys[&0].0;
        assert!(p[0].length() < 1e-6);
        assert!((p[1] - DVec2::new(50.0, 0.0)).length() < 1e-5, "{p:?}");
        assert!((p[2] - DVec2::new(50.0, 25.0)).length() < 1e-5, "{p:?}");
        assert!((p[3] - DVec2::new(0.0, 25.0)).length() < 1e-5, "{p:?}");
    }

    #[test]
    fn symmetric_angle_and_conflicts() {
        let mut polys: Polys = HashMap::new();
        polys.insert(
            0,
            (
                vec![
                    DVec2::new(-10.0, 0.0),
                    DVec2::new(11.0, 0.5),
                    DVec2::new(1.0, 15.0),
                ],
                true,
            ),
        );
        let cons = vec![
            Constraint::Symmetric {
                a: PointRef {
                    entity: 0,
                    index: 0,
                },
                b: PointRef {
                    entity: 0,
                    index: 1,
                },
                axis: "y".into(),
            },
            Constraint::Position {
                p: PointRef {
                    entity: 0,
                    index: 2,
                },
                x: "0".into(),
                y: "20".into(),
            },
            Constraint::Angle {
                seg: SegRef {
                    entity: 0,
                    segment: 0,
                },
                value: "0".into(),
            },
            Constraint::Length {
                seg: SegRef {
                    entity: 0,
                    segment: 0,
                },
                value: "30".into(),
            },
        ];
        solve(&mut polys, &cons, &HashMap::new()).unwrap();
        let p = &polys[&0].0;
        assert!(
            (p[0] - DVec2::new(-15.0, p[0].y)).length() < 1e-5 && (p[1].x - 15.0).abs() < 1e-5,
            "{p:?}"
        );
        let bad = vec![
            Constraint::Length {
                seg: SegRef {
                    entity: 0,
                    segment: 0,
                },
                value: "30".into(),
            },
            Constraint::Length {
                seg: SegRef {
                    entity: 0,
                    segment: 0,
                },
                value: "40".into(),
            },
        ];
        assert!(solve(&mut polys, &bad, &HashMap::new()).is_err());
    }
}
