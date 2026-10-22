//! Terrain commands built on `cadcraft-terrain`: triangulating a point cloud into a TIN,
//! querying its elevation at a point, and extracting contour-line segments - each as a stateless
//! calculation over the points given in the call (no persistent terrain object yet), consistent
//! with "programmatic calls never open dialogs". `terrain.create_mnt` is the one exception that
//! reads the drawing itself: it gathers the points for `terrain.triangulate` from the active
//! space instead of making the caller assemble `points` by hand, the way a MNT/TIN creation
//! wizard needs to (object selection by block/layer or a manual pick, altitude filtering).

use std::collections::HashSet;

use cadcraft_doc::{EntityKind, Handle, Line};
use cadcraft_geom::Vec3;
use cadcraft_terrain::{TerrainError, contours, triangulate};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("terrain.triangulate", "Triangulate (TIN)", run_triangulate)
            .menu(&["Topo 3D", "TIN", "Triangulate"])
            .params("{points: [[x,y,z], ...], draw?: bool}"),
        CommandSpec::new("terrain.elevation_at", "Elevation At Point", run_elevation_at)
            .menu(&["Topo 3D", "TIN", "Elevation At Point"])
            .params("{points: [[x,y,z], ...], x: number, y: number}"),
        CommandSpec::new("terrain.contours", "Contour Lines", run_contours)
            .menu(&["Topo 3D", "TIN", "Contour Lines"])
            .alias(&["courbes"])
            .params("{points: [[x,y,z], ...], interval: number, draw?: bool}"),
        CommandSpec::new("terrain.create_mnt", "Create TIN...", run_create_mnt).menu(&["Topo 3D", "TIN", "Create TIN..."]).params(
            "{selection: \"auto\"|\"manual\" (default \"auto\"), blockNames?: [string], layers?: [string] \
             (both narrow \"auto\"; empty/omitted means every point block and layer), \
             handles?: [string] (required for \"manual\"), ignoreNullAltitude?: bool (default true), \
             draw?: bool (default true)} → {pointCount, skippedNullAltitude, triangleCount, edgeCount, \
             elevationRange}. Points come from every topo point block (any INSERT with an ALT attribute, \
             at its insertion XY/ALT) and bare POINT entities (at their own XYZ) in the active space.",
        ),
    ]
}

/// One point usable for a MNT: its handle (for a manual selection to filter by), block name
/// (empty for a bare POINT, which `blockNames`/`layers` never narrow out), layer and XYZ.
struct MntCandidate {
    handle: Handle,
    block: String,
    layer: String,
    p: Vec3,
}

fn alt_attrib(ins: &cadcraft_doc::Insert) -> Option<f64> {
    ins.attribs.iter().find(|a| a.tag.eq_ignore_ascii_case("ALT")).and_then(|a| a.text.value.trim().parse().ok())
}

/// Every point in the active space a MNT could use: a topo point block (any INSERT carrying an
/// `ALT` attribute, regardless of which block definition it uses) or a bare POINT entity.
fn all_mnt_candidates(s: &Session) -> Result<Vec<MntCandidate>> {
    let st = s.state()?;
    let Some(store) = st.doc.space(&st.space) else { return Ok(Vec::new()) };
    let mut out = Vec::new();
    for e in store.iter() {
        match &e.kind {
            EntityKind::Insert(ins) => {
                if let Some(alt) = alt_attrib(ins) {
                    out.push(MntCandidate {
                        handle: e.handle,
                        block: ins.block.clone(),
                        layer: e.common.layer.clone(),
                        p: Vec3::new(ins.insert.x, ins.insert.y, alt),
                    });
                }
            }
            EntityKind::Point(pt) => {
                out.push(MntCandidate { handle: e.handle, block: String::new(), layer: e.common.layer.clone(), p: pt.p });
            }
            _ => {}
        }
    }
    Ok(out)
}

fn names_param(p: &Value, key: &str) -> Vec<String> {
    p.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default()
}

fn matches_names(name: &str, names: &[String]) -> bool {
    names.is_empty() || names.iter().any(|n| n.eq_ignore_ascii_case(name))
}

fn run_create_mnt(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "terrain.create_mnt";
    let selection = str_param(p, "selection").unwrap_or("auto");
    let block_names = names_param(p, "blockNames");
    let layers = names_param(p, "layers");
    let ignore_null = bool_or(p, "ignoreNullAltitude", true);
    let draw = bool_or(p, "draw", true);

    let all = all_mnt_candidates(s)?;
    let mut chosen: Vec<Vec3> = match selection {
        "manual" => {
            let hs = p.get("handles").and_then(Value::as_array).ok_or_else(|| bad(CMD, "`handles` is required for `selection: \"manual\"`"))?;
            let wanted: HashSet<Handle> = hs.iter().filter_map(Value::as_str).filter_map(Handle::parse_hex).collect();
            all.into_iter().filter(|c| wanted.contains(&c.handle)).map(|c| c.p).collect()
        }
        "auto" => all.into_iter().filter(|c| matches_names(&c.block, &block_names) && matches_names(&c.layer, &layers)).map(|c| c.p).collect(),
        other => return Err(bad(CMD, format!("unknown `selection` \"{other}\" (expected \"auto\" or \"manual\")"))),
    };

    let before = chosen.len();
    let skipped = if ignore_null {
        chosen.retain(|p| p.z != 0.0);
        before - chosen.len()
    } else {
        0
    };
    if chosen.len() < 3 {
        return Err(bad(
            CMD,
            format!(
                "not enough points with an altitude to triangulate: {} found (need at least 3); \
                 {skipped} point(s) were skipped for having no altitude",
                chosen.len()
            ),
        ));
    }

    let tin = triangulate(&chosen).map_err(|e| terrain_err(CMD, e))?;
    let mut edge_count = 0usize;
    if draw {
        let mut drawn: std::collections::HashSet<(usize, usize)> = std::collections::HashSet::new();
        for tri in &tin.triangles {
            for (i, j) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                let key = if i < j { (i, j) } else { (j, i) };
                if !drawn.insert(key) {
                    continue;
                }
                let (Some(a), Some(b)) = (tin.points.get(i), tin.points.get(j)) else { continue };
                s.add_entity(EntityKind::Line(Line { a: *a, b: *b }))?;
                edge_count += 1;
            }
        }
    }

    Ok(json!({
        "pointCount": tin.points.len(),
        "skippedNullAltitude": skipped,
        "triangleCount": tin.triangles.len(),
        "edgeCount": edge_count,
        "elevationRange": tin.elevation_range(),
    }))
}

fn points_param(cmd: &str, p: &Value) -> Result<Vec<Vec3>> {
    let arr = p.get("points").and_then(Value::as_array).ok_or_else(|| bad(cmd, "`points` (array of [x, y, z]) is required"))?;
    arr.iter()
        .enumerate()
        .map(|(i, v)| {
            let a = v.as_array().ok_or_else(|| bad(cmd, format!("points[{i}] must be an array [x, y, z]")))?;
            let get = |k: usize| a.get(k).and_then(Value::as_f64).filter(|n| n.is_finite());
            match (get(0), get(1)) {
                (Some(x), Some(y)) => Ok(Vec3::new(x, y, get(2).unwrap_or(0.0))),
                _ => Err(bad(cmd, format!("points[{i}] must be [x, y] or [x, y, z] of finite numbers"))),
            }
        })
        .collect()
}

fn terrain_err(cmd: &str, e: TerrainError) -> crate::EngineError {
    bad(cmd, e.to_string())
}

fn run_triangulate(s: &mut Session, p: &Value) -> Result<Value> {
    let pts = points_param("terrain.triangulate", p)?;
    let tin = triangulate(&pts).map_err(|e| terrain_err("terrain.triangulate", e))?;
    let draw = bool_or(p, "draw", true);

    let mut edge_count = 0usize;
    if draw {
        // Each undirected edge drawn once, even though it borders two triangles.
        let mut drawn: std::collections::HashSet<(usize, usize)> = std::collections::HashSet::new();
        for tri in &tin.triangles {
            for (i, j) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                let key = if i < j { (i, j) } else { (j, i) };
                if !drawn.insert(key) {
                    continue;
                }
                let (Some(a), Some(b)) = (tin.points.get(i), tin.points.get(j)) else { continue };
                s.add_entity(EntityKind::Line(Line { a: *a, b: *b }))?;
                edge_count += 1;
            }
        }
    }

    Ok(json!({
        "pointCount": tin.points.len(),
        "triangleCount": tin.triangles.len(),
        "edgeCount": edge_count,
        "elevationRange": tin.elevation_range(),
    }))
}

fn run_elevation_at(_s: &mut Session, p: &Value) -> Result<Value> {
    let pts = points_param("terrain.elevation_at", p)?;
    let x = f64_req("terrain.elevation_at", p, "x")?;
    let y = f64_req("terrain.elevation_at", p, "y")?;
    let tin = triangulate(&pts).map_err(|e| terrain_err("terrain.elevation_at", e))?;
    Ok(json!({ "elevation": tin.elevation_at(x, y) }))
}

/// Extracts and optionally draws contour-line segments. **Limitation, stated plainly**: each
/// segment is emitted per-triangle and is not yet chained into continuous polylines across
/// triangles - that stitching is a later increment.
fn run_contours(s: &mut Session, p: &Value) -> Result<Value> {
    let pts = points_param("terrain.contours", p)?;
    let interval = f64_req("terrain.contours", p, "interval")?;
    if !interval.is_finite() || interval <= 0.0 {
        return Err(bad("terrain.contours", "`interval` must be a positive finite number"));
    }
    let tin = triangulate(&pts).map_err(|e| terrain_err("terrain.contours", e))?;
    let segments = contours(&tin, interval);
    let draw = bool_or(p, "draw", true);

    if draw {
        for seg in &segments {
            s.add_entity(EntityKind::Line(Line { a: seg.start.to3(seg.elevation), b: seg.end.to3(seg.elevation) }))?;
        }
    }

    let mut levels: Vec<f64> = segments.iter().map(|sg| sg.elevation).collect();
    levels.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    levels.dedup();

    Ok(json!({ "segmentCount": segments.len(), "levels": levels }))
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::Session;

    fn session_with_doc() -> Session {
        let mut s = Session::new();
        s.execute("new", &json!({})).expect("new drawing should succeed");
        s
    }

    fn square_pts() -> Value {
        json!([[0.0, 0.0, 0.0], [10.0, 0.0, 10.0], [10.0, 10.0, 10.0], [0.0, 10.0, 0.0]])
    }

    #[test]
    fn triangulate_draws_edges_and_reports_counts() {
        let mut s = session_with_doc();
        let out = s.execute("terrain.triangulate", &json!({ "points": square_pts() })).expect("triangulate should succeed");
        assert_eq!(out["triangleCount"].as_u64(), Some(2));
        assert_eq!(out["pointCount"].as_u64(), Some(4));
        // A convex quad split into 2 triangles has 5 undirected edges (4 sides + 1 diagonal).
        assert_eq!(out["edgeCount"].as_u64(), Some(5));
    }

    #[test]
    fn triangulate_without_draw_inserts_nothing() {
        let mut s = session_with_doc();
        let out = s.execute("terrain.triangulate", &json!({ "points": square_pts(), "draw": false })).expect("should succeed");
        assert_eq!(out["edgeCount"].as_u64(), Some(0));
    }

    #[test]
    fn triangulate_rejects_too_few_points_as_an_error_not_a_panic() {
        let mut s = session_with_doc();
        let out = s.execute("terrain.triangulate", &json!({ "points": [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]] }));
        assert!(out.is_err());
    }

    #[test]
    fn elevation_at_interpolates_on_the_tilted_plane() {
        let mut s = session_with_doc();
        let out = s.execute("terrain.elevation_at", &json!({ "points": square_pts(), "x": 5.0, "y": 5.0 })).expect("should succeed");
        let z = out["elevation"].as_f64().expect("elevation should be present inside the hull");
        assert!((z - 5.0).abs() < 1e-6);
    }

    #[test]
    fn elevation_at_outside_the_hull_is_null() {
        let mut s = session_with_doc();
        let out = s.execute("terrain.elevation_at", &json!({ "points": square_pts(), "x": -50.0, "y": -50.0 })).expect("should succeed");
        assert!(out["elevation"].is_null());
    }

    #[test]
    fn contours_draws_segments_and_reports_levels() {
        let mut s = session_with_doc();
        let out = s.execute("terrain.contours", &json!({ "points": square_pts(), "interval": 2.0 })).expect("should succeed");
        assert_eq!(out["levels"].as_array().map(|a| a.len()), Some(4));
        assert!(out["segmentCount"].as_u64().unwrap_or(0) > 0);
    }

    #[test]
    fn contours_rejects_a_non_positive_interval() {
        let mut s = session_with_doc();
        let out = s.execute("terrain.contours", &json!({ "points": square_pts(), "interval": 0.0 }));
        assert!(out.is_err());
    }

    /// Imports a one-line CSV as a topo point block, the same way other topo command tests do
    /// (`topo_altitudes.rs`'s own test helper), so `terrain.create_mnt` has a real ALT-bearing
    /// INSERT to find without duplicating `topo.import_points`' own import logic here.
    fn import_tcpoint(s: &mut Session, id: &str, x: f64, y: f64, z: f64) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("cadcraft-terrain-mnt-test-{}-{id}-{n}.csv", std::process::id()));
        std::fs::write(&path, format!("ID;X;Y;Z;CODE\n{id};{x};{y};{z};P\n")).expect("writing the temp points file should succeed");
        s.execute("topo.import_points", &json!({ "path": path.to_string_lossy(), "label": false })).expect("import should succeed");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn create_mnt_triangulates_every_topo_point_automatically() {
        let mut s = session_with_doc();
        import_tcpoint(&mut s, "1", 0.0, 0.0, 10.0);
        import_tcpoint(&mut s, "2", 10.0, 0.0, 12.0);
        import_tcpoint(&mut s, "3", 5.0, 10.0, 15.0);

        let out = s.execute("terrain.create_mnt", &json!({ "selection": "auto" })).expect("should succeed");
        assert_eq!(out["pointCount"].as_u64(), Some(3));
        assert_eq!(out["triangleCount"].as_u64(), Some(1));
        assert_eq!(out["skippedNullAltitude"].as_u64(), Some(0));
    }

    #[test]
    fn create_mnt_skips_points_with_a_null_altitude_by_default() {
        let mut s = session_with_doc();
        import_tcpoint(&mut s, "1", 0.0, 0.0, 10.0);
        import_tcpoint(&mut s, "2", 10.0, 0.0, 12.0);
        import_tcpoint(&mut s, "3", 5.0, 10.0, 15.0);
        // A 2D-only point (no real altitude): skipped by default, included with the flag off.
        import_tcpoint(&mut s, "4", 5.0, 5.0, 0.0);

        let out = s.execute("terrain.create_mnt", &json!({ "selection": "auto" })).expect("should succeed");
        assert_eq!(out["pointCount"].as_u64(), Some(3));
        assert_eq!(out["skippedNullAltitude"].as_u64(), Some(1));

        let out = s.execute("terrain.create_mnt", &json!({ "selection": "auto", "ignoreNullAltitude": false })).expect("should succeed");
        assert_eq!(out["pointCount"].as_u64(), Some(4));
        assert_eq!(out["skippedNullAltitude"].as_u64(), Some(0));
    }

    #[test]
    fn create_mnt_rejects_fewer_than_three_usable_points() {
        let mut s = session_with_doc();
        import_tcpoint(&mut s, "1", 0.0, 0.0, 10.0);
        import_tcpoint(&mut s, "2", 10.0, 0.0, 12.0);
        let out = s.execute("terrain.create_mnt", &json!({ "selection": "auto" }));
        assert!(out.is_err());
    }

    #[test]
    fn create_mnt_manual_selection_uses_only_the_given_handles() {
        let mut s = session_with_doc();
        import_tcpoint(&mut s, "1", 0.0, 0.0, 10.0);
        import_tcpoint(&mut s, "2", 10.0, 0.0, 12.0);
        import_tcpoint(&mut s, "3", 5.0, 10.0, 15.0);
        // A fourth point that manual selection will leave out.
        import_tcpoint(&mut s, "4", 20.0, 20.0, 20.0);

        let handles: Vec<String> = s
            .doc()
            .unwrap()
            .model
            .iter()
            .filter(|e| matches!(&e.kind, cadcraft_doc::EntityKind::Insert(_)))
            .take(3)
            .map(|e| e.handle.hex())
            .collect();
        let out = s.execute("terrain.create_mnt", &json!({ "selection": "manual", "handles": handles })).expect("should succeed");
        assert_eq!(out["pointCount"].as_u64(), Some(3));
    }

    #[test]
    fn create_mnt_without_draw_inserts_nothing() {
        let mut s = session_with_doc();
        import_tcpoint(&mut s, "1", 0.0, 0.0, 10.0);
        import_tcpoint(&mut s, "2", 10.0, 0.0, 12.0);
        import_tcpoint(&mut s, "3", 5.0, 10.0, 15.0);
        let before = s.doc().unwrap().model.iter().count();
        let out = s.execute("terrain.create_mnt", &json!({ "selection": "auto", "draw": false })).expect("should succeed");
        assert_eq!(out["edgeCount"].as_u64(), Some(0));
        assert_eq!(s.doc().unwrap().model.iter().count(), before);
    }
}
