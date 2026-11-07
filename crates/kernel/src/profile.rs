//! 2D closed loops, region detection (outer loops with holes), and the
//! sweep operations that turn regions into solids: extrude and revolve.

use crate::csg::{Polygon, Solid};
use glam::{DQuat, DVec2, DVec3};
use serde::{Deserialize, Serialize};

/// A closed 2D polyline (implicitly closed; the last point is not repeated).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Loop2 {
    pub pts: Vec<DVec2>,
}

impl Loop2 {
    pub fn new(mut pts: Vec<DVec2>) -> Loop2 {
        pts.dedup_by(|a, b| a.distance(*b) < 1e-9);
        if pts.len() > 1 && pts[0].distance(*pts.last().unwrap()) < 1e-9 {
            pts.pop();
        }
        Loop2 { pts }
    }

    pub fn signed_area(&self) -> f64 {
        let n = self.pts.len();
        (0..n)
            .map(|i| {
                let a = self.pts[i];
                let b = self.pts[(i + 1) % n];
                a.x * b.y - b.x * a.y
            })
            .sum::<f64>()
            * 0.5
    }

    pub fn area(&self) -> f64 {
        self.signed_area().abs()
    }

    pub fn contains(&self, p: DVec2) -> bool {
        let n = self.pts.len();
        let mut inside = false;
        let mut j = n - 1;
        for i in 0..n {
            let (a, b) = (self.pts[i], self.pts[j]);
            if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
                inside = !inside;
            }
            j = i;
        }
        inside
    }

    fn oriented(&self, ccw: bool) -> Loop2 {
        let mut l = self.clone();
        if (l.signed_area() > 0.0) != ccw {
            l.pts.reverse();
        }
        l
    }

    /// A point guaranteed to be strictly inside this loop (for containment tests).
    fn interior_sample(&self) -> DVec2 {
        // midpoint of first edge nudged toward the inside
        let l = self.oriented(true);
        let a = l.pts[0];
        let b = l.pts[1 % l.pts.len()];
        let d = (b - a).normalize_or_zero();
        let inward = DVec2::new(-d.y, d.x);
        (a + b) * 0.5 + inward * 1e-6
    }

    pub fn circle(center: DVec2, r: f64, segments: usize) -> Loop2 {
        Loop2::new(
            (0..segments)
                .map(|i| {
                    let t = i as f64 / segments as f64 * std::f64::consts::TAU;
                    center + DVec2::new(t.cos(), t.sin()) * r
                })
                .collect(),
        )
    }
}

/// A face region: one outer boundary (CCW) and zero or more holes (CW).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Region {
    pub outer: Loop2,
    pub holes: Vec<Loop2>,
    /// Nesting depth of the outer loop (0 = not inside any other loop).
    #[serde(default)]
    pub depth: usize,
}

impl Region {
    pub fn contains(&self, p: DVec2) -> bool {
        self.outer.contains(p) && !self.holes.iter().any(|h| h.contains(p))
    }

    pub fn area(&self) -> f64 {
        self.outer.area() - self.holes.iter().map(|h| h.area()).sum::<f64>()
    }

    /// Triangulate (ear clipping with hole bridging via earcutr).
    pub fn triangulate(&self) -> Vec<[DVec2; 3]> {
        let mut coords: Vec<f64> = Vec::new();
        let mut hole_idx = Vec::new();
        let mut all: Vec<DVec2> = Vec::new();
        for p in &self.outer.pts {
            coords.extend([p.x, p.y]);
            all.push(*p);
        }
        for h in &self.holes {
            hole_idx.push(all.len());
            for p in &h.pts {
                coords.extend([p.x, p.y]);
                all.push(*p);
            }
        }
        let idx = earcutr::earcut(&coords, &hole_idx, 2).unwrap_or_default();
        idx.chunks_exact(3)
            .map(|t| {
                let tri = [all[t[0]], all[t[1]], all[t[2]]];
                // force CCW
                let cross = (tri[1] - tri[0]).perp_dot(tri[2] - tri[0]);
                if cross < 0.0 {
                    [tri[0], tri[2], tri[1]]
                } else {
                    tri
                }
            })
            .collect()
    }
}

/// Build face regions from a set of closed, non-intersecting loops.
///
/// Every loop yields one region whose holes are the loops directly nested in it,
/// so a circle inside a rectangle yields two regions: the plate-with-hole and the disc.
pub fn find_regions(loops: &[Loop2]) -> Vec<Region> {
    let loops: Vec<Loop2> = loops.iter().filter(|l| l.pts.len() >= 3 && l.area() > 1e-9).cloned().collect();
    // parent[i] = smallest loop that contains loop i
    let parent: Vec<Option<usize>> = (0..loops.len())
        .map(|i| {
            let s = loops[i].interior_sample();
            (0..loops.len())
                .filter(|&j| j != i && loops[j].area() > loops[i].area() && loops[j].contains(s))
                .min_by(|&a, &b| loops[a].area().partial_cmp(&loops[b].area()).unwrap())
        })
        .collect();
    let depth = |mut i: usize| {
        let mut d = 0;
        while let Some(p) = parent[i] {
            d += 1;
            i = p;
        }
        d
    };
    (0..loops.len())
        .map(|i| Region {
            depth: depth(i),
            outer: loops[i].oriented(true),
            holes: (0..loops.len())
                .filter(|&j| parent[j] == Some(i))
                .map(|j| loops[j].oriented(false))
                .collect(),
        })
        .collect()
}

/// An oriented plane frame in 3D: `origin + u*x + v*y`, normal = u × v.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub origin: DVec3,
    pub u: DVec3,
    pub v: DVec3,
}

impl Frame {
    pub fn normal(&self) -> DVec3 {
        self.u.cross(self.v).normalize()
    }
    pub fn to_3d(&self, p: DVec2) -> DVec3 {
        self.origin + self.u * p.x + self.v * p.y
    }
    pub fn to_2d(&self, p: DVec3) -> DVec2 {
        let d = p - self.origin;
        DVec2::new(d.dot(self.u), d.dot(self.v))
    }
}

fn orient_outward(mut s: Solid) -> Solid {
    if s.volume() < 0.0 {
        for p in &mut s.polygons {
            p.vertices.reverse();
            p.plane.normal = -p.plane.normal;
            p.plane.w = -p.plane.w;
        }
    }
    s
}

/// Extrude a region along the frame normal from offset `d0` to `d1`.
pub fn extrude(frame: &Frame, region: &Region, d0: f64, d1: f64) -> Solid {
    let (d0, d1) = if d0 <= d1 { (d0, d1) } else { (d1, d0) };
    if (d1 - d0).abs() < 1e-9 {
        return Solid::default();
    }
    let n = frame.normal();
    let at = |p: DVec2, d: f64| frame.to_3d(p) + n * d;
    let mut polys = Vec::new();
    for t in region.triangulate() {
        polys.extend(Polygon::new(vec![at(t[0], d1), at(t[1], d1), at(t[2], d1)]));
        polys.extend(Polygon::new(vec![at(t[0], d0), at(t[2], d0), at(t[1], d0)]));
    }
    for l in std::iter::once(&region.outer).chain(region.holes.iter()) {
        let k = l.pts.len();
        for i in 0..k {
            let a = l.pts[i];
            let b = l.pts[(i + 1) % k];
            polys.extend(Polygon::new(vec![at(a, d0), at(b, d0), at(b, d1), at(a, d1)]));
        }
    }
    orient_outward(Solid::from_polygons(polys))
}

/// Offset a loop to its left by `d` (inward for CCW outer loops, outward for CW holes)
/// using mitred corners. Returns `None` if the offset collapses or flips the loop.
pub fn offset_loop(l: &Loop2, d: f64) -> Option<Loop2> {
    let n = l.pts.len();
    if n < 3 {
        return None;
    }
    if d.abs() < 1e-12 {
        return Some(l.clone());
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let p0 = l.pts[(i + n - 1) % n];
        let p1 = l.pts[i];
        let p2 = l.pts[(i + 1) % n];
        let e0 = (p1 - p0).normalize_or_zero();
        let e1 = (p2 - p1).normalize_or_zero();
        let n0 = DVec2::new(-e0.y, e0.x);
        let n1 = DVec2::new(-e1.y, e1.x);
        let bis = n0 + n1;
        let cos_half = (bis.length() / 2.0).max(0.25); // limit spikes on very sharp corners
        let m = if bis.length() < 1e-9 { n0 } else { bis.normalize() };
        out.push(p1 + m * (d / cos_half));
    }
    let r = Loop2 { pts: out };
    let (a0, a1) = (l.signed_area(), r.signed_area());
    if a0.signum() != a1.signum() || a1.abs() < 1e-9 {
        return None;
    }
    // inward offsets must shrink, outward must grow
    if (d > 0.0) == (a0 > 0.0) && a1.abs() >= a0.abs() || (d < 0.0) == (a0 > 0.0) && a1.abs() <= a0.abs() {
        return None;
    }
    // reject self-crossing (edges reversed)
    for i in 0..n {
        let e_old = l.pts[(i + 1) % n] - l.pts[i];
        let e_new = r.pts[(i + 1) % n] - r.pts[i];
        if e_old.dot(e_new) < 0.0 {
            return None;
        }
    }
    Some(r)
}

/// Shrink a region into its material by `d` (outer loop moves in, holes grow).
pub fn offset_region(r: &Region, d: f64) -> Option<Region> {
    Some(Region {
        outer: offset_loop(&r.outer, d)?,
        holes: r.holes.iter().map(|h| offset_loop(h, d)).collect::<Option<Vec<_>>>()?,
        depth: r.depth,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeKind {
    Fillet,
    Chamfer,
}

/// Extrude with rounded or chamfered cap edges. `start`/`end` give the treatment size at
/// the `d0` / `d1` caps (0 = sharp).
pub fn extrude_treated(frame: &Frame, region: &Region, d0: f64, d1: f64, kind: EdgeKind, start: f64, end: f64) -> Result<Solid, String> {
    let (d0, d1, start, end) = if d0 <= d1 { (d0, d1, start, end) } else { (d1, d0, end, start) };
    if start <= 0.0 && end <= 0.0 {
        return Ok(extrude(frame, region, d0, d1));
    }
    if start + end >= (d1 - d0) - 1e-9 {
        return Err("fillet/chamfer is larger than the extrusion height".into());
    }
    // (height, inward offset) layers from bottom to top
    let steps = if kind == EdgeKind::Fillet { 8 } else { 1 };
    let profile = |s: f64, t: f64| -> (f64, f64) {
        // t: 0 at the cap, 1 where the side wall starts
        match kind {
            EdgeKind::Chamfer => (s * t, s * (1.0 - t)),
            EdgeKind::Fillet => {
                let a = t * std::f64::consts::FRAC_PI_2;
                (s - s * a.cos(), s - s * a.sin())
            }
        }
    };
    let mut layers: Vec<(f64, f64)> = Vec::new();
    if start > 0.0 {
        for i in 0..=steps {
            let (h, o) = profile(start, i as f64 / steps as f64);
            layers.push((d0 + h, o));
        }
    } else {
        layers.push((d0, 0.0));
    }
    if end > 0.0 {
        for i in (0..=steps).rev() {
            let (h, o) = profile(end, i as f64 / steps as f64);
            layers.push((d1 - h, o));
        }
    } else {
        layers.push((d1, 0.0));
    }
    let regions: Vec<Region> = layers
        .iter()
        .map(|&(_, o)| offset_region(region, o).ok_or_else(|| "fillet/chamfer is too large for this profile".to_string()))
        .collect::<Result<_, _>>()?;
    let nrm = frame.normal();
    let at = |p: DVec2, h: f64| frame.to_3d(p) + nrm * h;
    let mut polys = Vec::new();
    let (first, last) = (&regions[0], &regions[regions.len() - 1]);
    for t in first.triangulate() {
        polys.extend(Polygon::new(vec![at(t[0], layers[0].0), at(t[2], layers[0].0), at(t[1], layers[0].0)]));
    }
    let top = layers[layers.len() - 1].0;
    for t in last.triangulate() {
        polys.extend(Polygon::new(vec![at(t[0], top), at(t[1], top), at(t[2], top)]));
    }
    for k in 0..layers.len() - 1 {
        let (ha, hb) = (layers[k].0, layers[k + 1].0);
        if (hb - ha).abs() < 1e-12 && layers[k].1 == layers[k + 1].1 {
            continue;
        }
        let la: Vec<&Loop2> = std::iter::once(&regions[k].outer).chain(regions[k].holes.iter()).collect();
        let lb: Vec<&Loop2> = std::iter::once(&regions[k + 1].outer).chain(regions[k + 1].holes.iter()).collect();
        for (a, b) in la.iter().zip(lb.iter()) {
            let n = a.pts.len();
            for i in 0..n {
                let j = (i + 1) % n;
                let quad = [at(a.pts[i], ha), at(a.pts[j], ha), at(b.pts[j], hb), at(b.pts[i], hb)];
                // split into triangles: the quad may be slightly non-planar
                polys.extend(Polygon::new(vec![quad[0], quad[1], quad[2]]));
                polys.extend(Polygon::new(vec![quad[0], quad[2], quad[3]]));
            }
        }
    }
    Ok(orient_outward(Solid::from_polygons(polys)))
}

/// Revolve a region about an axis lying in the sketch plane.
/// `axis_point`/`axis_dir` are in sketch (2D) coordinates; `angle_deg` in degrees.
pub fn revolve(
    frame: &Frame,
    region: &Region,
    axis_point: DVec2,
    axis_dir: DVec2,
    angle_deg: f64,
) -> Result<Solid, String> {
    let dir2 = axis_dir.normalize_or_zero();
    if dir2 == DVec2::ZERO {
        return Err("revolve axis has zero length".into());
    }
    // The profile must lie entirely on one side of the axis.
    let side = |p: DVec2| dir2.perp_dot(p - axis_point);
    let sides: Vec<f64> = region.outer.pts.iter().map(|p| side(*p)).collect();
    let (mn, mx) = sides.iter().fold((f64::MAX, f64::MIN), |(a, b), &s| (a.min(s), b.max(s)));
    if mn < -1e-7 && mx > 1e-7 {
        return Err("profile crosses the revolve axis".into());
    }
    let angle = angle_deg.clamp(-360.0, 360.0);
    if angle.abs() < 1e-9 {
        return Err("revolve angle is zero".into());
    }
    let full = (angle.abs() - 360.0).abs() < 1e-9;
    let steps = ((angle.abs() / 360.0) * 72.0).ceil().max(2.0) as usize;
    let a0 = frame.to_3d(axis_point);
    let ad = (frame.u * dir2.x + frame.v * dir2.y).normalize();
    let rot = |p: DVec2, k: usize| {
        let t = (angle / steps as f64 * k as f64).to_radians();
        let q = DQuat::from_axis_angle(ad, t);
        a0 + q * (frame.to_3d(p) - a0)
    };
    let mut polys = Vec::new();
    for l in std::iter::once(&region.outer).chain(region.holes.iter()) {
        let n = l.pts.len();
        for i in 0..n {
            let a = l.pts[i];
            let b = l.pts[(i + 1) % n];
            for k in 0..steps {
                let mut vs = vec![rot(a, k), rot(b, k), rot(b, k + 1), rot(a, k + 1)];
                vs.dedup_by(|x, y| x.distance(*y) < 1e-9);
                if vs.len() > 1 && vs[0].distance(*vs.last().unwrap()) < 1e-9 {
                    vs.pop();
                }
                polys.extend(Polygon::new(vs));
            }
        }
    }
    if !full {
        for t in region.triangulate() {
            polys.extend(Polygon::new(vec![rot(t[0], 0), rot(t[2], 0), rot(t[1], 0)]));
            polys.extend(Polygon::new(vec![rot(t[0], steps), rot(t[1], steps), rot(t[2], steps)]));
        }
    }
    Ok(orient_outward(Solid::from_polygons(polys)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Loop2 {
        Loop2::new(vec![DVec2::new(x0, y0), DVec2::new(x1, y0), DVec2::new(x1, y1), DVec2::new(x0, y1)])
    }

    #[test]
    fn nested_regions() {
        let loops = vec![rect(0.0, 0.0, 10.0, 10.0), Loop2::circle(DVec2::splat(5.0), 2.0, 64)];
        let regions = find_regions(&loops);
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[0].holes.len(), 1);
        assert_eq!((regions[0].depth, regions[1].depth), (0, 1));
        assert!(regions[0].contains(DVec2::new(1.0, 1.0)));
        assert!(!regions[0].contains(DVec2::new(5.0, 5.0)));
        assert!(regions[1].contains(DVec2::new(5.0, 5.0)));
    }

    #[test]
    fn extrude_plate_with_hole() {
        let f = Frame { origin: DVec3::ZERO, u: DVec3::X, v: DVec3::Y };
        let loops = vec![rect(0.0, 0.0, 10.0, 10.0), rect(4.0, 4.0, 6.0, 6.0)];
        let r = &find_regions(&loops)[0];
        let s = extrude(&f, r, 0.0, 3.0);
        assert!((s.volume() - 96.0 * 3.0).abs() < 1e-6);
        let s = extrude(&f, r, 0.0, -3.0); // reversed direction still outward
        assert!((s.volume() - 96.0 * 3.0).abs() < 1e-6);
    }

    #[test]
    fn offsets_and_treated_extrudes() {
        let f = Frame { origin: DVec3::ZERO, u: DVec3::X, v: DVec3::Y };
        let r = &find_regions(&[rect(0.0, 0.0, 20.0, 10.0), rect(8.0, 3.0, 12.0, 7.0)])[0];
        let o = offset_region(r, 1.0).unwrap();
        assert!((o.outer.area() - 18.0 * 8.0).abs() < 1e-9);
        assert!((o.holes[0].area() - 6.0 * 6.0).abs() < 1e-9);
        assert!(offset_region(r, 6.0).is_none());
        let plain = extrude(&f, r, 0.0, 5.0).volume();
        let ch = extrude_treated(&f, r, 0.0, 5.0, EdgeKind::Chamfer, 0.0, 1.0).unwrap().volume();
        let fi = extrude_treated(&f, r, 0.0, 5.0, EdgeKind::Fillet, 0.0, 1.0).unwrap().volume();
        assert!(ch < plain && fi < plain && fi > ch, "{plain} {fi} {ch}");
        // chamfer removes a triangular prism (0.5 mm²) along every edge (perimeter 60 + 16)
        let expect = plain - 0.5 * (60.0 + 16.0) + 4.0 * (1.0 / 3.0) - 4.0 * (1.0 / 3.0);
        assert!((ch - expect).abs() < 1.5, "{ch} vs {expect}");
        assert!(extrude_treated(&f, r, 0.0, 5.0, EdgeKind::Fillet, 3.0, 3.0).is_err());
    }

    #[test]
    fn revolve_rect_into_tube() {
        let f = Frame { origin: DVec3::ZERO, u: DVec3::X, v: DVec3::Y };
        let r = &find_regions(&[rect(5.0, 0.0, 10.0, 10.0)])[0];
        let s = revolve(&f, r, DVec2::ZERO, DVec2::Y, 360.0).unwrap();
        let exact = std::f64::consts::PI * (100.0 - 25.0) * 10.0;
        assert!((s.volume() - exact).abs() / exact < 0.01, "{}", s.volume());
        let half = revolve(&f, r, DVec2::ZERO, DVec2::Y, 180.0).unwrap();
        assert!((half.volume() - exact / 2.0).abs() / exact < 0.01);
        let bad = &find_regions(&[rect(-1.0, 0.0, 10.0, 10.0)])[0];
        assert!(revolve(&f, bad, DVec2::ZERO, DVec2::Y, 360.0).is_err());
    }
}
