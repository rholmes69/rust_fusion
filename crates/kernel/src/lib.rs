//! cad-kernel: the geometry core of Rust Fusion.
//!
//! * [`csg`] – polygonal solids and BSP boolean operations
//! * [`profile`] – 2D loops/regions and extrude / revolve sweeps
//! * [`primitives`] – box, cylinder, sphere
//! * [`expr`] – the parameter expression language
//! * [`edges`] – feature edge extraction for display
//! * [`stl`] – STL export

pub mod csg;
pub mod edges;
pub mod expr;
pub mod primitives;
pub mod profile;
pub mod stl;

pub use csg::{BoolOp, Polygon, Solid};
pub use glam::{DVec2, DVec3};
pub use profile::{Frame, Loop2, Region};
