//! Foncier / Parcellaire: a first slice of the land-parcel tools - merging adjacent parcels and
//! a parcel-state listing (reference, surface, layer).
//!
//! Merging turned out not to be a thin wrapper over CADCraft's existing REGION/UNION boolean:
//! that boolean explicitly declines boundaries that coincide ("overlap" in its own wording),
//! which is exactly the ordinary case of two parcels sharing a cadastral boundary line. So this
//! file carries its own weld-and-trace merge instead: cancel each pair of shared edges (same two
//! endpoints, opposite direction) between the straight-sided polygons, then chain what is left
//! into the merged outer boundary - the same "a triangle edge not matched by its neighbour is on
//! the hull" idea `cadcraft-terrain`'s triangulation already uses for its own boundary.
//!
//! **Scope, stated plainly**: curved parcel edges (a bulged polyline vertex) are refused rather
//! than silently flattened. `topo.inspect_edigeo` reads the EDIGEO (ISO 8211) cadastral exchange
//! container and reports its real field/subfield shape, but does not yet reconstruct parcel
//! geometry from it: which tags hold a point's coordinates, an arc's point list, or a parcel's
//! boundary is defined by EDIGEO's object nomenclature, not by the container format, and is
//! deliberately not guessed from memory without a real export to check the mapping against -
//! see `cadcraft_topo_io::edigeo` for why. `topo.import_edigeo` (turning that into parcel
//! polylines) is the next increment once a real file's shape is known.

use cadcraft_doc::{EntityKind, Handle};
use cadcraft_geom::{Polyline, Vec2};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("topo.merge_parcels", "Merge Parcels", run_merge_parcels)
            .menu(&["Topo", "Foncier", "Merge Parcels"])
            .params("{handles: [handle, ...]} (closed polylines or regions, at least 2) → {handle, area}"),
        CommandSpec::new("topo.parcel_state", "Parcel State (listing)", run_parcel_state)
            .menu(&["Topo", "Foncier", "Parcel State"])
            .params("{layer?: string, path?: string (also writes a CSV)} → {parcels: [{reference, handle, layer, area}], path?}")
            .noundo(),
        CommandSpec::new("topo.inspect_edigeo", "Inspect EDIGEO File", run_inspect_edigeo)
            .menu(&["Topo", "Foncier", "Référentiel fonds (EDIGEO)", "Inspecter"])
            .params(
                "{path: string} → {totalRecords, hasDdr, fieldTags: [{tag, recordCount, subfieldCountsSeen}], \
                 sample: [{leaderIdentifier, fields: [{tag, subfields}]}]} (read-only: reports the file's real \
                 field/subfield shape, does not import anything yet)",
            )
            .noundo(),
    ]
}

fn handle_list(cmd: &str, p: &Value, key: &str) -> Result<Vec<Handle>> {
    let a = p.get(key).and_then(Value::as_array).ok_or_else(|| bad(cmd, format!("`{key}` (array of handles) is required")))?;
    a.iter().map(|v| v.as_str().and_then(Handle::parse_hex).ok_or_else(|| bad(cmd, format!("`{key}`: {v} is not an object handle")))).collect()
}

/// The area of a closed curve or region entity, or `None` for anything else / an open curve.
fn parcel_area(e: &cadcraft_doc::Entity) -> Option<f64> {
    match &e.kind {
        EntityKind::Region(r) => Some(r.area()),
        EntityKind::LwPolyline(p) if p.closed => Some(Polyline { vertices: p.vertices.clone(), closed: true }.area().abs()),
        EntityKind::Polyline3d(p) if p.closed => {
            let vertices = p.points.iter().map(|v| cadcraft_geom::PolyVertex::new(v.xy())).collect();
            Some(Polyline { vertices, closed: true }.area().abs())
        }
        _ => None,
    }
}

const EDGE_TOL: f64 = 1e-6;

/// `h`'s boundary as a closed, straight-sided polygon (bulges refused rather than flattened),
/// oriented counter-clockwise so edge directions are comparable across parcels.
pub(crate) fn straight_polygon(cmd: &str, s: &Session, h: Handle) -> Result<Vec<Vec2>> {
    let e = s.doc()?.entity(h).ok_or_else(|| bad(cmd, format!("no such object: {}", h.hex())))?;
    let EntityKind::LwPolyline(p) = &e.kind else { return Err(bad(cmd, format!("{}: not a closed polyline", h.hex()))) };
    if !p.closed {
        return Err(bad(cmd, format!("{}: the polyline is not closed", h.hex())));
    }
    if p.vertices.iter().any(|v| v.bulge != 0.0) {
        return Err(bad(cmd, format!("{}: merging curved (bulged) parcel edges is not supported yet", h.hex())));
    }
    let mut pts: Vec<Vec2> = p.vertices.iter().map(|v| v.p).collect();
    if pts.len() < 3 {
        return Err(bad(cmd, format!("{}: a parcel needs at least 3 vertices", h.hex())));
    }
    if cadcraft_geom::shoelace(&pts) < 0.0 {
        pts.reverse();
    }
    Ok(pts)
}

/// Merges adjacent parcels into one, removing the shared boundary between them. See the module
/// doc comment for why this doesn't just defer to REGION/UNION.
fn run_merge_parcels(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "topo.merge_parcels";
    let hs = handle_list(CMD, p, "handles")?;
    if hs.len() < 2 {
        return Err(bad(CMD, "`handles` needs at least 2 parcels to merge"));
    }
    let layer = s.doc()?.entity(hs[0]).map(|e| e.common.layer.clone()).unwrap_or_else(|| "0".into());

    // Every polygon's directed edges (a -> b, CCW), tagged with their source so a match removes
    // both sides of a shared edge: edge (a, b) in one parcel cancels edge (b, a) in another.
    let mut edges: Vec<(Vec2, Vec2)> = Vec::new();
    for &h in &hs {
        let pts = straight_polygon(CMD, s, h)?;
        let n = pts.len();
        for i in 0..n {
            let (Some(&a), Some(&b)) = (pts.get(i), pts.get((i + 1) % n)) else { continue };
            edges.push((a, b));
        }
    }

    let mut cancelled = vec![false; edges.len()];
    let mut shared_edges = 0usize;
    for i in 0..edges.len() {
        if cancelled[i] {
            continue;
        }
        let (a, b) = edges[i];
        if let Some(j) = (i + 1..edges.len()).find(|&j| !cancelled[j] && edges[j].0.near(b, EDGE_TOL) && edges[j].1.near(a, EDGE_TOL)) {
            cancelled[i] = true;
            cancelled[j] = true;
            shared_edges += 1;
        }
    }
    if shared_edges == 0 {
        return Err(bad(CMD, "these parcels don't share a boundary edge - nothing to merge"));
    }

    let mut remaining: Vec<(Vec2, Vec2)> = edges.into_iter().zip(cancelled).filter(|(_, c)| !c).map(|(e, _)| e).collect();
    if remaining.is_empty() {
        return Err(bad(CMD, "the merge left no boundary: the parcels entirely cancel each other out"));
    }

    // Chain the surviving edges into one closed loop: each next edge starts where the last one
    // ended, exactly the boundary-tracing step `cadcraft-terrain`'s triangulation also uses.
    let (first_a, first_b) = remaining.remove(0);
    let mut chain = vec![first_a, first_b];
    while let Some(end) = chain.last().copied() {
        if end.near(first_a, EDGE_TOL) && chain.len() > 2 {
            break;
        }
        let Some(i) = remaining.iter().position(|(a, _)| a.near(end, EDGE_TOL)) else {
            return Err(bad(CMD, "could not trace a single merged boundary: the parcels may not form one simple shape"));
        };
        let (_, b) = remaining.remove(i);
        chain.push(b);
    }
    chain.pop(); // closing vertex duplicates the first

    let area = cadcraft_geom::shoelace(&chain).abs();
    for h in &hs {
        s.doc_mut()?.remove_entity(*h);
    }
    let kind = EntityKind::LwPolyline(cadcraft_doc::LwPolyline {
        vertices: chain.into_iter().map(cadcraft_geom::PolyVertex::new).collect(),
        closed: true,
        const_width: 0.0,
        elevation: 0.0,
        plinegen: false,
    });
    let handle = s.add_entity(kind)?;
    s.doc_mut()?.modify_entity(handle, |e| e.common.layer = layer).map_err(|e| bad(CMD, e.to_string()))?;

    Ok(json!({ "handle": handle.hex(), "area": area }))
}

/// Escapes a field for a `;`-delimited CSV cell, matching `topo.listing_points`'s convention.
fn csv_field(s: &str) -> String {
    if s.contains(';') || s.contains('"') || s.contains('\n') || s.contains('\r') { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() }
}

fn run_parcel_state(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "topo.parcel_state";
    let layer = str_param(p, "layer").map(str::to_ascii_lowercase);

    let st = s.state()?;
    let Some(store) = st.doc.space(&st.space) else {
        return Ok(json!({ "parcels": [] }));
    };

    let mut parcels = Vec::new();
    let mut n = 0usize;
    for e in store.iter() {
        if layer.as_ref().is_some_and(|l| e.common.layer.to_ascii_lowercase() != *l) {
            continue;
        }
        if let Some(area) = parcel_area(e) {
            n += 1;
            parcels.push(json!({ "reference": format!("P{n}"), "handle": e.handle.hex(), "layer": e.common.layer, "area": area }));
        }
    }

    let mut out = json!({ "parcels": parcels });
    if let Some(path) = str_param(p, "path") {
        let mut csv = String::from("Reference;Handle;Layer;Area\n");
        for row in out["parcels"].as_array().into_iter().flatten() {
            csv.push_str(&format!(
                "{};{};{};{}\n",
                csv_field(row["reference"].as_str().unwrap_or("")),
                csv_field(row["handle"].as_str().unwrap_or("")),
                csv_field(row["layer"].as_str().unwrap_or("")),
                row["area"].as_f64().unwrap_or(0.0),
            ));
        }
        std::fs::write(path, &csv).map_err(|e| bad(CMD, format!("{path}: {e}")))?;
        if let Some(o) = out.as_object_mut() {
            o.insert("path".into(), Value::String(path.to_string()));
        }
    }
    Ok(out)
}

fn run_inspect_edigeo(_s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "topo.inspect_edigeo";
    let path = str_param(p, "path").ok_or_else(|| bad(CMD, "`path` is required"))?;
    let bytes = std::fs::read(path).map_err(|e| bad(CMD, format!("{path}: {e}")))?;
    let summary = cadcraft_topo_io::edigeo::inspect(&bytes).map_err(|e| bad(CMD, format!("{path}: {e}")))?;

    let field_tags: Vec<Value> = summary
        .field_tags
        .iter()
        .map(|(tag, stats)| json!({ "tag": tag, "recordCount": stats.record_count, "subfieldCountsSeen": stats.subfield_counts_seen }))
        .collect();
    let sample: Vec<Value> = summary
        .sample
        .iter()
        .map(|r| {
            let fields: Vec<Value> = r.fields.iter().map(|f| json!({ "tag": f.tag, "subfields": f.subfields })).collect();
            json!({ "leaderIdentifier": r.leader_identifier.to_string(), "fields": fields })
        })
        .collect();

    Ok(json!({
        "totalRecords": summary.total_records,
        "hasDdr": summary.has_ddr,
        "fieldTags": field_tags,
        "sample": sample,
    }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::Session;

    fn session_with_doc() -> Session {
        let mut s = Session::new();
        s.execute("new", &json!({})).expect("new drawing should succeed");
        s
    }

    fn square(s: &mut Session, x0: f64, y0: f64, x1: f64, y1: f64) -> String {
        let out = s
            .execute("line", &json!({ "points": [[x0, y0], [x1, y0], [x1, y1], [x0, y1]], "closed": true }))
            .expect("drawing the square should succeed");
        // 4 separate LINE segments aren't a closed polyline for `region`/`parcel_area`, so JOIN
        // them into one, matching how a real parcel boundary would be drawn.
        let handles = out["handles"].as_array().expect("handles array").clone();
        let joined = s.execute("join", &json!({ "handles": handles })).expect("joining into a polyline should succeed");
        joined["handle"].as_str().expect("join should return the joined polyline's handle").to_string()
    }

    #[test]
    fn merge_parcels_combines_two_adjacent_squares() {
        let mut s = session_with_doc();
        let a = square(&mut s, 0.0, 0.0, 10.0, 10.0);
        let b = square(&mut s, 10.0, 0.0, 20.0, 10.0);

        let out = s.execute("topo.merge_parcels", &json!({ "handles": [a, b] })).expect("merge should succeed");
        assert!((out["area"].as_f64().unwrap_or(0.0) - 200.0).abs() < 1e-6, "{out}");
    }

    #[test]
    fn merge_parcels_rejects_a_single_handle() {
        let mut s = session_with_doc();
        let a = square(&mut s, 0.0, 0.0, 10.0, 10.0);
        let out = s.execute("topo.merge_parcels", &json!({ "handles": [a] }));
        assert!(out.is_err());
    }

    #[test]
    fn parcel_state_lists_closed_polylines_with_areas() {
        let mut s = session_with_doc();
        square(&mut s, 0.0, 0.0, 10.0, 10.0);
        square(&mut s, 100.0, 100.0, 105.0, 105.0);
        // An open polyline (not closed) must not be counted as a parcel.
        s.execute("line", &json!({ "points": [[0.0, 0.0], [1.0, 0.0]] })).expect("should succeed");

        let out = s.execute("topo.parcel_state", &json!({})).expect("should succeed");
        let parcels = out["parcels"].as_array().expect("parcels array");
        assert_eq!(parcels.len(), 2);
        let areas: Vec<f64> = parcels.iter().map(|p| p["area"].as_f64().unwrap_or(0.0)).collect();
        assert!(areas.contains(&100.0) || areas.iter().any(|a| (a - 100.0).abs() < 1e-6));
        assert!(areas.iter().any(|a| (a - 25.0).abs() < 1e-6));
    }

    #[test]
    fn parcel_state_exports_a_csv_when_path_is_given() {
        let mut s = session_with_doc();
        square(&mut s, 0.0, 0.0, 10.0, 10.0);
        let path = std::env::temp_dir().join(format!("cadcraft-parcel-state-test-{}.csv", std::process::id()));
        let out = s.execute("topo.parcel_state", &json!({ "path": path.to_string_lossy() }));
        let content = std::fs::read_to_string(&path);
        let _ = std::fs::remove_file(&path);
        out.expect("should succeed");
        let content = content.expect("the CSV should have been written");
        assert!(content.starts_with("Reference;Handle;Layer;Area\n"));
        assert!(content.contains(";100\n") || content.contains(";100;"));
    }

    #[test]
    fn inspect_edigeo_reports_field_tags_and_a_sample() {
        use cadcraft_topo_io::iso8211::test_support::build_record;

        let mut bytes = build_record(b'L', &[("0000", &["DDR"])]);
        bytes.extend(build_record(b'D', &[("GEO", &["100.0", "200.0"]), ("LAB", &["P1"])]));

        let mut s = session_with_doc();
        let path = std::env::temp_dir().join(format!("cadcraft-edigeo-inspect-test-{}.bin", std::process::id()));
        std::fs::write(&path, &bytes).expect("writing the synthetic EDIGEO file should succeed");
        let out = s.execute("topo.inspect_edigeo", &json!({ "path": path.to_string_lossy() }));
        let _ = std::fs::remove_file(&path);
        let out = out.expect("should succeed");

        assert_eq!(out["totalRecords"].as_u64(), Some(2));
        assert_eq!(out["hasDdr"].as_bool(), Some(true));
        let tags = out["fieldTags"].as_array().expect("fieldTags array");
        assert!(tags.iter().any(|t| t["tag"] == "GEO" && t["recordCount"] == 1));
        let sample = out["sample"].as_array().expect("sample array");
        assert_eq!(sample.len(), 2);
    }

    #[test]
    fn inspect_edigeo_reports_a_missing_file_as_an_error() {
        let mut s = session_with_doc();
        let out = s.execute("topo.inspect_edigeo", &json!({ "path": "/nonexistent/path/does-not-exist.edi" }));
        assert!(out.is_err());
    }
}
