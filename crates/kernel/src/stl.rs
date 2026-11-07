//! STL export (binary and ASCII).

use crate::csg::Solid;
use std::io::{self, Write};

pub fn write_binary<W: Write>(w: &mut W, solids: &[&Solid]) -> io::Result<()> {
    let tris: Vec<_> = solids.iter().flat_map(|s| s.triangles()).collect();
    let mut header = [0u8; 80];
    let tag = b"rust-fusion STL export";
    header[..tag.len()].copy_from_slice(tag);
    w.write_all(&header)?;
    w.write_all(&(tris.len() as u32).to_le_bytes())?;
    for t in &tris {
        let n = (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero();
        for c in [n.x, n.y, n.z] {
            w.write_all(&(c as f32).to_le_bytes())?;
        }
        for v in t {
            for c in [v.x, v.y, v.z] {
                w.write_all(&(c as f32).to_le_bytes())?;
            }
        }
        w.write_all(&[0, 0])?;
    }
    Ok(())
}

pub fn write_ascii<W: Write>(w: &mut W, name: &str, solids: &[&Solid]) -> io::Result<()> {
    writeln!(w, "solid {name}")?;
    for s in solids {
        for t in s.triangles() {
            let n = (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero();
            writeln!(w, "  facet normal {} {} {}", n.x, n.y, n.z)?;
            writeln!(w, "    outer loop")?;
            for v in t {
                writeln!(w, "      vertex {} {} {}", v.x, v.y, v.z)?;
            }
            writeln!(w, "    endloop")?;
            writeln!(w, "  endfacet")?;
        }
    }
    writeln!(w, "endsolid {name}")
}

#[cfg(test)]
mod tests {
    #[test]
    fn binary_size() {
        let s = crate::primitives::cuboid(glam::DVec3::ZERO, glam::DVec3::ONE);
        let mut buf = Vec::new();
        super::write_binary(&mut buf, &[&s]).unwrap();
        assert_eq!(buf.len(), 84 + 12 * 50);
    }
}
