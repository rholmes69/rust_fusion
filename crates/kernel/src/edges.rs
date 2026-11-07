//! Feature-edge extraction for display: the visible "sharp" edges of a solid.
//!
//! BSP booleans leave T-junctions, so instead of matching triangle edges exactly we
//! bucket polygon edges by their supporting line and keep segments that are shared
//! by polygons whose normals differ by more than a crease angle.

use crate::csg::Solid;
use glam::DVec3;
use std::collections::HashMap;

pub fn feature_edges(solid: &Solid, crease_deg: f64) -> Vec<[DVec3; 2]> {
    let cos_crease = crease_deg.to_radians().cos();
    // key: quantised line (direction + closest point to origin)
    let q = |x: f64, s: f64| (x * s).round() as i64;
    let mut buckets: HashMap<[i64; 6], Vec<(f64, f64, DVec3, DVec3, DVec3)>> = HashMap::new();
    for p in &solid.polygons {
        let n = p.vertices.len();
        for i in 0..n {
            let a = p.vertices[i];
            let b = p.vertices[(i + 1) % n];
            let mut d = b - a;
            let len = d.length();
            if len < 1e-7 {
                continue;
            }
            d /= len;
            // canonical direction sign
            if d.x < -1e-9 || (d.x.abs() <= 1e-9 && (d.y < -1e-9 || (d.y.abs() <= 1e-9 && d.z < 0.0))) {
                d = -d;
            }
            let c = a - d * a.dot(d);
            let key = [q(d.x, 1e4), q(d.y, 1e4), q(d.z, 1e4), q(c.x, 1e3), q(c.y, 1e3), q(c.z, 1e3)];
            let (ta, tb) = (a.dot(d), b.dot(d));
            buckets.entry(key).or_default().push((ta.min(tb), ta.max(tb), p.plane.normal, c, d));
        }
    }
    let mut out = Vec::new();
    for segs in buckets.values() {
        // Split the line into elementary intervals and test each one.
        let mut cuts: Vec<f64> = segs.iter().flat_map(|s| [s.0, s.1]).collect();
        cuts.sort_by(|a, b| a.partial_cmp(b).unwrap());
        cuts.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
        let (c, d) = (segs[0].3, segs[0].4);
        let mut run: Option<(f64, f64)> = None;
        for w in cuts.windows(2) {
            let mid = (w[0] + w[1]) * 0.5;
            let normals: Vec<DVec3> =
                segs.iter().filter(|s| s.0 <= mid && mid <= s.1).map(|s| s.2).collect();
            let sharp = normals.len() == 1
                || normals.iter().enumerate().any(|(i, a)| {
                    normals[i + 1..].iter().any(|b| a.dot(*b) < cos_crease)
                });
            if sharp {
                run = Some(match run {
                    Some((s, e)) if (e - w[0]).abs() < 1e-6 => (s, w[1]),
                    Some((s, e)) => {
                        out.push([c + d * s, c + d * e]);
                        (w[0], w[1])
                    }
                    None => (w[0], w[1]),
                });
            }
        }
        if let Some((s, e)) = run {
            out.push([c + d * s, c + d * e]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn cube_has_twelve_edges() {
        let s = crate::primitives::cuboid(glam::DVec3::ZERO, glam::DVec3::ONE);
        assert_eq!(super::feature_edges(&s, 20.0).len(), 12);
    }
}
