//! "Cotations et étiquettes" (the dynamic-label half, not a true AutoCAD DIMENSION): a TEXT
//! label whose content is computed from a source POINT entity's elevation, tagged with its own
//! xdata so it can later be re-evaluated if the point moves.
//!
//! **Scope, stated plainly**: CADCraft has no FIELD/expression-evaluation engine yet, so this is
//! not continuously live like AutoCAD's associative dimensions - `topo.label_elevation` creates
//! the label once from the point's current elevation, and `topo.refresh_labels` re-reads every
//! such label afterwards and updates its text to match. That refresh-on-demand link is real and
//! tested; silently pretending it updates on every point move would not be.

use cadcraft_doc::{EntityKind, Handle, RawTag};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

/// The xdata "application" name marking a label this command created and can refresh.
const APPID: &str = "CADCRAFT_TOPO_LABEL";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("topo.label_elevation", "Label: Point Elevation", run_label_elevation)
            .menu(&["Topo 2D", "Labels", "Elevation"])
            .params("{point: handle, at?: [x,y,z], prefix?: string, decimals?: integer, textHeight?: number}"),
        CommandSpec::new("topo.refresh_labels", "Refresh Elevation Labels", run_refresh_labels).menu(&["Topo 2D", "Labels", "Refresh"]).params("{}"),
    ]
}

fn handle_param(cmd: &str, p: &Value, key: &str) -> Result<Handle> {
    let raw = str_param(p, key).ok_or_else(|| bad(cmd, format!("`{key}` (an object handle) is required")))?;
    Handle::parse_hex(raw).ok_or_else(|| bad(cmd, format!("`{key}`: \"{raw}\" is not a valid object handle")))
}

fn point_elevation(s: &Session, cmd: &str, h: Handle) -> Result<f64> {
    let e = s.doc()?.entity(h).ok_or_else(|| bad(cmd, "no such object"))?;
    match &e.kind {
        EntityKind::Point(pt) => Ok(pt.p.z),
        _ => Err(bad(cmd, "`point` must be the handle of a POINT entity")),
    }
}

fn label_text(prefix: &str, decimals: usize, z: f64) -> String {
    format!("{prefix}{z:.decimals$}")
}

fn run_label_elevation(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "topo.label_elevation";
    let point = handle_param(CMD, p, "point")?;
    let z = point_elevation(s, CMD, point)?;
    let prefix = str_param(p, "prefix").unwrap_or("Z=").to_string();
    let decimals = p.get("decimals").and_then(Value::as_u64).unwrap_or(2).min(10) as usize;
    let text_height = f64_or(p, "textHeight", s.doc()?.header.f64("TEXTSIZE", 0.2));

    let at = match super::topo::vec3_req(CMD, p, "at") {
        Ok(v) => v,
        Err(_) if p.get("at").is_none() => {
            let e = s.doc()?.entity(point).ok_or_else(|| bad(CMD, "no such object"))?;
            let EntityKind::Point(pt) = &e.kind else { return Err(bad(CMD, "`point` must be the handle of a POINT entity")) };
            cadcraft_geom::Vec3::new(pt.p.x + 0.4, pt.p.y, pt.p.z)
        }
        Err(e) => return Err(e),
    };

    let value = label_text(&prefix, decimals, z);
    let out = s.execute("text", &json!({ "at": [at.x, at.y, at.z], "text": value, "height": text_height }))?;
    let handle =
        out.get("handle").and_then(Value::as_str).and_then(Handle::parse_hex).ok_or_else(|| bad(CMD, "text insertion did not return a handle"))?;

    s.doc_mut()?
        .modify_entity(handle, |e| {
            e.common.xdata.push(RawTag { code: 1001, value: APPID.into() });
            e.common.xdata.push(RawTag { code: 1000, value: point.hex() });
            e.common.xdata.push(RawTag { code: 1000, value: format!("prefix:{prefix}") });
            e.common.xdata.push(RawTag { code: 1070, value: decimals.to_string() });
        })
        .map_err(|e| bad(CMD, e.to_string()))?;

    Ok(json!({ "handle": handle.hex(), "text": value }))
}

/// Parses one label's link back out of its xdata (the shape `run_label_elevation` wrote above):
/// `(source point handle, prefix, decimals)`, or `None` if it isn't one of ours or is malformed.
fn parse_link(xdata: &[RawTag]) -> Option<(Handle, String, usize)> {
    let start = xdata.iter().position(|t| t.code == 1001 && t.value == APPID)?;
    let rest = xdata.get(start + 1..)?;
    let point = rest.first().filter(|t| t.code == 1000).and_then(|t| Handle::parse_hex(&t.value))?;
    let prefix = rest.get(1).filter(|t| t.code == 1000).and_then(|t| t.value.strip_prefix("prefix:")).unwrap_or("").to_string();
    let decimals = rest.get(2).filter(|t| t.code == 1070).and_then(|t| t.value.parse().ok()).unwrap_or(2);
    Some((point, prefix, decimals))
}

fn run_refresh_labels(s: &mut Session, _p: &Value) -> Result<Value> {
    const CMD: &str = "topo.refresh_labels";
    let st = s.state()?;
    let Some(store) = st.doc.space(&st.space) else {
        return Ok(json!({ "updated": 0, "dangling": 0 }));
    };

    let mut jobs: Vec<(Handle, Handle, String, usize)> = Vec::new();
    for e in store.iter() {
        if matches!(e.kind, EntityKind::Text(_))
            && let Some((point, prefix, decimals)) = parse_link(&e.common.xdata)
        {
            jobs.push((e.handle, point, prefix, decimals));
        }
    }

    let mut updated = 0usize;
    let mut dangling = 0usize;
    for (label, point, prefix, decimals) in jobs {
        match point_elevation(s, CMD, point) {
            Ok(z) => {
                let value = label_text(&prefix, decimals, z);
                let ok = s
                    .doc_mut()?
                    .modify_entity(label, |e| {
                        if let EntityKind::Text(t) = &mut e.kind {
                            t.value = value;
                        }
                    })
                    .is_ok();
                if ok {
                    updated += 1;
                } else {
                    dangling += 1;
                }
            }
            Err(_) => dangling += 1,
        }
    }
    Ok(json!({ "updated": updated, "dangling": dangling }))
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
    fn label_elevation_creates_a_text_with_the_formatted_altitude() {
        let mut s = session_with_doc();
        let pt = s
            .execute("topo.point_at", &json!({ "from": [10.0, 20.0, 3.456], "gisement": 0.0, "distance": 0.0, "label": false }))
            .expect("should succeed");
        let point_handle = pt["point"]["handle"].as_str().expect("point handle").to_string();

        let out = s.execute("topo.label_elevation", &json!({ "point": point_handle })).expect("should succeed");
        assert_eq!(out["text"].as_str(), Some("Z=3.46"));
    }

    #[test]
    fn label_elevation_rejects_a_non_point_handle() {
        let mut s = session_with_doc();
        let line = s.execute("line", &json!({ "points": [[0.0, 0.0], [1.0, 1.0]] })).expect("should succeed");
        let h = line["handles"][0].as_str().expect("line handle").to_string();
        let out = s.execute("topo.label_elevation", &json!({ "point": h }));
        assert!(out.is_err());
    }

    #[test]
    fn refresh_labels_picks_up_a_point_that_moved() {
        let mut s = session_with_doc();
        let pt = s
            .execute("topo.point_at", &json!({ "from": [0.0, 0.0, 10.0], "gisement": 0.0, "distance": 0.0, "label": false }))
            .expect("should succeed");
        let point_handle = pt["point"]["handle"].as_str().expect("point handle").to_string();
        let point_h = cadcraft_doc::Handle::parse_hex(&point_handle).expect("valid handle");

        s.execute("topo.label_elevation", &json!({ "point": point_handle })).expect("should succeed");

        // Move the point directly (simulating an edit elsewhere in the drawing).
        s.doc_mut()
            .expect("doc")
            .modify_entity(point_h, |e| {
                if let cadcraft_doc::EntityKind::Point(p) = &mut e.kind {
                    p.p.z = 99.0;
                }
            })
            .expect("moving the point should succeed");

        let out = s.execute("topo.refresh_labels", &json!({})).expect("should succeed");
        assert_eq!(out["updated"].as_u64(), Some(1));
        assert_eq!(out["dangling"].as_u64(), Some(0));
    }

    #[test]
    fn refresh_labels_reports_a_deleted_source_point_as_dangling() {
        let mut s = session_with_doc();
        let pt = s
            .execute("topo.point_at", &json!({ "from": [0.0, 0.0, 10.0], "gisement": 0.0, "distance": 0.0, "label": false }))
            .expect("should succeed");
        let point_handle = pt["point"]["handle"].as_str().expect("point handle").to_string();
        let point_h = cadcraft_doc::Handle::parse_hex(&point_handle).expect("valid handle");
        s.execute("topo.label_elevation", &json!({ "point": point_handle })).expect("should succeed");

        s.doc_mut().expect("doc").remove_entity(point_h);

        let out = s.execute("topo.refresh_labels", &json!({})).expect("should succeed");
        assert_eq!(out["updated"].as_u64(), Some(0));
        assert_eq!(out["dangling"].as_u64(), Some(1));
    }
}
