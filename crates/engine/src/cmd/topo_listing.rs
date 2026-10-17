//! Listings: tabular export of the drawing's topographic points - a first slice of the spec's
//! "Listings" bullet ("tableau de points, carnet de coordonnées, métré"), reusing the same
//! entity inspection already exposed to agents via `entities`/`drawing.inspect`.
//!
//! **Scope of this increment, stated plainly**: only POINT entities, only CSV, and only the
//! coordinates and layer CADCraft already has on each point (no code/matricule column - that
//! lives on a point's separate TEXT label, if any, and correlating the two is a follow-up
//! increment). A general "export any entity type as a listing" tool and an XLSX writer are
//! later increments too, not pretended-complete here.

use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("topo.listing_points", "Listing: Points (CSV)", run_listing_points)
            .menu(&["Topo", "Listings", "Points (CSV)"])
            .params("{path: string, layer?: string}")
            .noundo(),
    ]
}

/// Escapes a field for a `;`-delimited CSV cell: wraps in double quotes (doubling any inner
/// quote) when it contains the delimiter, a quote or a newline, leaving everything else as-is.
fn csv_field(s: &str) -> String {
    if s.contains(';') || s.contains('"') || s.contains('\n') || s.contains('\r') { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() }
}

fn run_listing_points(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "topo.listing_points";
    let path = str_param(p, "path").ok_or_else(|| bad(CMD, "`path` is required"))?;
    let layer = str_param(p, "layer").map(str::to_ascii_lowercase);

    let st = s.state()?;
    let Some(store) = st.doc.space(&st.space) else {
        return Err(bad(CMD, "no active drawing space"));
    };

    let mut rows: Vec<(String, String, f64, f64, f64)> = Vec::new();
    for e in store.iter() {
        if layer.as_ref().is_some_and(|l| e.common.layer.to_ascii_lowercase() != *l) {
            continue;
        }
        if let cadcraft_doc::EntityKind::Point(pt) = &e.kind {
            rows.push((e.handle.hex(), e.common.layer.clone(), pt.p.x, pt.p.y, pt.p.z));
        }
    }

    let mut csv = String::from("ID;X;Y;Z;Layer\n");
    for (id, layer, x, y, z) in &rows {
        csv.push_str(&format!("{};{x};{y};{z};{}\n", csv_field(id), csv_field(layer)));
    }

    std::fs::write(path, &csv).map_err(|e| bad(CMD, format!("{path}: {e}")))?;
    Ok(json!({ "path": path, "count": rows.len() }))
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

    #[test]
    fn exports_points_to_a_csv_file() {
        let mut s = session_with_doc();
        // `point`'s `at` is 2D only (CADCraft's generic draw commands don't carry a Z), so these
        // use `topo.point_at` with a zero distance to insert points at an exact 3D coordinate.
        s.execute("topo.point_at", &json!({ "from": [10.0, 20.0, 1.5], "gisement": 0.0, "distance": 0.0, "label": false }))
            .expect("inserting a point should succeed");
        s.execute("topo.point_at", &json!({ "from": [30.0, 40.0, 2.5], "gisement": 0.0, "distance": 0.0, "label": false }))
            .expect("inserting a point should succeed");
        // A non-point entity must not show up in the listing.
        s.execute("line", &json!({ "points": [[0.0, 0.0], [1.0, 1.0]] })).expect("inserting a line should succeed");

        let path = std::env::temp_dir().join(format!("cadcraft-listing-test-{}.csv", std::process::id()));
        let out = s.execute("topo.listing_points", &json!({ "path": path.to_string_lossy() }));
        let content = std::fs::read_to_string(&path);
        let _ = std::fs::remove_file(&path);
        let out = out.expect("listing export should succeed");
        let content = content.expect("the CSV file should have been written");

        assert_eq!(out["count"].as_u64(), Some(2));
        assert!(content.starts_with("ID;X;Y;Z;Layer\n"));
        assert!(content.contains(";10;20;1.5;"));
        assert!(content.contains(";30;40;2.5;"));
    }

    #[test]
    fn filters_by_layer() {
        let mut s = session_with_doc();
        s.execute("layer.new", &json!({ "name": "TOPO", "current": true })).expect("creating the layer should succeed");
        s.execute("point", &json!({ "at": [1.0, 1.0, 0.0] })).expect("inserting a point should succeed");
        s.execute("layer.current", &json!({ "name": "0" })).expect("switching back to layer 0 should succeed");
        s.execute("point", &json!({ "at": [2.0, 2.0, 0.0] })).expect("inserting a point should succeed");

        let path = std::env::temp_dir().join(format!("cadcraft-listing-filter-test-{}.csv", std::process::id()));
        let out = s.execute("topo.listing_points", &json!({ "path": path.to_string_lossy(), "layer": "TOPO" }));
        let _ = std::fs::remove_file(&path);
        let out = out.expect("listing export should succeed");
        assert_eq!(out["count"].as_u64(), Some(1));
    }

    #[test]
    fn reports_an_unwritable_path_as_an_error_not_a_panic() {
        let mut s = session_with_doc();
        let out = s.execute("topo.listing_points", &json!({ "path": "/nonexistent-dir/cannot-write.csv" }));
        assert!(out.is_err());
    }
}
