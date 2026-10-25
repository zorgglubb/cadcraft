//! `data.points_to_tcpoint` ("Point->TCPOINT"): converts each selected POINT entity into a
//! CADCRAFT_TCPOINT block insert, filling COD/ALT/MAT from fields the point already carries in
//! one of its attribute tables - a user-defined table, a Shapefile import, or a DWG's recovered
//! Map 3D "Object Data" (whichever `data::entity_table` finds, same lookup DATA.SEARCH itself
//! uses), and copying that table's XDATA onto the new insert so it stays findable by DATA.SEARCH
//! and friends. The source POINT is deleted.

use cadcraft_doc::{EntityKind, Handle};
use serde_json::{Value, json};

use super::topo::{TCPOINT_BLOCK, insert_tcpoint_block_only};
use super::*;
use crate::{Result, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec::new("data.points_to_tcpoint", "Point->TCPOINT", run_points_to_tcpoint).menu(&["Data", "Point->TCPOINT"]).params(
        "{handles?: [string] (default: current selection, POINT entities only), codField?: string, \
         altField?: string, matField?: string, blockName?: string (default \"CADCRAFT_TCPOINT\")} → \
         {created, skipped}. For each targeted POINT carrying an attribute table, inserts a TCPOINT \
         block at its location with COD/ALT/MAT set from the named fields (any left unset stays \
         blank), carries the point's own attribute-table data over onto the new insert, and deletes \
         the source point. A point with no attribute table is skipped, not an error, unless none of \
         the targets had one, in which case the whole command errors.",
    )]
}

fn run_points_to_tcpoint(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "data.points_to_tcpoint";
    let cod_field = str_param(p, "codField").map(str::to_string);
    let alt_field = str_param(p, "altField").map(str::to_string);
    let mat_field = str_param(p, "matField").map(str::to_string);
    let block = str_param(p, "blockName").unwrap_or(TCPOINT_BLOCK).to_string();
    let hs = targets(s, p)?;
    if hs.is_empty() {
        return Err(bad(CMD, "nothing selected"));
    }

    struct Pending {
        h: Handle,
        at: cadcraft_geom::Vec2,
        xdata: Vec<cadcraft_doc::RawTag>,
        cod: Option<String>,
        alt: Option<String>,
        mat: Option<String>,
    }
    let mut pending: Vec<Pending> = Vec::new();
    {
        let d = s.doc()?;
        for h in &hs {
            let Some(e) = d.entity(*h) else { continue };
            let EntityKind::Point(pt) = &e.kind else { continue };
            let Some((_, fields)) = super::data::entity_table(e) else { continue };
            let get =
                |key: &Option<String>| key.as_deref().and_then(|k| fields.iter().find(|(n, _)| n.eq_ignore_ascii_case(k))).map(|(_, v)| v.clone());
            pending.push(Pending {
                h: *h,
                at: cadcraft_geom::Vec2::new(pt.p.x, pt.p.y),
                xdata: e.common.xdata.clone(),
                cod: get(&cod_field),
                alt: get(&alt_field),
                mat: get(&mat_field),
            });
        }
    }
    if pending.is_empty() {
        return Err(bad(CMD, "none of the selected POINTs carry an attribute table"));
    }

    insert_tcpoint_block_only(s, &block)?;

    let mut created = 0usize;
    let skipped = hs.len().saturating_sub(pending.len());
    for item in pending {
        let mut attribs = serde_json::Map::new();
        if let Some(c) = &item.cod {
            attribs.insert("COD".into(), Value::String(c.clone()));
        }
        if let Some(a) = &item.alt {
            attribs.insert("ALT".into(), Value::String(a.clone()));
        }
        if let Some(m) = &item.mat {
            attribs.insert("MAT".into(), Value::String(m.clone()));
        }
        let out = s.execute("insert", &json!({ "name": block, "at": [item.at.x, item.at.y], "attribs": Value::Object(attribs) }))?;
        if let Some(hd) = out.get("handle").and_then(Value::as_str).and_then(Handle::parse_hex) {
            let xdata = item.xdata.clone();
            s.doc_mut()?.modify_entity(hd, move |e| e.common.xdata = xdata).map_err(|e| bad(CMD, e.to_string()))?;
        }
        s.doc_mut()?.remove_entity(item.h);
        created += 1;
    }
    s.touch();
    Ok(json!({ "created": created, "skipped": skipped }))
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

    fn with_table(s: &mut Session, h: cadcraft_doc::Handle, table: &str, fields: &[(&str, &str)]) {
        let xdata = cadcraft_doc::import_attrs::tag_fields(
            super::super::data::ATTR_TABLE_APPID,
            table,
            &fields.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<Vec<_>>(),
        );
        s.doc_mut().expect("doc").modify_entity(h, move |e| e.common.xdata = xdata).expect("tag entity");
    }

    #[test]
    fn converts_a_point_with_fields_into_a_tcpoint_block() {
        let mut s = session_with_doc();
        let h = s
            .add_entity(cadcraft_doc::EntityKind::Point(cadcraft_doc::Point { p: cadcraft_geom::Vec3::new(5.0, 7.0, 0.0), angle: 0.0 }))
            .expect("add point");
        with_table(&mut s, h, "Points", &[("Code", "BORNE"), ("Z", "12.5"), ("Mat", "P1")]);

        let out = s
            .execute("data.points_to_tcpoint", &json!({ "handles": [h.hex()], "codField": "Code", "altField": "Z", "matField": "Mat" }))
            .expect("conversion should succeed");
        assert_eq!(out["created"].as_u64(), Some(1));
        assert_eq!(out["skipped"].as_u64(), Some(0));

        let d = s.doc().expect("doc");
        assert!(d.entity(h).is_none(), "the source point should be deleted");
        let found = d.model.iter().find_map(|e| match &e.kind {
            cadcraft_doc::EntityKind::Insert(i) if i.block.eq_ignore_ascii_case(super::super::topo::TCPOINT_BLOCK) => Some(i),
            _ => None,
        });
        let ins = found.expect("a TCPOINT insert should have been created");
        let attr = |tag: &str| ins.attribs.iter().find(|a| a.tag == tag).map(|a| a.text.value.clone());
        assert_eq!(attr("COD"), Some("BORNE".to_string()));
        assert_eq!(attr("ALT"), Some("12.5".to_string()));
        assert_eq!(attr("MAT"), Some("P1".to_string()));
    }

    #[test]
    fn a_point_with_no_attribute_table_is_skipped_not_an_error() {
        let mut s = session_with_doc();
        let h1 = s
            .add_entity(cadcraft_doc::EntityKind::Point(cadcraft_doc::Point { p: cadcraft_geom::Vec3::new(0.0, 0.0, 0.0), angle: 0.0 }))
            .expect("add h1");
        let h2 = s
            .add_entity(cadcraft_doc::EntityKind::Point(cadcraft_doc::Point { p: cadcraft_geom::Vec3::new(1.0, 1.0, 0.0), angle: 0.0 }))
            .expect("add h2");
        with_table(&mut s, h2, "Points", &[("Code", "X")]);

        let out = s.execute("data.points_to_tcpoint", &json!({ "handles": [h1.hex(), h2.hex()], "codField": "Code" })).expect("should succeed");
        assert_eq!(out["created"].as_u64(), Some(1));
        assert_eq!(out["skipped"].as_u64(), Some(1));
        assert!(s.doc().expect("doc").entity(h1).is_some(), "the untagged point should be left alone");
    }

    #[test]
    fn errors_when_nothing_selected_carries_a_table() {
        let mut s = session_with_doc();
        let h = s.add_entity(cadcraft_doc::EntityKind::Point(cadcraft_doc::Point { p: cadcraft_geom::Vec3::ZERO, angle: 0.0 })).expect("add point");
        let err = s.execute("data.points_to_tcpoint", &json!({ "handles": [h.hex()] })).unwrap_err();
        assert!(err.to_string().contains("attribute table"), "{err}");
    }
}
