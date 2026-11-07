//! Exporters: binary glTF 2.0 (.glb) for Rust_Blender and other DCC tools, and STEP
//! (ISO 10303-21, AP214 faceted B-rep) for CAD exchange. Plus "Send to Rust_Blender",
//! which exports a .glb and asks the running Rust_Blender to import it over its MCP server.

use crate::app::App;
use cad_kernel::Solid;
use glam::DVec3;
use serde_json::json;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;

fn visible(app: &App) -> Vec<(String, &Solid)> {
    app.model
        .bodies
        .iter()
        .filter(|b| !app.hidden_bodies.contains(&b.name) && !b.solid.is_empty())
        .map(|b| (b.name.clone(), &b.solid))
        .collect()
}

/// Build a .glb (positions in metres, flat normals, one mesh + material per body).
pub fn glb_bytes(name: &str, bodies: &[(String, &Solid)]) -> Vec<u8> {
    let mut bin: Vec<u8> = Vec::new();
    let mut accessors = Vec::new();
    let mut views = Vec::new();
    let mut meshes = Vec::new();
    let mut nodes = Vec::new();
    for (i, (bname, solid)) in bodies.iter().enumerate() {
        let mut pos: Vec<f32> = Vec::new();
        let mut nrm: Vec<f32> = Vec::new();
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for p in &solid.polygons {
            let n = p.plane.normal;
            for k in 1..p.vertices.len() - 1 {
                for v in [p.vertices[0], p.vertices[k], p.vertices[k + 1]] {
                    let m = [(v.x * 0.001) as f32, (v.y * 0.001) as f32, (v.z * 0.001) as f32];
                    for c in 0..3 {
                        lo[c] = lo[c].min(m[c]);
                        hi[c] = hi[c].max(m[c]);
                    }
                    pos.extend(m);
                    nrm.extend([n.x as f32, n.y as f32, n.z as f32]);
                }
            }
        }
        let count = pos.len() / 3;
        for (data, is_pos) in [(&pos, true), (&nrm, false)] {
            let offset = bin.len();
            for f in data.iter() {
                bin.extend(f.to_le_bytes());
            }
            views.push(json!({"buffer": 0, "byteOffset": offset, "byteLength": data.len() * 4, "target": 34962}));
            let mut acc = json!({"bufferView": views.len() - 1, "componentType": 5126, "count": count, "type": "VEC3"});
            if is_pos {
                acc["min"] = json!(lo);
                acc["max"] = json!(hi);
            }
            accessors.push(acc);
        }
        let a = accessors.len();
        meshes.push(json!({"name": bname, "primitives": [{"attributes": {"POSITION": a - 2, "NORMAL": a - 1}, "material": i}]}));
        nodes.push(json!({"name": bname, "mesh": i}));
    }
    let materials: Vec<_> = bodies
        .iter()
        .map(|(n, _)| json!({"name": format!("{n} steel"), "pbrMetallicRoughness": {"baseColorFactor": [0.70, 0.73, 0.77, 1.0], "metallicFactor": 0.6, "roughnessFactor": 0.4}}))
        .collect();
    let doc = json!({
        "asset": {"version": "2.0", "generator": "Rust Fusion"},
        "scene": 0,
        "scenes": [{"name": name, "nodes": (0..nodes.len()).collect::<Vec<_>>()}],
        "nodes": nodes,
        "meshes": meshes,
        "materials": materials,
        "accessors": accessors,
        "bufferViews": views,
        "buffers": [{"byteLength": bin.len()}],
    });
    let mut js = doc.to_string().into_bytes();
    while js.len() % 4 != 0 {
        js.push(b' ');
    }
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
    let total = 12 + 8 + js.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend(b"glTF");
    out.extend(2u32.to_le_bytes());
    out.extend((total as u32).to_le_bytes());
    out.extend((js.len() as u32).to_le_bytes());
    out.extend(b"JSON");
    out.extend(js);
    out.extend((bin.len() as u32).to_le_bytes());
    out.extend(b"BIN\0");
    out.extend(bin);
    out
}

pub fn write_glb(app: &App, path: &Path) -> Result<String, String> {
    let bodies = visible(app);
    if bodies.is_empty() {
        return Err("Nothing to export: no visible bodies.".into());
    }
    std::fs::write(path, glb_bytes(&app.doc.name, &bodies)).map_err(|e| format!("Export failed: {e}"))?;
    Ok(format!("Exported {} body(ies) to {}", bodies.len(), path.display()))
}

fn step_str(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii() && *c != '\'' && *c != '\\').collect()
}

fn f(v: f64) -> String {
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0');
    if s.ends_with('.') { format!("{s}0") } else { s.to_string() }
}

/// STEP AP214 text with one faceted B-rep solid per body (millimetres).
pub fn step_text(name: &str, bodies: &[(String, &Solid)]) -> String {
    let name = step_str(name);
    let mut e = String::new();
    let mut id = 0usize;
    let mut next = |e: &mut String, body: String| -> usize {
        id += 1;
        let _ = writeln!(e, "#{id}={body};");
        id
    };
    let app_ctx = next(&mut e, "APPLICATION_CONTEXT('core data for automotive mechanical design processes')".into());
    next(&mut e, format!("APPLICATION_PROTOCOL_DEFINITION('international standard','automotive_design',2000,#{app_ctx})"));
    let prod_ctx = next(&mut e, format!("PRODUCT_CONTEXT('',#{app_ctx},'mechanical')"));
    let product = next(&mut e, format!("PRODUCT('{name}','{name}','',(#{prod_ctx}))"));
    next(&mut e, format!("PRODUCT_RELATED_PRODUCT_CATEGORY('part',$,(#{product}))"));
    let pdf = next(&mut e, format!("PRODUCT_DEFINITION_FORMATION('','',#{product})"));
    let pd_ctx = next(&mut e, format!("PRODUCT_DEFINITION_CONTEXT('part definition',#{app_ctx},'design')"));
    let pd = next(&mut e, format!("PRODUCT_DEFINITION('design','',#{pdf},#{pd_ctx})"));
    let pds = next(&mut e, format!("PRODUCT_DEFINITION_SHAPE('','',#{pd})"));
    let len_unit = next(&mut e, "(LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.))".into());
    let ang_unit = next(&mut e, "(NAMED_UNIT(*) PLANE_ANGLE_UNIT() SI_UNIT($,.RADIAN.))".into());
    let sol_unit = next(&mut e, "(NAMED_UNIT(*) SI_UNIT($,.STERADIAN.) SOLID_ANGLE_UNIT())".into());
    let unc = next(&mut e, format!("UNCERTAINTY_MEASURE_WITH_UNIT(LENGTH_MEASURE(1.E-06),#{len_unit},'distance_accuracy_value','confusion accuracy')"));
    let ctx = next(
        &mut e,
        format!("(GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT((#{unc})) GLOBAL_UNIT_ASSIGNED_CONTEXT((#{len_unit},#{ang_unit},#{sol_unit})) REPRESENTATION_CONTEXT('Context3D','3D Context with UNIT and UNCERTAINTY'))"),
    );
    let o = next(&mut e, "CARTESIAN_POINT('',(0.,0.,0.))".into());
    let dz = next(&mut e, "DIRECTION('',(0.,0.,1.))".into());
    let dx = next(&mut e, "DIRECTION('',(1.,0.,0.))".into());
    let axis = next(&mut e, format!("AXIS2_PLACEMENT_3D('',#{o},#{dz},#{dx})"));
    let mut items = vec![axis];
    for (bname, solid) in bodies {
        let mut points: HashMap<[i64; 3], usize> = HashMap::new();
        let mut faces = Vec::new();
        for p in &solid.polygons {
            let mut refs = Vec::new();
            for v in &p.vertices {
                let key = [(v.x * 1e6).round() as i64, (v.y * 1e6).round() as i64, (v.z * 1e6).round() as i64];
                let pid = match points.get(&key) {
                    Some(&i) => i,
                    None => {
                        let i = next(&mut e, format!("CARTESIAN_POINT('',({},{},{}))", f(v.x), f(v.y), f(v.z)));
                        points.insert(key, i);
                        i
                    }
                };
                if refs.last() != Some(&pid) {
                    refs.push(pid);
                }
            }
            if refs.len() > 1 && refs.first() == refs.last() {
                refs.pop();
            }
            if refs.len() < 3 {
                continue;
            }
            let list = refs.iter().map(|r| format!("#{r}")).collect::<Vec<_>>().join(",");
            let lp = next(&mut e, format!("POLY_LOOP('',({list}))"));
            let bound = next(&mut e, format!("FACE_OUTER_BOUND('',#{lp},.T.)"));
            faces.push(next(&mut e, format!("FACE('',(#{bound}))")));
        }
        let list = faces.iter().map(|r| format!("#{r}")).collect::<Vec<_>>().join(",");
        let shell = next(&mut e, format!("CLOSED_SHELL('',({list}))"));
        items.push(next(&mut e, format!("FACETED_BREP('{}',#{shell})", step_str(bname))));
    }
    let list = items.iter().map(|r| format!("#{r}")).collect::<Vec<_>>().join(",");
    let rep = next(&mut e, format!("FACETED_BREP_SHAPE_REPRESENTATION('{name}',({list}),#{ctx})"));
    next(&mut e, format!("SHAPE_DEFINITION_REPRESENTATION(#{pds},#{rep})"));
    let _ = DVec3::ZERO;
    format!(
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('Rust Fusion faceted solid'),'2;1');\nFILE_NAME('{name}.step','{}',(''),(''),'Rust Fusion','Rust Fusion','');\nFILE_SCHEMA(('AUTOMOTIVE_DESIGN {{ 1 0 10303 214 1 1 1 1 }}'));\nENDSEC;\nDATA;\n{e}ENDSEC;\nEND-ISO-10303-21;\n",
        "2026-01-01T00:00:00"
    )
}

pub fn write_step(app: &App, path: &Path) -> Result<String, String> {
    let bodies = visible(app);
    if bodies.is_empty() {
        return Err("Nothing to export: no visible bodies.".into());
    }
    std::fs::write(path, step_text(&app.doc.name, &bodies)).map_err(|e| format!("Export failed: {e}"))?;
    Ok(format!("Exported {} body(ies) to {} (STEP faceted B-rep)", bodies.len(), path.display()))
}

/// Export a .glb to the temp folder and import it into a running Rust_Blender over MCP.
pub fn send_to_blender(app: &mut App) -> Result<String, String> {
    let bodies = visible(app);
    if bodies.is_empty() {
        return Err("Nothing to send: no visible bodies.".into());
    }
    let dir = std::env::temp_dir().join("rust_fusion");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let safe: String = app.doc.name.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
    let path = dir.join(format!("{safe}.glb"));
    std::fs::write(&path, glb_bytes(&app.doc.name, &bodies)).map_err(|e| e.to_string())?;
    let port = rust_blender_port();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(std::time::Duration::from_secs(60)))
        .build()
        .into();
    let call = |method: &str, params: serde_json::Value| {
        agent
            .post(&format!("http://127.0.0.1:{port}/mcp"))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .send_json(&json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}))
    };
    let _ = call("initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "rust-fusion", "version": env!("CARGO_PKG_VERSION")}}))
        .map_err(|_| format!("Rust_Blender isn't running (no MCP server on port {port}). The model was saved to {}", path.display()))?;
    let mut r = call("tools/call", json!({"name": "import_gltf", "arguments": {"path": path.display().to_string()}}))
        .map_err(|e| format!("Rust_Blender import failed: {e}"))?;
    let v: serde_json::Value = r.body_mut().read_json().unwrap_or_default();
    if v["result"]["isError"].as_bool() == Some(true) || v.get("error").is_some() {
        let msg = v["result"]["content"][0]["text"].as_str().or(v["error"]["message"].as_str()).unwrap_or("unknown error");
        return Err(format!("Rust_Blender: {msg}"));
    }
    Ok(format!("Sent {} body(ies) to Rust_Blender ({})", bodies.len(), path.display()))
}

/// Rust_Blender's MCP port from its settings (default 7878).
fn rust_blender_port() -> u16 {
    let p = dirs::config_dir().unwrap_or_default().join("rust_blender").join("settings.json");
    std::fs::read_to_string(p)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v["mcp_port"].as_u64())
        .map(|p| p as u16)
        .unwrap_or(7878)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glb_layout_is_valid() {
        let s = cad_kernel::primitives::cuboid(DVec3::ZERO, DVec3::splat(10.0));
        let b = glb_bytes("t", &[("Body1".into(), &s)]);
        assert_eq!(&b[0..4], b"glTF");
        let total = u32::from_le_bytes(b[8..12].try_into().unwrap()) as usize;
        assert_eq!(total, b.len());
        let jlen = u32::from_le_bytes(b[12..16].try_into().unwrap()) as usize;
        let js: serde_json::Value = serde_json::from_slice(&b[20..20 + jlen]).unwrap();
        assert_eq!(js["accessors"][0]["count"], 36);
        assert!((js["accessors"][0]["max"][0].as_f64().unwrap() - 0.01).abs() < 1e-6);
        let blen = u32::from_le_bytes(b[20 + jlen..24 + jlen].try_into().unwrap()) as usize;
        assert_eq!(blen, js["buffers"][0]["byteLength"].as_u64().unwrap() as usize);
    }

    #[test]
    fn step_has_shared_points_and_faces() {
        let s = cad_kernel::primitives::cuboid(DVec3::ZERO, DVec3::splat(10.0));
        let t = step_text("Cube", &[("Body1".into(), &s)]);
        assert!(t.starts_with("ISO-10303-21;"));
        assert_eq!(t.matches("CARTESIAN_POINT('',(").count(), 8 + 1);
        assert_eq!(t.matches("=FACE(").count(), 6);
        assert!(t.contains("FACETED_BREP('Body1'"));
        assert!(t.trim_end().ends_with("END-ISO-10303-21;"));
    }
}
