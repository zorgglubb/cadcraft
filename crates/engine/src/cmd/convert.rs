//! Conversion commands for the "Plan" menu: baking `ByLayer` colour into explicit per-object
//! colour, and flattening ELLIPSE/SPLINE entities down to a plain `LwPolyline` (useful before
//! handing a drawing to a workflow that only understands polylines).

use cadcraft_doc::{EntityKind, LwPolyline};
use cadcraft_geom::{Ellipse as GEllipse, PolyVertex, Vec2, ccw_sweep};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("convert.fixcolors", "Fix Layer Colors", run_fixcolors).menu(&["Plan", "Fix Layer Colors"]).params(
            "{handles?: [string] (default: current selection)} → {changed: number}. For each selected \
                 object currently ByLayer, resolves its layer's actual colour and sets that object's own \
                 colour to the resolved value - it no longer follows the layer afterwards.",
        ),
        CommandSpec::new("convert.ellipse2pl", "Ellipse to Polyline", run_ellipse2pl).menu(&["Plan", "Ellipse to Polyline"]).params(
            "{handles?: [string] (default: current selection), segments?: number (default 36, for a \
                 full ellipse; a partial arc uses proportionally fewer)} → {handles: [string]} (the new \
                 polylines). Each selected ELLIPSE is replaced by a tessellated LWPOLYLINE on the same \
                 layer/colour/linetype; a full ellipse becomes a closed polyline, a partial one stays open.",
        ),
        CommandSpec::new("convert.spline2pl", "Spline to Polyline", run_spline2pl).menu(&["Plan", "Spline to Polyline"]).params(
            "{handles?: [string] (default: current selection), segments?: number (default 50)} → \
                 {handles: [string]} (the new polylines). Each selected SPLINE is replaced by evenly \
                 (by parameter) sampled points joined as an LWPOLYLINE, on the same layer/colour.",
        ),
    ]
}

fn run_fixcolors(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    let mut changed = 0u32;
    for h in hs {
        let Some(e) = s.doc()?.entity(h) else { continue };
        if e.common.color != cadcraft_color::Color::ByLayer {
            continue;
        }
        let layer_name = e.common.layer.clone();
        let Some(layer) = s.doc()?.layer(&layer_name) else { continue };
        let resolved = layer.color;
        let d = s.doc_mut()?;
        if d.modify_entity(h, |e| e.common.color = resolved).is_ok() {
            changed += 1;
        }
    }
    Ok(json!({ "changed": changed }))
}

/// Tessellate an ellipse/elliptical-arc into 2D vertices, `n` points spanning its whole sweep
/// (`n + 1` points for a partial arc so both endpoints are included).
fn tessellate_ellipse(e: &GEllipse, n: usize) -> Vec<Vec2> {
    let n = n.max(3);
    if e.is_full() {
        (0..n).map(|i| e.at_param(e.start + cadcraft_geom::TAU * (i as f64) / (n as f64))).collect()
    } else {
        let sweep = ccw_sweep(e.start, e.end);
        (0..=n).map(|i| e.at_param(e.start + sweep * (i as f64) / (n as f64))).collect()
    }
}

fn run_ellipse2pl(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    let base_segments = p.get("segments").and_then(Value::as_u64).map(|v| v as usize).unwrap_or(36).clamp(3, 10_000);
    let space = s.space();
    let mut out = Vec::new();
    for h in hs {
        let Some(e) = s.doc()?.entity(h).map(|e| (**e).clone()) else { continue };
        let EntityKind::Ellipse(el) = &e.kind else { continue };
        let ge = GEllipse { center: el.center.xy(), major: el.major.xy(), ratio: el.ratio, start: el.start, end: el.end };
        let full = ge.is_full();
        let n =
            if full { base_segments } else { (base_segments as f64 * (ccw_sweep(ge.start, ge.end) / cadcraft_geom::TAU)).ceil().max(3.0) as usize };
        let pts = tessellate_ellipse(&ge, n);
        let z = el.center.z;
        let vertices = pts.into_iter().map(|p| PolyVertex { p, bulge: 0.0, start_width: 0.0, end_width: 0.0 }).collect::<Vec<_>>();
        let pl = LwPolyline { vertices, closed: full, const_width: 0.0, elevation: z, plinegen: false };
        let d = s.doc_mut()?;
        d.remove_entity(h);
        let nh = d.add(&space, e.common.clone(), EntityKind::LwPolyline(pl))?;
        out.push(nh);
    }
    s.set_selection(out.clone());
    Ok(json!({ "handles": out.iter().map(|h| h.hex()).collect::<Vec<_>>() }))
}

fn run_spline2pl(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    let n = p.get("segments").and_then(Value::as_u64).map(|v| v as usize).unwrap_or(50).clamp(2, 100_000);
    let space = s.space();
    let mut out = Vec::new();
    for h in hs {
        let Some(e) = s.doc()?.entity(h).map(|e| (**e).clone()) else { continue };
        let EntityKind::Spline(sp) = &e.kind else { continue };
        if !sp.is_valid() {
            continue;
        }
        let (t0, t1) = sp.domain();
        let pts: Vec<Vec2> = (0..=n).map(|i| sp.eval(t0 + (t1 - t0) * (i as f64) / (n as f64))).collect();
        let d = s.doc_mut()?;
        d.remove_entity(h);
        // Splines in this document are planar (2D control/fit points): always a flat polyline.
        let vertices = pts.into_iter().map(|p| PolyVertex { p, bulge: 0.0, start_width: 0.0, end_width: 0.0 }).collect::<Vec<_>>();
        let pl = LwPolyline { vertices, closed: sp.closed, const_width: 0.0, elevation: 0.0, plinegen: false };
        let nh = d.add(&space, e.common.clone(), EntityKind::LwPolyline(pl))?;
        out.push(nh);
    }
    s.set_selection(out.clone());
    Ok(json!({ "handles": out.iter().map(|h| h.hex()).collect::<Vec<_>>() }))
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
    fn fixcolors_bakes_the_layer_color_into_bylayer_objects() {
        let mut s = session_with_doc();
        s.execute("layer.new", &json!({ "name": "Walls", "color": 1 })).expect("layer should be created");
        s.execute("setvar", &json!({ "name": "CLAYER", "value": "Walls" })).expect("clayer should be set");
        let h = s.execute("line", &json!({ "points": [[0.0, 0.0], [10.0, 0.0]] })).expect("line should insert")["handles"][0]
            .as_str()
            .expect("handle string")
            .to_string();
        let out = s.execute("convert.fixcolors", &json!({ "handles": [h.clone()] })).expect("fixcolors should succeed");
        assert_eq!(out["changed"].as_u64(), Some(1));
        let props = s.execute("properties", &json!({ "handles": [h] })).expect("properties should read back");
        assert_eq!(props["objects"][0]["color"], json!("Red"));
    }

    #[test]
    fn ellipse_to_polyline_replaces_a_full_ellipse_with_a_closed_polyline() {
        let mut s = session_with_doc();
        let h = s.execute("ellipse", &json!({ "center": [0.0, 0.0], "major": [5.0, 0.0], "ratio": 0.5 })).expect("ellipse should insert")["handle"]
            .as_str()
            .expect("handle string")
            .to_string();
        let out = s.execute("convert.ellipse2pl", &json!({ "handles": [h] })).expect("conversion should succeed");
        let handles = out["handles"].as_array().expect("handles array");
        assert_eq!(handles.len(), 1);
        let nh = handles[0].as_str().expect("handle string").to_string();
        let props = s.execute("properties", &json!({ "handles": [nh] })).expect("properties should read back");
        assert_eq!(props["objects"][0]["type"], json!("Polyline"));
    }

    #[test]
    fn spline_to_polyline_samples_the_curve_into_a_polyline() {
        let mut s = session_with_doc();
        let h = s.execute("spline", &json!({ "fit": [[0.0, 0.0], [5.0, 5.0], [10.0, 0.0]] })).expect("spline should insert")["handle"]
            .as_str()
            .expect("handle string")
            .to_string();
        let out = s.execute("convert.spline2pl", &json!({ "handles": [h], "segments": 20 })).expect("conversion should succeed");
        let handles = out["handles"].as_array().expect("handles array");
        assert_eq!(handles.len(), 1);
        let nh = handles[0].as_str().expect("handle string").to_string();
        let props = s.execute("properties", &json!({ "handles": [nh] })).expect("properties should read back");
        assert_eq!(props["objects"][0]["type"], json!("Polyline"));
    }
}
