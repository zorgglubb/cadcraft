//! `plan.merge_polygons` ("Fusionner 2 polygones"): merges exactly 2 selected closed, straight-
//! edged LWPOLYLINEs into one, keeping the first polygon's layer, color and attribute-table
//! XDATA (its whole `Common`, in fact - same reasoning as the original AutoLISP tool this is
//! based on, which captured and restored the first polygon's Object Data tables). Only works
//! when the two polygons' union is a single simple closed loop (they touch or overlap into one
//! blob); disjoint polygons, or an overlap that would leave a hole, are reported as errors
//! rather than guessed at - see `cadcraft_geom::polygon_union`'s module doc for the exact scope.
//! Arcs (bulges) on either polyline aren't supported: this tool compares and stitches straight
//! edges only, so a polyline with any nonzero bulge is rejected up front with a clear message.

use cadcraft_doc::{EntityKind, LwPolyline};
use cadcraft_geom::{Vec2, polygon_union};
use serde_json::{Value, json};

use super::*;
use crate::{EngineError, Result, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec::new("plan.merge_polygons", "Fusionner 2 polygones", run_merge_polygons).menu(&["Plan", "Fusionner 2 polygones"]).params(
        "{handles: [hex, hex]} (exactly 2 closed, straight-edged LWPOLYLINEs; default: the current \
                 selection) → {handle}. Merges the two into one polyline carrying the FIRST one's layer, \
                 color and attribute table; both originals are deleted. Only succeeds when the two polygons \
                 touch or overlap into a single simple closed boundary - errs for disjoint polygons, for a \
                 union that would leave a hole, or for a polyline with any bulge (arc) segment.",
    )]
}

fn straight_polygon(d: &cadcraft_doc::Drawing, h: Handle) -> Result<Vec<Vec2>> {
    let e = d.entity(h).ok_or_else(|| EngineError::Other("no such object".into()))?;
    let EntityKind::LwPolyline(pl) = &e.kind else {
        return Err(EngineError::Other("Select a closed LWPOLYLINE.".into()));
    };
    if !pl.closed {
        return Err(EngineError::Other("The polyline must be closed.".into()));
    }
    if pl.vertices.iter().any(|v| v.bulge.abs() > 1e-9) {
        return Err(EngineError::Other("This tool only supports straight-edged polylines (no arc segments).".into()));
    }
    if pl.vertices.len() < 3 {
        return Err(EngineError::Other("The polyline needs at least 3 vertices.".into()));
    }
    Ok(pl.vertices.iter().map(|v| v.p).collect())
}

fn run_merge_polygons(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    if hs.len() != 2 {
        return Err(EngineError::Other("Select exactly 2 closed polylines to merge.".into()));
    }
    let (h1, h2) = (hs[0], hs[1]);

    let (pts1, pts2, common1, elevation) = {
        let d = s.doc()?;
        let pts1 = straight_polygon(d, h1)?;
        let pts2 = straight_polygon(d, h2)?;
        let common1 = d.entity(h1).ok_or_else(|| EngineError::Other("no such object".into()))?.common.clone();
        let elevation = match d.entity(h1).map(|e| &e.kind) {
            Some(EntityKind::LwPolyline(pl)) => pl.elevation,
            _ => 0.0,
        };
        (pts1, pts2, common1, elevation)
    };

    let merged = polygon_union::union(&pts1, &pts2).map_err(|e| EngineError::Other(format!("Could not merge the polygons: {e}")))?;

    let space = s.space();
    let vertices = merged.into_iter().map(|p| cadcraft_geom::PolyVertex { p, bulge: 0.0, start_width: 0.0, end_width: 0.0 }).collect();
    let kind = EntityKind::LwPolyline(LwPolyline { vertices, closed: true, const_width: 0.0, elevation, plinegen: false });

    let d = s.doc_mut()?;
    d.remove_entity(h1);
    d.remove_entity(h2);
    let new_h = d.add(&space, common1, kind)?;

    Ok(json!({ "handle": new_h.hex() }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::Session;

    fn session_with_doc() -> Session {
        let mut s = Session::new();
        s.execute("new", &json!({})).expect("new drawing should succeed");
        s
    }

    fn square(x0: f64, y0: f64, s: f64) -> cadcraft_doc::LwPolyline {
        let v = |x: f64, y: f64| cadcraft_geom::PolyVertex { p: Vec2::new(x, y), bulge: 0.0, start_width: 0.0, end_width: 0.0 };
        LwPolyline {
            vertices: vec![v(x0, y0), v(x0 + s, y0), v(x0 + s, y0 + s), v(x0, y0 + s)],
            closed: true,
            const_width: 0.0,
            elevation: 0.0,
            plinegen: false,
        }
    }

    #[test]
    fn merges_two_overlapping_squares_into_one_polyline() {
        let mut s = session_with_doc();
        let h1 = s.add_entity(EntityKind::LwPolyline(square(0.0, 0.0, 2.0))).expect("add h1");
        let h2 = s.add_entity(EntityKind::LwPolyline(square(1.0, 1.0, 2.0))).expect("add h2");
        let out = s.execute("plan.merge_polygons", &json!({ "handles": [h1.hex(), h2.hex()] })).expect("merge");
        let new_handle = out.get("handle").and_then(|v| v.as_str()).expect("handle in result");
        let h = Handle::parse_hex(new_handle).expect("valid handle");
        let d = s.doc().expect("doc");
        assert!(d.entity(h1).is_none(), "h1 should be deleted");
        assert!(d.entity(h2).is_none(), "h2 should be deleted");
        let e = d.entity(h).expect("merged entity exists");
        match &e.kind {
            EntityKind::LwPolyline(pl) => {
                assert!(pl.closed);
                let area = cadcraft_geom::shoelace(&pl.vertices.iter().map(|v| v.p).collect::<Vec<_>>()).abs();
                assert!((area - 7.0).abs() < 1e-6, "area={area}");
            }
            _ => panic!("expected LwPolyline"),
        }
    }

    #[test]
    fn disjoint_polygons_produce_a_clear_error() {
        let mut s = session_with_doc();
        let h1 = s.add_entity(EntityKind::LwPolyline(square(0.0, 0.0, 1.0))).expect("add h1");
        let h2 = s.add_entity(EntityKind::LwPolyline(square(100.0, 100.0, 1.0))).expect("add h2");
        let err = s.execute("plan.merge_polygons", &json!({ "handles": [h1.hex(), h2.hex()] })).unwrap_err();
        assert!(err.to_string().contains("Could not merge"), "{err}");
    }

    #[test]
    fn wrong_selection_count_is_rejected() {
        let mut s = session_with_doc();
        let h1 = s.add_entity(EntityKind::LwPolyline(square(0.0, 0.0, 1.0))).expect("add h1");
        let err = s.execute("plan.merge_polygons", &json!({ "handles": [h1.hex()] })).unwrap_err();
        assert!(err.to_string().contains("exactly 2"), "{err}");
    }
}
