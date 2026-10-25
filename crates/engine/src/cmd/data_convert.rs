//! Two Data-menu commands derived from a user's AutoLISP toolbox, generalised away from their
//! original hardcoded table/field choices into ordinary JSON parameters (programmatic calls
//! never open dialogs - see `CLAUDE.md`):
//!
//! - DATA.COD_TO_NUM ("Point cod->num"): for each selected polyline, finds every block insert
//!   touching it, tallies one of the block's attribute values (default tag `COD`), and writes
//!   the most common one into a chosen attribute-table field on the polyline.
//! - DATA.EXTRACT_TO_TEXT ("Extraire donnée->texte"): for each selected entity, reads one or
//!   more attribute-table fields it carries and writes each value as a new TEXT entity near the
//!   entity's bounding-box centre, stacking multiple lines vertically.

use cadcraft_color::Color;
use cadcraft_doc::{EntityKind, Handle};
use cadcraft_geom::{Polyline, Vec2, Vec3};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("data.cod_to_num", "Point cod->num", run_cod_to_num).menu(&["Data", "Point cod->num"]).params(
            "{table: string, field: string, attr?: string (default \"COD\"), tolerance?: number (default 1e-6), \
             handles?: [string] (default: current selection, must be polylines)} → {updated}. For each \
             targeted polyline, finds every block insert whose insertion point touches it (within \
             `tolerance`), reads that block's `attr` attribute, and writes the most frequent value into \
             `field` of the polyline's `table` (an existing attribute table; the polyline is tagged with \
             it if it didn't already carry one).",
        ),
        CommandSpec::new("data.extract_to_text", "Extraire donnée->texte", run_extract_to_text).menu(&["Data", "Extraire donnée->texte"]).params(
            "{fields: [{table: string, field: string}], layer: string, color?: integer (ACI, default 7), \
             height?: number (default 2.5), dx?: number (default 0), dy?: number (default 0), \
             handles?: [string] (default: current selection)} → {created}. For each targeted entity that \
             carries one of the requested tables, writes that field's value as a new TEXT entity at the \
             entity's bounding-box centre (offset by dx/dy), on `layer` (created if missing) in `color`. An \
             entity matching several requested fields gets one stacked text per match.",
        ),
    ]
}

fn attribute_table_appid() -> &'static str {
    super::data::ATTR_TABLE_APPID
}

/// The polyline path of `e`, if it is one (world coordinates, 2D).
fn entity_polyline(e: &cadcraft_doc::Entity) -> Option<Polyline> {
    match &e.kind {
        EntityKind::LwPolyline(pl) => Some(Polyline { vertices: pl.vertices.clone(), closed: pl.closed }),
        EntityKind::Polyline3d(pl) => Some(Polyline::from_points(&pl.points.iter().map(|p| Vec2::new(p.x, p.y)).collect::<Vec<_>>(), pl.closed)),
        _ => None,
    }
}

/// Reads attribute `tag` (case-insensitive) off a block-insert entity, if present and non-empty.
fn insert_attr(e: &cadcraft_doc::Entity, tag: &str) -> Option<String> {
    let EntityKind::Insert(i) = &e.kind else { return None };
    i.attribs.iter().find(|a| a.tag.eq_ignore_ascii_case(tag)).map(|a| a.text.value.clone()).filter(|v| !v.trim().is_empty())
}

fn run_cod_to_num(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "data.cod_to_num";
    let table = str_param(p, "table").ok_or_else(|| bad(CMD, "`table` is required"))?.to_string();
    let field = str_param(p, "field").ok_or_else(|| bad(CMD, "`field` is required"))?.to_string();
    let attr = str_param(p, "attr").unwrap_or("COD").to_string();
    let tol = f64_or(p, "tolerance", 1e-6).max(0.0);
    let hs = targets(s, p)?;
    if hs.is_empty() {
        return Err(bad(CMD, "nothing selected"));
    }

    let d = s.doc()?;
    if d.attribute_table(&table).is_none() {
        return Err(bad(CMD, format!("no attribute table named `{table}`")));
    }
    let def = d.attribute_table(&table).cloned().unwrap_or_default();
    if !def.fields.is_empty() && !def.fields.iter().any(|f| f.name == field) {
        return Err(bad(CMD, format!("table `{table}` has no field `{field}`")));
    }

    // Collect every block insert's attribute point once, up front.
    let inserts: Vec<(Vec2, String)> = d
        .model
        .iter()
        .filter_map(|e| {
            let EntityKind::Insert(i) = &e.kind else { return None };
            insert_attr(e, &attr).map(|v| (Vec2::new(i.insert.x, i.insert.y), v))
        })
        .collect();

    let mut updates: Vec<(Handle, String)> = Vec::new();
    for h in &hs {
        let Some(e) = d.entity(*h) else { continue };
        let Some(pl) = entity_polyline(e) else { continue };
        if pl.vertices.is_empty() {
            continue;
        }
        let mut tally: Vec<(String, usize)> = Vec::new();
        for (pt, val) in &inserts {
            let Some(c) = pl.closest(*pt) else { continue };
            if c.dist(*pt) > tol {
                continue;
            }
            match tally.iter_mut().find(|(v, _)| v == val) {
                Some((_, n)) => *n += 1,
                None => tally.push((val.clone(), 1)),
            }
        }
        if let Some((best, _)) = tally.into_iter().max_by_key(|(_, n)| *n) {
            updates.push((*h, best));
        }
    }

    let mut updated = 0usize;
    for (h, value) in updates {
        let d = s.doc()?;
        let e = d.entity(h).ok_or_else(|| bad(CMD, "no such entity"))?;
        let (cur_table, mut fields) = cadcraft_doc::import_attrs::fields(e, attribute_table_appid())
            .filter(|(t, _)| t == &table)
            .unwrap_or_else(|| (table.clone(), def.fields.iter().map(|f| (f.name.clone(), String::new())).collect()));
        match fields.iter_mut().find(|(k, _)| *k == field) {
            Some((_, v)) => *v = value,
            None => fields.push((field.clone(), value)),
        }
        let xdata = cadcraft_doc::import_attrs::tag_fields(attribute_table_appid(), &cur_table, &fields);
        s.doc_mut()?.modify_entity(h, move |e| e.common.xdata = xdata).map_err(|e| bad(CMD, e.to_string()))?;
        updated += 1;
    }
    s.touch();
    Ok(json!({ "updated": updated }))
}

fn entity_center(d: &cadcraft_doc::Drawing, e: &cadcraft_doc::Entity) -> Vec2 {
    let b = crate::select::bounds_of(d, e, None);
    if b.is_empty() { Vec2::ZERO } else { b.min.lerp(b.max, 0.5) }
}

fn ensure_layer(d: &mut cadcraft_doc::Drawing, name: &str, color: Color) {
    if !d.layers.iter().any(|l| l.name.eq_ignore_ascii_case(name)) {
        let mut l = cadcraft_doc::Layer::new(name);
        l.color = color;
        d.layers.push(l);
    }
}

fn run_extract_to_text(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "data.extract_to_text";
    let wants = p.get("fields").and_then(Value::as_array).ok_or_else(|| bad(CMD, "`fields` ([{table, field}]) is required"))?;
    if wants.is_empty() {
        return Err(bad(CMD, "`fields` must list at least one {table, field}"));
    }
    let wants: Vec<(String, String)> = wants
        .iter()
        .map(|w| {
            let t = str_param(w, "table").ok_or_else(|| bad(CMD, "a `fields` entry is missing `table`"))?.to_string();
            let f = str_param(w, "field").ok_or_else(|| bad(CMD, "a `fields` entry is missing `field`"))?.to_string();
            Ok((t, f))
        })
        .collect::<Result<_>>()?;
    let layer = str_param(p, "layer").ok_or_else(|| bad(CMD, "`layer` is required"))?.to_string();
    let aci = p.get("color").and_then(Value::as_i64).unwrap_or(7).clamp(1, 255) as u8;
    let color = Color::Index(aci);
    let height = f64_or(p, "height", 2.5);
    if height <= 0.0 {
        return Err(bad(CMD, "`height` must be positive"));
    }
    let dx = f64_or(p, "dx", 0.0);
    let dy = f64_or(p, "dy", 0.0);
    let hs = targets(s, p)?;
    if hs.is_empty() {
        return Err(bad(CMD, "nothing selected"));
    }

    ensure_layer(s.doc_mut()?, &layer, color);

    let mut created = 0usize;
    let mut to_add: Vec<EntityKind> = Vec::new();
    {
        let d = s.doc()?;
        for h in &hs {
            let Some(e) = d.entity(*h) else { continue };
            let mut decal = 0.0f64;
            for (table, field) in &wants {
                let Some((etable, fields)) = cadcraft_doc::import_attrs::fields(e, attribute_table_appid())
                    .or_else(|| cadcraft_doc::import_attrs::fields(e, cadcraft_doc::import_attrs::SHP_APPID))
                    .or_else(|| cadcraft_doc::import_attrs::fields(e, cadcraft_doc::import_attrs::DWG_MAP_APPID))
                else {
                    continue;
                };
                if !etable.eq_ignore_ascii_case(table) {
                    continue;
                }
                let Some((_, value)) = fields.iter().find(|(k, _)| k.eq_ignore_ascii_case(field)) else { continue };
                if value.trim().is_empty() {
                    continue;
                }
                let center = entity_center(d, e);
                let at = Vec2::new(center.x + dx, center.y + dy - decal);
                to_add.push(EntityKind::Text(cadcraft_doc::Text {
                    insert: Vec3::new(at.x, at.y, 0.0),
                    align_pt: None,
                    height,
                    value: value.clone(),
                    rotation: 0.0,
                    width_factor: 1.0,
                    oblique: 0.0,
                    style: d.header.str("TEXTSTYLE", "Standard"),
                    halign: cadcraft_doc::HAlign::Left,
                    valign: cadcraft_doc::VAlign::Baseline,
                }));
                decal += height * 1.5;
            }
        }
    }
    for k in to_add {
        let h = s.add_entity(k)?;
        s.doc_mut()?.modify_entity(h, |e| {
            e.common.layer = layer.clone();
            e.common.color = color;
        })?;
        created += 1;
    }
    s.touch();
    Ok(json!({ "created": created }))
}
