//! Boundary-representation-lite solids as polygon soups, with boolean operations
//! implemented by binary space partitioning (the classic BSP-CSG technique
//! popularised by Evan Wallace's csg.js, re-implemented here in Rust / f64).
//!
//! A [`Solid`] is a closed set of convex planar polygons with outward normals.

use glam::DVec3;
use serde::{Deserialize, Serialize};

const EPS: f64 = 1e-6;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Plane {
    pub normal: DVec3,
    pub w: f64,
}

impl Plane {
    pub fn from_points(a: DVec3, b: DVec3, c: DVec3) -> Option<Plane> {
        let n = (b - a).cross(c - a);
        let len = n.length();
        if len < 1e-12 {
            return None;
        }
        let n = n / len;
        Some(Plane { normal: n, w: n.dot(a) })
    }
    fn flip(&mut self) {
        self.normal = -self.normal;
        self.w = -self.w;
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Polygon {
    pub vertices: Vec<DVec3>,
    pub plane: Plane,
}

impl Polygon {
    /// Build a polygon; returns `None` for degenerate input.
    pub fn new(vertices: Vec<DVec3>) -> Option<Polygon> {
        if vertices.len() < 3 {
            return None;
        }
        // Newell's method: robust normal for (nearly) planar polygons.
        let mut n = DVec3::ZERO;
        for i in 0..vertices.len() {
            let a = vertices[i];
            let b = vertices[(i + 1) % vertices.len()];
            n.x += (a.y - b.y) * (a.z + b.z);
            n.y += (a.z - b.z) * (a.x + b.x);
            n.z += (a.x - b.x) * (a.y + b.y);
        }
        let len = n.length();
        if len < 1e-12 {
            return None;
        }
        let n = n / len;
        let plane = Plane { normal: n, w: n.dot(vertices[0]) };
        Some(Polygon { vertices, plane })
    }

    fn flip(&mut self) {
        self.vertices.reverse();
        self.plane.flip();
    }
}

const COPLANAR: u8 = 0;
const FRONT: u8 = 1;
const BACK: u8 = 2;
const SPANNING: u8 = 3;

fn split_polygon(
    plane: &Plane,
    poly: &Polygon,
    coplanar_front: &mut Vec<Polygon>,
    coplanar_back: &mut Vec<Polygon>,
    front: &mut Vec<Polygon>,
    back: &mut Vec<Polygon>,
) {
    let mut poly_type = 0u8;
    let mut types = Vec::with_capacity(poly.vertices.len());
    for v in &poly.vertices {
        let t = plane.normal.dot(*v) - plane.w;
        let ty = if t < -EPS {
            BACK
        } else if t > EPS {
            FRONT
        } else {
            COPLANAR
        };
        poly_type |= ty;
        types.push(ty);
    }
    match poly_type {
        COPLANAR => {
            if plane.normal.dot(poly.plane.normal) > 0.0 {
                coplanar_front.push(poly.clone());
            } else {
                coplanar_back.push(poly.clone());
            }
        }
        FRONT => front.push(poly.clone()),
        BACK => back.push(poly.clone()),
        _ => {
            let mut f = Vec::new();
            let mut b = Vec::new();
            let n = poly.vertices.len();
            for i in 0..n {
                let j = (i + 1) % n;
                let (ti, tj) = (types[i], types[j]);
                let (vi, vj) = (poly.vertices[i], poly.vertices[j]);
                if ti != BACK {
                    f.push(vi);
                }
                if ti != FRONT {
                    b.push(vi);
                }
                if (ti | tj) == SPANNING {
                    let t = (plane.w - plane.normal.dot(vi)) / plane.normal.dot(vj - vi);
                    let v = vi.lerp(vj, t);
                    f.push(v);
                    b.push(v);
                }
            }
            if f.len() >= 3 {
                front.push(Polygon { vertices: f, plane: poly.plane });
            }
            if b.len() >= 3 {
                back.push(Polygon { vertices: b, plane: poly.plane });
            }
        }
    }
}

#[derive(Default)]
struct Node {
    plane: Option<Plane>,
    front: Option<Box<Node>>,
    back: Option<Box<Node>>,
    polygons: Vec<Polygon>,
}

impl Node {
    fn new(polys: Vec<Polygon>) -> Node {
        let mut n = Node::default();
        n.build(polys);
        n
    }

    fn invert(&mut self) {
        // iterative to avoid deep recursion on large trees
        let mut stack: Vec<&mut Node> = vec![self];
        while let Some(n) = stack.pop() {
            for p in &mut n.polygons {
                p.flip();
            }
            if let Some(pl) = &mut n.plane {
                pl.flip();
            }
            std::mem::swap(&mut n.front, &mut n.back);
            if let Some(f) = n.front.as_deref_mut() {
                stack.push(f);
            }
            if let Some(b) = n.back.as_deref_mut() {
                stack.push(b);
            }
        }
    }

    fn clip_polygons(&self, polys: Vec<Polygon>) -> Vec<Polygon> {
        let Some(plane) = &self.plane else {
            return polys;
        };
        let mut front = Vec::new();
        let mut back = Vec::new();
        for p in &polys {
            let mut cf = Vec::new();
            let mut cb = Vec::new();
            split_polygon(plane, p, &mut cf, &mut cb, &mut front, &mut back);
            front.extend(cf);
            back.extend(cb);
        }
        let front = match &self.front {
            Some(f) => f.clip_polygons(front),
            None => front,
        };
        let back = match &self.back {
            Some(b) => b.clip_polygons(back),
            None => Vec::new(),
        };
        let mut out = front;
        out.extend(back);
        out
    }

    fn clip_to(&mut self, bsp: &Node) {
        let mut stack: Vec<&mut Node> = vec![self];
        while let Some(n) = stack.pop() {
            n.polygons = bsp.clip_polygons(std::mem::take(&mut n.polygons));
            if let Some(f) = n.front.as_deref_mut() {
                stack.push(f);
            }
            if let Some(b) = n.back.as_deref_mut() {
                stack.push(b);
            }
        }
    }

    fn all_polygons(&self) -> Vec<Polygon> {
        let mut out = Vec::new();
        let mut stack: Vec<&Node> = vec![self];
        while let Some(n) = stack.pop() {
            out.extend(n.polygons.iter().cloned());
            if let Some(f) = n.front.as_deref() {
                stack.push(f);
            }
            if let Some(b) = n.back.as_deref() {
                stack.push(b);
            }
        }
        out
    }

    fn build(&mut self, polys: Vec<Polygon>) {
        if polys.is_empty() {
            return;
        }
        let plane = *self.plane.get_or_insert(polys[0].plane);
        let mut front = Vec::new();
        let mut back = Vec::new();
        for p in &polys {
            let mut cf = Vec::new();
            let mut cb = Vec::new();
            split_polygon(&plane, p, &mut cf, &mut cb, &mut front, &mut back);
            self.polygons.extend(cf);
            self.polygons.extend(cb);
        }
        if !front.is_empty() {
            self.front.get_or_insert_with(Default::default).build(front);
        }
        if !back.is_empty() {
            self.back.get_or_insert_with(Default::default).build(back);
        }
    }
}

/// A closed polyhedral solid.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Solid {
    pub polygons: Vec<Polygon>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BoolOp {
    Union,
    Subtract,
    Intersect,
}

impl Solid {
    pub fn from_polygons(polygons: Vec<Polygon>) -> Solid {
        Solid { polygons }
    }

    pub fn is_empty(&self) -> bool {
        self.polygons.is_empty()
    }

    pub fn bounds(&self) -> Option<(DVec3, DVec3)> {
        let mut it = self.polygons.iter().flat_map(|p| p.vertices.iter());
        let first = *it.next()?;
        let (mut lo, mut hi) = (first, first);
        for v in it {
            lo = lo.min(*v);
            hi = hi.max(*v);
        }
        Some((lo, hi))
    }

    fn bounds_overlap(&self, other: &Solid) -> bool {
        match (self.bounds(), other.bounds()) {
            (Some((alo, ahi)), Some((blo, bhi))) => {
                alo.x <= bhi.x + EPS
                    && blo.x <= ahi.x + EPS
                    && alo.y <= bhi.y + EPS
                    && blo.y <= ahi.y + EPS
                    && alo.z <= bhi.z + EPS
                    && blo.z <= ahi.z + EPS
            }
            _ => false,
        }
    }

    pub fn union(&self, other: &Solid) -> Solid {
        if self.is_empty() {
            return other.clone();
        }
        if other.is_empty() {
            return self.clone();
        }
        if !self.bounds_overlap(other) {
            let mut polys = self.polygons.clone();
            polys.extend(other.polygons.iter().cloned());
            return Solid { polygons: polys };
        }
        let mut a = Node::new(self.polygons.clone());
        let mut b = Node::new(other.polygons.clone());
        a.clip_to(&b);
        b.clip_to(&a);
        b.invert();
        b.clip_to(&a);
        b.invert();
        a.build(b.all_polygons());
        Solid { polygons: a.all_polygons() }
    }

    pub fn subtract(&self, other: &Solid) -> Solid {
        if self.is_empty() || other.is_empty() || !self.bounds_overlap(other) {
            return self.clone();
        }
        let mut a = Node::new(self.polygons.clone());
        let mut b = Node::new(other.polygons.clone());
        a.invert();
        a.clip_to(&b);
        b.clip_to(&a);
        b.invert();
        b.clip_to(&a);
        b.invert();
        a.build(b.all_polygons());
        a.invert();
        Solid { polygons: a.all_polygons() }
    }

    pub fn intersect(&self, other: &Solid) -> Solid {
        if self.is_empty() || other.is_empty() || !self.bounds_overlap(other) {
            return Solid::default();
        }
        let mut a = Node::new(self.polygons.clone());
        let mut b = Node::new(other.polygons.clone());
        a.invert();
        b.clip_to(&a);
        b.invert();
        a.clip_to(&b);
        b.clip_to(&a);
        a.build(b.all_polygons());
        a.invert();
        Solid { polygons: a.all_polygons() }
    }

    pub fn boolean(&self, other: &Solid, op: BoolOp) -> Solid {
        match op {
            BoolOp::Union => self.union(other),
            BoolOp::Subtract => self.subtract(other),
            BoolOp::Intersect => self.intersect(other),
        }
    }

    pub fn transformed(&self, m: &glam::DMat4) -> Solid {
        let flip = m.determinant() < 0.0;
        let polygons = self
            .polygons
            .iter()
            .filter_map(|p| {
                let mut vs: Vec<DVec3> = p.vertices.iter().map(|v| m.transform_point3(*v)).collect();
                if flip {
                    vs.reverse();
                }
                Polygon::new(vs)
            })
            .collect();
        Solid { polygons }
    }

    /// Signed volume via the divergence theorem (positive for outward-facing solids).
    pub fn volume(&self) -> f64 {
        let mut v = 0.0;
        for p in &self.polygons {
            let a = p.vertices[0];
            for i in 1..p.vertices.len() - 1 {
                let b = p.vertices[i];
                let c = p.vertices[i + 1];
                v += a.dot(b.cross(c));
            }
        }
        v / 6.0
    }

    /// Total surface area.
    pub fn area(&self) -> f64 {
        let mut s = 0.0;
        for p in &self.polygons {
            let a = p.vertices[0];
            for i in 1..p.vertices.len() - 1 {
                s += (p.vertices[i] - a).cross(p.vertices[i + 1] - a).length() * 0.5;
            }
        }
        s
    }

    /// Fan-triangulate every (convex) polygon.
    pub fn triangles(&self) -> Vec<[DVec3; 3]> {
        let mut out = Vec::new();
        for p in &self.polygons {
            for i in 1..p.vertices.len() - 1 {
                out.push([p.vertices[0], p.vertices[i], p.vertices[i + 1]]);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use crate::primitives::cuboid;
    use glam::DVec3;

    #[test]
    fn boolean_volumes() {
        let a = cuboid(DVec3::ZERO, DVec3::splat(10.0));
        let b = cuboid(DVec3::splat(5.0), DVec3::splat(15.0));
        assert!((a.volume() - 1000.0).abs() < 1e-6);
        assert!((a.union(&b).volume() - 1875.0).abs() < 1e-6);
        assert!((a.subtract(&b).volume() - 875.0).abs() < 1e-6);
        assert!((a.intersect(&b).volume() - 125.0).abs() < 1e-6);
    }

    #[test]
    fn disjoint_and_empty() {
        let a = cuboid(DVec3::ZERO, DVec3::splat(1.0));
        let b = cuboid(DVec3::splat(5.0), DVec3::splat(6.0));
        assert!(a.intersect(&b).is_empty());
        assert!((a.union(&b).volume() - 2.0).abs() < 1e-9);
        assert!((a.subtract(&b).volume() - 1.0).abs() < 1e-9);
    }
}
