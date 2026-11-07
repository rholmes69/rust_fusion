//! Primitive solids.

use crate::csg::{Polygon, Solid};
use glam::{DQuat, DVec3};

/// Axis-aligned box between two corners.
pub fn cuboid(min: DVec3, max: DVec3) -> Solid {
    let (lo, hi) = (min.min(max), min.max(max));
    let c = |x: bool, y: bool, z: bool| {
        DVec3::new(if x { hi.x } else { lo.x }, if y { hi.y } else { lo.y }, if z { hi.z } else { lo.z })
    };
    let faces = [
        [c(false, false, false), c(false, false, true), c(false, true, true), c(false, true, false)], // -x
        [c(true, false, false), c(true, true, false), c(true, true, true), c(true, false, true)],     // +x
        [c(false, false, false), c(true, false, false), c(true, false, true), c(false, false, true)], // -y
        [c(false, true, false), c(false, true, true), c(true, true, true), c(true, true, false)],     // +y
        [c(false, false, false), c(false, true, false), c(true, true, false), c(true, false, false)], // -z
        [c(false, false, true), c(true, false, true), c(true, true, true), c(false, true, true)],     // +z
    ];
    Solid::from_polygons(faces.iter().filter_map(|f| Polygon::new(f.to_vec())).collect())
}

/// Cylinder with its base centred at `base`, extending `height` along `axis`.
pub fn cylinder(base: DVec3, axis: DVec3, radius: f64, height: f64, segments: usize) -> Solid {
    let axis = axis.normalize();
    let rot = DQuat::from_rotation_arc(DVec3::Z, axis);
    let top = base + axis * height;
    let ring: Vec<DVec3> = (0..segments)
        .map(|i| {
            let t = i as f64 / segments as f64 * std::f64::consts::TAU;
            rot * DVec3::new(t.cos() * radius, t.sin() * radius, 0.0)
        })
        .collect();
    let mut polys = Vec::new();
    // bottom (normal -axis): reverse order
    polys.extend(Polygon::new(ring.iter().rev().map(|p| base + *p).collect()));
    polys.extend(Polygon::new(ring.iter().map(|p| top + *p).collect()));
    for i in 0..segments {
        let a = ring[i];
        let b = ring[(i + 1) % segments];
        polys.extend(Polygon::new(vec![base + a, base + b, top + b, top + a]));
    }
    Solid::from_polygons(polys)
}

/// UV sphere.
pub fn sphere(center: DVec3, radius: f64, slices: usize, stacks: usize) -> Solid {
    let pt = |i: usize, j: usize| {
        let theta = i as f64 / slices as f64 * std::f64::consts::TAU;
        let phi = j as f64 / stacks as f64 * std::f64::consts::PI;
        center + DVec3::new(theta.cos() * phi.sin(), theta.sin() * phi.sin(), phi.cos()) * radius
    };
    let mut polys = Vec::new();
    for i in 0..slices {
        for j in 0..stacks {
            let mut v = vec![pt(i, j)];
            if j > 0 {
                v.push(pt(i + 1, j));
            }
            if j < stacks - 1 {
                v.push(pt(i + 1, j + 1));
            }
            v.push(pt(i, j + 1));
            v.reverse();
            polys.extend(Polygon::new(v));
        }
    }
    Solid::from_polygons(polys)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn primitive_volumes_are_positive() {
        let c = cylinder(DVec3::ZERO, DVec3::Y, 5.0, 10.0, 128);
        let exact = std::f64::consts::PI * 25.0 * 10.0;
        assert!((c.volume() - exact).abs() / exact < 0.01);
        let s = sphere(DVec3::ZERO, 10.0, 64, 32);
        let exact = 4.0 / 3.0 * std::f64::consts::PI * 1000.0;
        assert!(s.volume() > 0.0 && (s.volume() - exact).abs() / exact < 0.02);
    }
}
