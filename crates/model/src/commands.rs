//! Flat, validated editing commands shared by the AI copilot, voice control and the MCP
//! server. AI output never touches the document directly: it is parsed into [`Command`]s,
//! which [`Executor::apply`] checks and applies (the app wraps a batch in one undo step).

use crate::sketch::{Entity, PlaneKind, Sketch};
use crate::{
    Axis, Document, ExtentKind, FeatureId, FeatureKind, Model, Operation, Parameter, RevolveAxis,
};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};

/// A value that may be written as a JSON number or an expression string.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Ex(pub String);

impl<'de> Deserialize<'de> for Ex {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Ex, D::Error> {
        let v = Value::deserialize(d)?;
        Ok(Ex(match v {
            Value::Number(n) => trim_num(n.as_f64().unwrap_or(0.0)),
            Value::String(s) => s,
            other => other.to_string(),
        }))
    }
}

impl From<&str> for Ex {
    fn from(s: &str) -> Ex {
        Ex(s.to_string())
    }
}

fn trim_num(v: f64) -> String {
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".into()
    } else {
        s.to_string()
    }
}

fn d0() -> Ex {
    Ex("0".into())
}
fn yes() -> bool {
    true
}

/// App-level actions that don't change the document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AppAction {
    Undo,
    Redo,
    View(String),
    FitAll,
    ExportStl(Option<String>),
    ExportGltf(Option<String>),
    ExportStep(Option<String>),
    Save(Option<String>),
    NewDesign,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Command {
    /// Create or update a user parameter.
    SetParameter {
        name: String,
        value: Ex,
        #[serde(default)]
        comment: Option<String>,
    },
    DeleteParameter {
        name: String,
    },
    /// Start a new sketch; it becomes the current sketch for add_* commands.
    NewSketch {
        #[serde(default)]
        name: Option<String>,
        plane: String,
        #[serde(default = "d0")]
        offset: Ex,
    },
    /// Corner rectangle (x, y = one corner).
    AddRectangle {
        #[serde(default)]
        sketch: Option<String>,
        x: Ex,
        y: Ex,
        width: Ex,
        height: Ex,
    },
    /// Rectangle centred at (cx, cy).
    AddCenterRectangle {
        #[serde(default)]
        sketch: Option<String>,
        #[serde(default = "d0")]
        cx: Ex,
        #[serde(default = "d0")]
        cy: Ex,
        width: Ex,
        height: Ex,
    },
    AddCircle {
        #[serde(default)]
        sketch: Option<String>,
        #[serde(default = "d0")]
        cx: Ex,
        #[serde(default = "d0")]
        cy: Ex,
        radius: Ex,
    },
    /// Polyline through points; closed by default (closed = a profile).
    AddPolygon {
        #[serde(default)]
        sketch: Option<String>,
        points: Vec<[f64; 2]>,
        #[serde(default = "yes")]
        closed: bool,
    },
    /// Regular polygon with `sides` corners on a circle of `radius`.
    AddRegularPolygon {
        #[serde(default)]
        sketch: Option<String>,
        #[serde(default = "d0")]
        cx: Ex,
        #[serde(default = "d0")]
        cy: Ex,
        radius: Ex,
        sides: u32,
    },
    Extrude {
        #[serde(default)]
        sketch: Option<String>,
        /// Profile indices; omitted = every outer profile (holes stay open).
        #[serde(default)]
        profiles: Option<Vec<usize>>,
        distance: Ex,
        #[serde(default)]
        symmetric: bool,
        #[serde(default)]
        flip: bool,
        #[serde(default)]
        operation: Option<String>,
        #[serde(default)]
        name: Option<String>,
    },
    Revolve {
        #[serde(default)]
        sketch: Option<String>,
        #[serde(default)]
        profiles: Option<Vec<usize>>,
        /// "horizontal" (sketch x axis) or "vertical" (sketch y axis)
        #[serde(default)]
        axis: Option<String>,
        #[serde(default)]
        angle: Option<Ex>,
        #[serde(default)]
        operation: Option<String>,
        #[serde(default)]
        name: Option<String>,
    },
    Box {
        corner: [Ex; 3],
        size: [Ex; 3],
        #[serde(default)]
        operation: Option<String>,
        #[serde(default)]
        name: Option<String>,
    },
    Cylinder {
        base: [Ex; 3],
        #[serde(default)]
        axis: Option<String>,
        radius: Ex,
        height: Ex,
        #[serde(default)]
        operation: Option<String>,
        #[serde(default)]
        name: Option<String>,
    },
    Sphere {
        center: [Ex; 3],
        radius: Ex,
        #[serde(default)]
        operation: Option<String>,
        #[serde(default)]
        name: Option<String>,
    },
    /// Change one value of an existing feature (distance, angle, radius, height, offset, ...).
    SetFeatureValue {
        feature: String,
        field: String,
        value: Ex,
    },
    SetOperation {
        feature: String,
        operation: String,
    },
    DeleteFeature {
        feature: String,
    },
    SuppressFeature {
        feature: String,
        #[serde(default = "yes")]
        suppressed: bool,
    },
    RenameFeature {
        feature: String,
        new_name: String,
    },
    /// Move the timeline marker: after the named feature, or to the end when omitted.
    RollTo {
        #[serde(default)]
        feature: Option<String>,
    },
    ClearDesign,
    SetDesignName {
        name: String,
    },
    // ---- app level
    Undo,
    Redo,
    View {
        view: String,
    },
    FitAll,
    ExportStl {
        #[serde(default)]
        path: Option<String>,
    },
    ExportGltf {
        #[serde(default)]
        path: Option<String>,
    },
    ExportStep {
        #[serde(default)]
        path: Option<String>,
    },
    Save {
        #[serde(default)]
        path: Option<String>,
    },
    NewDesign,
    // ---- edge treatment / shell
    /// Round the cap edges of an extrude.
    Fillet {
        feature: String,
        size: Ex,
        #[serde(default)]
        start: bool,
        #[serde(default = "yes")]
        end: bool,
    },
    /// Bevel the cap edges of an extrude.
    Chamfer {
        feature: String,
        size: Ex,
        #[serde(default)]
        start: bool,
        #[serde(default = "yes")]
        end: bool,
    },
    /// Hollow out an extruded solid.
    Shell {
        feature: String,
        thickness: Ex,
        /// "end" (default), "start" or "none"
        #[serde(default)]
        open: Option<String>,
        #[serde(default)]
        name: Option<String>,
    },
    // ---- constraints
    /// Add a constraint to the current (or named) sketch; see the constraint reference.
    AddConstraint {
        #[serde(default)]
        sketch: Option<String>,
        constraint: crate::constraints::Constraint,
    },
    /// Add horizontal / vertical / coincident constraints to nearly-aligned polyline geometry.
    AutoConstrain {
        #[serde(default)]
        sketch: Option<String>,
    },
    // ---- generators
    /// A whole building: shell with storeys, windows, door and roof.
    Building {
        #[serde(default)]
        width: Option<Ex>,
        #[serde(default)]
        depth: Option<Ex>,
        #[serde(default)]
        floors: Option<u32>,
        #[serde(default)]
        floor_height: Option<Ex>,
        #[serde(default)]
        wall_thickness: Option<Ex>,
        #[serde(default)]
        roof: Option<String>,
        #[serde(default)]
        roof_height: Option<Ex>,
        #[serde(default)]
        windows_front: Option<u32>,
        #[serde(default)]
        windows_side: Option<u32>,
        #[serde(default)]
        window_width: Option<Ex>,
        #[serde(default)]
        window_height: Option<Ex>,
    },
    /// A straight wall in plan (x, z) coordinates.
    Wall {
        from: [f64; 2],
        to: [f64; 2],
        height: Ex,
        #[serde(default)]
        thickness: Option<f64>,
        #[serde(default)]
        base: Option<Ex>,
        #[serde(default)]
        operation: Option<String>,
        #[serde(default)]
        name: Option<String>,
    },
    /// A window/door opening cut through walls, centred at plan (x, z).
    Opening {
        x: f64,
        z: f64,
        width: f64,
        height: Ex,
        #[serde(default)]
        sill: Option<Ex>,
        #[serde(default)]
        depth: Option<f64>,
        #[serde(default)]
        angle: Option<f64>,
        #[serde(default)]
        name: Option<String>,
    },
    /// A parametric library part (bolt, nut, washer, spur_gear, flange, tube, l_bracket, enclosure, stairs).
    Part {
        kind: String,
        #[serde(default)]
        params: serde_json::Map<String, Value>,
        #[serde(default)]
        position: Option<[f64; 3]>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// The document changed; message for the log.
    Changed(String),
    /// Nothing to change in the document; the app must perform this.
    App(AppAction),
}

/// Parse an operation word.
/// Evaluate an expression with no parameters known (macro helpers).
pub fn eval_num(e: &str, vars: &std::collections::HashMap<String, f64>) -> Result<f64, String> {
    cad_kernel::expr::eval(e, vars).map_err(|e| e.0)
}

pub fn parse_op(s: Option<&str>, default: Operation) -> Result<Operation, String> {
    let Some(s) = s else { return Ok(default) };
    Ok(
        match s.to_ascii_lowercase().replace([' ', '_', '-'], "").as_str() {
            "newbody" | "new" | "body" => Operation::NewBody,
            "join" | "add" | "union" => Operation::Join,
            "cut" | "subtract" | "difference" | "remove" => Operation::Cut,
            "intersect" | "intersection" | "common" => Operation::Intersect,
            other => {
                return Err(format!(
                    "unknown operation '{other}' (use new_body, join, cut, intersect)"
                ))
            }
        },
    )
}

pub fn parse_plane(s: &str) -> Result<PlaneKind, String> {
    Ok(
        match s.to_ascii_lowercase().replace([' ', '_', '-'], "").as_str() {
            "xy" | "front" | "xyplane" => PlaneKind::XY,
            "xz" | "top" | "ground" | "xzplane" | "floor" => PlaneKind::XZ,
            "yz" | "right" | "side" | "yzplane" => PlaneKind::YZ,
            other => return Err(format!("unknown plane '{other}' (use XY, XZ or YZ)")),
        },
    )
}

fn parse_axis(s: Option<&str>) -> Result<Axis, String> {
    Ok(match s.unwrap_or("y").to_ascii_lowercase().as_str() {
        "x" => Axis::X,
        "y" | "up" | "vertical" => Axis::Y,
        "z" => Axis::Z,
        other => return Err(format!("unknown axis '{other}'")),
    })
}

/// Executes commands against a document, remembering the "current" sketch.
#[derive(Debug, Default, Clone)]
pub struct Executor {
    pub current_sketch: Option<FeatureId>,
}

impl Executor {
    /// Apply generated sub-commands; stops at the first error.
    fn apply_all(&mut self, doc: &mut Document, cmds: &[Command]) -> Result<(), String> {
        for c in cmds {
            self.apply(doc, c)?;
        }
        Ok(())
    }

    /// Parameter values including the building spec's (for initial geometry guesses).
    fn vars(doc: &Document, spec: &crate::macros::BuildingSpec) -> std::collections::HashMap<String, f64> {
        let mut pairs: Vec<(String, String)> = doc.parameters.iter().map(|p| (p.name.clone(), p.expr.clone())).collect();
        let _ = spec;
        pairs.retain(|p| !p.0.is_empty());
        cad_kernel::expr::resolve_parameters(&pairs).0
    }

    fn find(&self, doc: &Document, name: &str) -> Result<FeatureId, String> {
        let lname = name.to_ascii_lowercase();
        doc.features
            .iter()
            .find(|f| f.name.to_ascii_lowercase() == lname)
            .or_else(|| {
                // "the last extrude", "extrude" → most recent of that type
                doc.features.iter().rev().find(|f| {
                    let t = f.kind.type_name().to_ascii_lowercase();
                    lname == t || lname == format!("last {t}") || lname == format!("the last {t}")
                })
            })
            .map(|f| f.id)
            .ok_or_else(|| format!("no feature named '{name}'"))
    }

    fn sketch_id(&self, doc: &Document, name: &Option<String>) -> Result<FeatureId, String> {
        let id = match name {
            Some(n) => self.find(doc, n)?,
            None => match self.current_sketch.filter(|id| doc.feature(*id).is_some()) {
                Some(id) => id,
                None => doc
                    .features
                    .iter()
                    .rev()
                    .find(|f| matches!(f.kind, FeatureKind::Sketch(_)))
                    .map(|f| f.id)
                    .ok_or("there is no sketch yet; use new_sketch first")?,
            },
        };
        match doc.feature(id).map(|f| &f.kind) {
            Some(FeatureKind::Sketch(_)) => Ok(id),
            _ => Err("that feature is not a sketch".into()),
        }
    }

    fn add_entity(
        &mut self,
        doc: &mut Document,
        sketch: &Option<String>,
        e: Entity,
    ) -> Result<Outcome, String> {
        let id = self.sketch_id(doc, sketch)?;
        let f = doc.feature_mut(id).unwrap();
        let name = f.name.clone();
        if let FeatureKind::Sketch(s) = &mut f.kind {
            let what = e.kind_name().to_lowercase();
            s.entities.push(e);
            self.current_sketch = Some(id);
            return Ok(Outcome::Changed(format!("added {what} to {name}")));
        }
        unreachable!()
    }

    /// Default profiles: every region that is not itself a hole (even nesting depth).
    fn default_profiles(doc: &Document, sketch: FeatureId) -> Result<Vec<usize>, String> {
        let pairs: Vec<(String, String)> = doc
            .parameters
            .iter()
            .map(|p| (p.name.clone(), p.expr.clone()))
            .collect();
        let (vars, _) = cad_kernel::expr::resolve_parameters(&pairs);
        let Some(FeatureKind::Sketch(s)) = doc.feature(sketch).map(|f| &f.kind) else {
            return Err("not a sketch".into());
        };
        let g = s.evaluate(&vars).map_err(|e| e.0)?;
        let p: Vec<usize> = (0..g.regions.len())
            .filter(|&i| g.regions[i].depth % 2 == 0)
            .collect();
        if p.is_empty() {
            return Err("the sketch has no closed profiles".into());
        }
        Ok(p)
    }

    fn default_op(doc: &Document) -> Operation {
        let has_solid = doc.features[..doc.active_len()]
            .iter()
            .any(|f| !f.suppressed && f.kind.operation().is_some());
        if has_solid {
            Operation::Join
        } else {
            Operation::NewBody
        }
    }

    fn insert(doc: &mut Document, name: &Option<String>, base: &str, kind: FeatureKind) -> String {
        let name = name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| doc.unique_name(base));
        doc.insert_feature(name.clone(), kind);
        name
    }

    pub fn apply(&mut self, doc: &mut Document, cmd: &Command) -> Result<Outcome, String> {
        use Command as C;
        let ok = |s: String| Ok(Outcome::Changed(s));
        match cmd {
            C::SetParameter {
                name,
                value,
                comment,
            } => {
                let valid = name
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_alphabetic() || c == '_')
                    && name.chars().all(|c| c.is_alphanumeric() || c == '_');
                if !valid {
                    return Err(format!("'{name}' is not a valid parameter name"));
                }
                if let Some(p) = doc.parameters.iter_mut().find(|p| &p.name == name) {
                    p.expr = value.0.clone();
                    if let Some(c) = comment {
                        p.comment = c.clone();
                    }
                    ok(format!("set {name} = {}", value.0))
                } else {
                    doc.parameters.push(Parameter {
                        name: name.clone(),
                        expr: value.0.clone(),
                        comment: comment.clone().unwrap_or_default(),
                    });
                    ok(format!("new parameter {name} = {}", value.0))
                }
            }
            C::DeleteParameter { name } => {
                let n = doc.parameters.len();
                doc.parameters.retain(|p| &p.name != name);
                if n == doc.parameters.len() {
                    return Err(format!("no parameter named '{name}'"));
                }
                ok(format!("deleted parameter {name}"))
            }
            C::NewSketch {
                name,
                plane,
                offset,
            } => {
                let mut s = Sketch::new(parse_plane(plane)?);
                s.offset = offset.0.clone();
                let n = Self::insert(doc, name, "Sketch", FeatureKind::Sketch(s));
                self.current_sketch = doc.features.iter().find(|f| f.name == n).map(|f| f.id);
                ok(format!("created {n} on {}", plane.to_uppercase()))
            }
            C::AddRectangle {
                sketch,
                x,
                y,
                width,
                height,
            } => self.add_entity(
                doc,
                sketch,
                Entity::Rectangle {
                    x: x.0.clone(),
                    y: y.0.clone(),
                    width: width.0.clone(),
                    height: height.0.clone(),
                },
            ),
            C::AddCenterRectangle {
                sketch,
                cx,
                cy,
                width,
                height,
            } => self.add_entity(
                doc,
                sketch,
                Entity::Rectangle {
                    x: format!("({}) - ({}) / 2", cx.0, width.0),
                    y: format!("({}) - ({}) / 2", cy.0, height.0),
                    width: width.0.clone(),
                    height: height.0.clone(),
                },
            ),
            C::AddCircle {
                sketch,
                cx,
                cy,
                radius,
            } => self.add_entity(
                doc,
                sketch,
                Entity::Circle {
                    cx: cx.0.clone(),
                    cy: cy.0.clone(),
                    radius: radius.0.clone(),
                },
            ),
            C::AddPolygon {
                sketch,
                points,
                closed,
            } => {
                if points.len() < 2 || (*closed && points.len() < 3) {
                    return Err("a polygon needs at least 3 points".into());
                }
                self.add_entity(
                    doc,
                    sketch,
                    Entity::Polyline {
                        points: points.clone(),
                        closed: *closed,
                    },
                )
            }
            C::AddRegularPolygon {
                sketch,
                cx,
                cy,
                radius,
                sides,
            } => {
                if !(3..=128).contains(sides) {
                    return Err("sides must be between 3 and 128".into());
                }
                let pairs: Vec<(String, String)> = doc
                    .parameters
                    .iter()
                    .map(|p| (p.name.clone(), p.expr.clone()))
                    .collect();
                let (vars, _) = cad_kernel::expr::resolve_parameters(&pairs);
                let ev = |e: &Ex| cad_kernel::expr::eval(&e.0, &vars).map_err(|e| e.0);
                let (cx, cy, r) = (ev(cx)?, ev(cy)?, ev(radius)?);
                let points = (0..*sides)
                    .map(|i| {
                        let t = std::f64::consts::TAU * i as f64 / *sides as f64
                            + std::f64::consts::FRAC_PI_2;
                        [cx + r * t.cos(), cy + r * t.sin()]
                    })
                    .collect();
                self.add_entity(
                    doc,
                    sketch,
                    Entity::Polyline {
                        points,
                        closed: true,
                    },
                )
            }
            C::Extrude {
                sketch,
                profiles,
                distance,
                symmetric,
                flip,
                operation,
                name,
            } => {
                let sk = self.sketch_id(doc, sketch)?;
                let profiles = match profiles {
                    Some(p) if !p.is_empty() => p.clone(),
                    _ => Self::default_profiles(doc, sk)?,
                };
                let op = parse_op(operation.as_deref(), Self::default_op(doc))?;
                let n = Self::insert(
                    doc,
                    name,
                    "Extrude",
                    FeatureKind::Extrude {
                        sketch: sk,
                        profiles,
                        distance: distance.0.clone(),
                        extent: if *symmetric {
                            ExtentKind::Symmetric
                        } else {
                            ExtentKind::OneSide
                        },
                        flip: *flip,
                        op,
                        edges: None,
                    },
                );
                ok(format!("{n}: extrude {} ({})", distance.0, op.label()))
            }
            C::Revolve {
                sketch,
                profiles,
                axis,
                angle,
                operation,
                name,
            } => {
                let sk = self.sketch_id(doc, sketch)?;
                let profiles = match profiles {
                    Some(p) if !p.is_empty() => p.clone(),
                    _ => Self::default_profiles(doc, sk)?,
                };
                let axis = match axis
                    .as_deref()
                    .unwrap_or("vertical")
                    .to_ascii_lowercase()
                    .as_str()
                {
                    "horizontal" | "x" | "u" => RevolveAxis::SketchU,
                    "vertical" | "y" | "v" => RevolveAxis::SketchV,
                    o => {
                        return Err(format!(
                            "unknown revolve axis '{o}' (horizontal or vertical)"
                        ))
                    }
                };
                let op = parse_op(operation.as_deref(), Self::default_op(doc))?;
                let angle = angle.clone().unwrap_or(Ex("360 deg".into()));
                let n = Self::insert(
                    doc,
                    name,
                    "Revolve",
                    FeatureKind::Revolve {
                        sketch: sk,
                        profiles,
                        axis,
                        angle: angle.0.clone(),
                        op,
                    },
                );
                ok(format!("{n}: revolve {} ({})", angle.0, op.label()))
            }
            C::Box {
                corner,
                size,
                operation,
                name,
            } => {
                let op = parse_op(operation.as_deref(), Self::default_op(doc))?;
                let n = Self::insert(
                    doc,
                    name,
                    "Box",
                    FeatureKind::Box {
                        corner: corner.clone().map(|e| e.0),
                        size: size.clone().map(|e| e.0),
                        op,
                    },
                );
                ok(format!("{n} ({})", op.label()))
            }
            C::Cylinder {
                base,
                axis,
                radius,
                height,
                operation,
                name,
            } => {
                let op = parse_op(operation.as_deref(), Self::default_op(doc))?;
                let n = Self::insert(
                    doc,
                    name,
                    "Cylinder",
                    FeatureKind::Cylinder {
                        base: base.clone().map(|e| e.0),
                        axis: parse_axis(axis.as_deref())?,
                        radius: radius.0.clone(),
                        height: height.0.clone(),
                        op,
                    },
                );
                ok(format!("{n} ({})", op.label()))
            }
            C::Sphere {
                center,
                radius,
                operation,
                name,
            } => {
                let op = parse_op(operation.as_deref(), Self::default_op(doc))?;
                let n = Self::insert(
                    doc,
                    name,
                    "Sphere",
                    FeatureKind::Sphere {
                        center: center.clone().map(|e| e.0),
                        radius: radius.0.clone(),
                        op,
                    },
                );
                ok(format!("{n} ({})", op.label()))
            }
            C::SetFeatureValue {
                feature,
                field,
                value,
            } => {
                let id = self.find(doc, feature)?;
                let f = doc.feature_mut(id).unwrap();
                let fname = f.name.clone();
                let v = value.0.clone();
                let field_l = field.to_ascii_lowercase();
                let slot: Option<&mut String> = match (&mut f.kind, field_l.as_str()) {
                    (
                        FeatureKind::Extrude { distance, .. },
                        "distance" | "depth" | "height" | "length",
                    ) => Some(distance),
                    (FeatureKind::Revolve { angle, .. }, "angle") => Some(angle),
                    (FeatureKind::Cylinder { radius, .. }, "radius") => Some(radius),
                    (FeatureKind::Cylinder { height, .. }, "height" | "length") => Some(height),
                    (FeatureKind::Sphere { radius, .. }, "radius") => Some(radius),
                    (FeatureKind::Sketch(s), "offset") => Some(&mut s.offset),
                    (FeatureKind::Shell { thickness, .. }, "thickness" | "wall") => Some(thickness),
                    (FeatureKind::Extrude { edges: Some(e), .. }, "fillet" | "chamfer" | "edge_size") => Some(&mut e.size),
                    (FeatureKind::Box { size, .. }, "length" | "size_x" | "x") => {
                        Some(&mut size[0])
                    }
                    (FeatureKind::Box { size, .. }, "height" | "size_y" | "y") => {
                        Some(&mut size[1])
                    }
                    (FeatureKind::Box { size, .. }, "width" | "size_z" | "z") => Some(&mut size[2]),
                    _ => None,
                };
                match slot {
                    Some(s) => {
                        *s = v.clone();
                        ok(format!("{fname}.{field} = {v}"))
                    }
                    None => Err(format!("{fname} has no field '{field}'")),
                }
            }
            C::SetOperation { feature, operation } => {
                let id = self.find(doc, feature)?;
                let newop = parse_op(Some(operation), Operation::NewBody)?;
                let f = doc.feature_mut(id).unwrap();
                match &mut f.kind {
                    FeatureKind::Extrude { op, .. }
                    | FeatureKind::Revolve { op, .. }
                    | FeatureKind::Box { op, .. }
                    | FeatureKind::Cylinder { op, .. }
                    | FeatureKind::Sphere { op, .. } => *op = newop,
                    FeatureKind::Sketch(_) | FeatureKind::Shell { .. } => {
                        return Err("this feature has no operation".into())
                    }
                }
                ok(format!("{} is now {}", f.name, newop.label()))
            }
            C::DeleteFeature { feature } => {
                let id = self.find(doc, feature)?;
                let name = doc.feature(id).unwrap().name.clone();
                doc.delete_feature(id);
                ok(format!("deleted {name}"))
            }
            C::SuppressFeature {
                feature,
                suppressed,
            } => {
                let id = self.find(doc, feature)?;
                let f = doc.feature_mut(id).unwrap();
                f.suppressed = *suppressed;
                ok(format!(
                    "{} {}",
                    if *suppressed {
                        "suppressed"
                    } else {
                        "unsuppressed"
                    },
                    f.name
                ))
            }
            C::RenameFeature { feature, new_name } => {
                let id = self.find(doc, feature)?;
                if doc
                    .features
                    .iter()
                    .any(|f| &f.name == new_name && f.id != id)
                {
                    return Err(format!("'{new_name}' is already used"));
                }
                doc.feature_mut(id).unwrap().name = new_name.clone();
                ok(format!("renamed to {new_name}"))
            }
            C::RollTo { feature } => {
                match feature {
                    Some(n) => {
                        let id = self.find(doc, n)?;
                        let i = doc.index_of(id).unwrap() + 1;
                        doc.rollback = if i >= doc.features.len() {
                            None
                        } else {
                            Some(i)
                        };
                    }
                    None => doc.rollback = None,
                }
                ok("moved the timeline marker".into())
            }
            C::ClearDesign => {
                let name = doc.name.clone();
                *doc = Document {
                    name,
                    ..Default::default()
                };
                self.current_sketch = None;
                ok("cleared the design".into())
            }
            C::SetDesignName { name } => {
                doc.name = name.clone();
                ok(format!("design renamed to {name}"))
            }
            C::Undo => Ok(Outcome::App(AppAction::Undo)),
            C::Redo => Ok(Outcome::App(AppAction::Redo)),
            C::View { view } => Ok(Outcome::App(AppAction::View(view.clone()))),
            C::FitAll => Ok(Outcome::App(AppAction::FitAll)),
            C::ExportStl { path } => Ok(Outcome::App(AppAction::ExportStl(path.clone()))),
            C::ExportGltf { path } => Ok(Outcome::App(AppAction::ExportGltf(path.clone()))),
            C::ExportStep { path } => Ok(Outcome::App(AppAction::ExportStep(path.clone()))),
            C::Save { path } => Ok(Outcome::App(AppAction::Save(path.clone()))),
            C::NewDesign => Ok(Outcome::App(AppAction::NewDesign)),
            C::Fillet { feature, size, start, end } | C::Chamfer { feature, size, start, end } => {
                let kind = if matches!(cmd, C::Fillet { .. }) {
                    cad_kernel::profile::EdgeKind::Fillet
                } else {
                    cad_kernel::profile::EdgeKind::Chamfer
                };
                let id = self.find(doc, feature)?;
                let f = doc.feature_mut(id).unwrap();
                match &mut f.kind {
                    FeatureKind::Extrude { edges, .. } => {
                        *edges = Some(crate::EdgeTreatment { kind, size: size.0.clone(), start: *start, end: *end });
                        ok(format!("{}: {:?} {} on {}", f.name, kind, size.0, match (start, end) {
                            (true, true) => "both caps",
                            (true, false) => "the start cap",
                            _ => "the end cap",
                        }))
                    }
                    _ => Err(format!("{} is not an extrude — fillet/chamfer works on extrudes", f.name)),
                }
            }
            C::Shell { feature, thickness, open, name } => {
                let id = self.find(doc, feature)?;
                if !matches!(doc.feature(id).map(|f| &f.kind), Some(FeatureKind::Extrude { .. })) {
                    return Err("shell works on extrudes".into());
                }
                let open = open.clone().unwrap_or_else(|| "end".into()).to_ascii_lowercase();
                if !["end", "start", "none", "top", "bottom"].contains(&open.as_str()) {
                    return Err("open must be end, start or none".into());
                }
                let open = match open.as_str() {
                    "top" => "end".to_string(),
                    "bottom" => "start".to_string(),
                    o => o.to_string(),
                };
                // the shell must come after its target
                let n = Self::insert(doc, name, "Shell", FeatureKind::Shell { target: id, thickness: thickness.0.clone(), open: open.clone() });
                ok(format!("{n}: shell {} open at {open}", thickness.0))
            }
            C::AddConstraint { sketch, constraint } => {
                let id = self.sketch_id(doc, sketch)?;
                let f = doc.feature_mut(id).unwrap();
                let name = f.name.clone();
                if let FeatureKind::Sketch(s) = &mut f.kind {
                    for e in constraint.entities() {
                        match s.entities.get(e) {
                            Some(Entity::Polyline { .. }) => {}
                            _ => return Err(format!("entity {} of {name} is not a polyline", e + 1)),
                        }
                    }
                    s.constraints.push(constraint.clone());
                }
                self.current_sketch = Some(id);
                ok(format!("{name}: {}", constraint.label()))
            }
            C::AutoConstrain { sketch } => {
                let id = self.sketch_id(doc, sketch)?;
                let f = doc.feature_mut(id).unwrap();
                let name = f.name.clone();
                if let FeatureKind::Sketch(s) = &mut f.kind {
                    let add = crate::constraints::auto_constraints(&s.polylines(), &s.constraints, 8.0);
                    let n = add.len();
                    s.constraints.extend(add);
                    return ok(format!("{name}: added {n} constraint(s)"));
                }
                unreachable!()
            }
            C::Building { width, depth, floors, floor_height, wall_thickness, roof, roof_height, windows_front, windows_side, window_width, window_height } => {
                let mut spec = crate::macros::BuildingSpec::default();
                let set = |dst: &mut String, v: &Option<Ex>| {
                    if let Some(v) = v {
                        *dst = v.0.clone();
                    }
                };
                set(&mut spec.width, width);
                set(&mut spec.depth, depth);
                set(&mut spec.floor_height, floor_height);
                set(&mut spec.wall, wall_thickness);
                set(&mut spec.roof_height, roof_height);
                set(&mut spec.window_width, window_width);
                set(&mut spec.window_height, window_height);
                if let Some(f) = floors {
                    spec.floors = (*f).clamp(1, 60);
                }
                if let Some(r) = roof {
                    spec.roof = r.clone();
                }
                if let Some(n) = windows_front {
                    spec.windows_front = *n;
                }
                if let Some(n) = windows_side {
                    spec.windows_side = *n;
                }
                let used: Vec<String> = doc.parameters.iter().map(|p| p.name.clone()).collect();
                let prefix = crate::macros::unique_prefix("bldg", &used);
                let vars = Self::vars(doc, &spec);
                let cmds = crate::macros::building(&spec, &prefix, &vars);
                self.apply_all(doc, &cmds)?;
                ok(format!("{prefix}: {}-storey building, {} roof ({} features)", spec.floors, spec.roof, cmds.len()))
            }
            C::Wall { from, to, height, thickness, base, operation, name } => {
                let name = name.clone().unwrap_or_else(|| doc.unique_name("Wall"));
                let op = operation.clone().unwrap_or_else(|| if Self::default_op(doc) == Operation::NewBody { "new_body".into() } else { "join".into() });
                let base = base.clone().map(|b| b.0).unwrap_or_else(|| "0".into());
                let cmds = crate::macros::wall(*from, *to, &height.0, thickness.unwrap_or(200.0), &base, &op, &name)?;
                self.apply_all(doc, &cmds)?;
                ok(format!("{name}: wall {:?} → {:?}", from, to))
            }
            C::Opening { x, z, width, height, sill, depth, angle, name } => {
                let name = name.clone().unwrap_or_else(|| doc.unique_name("Opening"));
                let sill = sill.clone().map(|s| s.0).unwrap_or_else(|| "0".into());
                let cmds = crate::macros::opening(*x, *z, *width, depth.unwrap_or(1000.0), &sill, &height.0, angle.unwrap_or(0.0), &name);
                self.apply_all(doc, &cmds)?;
                ok(format!("{name}: opening at ({x}, {z})"))
            }
            C::Part { kind, params, position } => {
                let used: Vec<String> = doc.parameters.iter().map(|p| p.name.clone()).collect();
                let base = kind.to_ascii_lowercase().replace([' ', '-'], "_");
                let prefix = crate::macros::unique_prefix(&base, &used);
                let cmds = crate::macros::part(kind, params, position.unwrap_or([0.0; 3]), &prefix)?;
                self.apply_all(doc, &cmds)?;
                ok(format!("{prefix}: {kind} ({} features)", cmds.iter().filter(|c| !matches!(c, Command::SetParameter { .. })).count()))
            }
        }
    }
}

/// Parse an AI answer: either `{"reply": "...", "commands": [...]}`, a bare command list,
/// or a single command, possibly wrapped in prose / code fences / <think> blocks.
/// Returns (reply text, parsed commands, per-command parse errors).
pub fn parse_ai_answer(text: &str) -> (String, Vec<Command>, Vec<String>) {
    let cleaned = strip_think(text);
    let Some(v) = extract_json(&cleaned) else {
        return (cleaned.trim().to_string(), Vec::new(), Vec::new());
    };
    let (reply, list) = match &v {
        Value::Array(a) => (String::new(), a.clone()),
        Value::Object(o) if o.contains_key("action") => (String::new(), vec![v.clone()]),
        Value::Object(o) => (
            o.get("reply")
                .or_else(|| o.get("message"))
                .and_then(|r| r.as_str())
                .unwrap_or("")
                .to_string(),
            o.get("commands")
                .and_then(|c| c.as_array())
                .cloned()
                .unwrap_or_default(),
        ),
        _ => (String::new(), Vec::new()),
    };
    let mut cmds = Vec::new();
    let mut errs = Vec::new();
    for c in list {
        match serde_json::from_value::<Command>(c.clone()) {
            Ok(cmd) => cmds.push(cmd),
            Err(e) => errs.push(format!(
                "{}: {e}",
                c.get("action").and_then(|a| a.as_str()).unwrap_or("?")
            )),
        }
    }
    (reply, cmds, errs)
}

fn strip_think(s: &str) -> String {
    let mut out = s.to_string();
    while let (Some(a), Some(b)) = (out.find("<think>"), out.find("</think>")) {
        if b < a {
            break;
        }
        out.replace_range(a..b + "</think>".len(), "");
    }
    out
}

/// Find the first balanced JSON object/array in `s` that parses.
pub fn extract_json(s: &str) -> Option<Value> {
    if let Ok(v) = serde_json::from_str::<Value>(s.trim()) {
        return Some(v);
    }
    let bytes: Vec<char> = s.chars().collect();
    for start in 0..bytes.len() {
        let open = bytes[start];
        if open != '{' && open != '[' {
            continue;
        }
        let close = if open == '{' { '}' } else { ']' };
        let (mut depth, mut in_str, mut esc) = (0i32, false, false);
        for end in start..bytes.len() {
            let c = bytes[end];
            if in_str {
                if esc {
                    esc = false;
                } else if c == '\\' {
                    esc = true;
                } else if c == '"' {
                    in_str = false;
                }
                continue;
            }
            match c {
                '"' => in_str = true,
                c if c == open => depth += 1,
                c if c == close => {
                    depth -= 1;
                    if depth == 0 {
                        let cand: String = bytes[start..=end].iter().collect();
                        if let Ok(v) = serde_json::from_str::<Value>(&cand) {
                            return Some(v);
                        }
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    None
}

/// A compact description of the design for the AI's context.
pub fn describe(doc: &Document, model: &Model) -> String {
    let mut s = format!("Design \"{}\" (units mm, degrees; Y is up).\n", doc.name);
    if doc.parameters.is_empty() {
        s.push_str("Parameters: none\n");
    } else {
        s.push_str("Parameters:\n");
        for p in &doc.parameters {
            let v = model
                .params
                .get(&p.name)
                .map(|v| trim_num(*v))
                .unwrap_or_else(|| "error".into());
            s.push_str(&format!("  {} = {} (= {v})\n", p.name, p.expr));
        }
    }
    s.push_str("Timeline:\n");
    if doc.features.is_empty() {
        s.push_str("  (empty)\n");
    }
    for (i, f) in doc.features.iter().enumerate() {
        let mut line = format!("  {}. {} [{}]", i + 1, f.name, f.kind.type_name());
        match &f.kind {
            FeatureKind::Shell { target, thickness, open } => {
                let tn = doc.feature(*target).map(|f| f.name.as_str()).unwrap_or("?");
                line += &format!(" of {tn} thickness {thickness} open {open}");
            }
            FeatureKind::Sketch(sk) => {
                let regions = model.sketches.get(&f.id).map_or(0, |g| g.regions.len());
                line += &format!(
                    " plane {:?} offset {} — {} entities, {} profiles",
                    sk.plane,
                    sk.offset,
                    sk.entities.len(),
                    regions
                );
                for (k, e) in sk.entities.iter().enumerate().take(12) {
                    line += &format!("\n      e{}: {}", k + 1, entity_text(e));
                }
            }
            FeatureKind::Extrude {
                sketch,
                profiles,
                distance,
                extent,
                op,
                ..
            } => {
                let sn = doc.feature(*sketch).map(|f| f.name.as_str()).unwrap_or("?");
                line += &format!(
                    " of {sn} profiles {profiles:?} distance {distance} {extent:?} {}",
                    op.label()
                );
            }
            FeatureKind::Revolve {
                sketch,
                angle,
                axis,
                op,
                ..
            } => {
                let sn = doc.feature(*sketch).map(|f| f.name.as_str()).unwrap_or("?");
                line += &format!(" of {sn} angle {angle} axis {axis:?} {}", op.label());
            }
            FeatureKind::Box { corner, size, op } => {
                line += &format!(" corner {corner:?} size {size:?} {}", op.label())
            }
            FeatureKind::Cylinder {
                base,
                axis,
                radius,
                height,
                op,
            } => {
                line += &format!(
                    " base {base:?} axis {axis:?} r {radius} h {height} {}",
                    op.label()
                )
            }
            FeatureKind::Sphere { center, radius, op } => {
                line += &format!(" center {center:?} r {radius} {}", op.label())
            }
        }
        if f.suppressed {
            line += " (suppressed)";
        }
        if let Some(e) = model.errors.get(&f.id) {
            line += &format!(" ERROR: {e}");
        }
        if i >= doc.active_len() {
            line += " (rolled back)";
        }
        s.push_str(&line);
        s.push('\n');
    }
    s.push_str("Bodies:\n");
    if model.bodies.is_empty() {
        s.push_str("  (none)\n");
    }
    for b in &model.bodies {
        if let Some((lo, hi)) = b.solid.bounds() {
            s.push_str(&format!(
                "  {}: bbox min [{}, {}, {}] max [{}, {}, {}], volume {} mm³\n",
                b.name,
                trim_num(lo.x),
                trim_num(lo.y),
                trim_num(lo.z),
                trim_num(hi.x),
                trim_num(hi.y),
                trim_num(hi.z),
                trim_num(b.solid.volume().round())
            ));
        }
    }
    s
}

fn entity_text(e: &Entity) -> String {
    match e {
        Entity::Rectangle {
            x,
            y,
            width,
            height,
        } => format!("rectangle x {x} y {y} w {width} h {height}"),
        Entity::Circle { cx, cy, radius } => format!("circle center ({cx}, {cy}) r {radius}"),
        Entity::Polyline { points, closed } => {
            format!(
                "{} polyline {} points",
                if *closed { "closed" } else { "open" },
                points.len()
            )
        }
    }
}

/// The command reference given to language models (and MCP clients).
pub const COMMAND_REFERENCE: &str = r#"Each command is a JSON object with an "action" field. Numbers may be JSON numbers or expression strings using parameters and units, e.g. "width/2", "3 in", "2*thickness".
Sketch planes (Y is up): "XZ" = ground/top plane, sketch x = world X, sketch y = world -Z, extrudes go up +Y.
"XY" = front plane, sketch x = X, sketch y = Y (up), extrudes go toward +Z. "YZ" = right plane, sketch x = -Z, sketch y = Y, extrudes toward +X.
Operations: "new_body", "join", "cut", "intersect" (default: new_body for the first solid, then join).
Actions:
{"action":"set_parameter","name":"width","value":"80 mm","comment":"optional"}
{"action":"delete_parameter","name":"width"}
{"action":"new_sketch","plane":"XZ","offset":0,"name":"optional"}      (becomes the current sketch)
{"action":"add_rectangle","x":0,"y":0,"width":40,"height":20}          (corner rectangle in the current sketch)
{"action":"add_center_rectangle","cx":0,"cy":0,"width":40,"height":20}
{"action":"add_circle","cx":0,"cy":0,"radius":10}
{"action":"add_polygon","points":[[0,0],[40,0],[20,30]],"closed":true}
{"action":"add_regular_polygon","cx":0,"cy":0,"radius":10,"sides":6}
{"action":"extrude","distance":10,"operation":"new_body","symmetric":false,"flip":false,"profiles":[0]}   (profiles optional: default = all outer profiles, inner circles stay as holes; "sketch":"Sketch1" optional)
{"action":"revolve","axis":"vertical","angle":360,"operation":"join"}   (axis through the sketch origin: "horizontal" or "vertical"; profile must not cross it)
{"action":"box","corner":[0,0,0],"size":[40,20,30],"operation":"join"}   (size = X, Y(height), Z)
{"action":"cylinder","base":[0,0,0],"axis":"y","radius":5,"height":20,"operation":"cut"}
{"action":"sphere","center":[0,0,0],"radius":10}
{"action":"set_feature_value","feature":"Extrude1","field":"distance","value":"15 mm"}   (fields: distance, angle, radius, height, offset, length, width)
{"action":"set_operation","feature":"Extrude2","operation":"cut"}
{"action":"delete_feature","feature":"Extrude2"}
{"action":"suppress_feature","feature":"Box1","suppressed":true}
{"action":"rename_feature","feature":"Extrude1","new_name":"Base plate"}
{"action":"roll_to","feature":"Extrude1"}   (omit feature to roll to the end)
{"action":"clear_design"}  {"action":"set_design_name","name":"Bracket"}
{"action":"undo"} {"action":"redo"} {"action":"view","view":"top|front|right|left|back|bottom|home"} {"action":"fit_all"}
{"action":"export_stl"} {"action":"export_gltf"} {"action":"export_step"} {"action":"save"}
{"action":"fillet","feature":"Extrude1","size":2,"end":true,"start":false}   (rounds the edges of an extrude's end cap and/or start cap; "chamfer" takes the same fields)
{"action":"shell","feature":"Extrude1","thickness":2,"open":"end"}   (hollows an extrude; open = end | start | none)
Sketch constraints (polylines; entity = 0-based index in the sketch, segment i joins point i and i+1):
{"action":"add_constraint","constraint":{"type":"horizontal","seg":{"entity":0,"segment":0}}}   (types: horizontal, vertical, length{value}, angle{value}, parallel{a,b}, perpendicular{a,b}, equal{a,b}, coincident{a,b points}, position{p,x,y}, symmetric{a,b,axis:"y"|"x"})
{"action":"auto_constrain"}   (adds horizontal/vertical/coincident constraints to nearly aligned lines of the current sketch)
Generators (all sizes become parameters you can edit later):
{"action":"building","width":12000,"depth":8000,"floors":2,"floor_height":3000,"wall_thickness":300,"roof":"gable|flat","roof_height":2500,"windows_front":4,"windows_side":2,"window_width":1200,"window_height":1400}
{"action":"wall","from":[0,0],"to":[5000,0],"height":2700,"thickness":200,"base":0}   (plan coordinates [x, z] in mm)
{"action":"opening","x":2500,"z":0,"width":1000,"height":1200,"sill":900,"depth":400,"angle":0}   (cuts a window/door through walls; angle = wall direction in plan, degrees)
{"action":"part","kind":"bolt","params":{"size":"M8","length":40},"position":[0,0,0]}   (kinds: bolt{size,length}, nut{size}, washer{size}, spur_gear{module,teeth,thickness,bore}, flange{outer_d,thickness,pcd,holes,hole_d,bore}, tube{outer_d,inner_d,length}, l_bracket{width,height,depth,thickness,hole_d}, enclosure{length,width,height,wall}, stairs{steps,rise,run,width})"#;

/// JSON schema for structured output (Ollama `format`).
pub fn answer_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "reply": {"type": "string"},
            "commands": {"type": "array", "items": {"type": "object", "properties": {"action": {"type": "string"}}, "required": ["action"]}}
        },
        "required": ["reply", "commands"]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(doc: &mut Document, ex: &mut Executor, json: &str) -> Vec<Result<Outcome, String>> {
        let (_, cmds, errs) = parse_ai_answer(json);
        assert!(errs.is_empty(), "{errs:?}");
        cmds.iter().map(|c| ex.apply(doc, c)).collect()
    }

    #[test]
    fn ai_style_plate_with_holes() {
        let mut d = Document::default();
        let mut ex = Executor::default();
        let answer = r#"<think>plan</think> Sure! ```json
        {"reply":"Made a plate","commands":[
          {"action":"set_parameter","name":"t","value":8},
          {"action":"new_sketch","plane":"XZ"},
          {"action":"add_center_rectangle","width":80,"height":50},
          {"action":"add_circle","cx":-28,"cy":0,"radius":6},
          {"action":"add_circle","cx":28,"cy":0,"radius":"12/2"},
          {"action":"extrude","distance":"t"}
        ]}```"#;
        let (reply, _, _) = parse_ai_answer(answer);
        assert_eq!(reply, "Made a plate");
        for r in run(&mut d, &mut ex, answer) {
            assert!(r.is_ok(), "{r:?}");
        }
        let m = d.regenerate();
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        let v = m.bodies[0].solid.volume();
        let exact = 80.0 * 50.0 * 8.0 - 2.0 * std::f64::consts::PI * 36.0 * 8.0;
        assert!((v - exact).abs() / exact < 0.01, "{v}");
        // edit + cut
        let r = run(
            &mut d,
            &mut ex,
            r#"[{"action":"set_parameter","name":"t","value":"10 mm"},
            {"action":"cylinder","base":[0,-1,0],"radius":5,"height":20,"operation":"cut"},
            {"action":"set_feature_value","feature":"extrude","field":"distance","value":"t*2"}]"#,
        );
        assert!(r.iter().all(|r| r.is_ok()), "{r:?}");
        let m = d.regenerate();
        assert!(m.errors.is_empty());
        assert!(m.bodies[0].solid.bounds().unwrap().1.y > 19.9);
        assert!(describe(&d, &m).contains("Extrude1"));
    }

    #[test]
    fn bad_commands_are_reported() {
        let (_, cmds, errs) = parse_ai_answer(
            r#"{"reply":"x","commands":[{"action":"teleport"},{"action":"undo"}]}"#,
        );
        assert_eq!(cmds.len(), 1);
        assert_eq!(errs.len(), 1);
        let mut d = Document::default();
        let mut ex = Executor::default();
        let c: Command = serde_json::from_str(r#"{"action":"add_circle","radius":3}"#).unwrap();
        assert!(ex.apply(&mut d, &c).is_err()); // no sketch yet
        let c: Command =
            serde_json::from_str(r#"{"action":"extrude","distance":3,"operation":"melt"}"#)
                .unwrap();
        assert!(ex.apply(&mut d, &c).is_err());
    }
}
