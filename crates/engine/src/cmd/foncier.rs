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
//! **Référentiel fonds (EDIGEO)**: `topo.inspect_edigeo` reports a file's raw field/subfield
//! shape (useful for either EDIGEO profile - see `cadcraft_topo_io::edigeo`'s doc comment for
//! why that was built first rather than guessed at). `topo.import_edigeo` builds on a real
//! Calvados PCI-Vecteur export checked while writing it: that export turned out to use EDIGEO's
//! plain-text ("ASCII profile") serialization, not binary ISO 8211 - see
//! `cadcraft_topo_io::edigeo_text`/`edigeo_model` for the reverse-engineered line shape and the
//! arc/face planar topology (each arc already carries its own full vertex list and knows which
//! face borders it on the left and right; a face's ring is the chain of its bordering arcs).
//!
//! **Scope, stated plainly**: curved parcel edges (a bulged polyline vertex, or a curved EDIGEO
//! arc) are refused/skipped rather than silently flattened. On the real export used to build
//! this, 596 of 713 faces (84%) chained into a closed ring; the rest are each bordered by only
//! a single, non-closing arc in this file's own relations - most plausibly a cadastral
//! subdivision (`SUBDSECT`) that shares most of its boundary with its parent `SECTION`'s own
//! arcs rather than re-declaring them, which this increment does not reconstruct. Those are
//! reported as skipped with a reason, not silently dropped or guessed at. Attribute class codes
//! (`IDU_id`, `SUPF_id`, ...) are carried through verbatim rather than resolved to their French
//! labels via the export's own `.DIC`/`.SCD` dictionary, a further increment.

use cadcraft_doc::{EntityKind, Handle, RawTag};
use cadcraft_geom::{Polyline, Vec2};
use serde_json::{Value, json};

use super::*;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("topo.merge_parcels", "Merge Parcels", run_merge_parcels)
            .menu(&["Topo 2D", "Foncier", "Merge Parcels"])
            .params("{handles: [handle, ...]} (closed polylines or regions, at least 2) → {handle, area}")
            // Menu/toolbar (no parameters): select the parcels on screen (closed polylines),
            // Enter to run once at least 2 are picked - the usual select-objects-then-Enter
            // sequence, not a popup form, matching "programmatic calls never open dialogs".
            .interactive(|_| Ok(Box::new(MergeParcelsM::default()))),
        CommandSpec::new("topo.parcel_state", "Parcel State (listing)", run_parcel_state)
            .menu(&["Topo 2D", "Foncier", "Parcel State"])
            .params("{layer?: string, path?: string (also writes a CSV)} → {parcels: [{reference, handle, layer, area}], path?}")
            .noundo(),
        CommandSpec::new("topo.inspect_edigeo", "Inspect EDIGEO File", run_inspect_edigeo)
            .menu(&["Topo 2D", "Foncier", "Land Records (EDIGEO)", "Inspect"])
            .params(
                "{path: string} → {totalRecords, hasDdr, fieldTags: [{tag, recordCount, subfieldCountsSeen}], \
                 sample: [{leaderIdentifier, fields: [{tag, subfields}]}]} (read-only: reports the file's real \
                 field/subfield shape, does not import anything yet)",
            )
            .noundo(),
        CommandSpec::new("topo.import_edigeo", "Import EDIGEO (Land Records)", run_import_edigeo)
            .menu(&["Topo 2D", "Foncier", "Land Records (EDIGEO)", "Import"])
            .params(
                "{paths: [string] (every .VEC file of one lot), layerPrefix?: string} → \
                 {imported, skipped: [{feature, class, reason}], featuresLinkedToAFace, facesKnown}",
            ),
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

/// TOPO.MERGE_PARCELS at the command line: repeated "Select objects to merge" picks (closed
/// polylines/regions), Enter to run once at least 2 are selected.
#[derive(Default)]
struct MergeParcelsM {
    objs: Vec<Handle>,
}

impl Interactive for MergeParcelsM {
    fn name(&self) -> &'static str {
        "TOPO.MERGE_PARCELS"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        Prompt::new("Select objects to merge", Accept::SELECT)
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match i {
            Input::Pick(hs) => {
                for h in hs {
                    if !self.objs.contains(&h) {
                        self.objs.push(h);
                    }
                }
                Ok(Step::Continue)
            }
            Input::Enter => {
                if self.objs.len() < 2 {
                    s.echo("Select at least 2 parcels to merge.");
                    return Ok(Step::Continue);
                }
                let handles: Vec<Value> = self.objs.iter().map(|h| json!(h.hex())).collect();
                run_merge_parcels(s, &json!({ "handles": handles }))?;
                Ok(Step::Done)
            }
            Input::Cancel => Ok(Step::Cancel),
            _ => Ok(Step::Continue),
        }
    }
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

/// The xdata "application" name marking a polyline `topo.import_edigeo` created, carrying the
/// source feature's id and raw attribute codes/values.
const EDIGEO_APPID: &str = "CADCRAFT_TOPO_EDIGEO";

fn sanitize_layer_name(prefix: &str, class: &str) -> String {
    let cleaned: String = class.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect();
    format!("{prefix}{}", cleaned.trim_end_matches("_id").to_ascii_uppercase())
}

fn run_import_edigeo(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "topo.import_edigeo";
    let paths = p.get("paths").and_then(Value::as_array).ok_or_else(|| bad(CMD, "`paths` (array of file paths) is required"))?;
    if paths.is_empty() {
        return Err(bad(CMD, "`paths` must name at least one file"));
    }
    let prefix = str_param(p, "layerPrefix").unwrap_or("EDIGEO_").to_string();

    let mut files = Vec::with_capacity(paths.len());
    for v in paths {
        let path = v.as_str().ok_or_else(|| bad(CMD, "`paths`: every entry must be a string"))?;
        let bytes = std::fs::read(path).map_err(|e| bad(CMD, format!("{path}: {e}")))?;
        files.push(cadcraft_topo_io::parse_edigeo_text(&bytes).map_err(|e| bad(CMD, format!("{path}: {e}")))?);
    }
    let model = cadcraft_topo_io::build_edigeo_model(&files);

    let mut imported = 0usize;
    let mut skipped = Vec::new();
    for (feature_id, face_id) in &model.feature_face {
        let Some(feature) = model.features.get(feature_id) else { continue };
        match cadcraft_topo_io::edigeo_face_ring(&model, face_id, cadcraft_topo_io::edigeo_model::DEFAULT_TOLERANCE) {
            Ok(ring) if ring.len() >= 3 => {
                let kind = EntityKind::LwPolyline(cadcraft_doc::LwPolyline {
                    vertices: ring.into_iter().map(cadcraft_geom::PolyVertex::new).collect(),
                    closed: true,
                    const_width: 0.0,
                    elevation: 0.0,
                    plinegen: false,
                });
                let handle = s.add_entity(kind)?;
                let layer = sanitize_layer_name(&prefix, &feature.class);
                let mut xdata =
                    vec![RawTag { code: 1001, value: EDIGEO_APPID.to_string() }, RawTag { code: 1000, value: format!("id={feature_id}") }];
                for attr in &feature.attributes {
                    xdata.push(RawTag { code: 1000, value: format!("{}={}", attr.class, attr.value) });
                }
                s.doc_mut()?
                    .modify_entity(handle, |e| {
                        e.common.layer = layer;
                        e.common.xdata = xdata;
                    })
                    .map_err(|e| bad(CMD, e.to_string()))?;
                imported += 1;
            }
            Ok(_) => {
                skipped.push(json!({ "feature": feature_id, "class": feature.class, "reason": "the reconstructed ring has fewer than 3 vertices" }))
            }
            Err(e) => skipped.push(json!({ "feature": feature_id, "class": feature.class, "reason": e.to_string() })),
        }
    }

    Ok(json!({
        "imported": imported,
        "skipped": skipped,
        "featuresLinkedToAFace": model.feature_face.len(),
        "facesKnown": model.face_sides.len(),
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
    fn merge_parcels_runs_interactively_from_the_menu() {
        use cadcraft_doc::Handle;

        use crate::Input;

        let mut s = session_with_doc();
        let a = square(&mut s, 0.0, 0.0, 10.0, 10.0);
        let b = square(&mut s, 10.0, 0.0, 20.0, 10.0);
        let ha = Handle::parse_hex(&a).expect("valid handle");
        let hb = Handle::parse_hex(&b).expect("valid handle");

        // The menu/toolbar/command-line route used to fail with "`handles` is required".
        s.start("topo.merge_parcels").expect("starting interactively should succeed");
        assert!(s.prompt_text().contains("Select objects to merge"), "{}", s.prompt_text());
        s.input(Input::Enter).expect("Enter with nothing picked yet should be tolerated");
        assert!(s.running.is_some(), "must not finish with fewer than 2 parcels picked");
        s.input(Input::Pick(vec![ha])).expect("picking the first parcel should succeed");
        assert!(s.running.is_some());
        s.input(Input::Pick(vec![hb])).expect("picking the second parcel should succeed");
        s.input(Input::Enter).expect("Enter with 2 parcels picked should finish the command");
        assert!(s.running.is_none());

        // The two original squares are gone, replaced by one merged parcel.
        let merged: Vec<_> = s.doc().expect("doc").model.iter().filter(|e| matches!(e.kind, cadcraft_doc::EntityKind::LwPolyline(_))).collect();
        assert_eq!(merged.len(), 1, "expected exactly one merged parcel left");
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

    /// A minimal lot in EDIGEO's real ASCII profile: one square face bounded by 4 straight arcs,
    /// one `FEA` of class `PARCELLE_id` linked to it - the same shapes a real PCI-Vecteur export
    /// uses, confirmed against one while building `topo.import_edigeo`.
    const SQUARE_LOT_TEXT: &str = "\
RTYSA03:PNO\r\nRIDSA02:N1\r\n\r\nCORCC14:+0.00;+0.00;\r\n\r\n\
RTYSA03:PNO\r\nRIDSA02:N2\r\n\r\nCORCC14:+10.0;+0.00;\r\n\r\n\
RTYSA03:PFE\r\nRIDSA06:Face_0\r\n\r\n\r\n\
RTYSA03:PAR\r\nRIDSA02:A1\r\n\r\nCORCC14:+0.00;+0.00;\r\nCORCC14:+10.0;+0.00;\r\n\r\n\
RTYSA03:PAR\r\nRIDSA02:A2\r\n\r\nCORCC14:+10.0;+0.00;\r\nCORCC16:+10.0;+10.0;\r\n\r\n\
RTYSA03:PAR\r\nRIDSA02:A3\r\n\r\nCORCC16:+10.0;+10.0;\r\nCORCC14:+0.00;+10.0;\r\n\r\n\
RTYSA03:PAR\r\nRIDSA02:A4\r\n\r\nCORCC14:+0.00;+10.0;\r\nCORCC14:+0.00;+0.00;\r\n\r\n\
RTYSA03:LNK\r\nRIDSA02:L1\r\n\r\nSCPCP18:LOT;SCP;REL;ID_S_RCO_FAC_GCHE\r\nFTCSN01:2\r\nFTPCP08:LOT;L;PAR;A1\r\nFTPCP08:LOT;L;PFE;Face_0\r\n\r\n\
RTYSA03:LNK\r\nRIDSA02:L2\r\n\r\nSCPCP18:LOT;SCP;REL;ID_S_RCO_FAC_GCHE\r\nFTCSN01:2\r\nFTPCP08:LOT;L;PAR;A2\r\nFTPCP08:LOT;L;PFE;Face_0\r\n\r\n\
RTYSA03:LNK\r\nRIDSA02:L3\r\n\r\nSCPCP18:LOT;SCP;REL;ID_S_RCO_FAC_GCHE\r\nFTCSN01:2\r\nFTPCP08:LOT;L;PAR;A3\r\nFTPCP08:LOT;L;PFE;Face_0\r\n\r\n\
RTYSA03:LNK\r\nRIDSA02:L4\r\n\r\nSCPCP18:LOT;SCP;REL;ID_S_RCO_FAC_GCHE\r\nFTCSN01:2\r\nFTPCP08:LOT;L;PAR;A4\r\nFTPCP08:LOT;L;PFE;Face_0\r\n\r\n\
RTYSA03:FEA\r\nRIDSA02:F1\r\n\r\nSCPCP21:LOT;SCP;OBJ;PARCELLE_id\r\nATPCP18:LOT;SCP;ATT;IDU_id\r\nATVSA04:0001\r\n\r\n\
RTYSA03:LNK\r\nRIDSA02:L5\r\n\r\nSCPCP21:LOT;SCP;REL;ID_S_RCO_PARCELLE_id\r\nFTCSN01:2\r\nFTPCP08:LOT;L;FEA;F1\r\nFTPCP08:LOT;L;PFE;Face_0\r\n\r\n\
";

    #[test]
    fn import_edigeo_reconstructs_a_parcel_polyline_with_its_attributes() {
        let mut s = session_with_doc();
        let path = std::env::temp_dir().join(format!("cadcraft-edigeo-import-test-{}.vec", std::process::id()));
        std::fs::write(&path, SQUARE_LOT_TEXT).expect("writing the synthetic lot should succeed");
        let out = s.execute("topo.import_edigeo", &json!({ "paths": [path.to_string_lossy()] }));
        let _ = std::fs::remove_file(&path);
        let out = out.expect("should succeed");

        assert_eq!(out["imported"].as_u64(), Some(1), "{out}");
        assert_eq!(out["skipped"].as_array().map(|a| a.len()), Some(0));

        let parcels = s.execute("topo.parcel_state", &json!({})).expect("should succeed");
        let list = parcels["parcels"].as_array().expect("parcels array");
        assert_eq!(list.len(), 1);
        assert!((list[0]["area"].as_f64().unwrap_or(0.0) - 100.0).abs() < 1e-6, "{list:?}");
        assert_eq!(list[0]["layer"].as_str(), Some("EDIGEO_PARCELLE"));
    }

    #[test]
    fn import_edigeo_reports_a_missing_file_as_an_error() {
        let mut s = session_with_doc();
        let out = s.execute("topo.import_edigeo", &json!({ "paths": ["/nonexistent/path/does-not-exist.vec"] }));
        assert!(out.is_err());
    }
}
