//! Two "Données" (Data) menu conversion commands ported from a user's personal AutoLISP
//! toolbox: `data.altitude_to_poly` (TCALT.lsp) and `data.axe_to_ellipse` (AXE2ELLIPSE.lsp).
//! Both read/write the same Object Data mechanism as `data.rs`/`plan.rs`'s `set_shp_attribute`.

use cadcraft_doc::{AttributeField, AttributeFieldKind, AttributeTableDef, Entity, EntityKind, Handle};
use cadcraft_geom::{PolyVertex, Vec2, Vec3};
use serde_json::{Value, json};

use super::data::ATTR_TABLE_APPID;
use super::*;
use crate::{Result, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("data.altitude_to_poly", "Altitude->poly", run_altitude_to_poly).menu(&["Data", "Altitude->poly"]).params(
            "{handles?: [string] (default: current selection; LINE/LWPOLYLINE only), \
                 blockName?: string (default \"TCPOINT\"), altAttr?: string (default \"ALT\")} → \
                 {changed}. For each targeted LINE/LWPOLYLINE, finds the single closest `blockName` \
                 block insert (anywhere in the drawing) carrying a numeric `altAttr` attribute, and \
                 sets the entity's elevation (LWPOLYLINE) or both endpoints' Z (LINE) to that value.",
        ),
        CommandSpec::new("data.axe_to_ellipse", "Axe->Ellipse", run_axe_to_ellipse).menu(&["Data", "Axe->Ellipse"]).params(
            "{handles?: [string] (default: current selection; LINE/LWPOLYLINE only), \
                 sourceTable?: string (default \"Axe\"), fieldNature?: string (default \"Nature\"), \
                 destTable?: string (default \"Structure\"), threshold?: number (default 0.08), \
                 sizeThreshold?: number (default 0.30), deleteTrigger?: string (default \"PLAN\"), \
                 remap?: [[string, string]] (default [])} → {created, deleted, simplified}. For \
                 each targeted entity carrying `sourceTable` with a non-empty `fieldNature` value, \
                 builds an elliptical LWPOLYLINE from 3 construction points walked off its vertex \
                 list, copies its Object Data (remapped if `remap` matches the Nature value) into \
                 `destTable` (auto-created with the source's field types plus a blank \"Datation\" \
                 text field, if missing), then deletes the source (if its Nature value matches \
                 `deleteTrigger`) or simplifies it to just its first two construction points.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------------
// data.altitude_to_poly
// ---------------------------------------------------------------------------------------------

/// Closest distance (squared) from `p` to the segment `a`-`b`, in the XY plane.
fn dist2_point_segment(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let ab = Vec2::new(b.x - a.x, b.y - a.y);
    let len2 = ab.x * ab.x + ab.y * ab.y;
    if len2 <= f64::EPSILON {
        let d = Vec2::new(p.x - a.x, p.y - a.y);
        return d.x * d.x + d.y * d.y;
    }
    let ap = Vec2::new(p.x - a.x, p.y - a.y);
    let t = ((ap.x * ab.x + ap.y * ab.y) / len2).clamp(0.0, 1.0);
    let closest = Vec2::new(a.x + ab.x * t, a.y + ab.y * t);
    let d = Vec2::new(p.x - closest.x, p.y - closest.y);
    d.x * d.x + d.y * d.y
}

/// The entity's own path as a list of XY points, in order (LINE: 2 points; LWPOLYLINE: its
/// vertices, closing the loop back to the first if `closed`).
fn entity_path(e: &Entity) -> Option<Vec<Vec2>> {
    match &e.kind {
        EntityKind::Line(l) => Some(vec![l.a.xy(), l.b.xy()]),
        EntityKind::LwPolyline(pl) => {
            let mut pts: Vec<Vec2> = pl.vertices.iter().map(|v| v.p).collect();
            if pl.closed && !pts.is_empty() {
                pts.push(pts[0]);
            }
            Some(pts)
        }
        _ => None,
    }
}

/// The closest distance (squared) from `p` to any segment of `path`.
fn dist2_point_path(p: Vec2, path: &[Vec2]) -> f64 {
    let mut best = f64::INFINITY;
    for w in path.windows(2) {
        let d = dist2_point_segment(p, w[0], w[1]);
        if d < best {
            best = d;
        }
    }
    best
}

fn attrib_value<'a>(i: &'a cadcraft_doc::Insert, tag: &str) -> Option<&'a str> {
    i.attribs.iter().find(|a| a.tag.eq_ignore_ascii_case(tag)).map(|a| a.text.value.as_str())
}

fn run_altitude_to_poly(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "data.altitude_to_poly";
    let block_name = str_param(p, "blockName").unwrap_or("TCPOINT").to_string();
    let alt_attr = str_param(p, "altAttr").unwrap_or("ALT").to_string();
    let hs = targets(s, p)?;
    if hs.is_empty() {
        return Err(bad(CMD, "nothing selected"));
    }

    let d = s.doc()?;
    // Every TCPOINT-like block insert with a numeric ALT value, anywhere in model space or a
    // layout - collected once, reused for every targeted entity.
    let mut points: Vec<(Vec2, f64)> = Vec::new();
    let stores = std::iter::once(&d.model).chain(d.layouts.iter().map(|l| &l.entities));
    for store in stores {
        for e in store.iter() {
            if let EntityKind::Insert(i) = &e.kind
                && i.block.eq_ignore_ascii_case(&block_name)
                && let Some(raw) = attrib_value(i, &alt_attr)
                && let Ok(alt) = raw.trim().parse::<f64>()
                && alt.is_finite()
            {
                points.push((i.insert.xy(), alt));
            }
        }
    }
    if points.is_empty() {
        return Err(bad(CMD, format!("no `{block_name}` block with a numeric `{alt_attr}` attribute was found")));
    }

    // Resolve each target's nearest altitude first (needs only the immutable borrow of `d`),
    // then apply the mutations in a second pass so the borrows never overlap.
    let mut resolved: Vec<(Handle, f64)> = Vec::new();
    for h in &hs {
        let Some(e) = d.entity(*h) else { continue };
        let Some(path) = entity_path(e) else { continue };
        if path.is_empty() {
            continue;
        }
        let mut best_alt = None;
        let mut best_d2 = f64::INFINITY;
        for (pt, alt) in &points {
            let d2 = dist2_point_path(*pt, &path);
            if d2 < best_d2 {
                best_d2 = d2;
                best_alt = Some(*alt);
            }
        }
        if let Some(alt) = best_alt {
            resolved.push((*h, alt));
        }
    }

    let mut changed = 0usize;
    for (h, alt) in resolved {
        s.doc_mut()?
            .modify_entity(h, move |e| match &mut e.kind {
                EntityKind::LwPolyline(pl) => pl.elevation = alt,
                EntityKind::Line(l) => {
                    l.a.z = alt;
                    l.b.z = alt;
                }
                _ => {}
            })
            .map_err(|e| bad(CMD, e.to_string()))?;
        changed += 1;
    }
    if changed == 0 {
        return Err(bad(CMD, "no LINE/LWPOLYLINE in the selection could be matched to a TCPOINT"));
    }
    s.touch();
    Ok(json!({ "changed": changed }))
}

// ---------------------------------------------------------------------------------------------
// data.axe_to_ellipse
// ---------------------------------------------------------------------------------------------

fn num_or(p: &Value, key: &str, default: f64) -> f64 {
    p.get(key).and_then(Value::as_f64).filter(|v| v.is_finite()).unwrap_or(default)
}

/// Walks `path` to find P0 (first vertex), PB (first vertex at distance >= `threshold` from P0)
/// and PC (first vertex after PB at distance >= `threshold` from PB; PC = PB if none found).
fn construction_points(path: &[Vec2], threshold: f64) -> Option<(Vec2, Vec2, Vec2)> {
    let p0 = *path.first()?;
    let dist = |a: Vec2, b: Vec2| ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt();
    let mut pb = None;
    let mut pb_idx = 0usize;
    for (idx, pt) in path.iter().enumerate().skip(1) {
        if dist(p0, *pt) >= threshold {
            pb = Some(*pt);
            pb_idx = idx;
            break;
        }
    }
    let pb = pb?;
    let mut pc = pb;
    for pt in path.iter().skip(pb_idx + 1) {
        if dist(pb, *pt) >= threshold {
            pc = *pt;
            break;
        }
    }
    Some((p0, pb, pc))
}

/// Tessellates an ellipse (center/major-radius/minor-radius/rotation) into a closed polygon of
/// `segments` points.
fn tessellate_ellipse(center: Vec2, major_r: f64, minor_r: f64, rotation: f64, segments: usize) -> Vec<Vec2> {
    let segments = segments.max(3);
    (0..segments)
        .map(|i| {
            let t = 2.0 * std::f64::consts::PI * (i as f64) / (segments as f64);
            let (lx, ly) = (major_r * t.cos(), minor_r * t.sin());
            let (cs, sn) = (rotation.cos(), rotation.sin());
            Vec2::new(center.x + lx * cs - ly * sn, center.y + lx * sn + ly * cs)
        })
        .collect()
}

fn remapped(value: &str, remap: &[(String, String)]) -> String {
    remap.iter().find(|(from, _)| from == value).map(|(_, to)| to.clone()).unwrap_or_else(|| value.to_string())
}

/// Ensures `dest_table` exists (creating it from `source_def`'s fields plus a blank "Datation"
/// text field if it's missing) and returns its definition.
fn ensure_dest_table(d: &mut cadcraft_doc::Drawing, dest_table: &str, source_def: &AttributeTableDef) -> AttributeTableDef {
    if let Some(existing) = d.attribute_table(dest_table) {
        return existing.clone();
    }
    let mut fields = source_def.fields.clone();
    if !fields.iter().any(|f| f.name.eq_ignore_ascii_case("Datation")) {
        fields.push(AttributeField { name: "Datation".to_string(), kind: AttributeFieldKind::Character });
    }
    let def = AttributeTableDef { name: dest_table.to_string(), fields };
    d.attribute_tables.push(def.clone());
    def
}

fn run_axe_to_ellipse(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "data.axe_to_ellipse";
    let source_table = str_param(p, "sourceTable").unwrap_or("Axe").to_string();
    let field_nature = str_param(p, "fieldNature").unwrap_or("Nature").to_string();
    let dest_table = str_param(p, "destTable").unwrap_or("Structure").to_string();
    let threshold = num_or(p, "threshold", 0.08).max(0.0);
    let size_threshold = num_or(p, "sizeThreshold", 0.30).max(0.0);
    let delete_trigger = str_param(p, "deleteTrigger").unwrap_or("PLAN").to_string();
    let remap: Vec<(String, String)> = p
        .get("remap")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|pair| {
                    let pair = pair.as_array()?;
                    Some((pair.first()?.as_str()?.to_string(), pair.get(1)?.as_str()?.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();

    let hs = targets(s, p)?;
    if hs.is_empty() {
        return Err(bad(CMD, "nothing selected"));
    }

    let mut created = 0usize;
    let mut deleted = 0usize;
    let mut simplified = 0usize;

    for h in hs {
        let d = s.doc()?;
        let Some(e) = d.entity(h) else { continue };
        let Some(path) = entity_path(e) else { continue };
        let Some((table, fields)) = cadcraft_doc::import_attrs::fields(e, ATTR_TABLE_APPID) else { continue };
        if !table.eq_ignore_ascii_case(&source_table) {
            continue;
        }
        let Some((_, nature_value)) = fields.iter().find(|(k, _)| k.eq_ignore_ascii_case(&field_nature)) else { continue };
        if nature_value.trim().is_empty() {
            continue;
        }
        let nature_value = nature_value.clone();
        let Some((p0, pb, pc)) = construction_points(&path, threshold) else { continue };

        let center = Vec2::new((p0.x + pb.x) / 2.0, (p0.y + pb.y) / 2.0);
        let major_r = ((pb.x - p0.x).powi(2) + (pb.y - p0.y).powi(2)).sqrt() / 2.0;
        let minor_r = ((pc.x - center.x).powi(2) + (pc.y - center.y).powi(2)).sqrt();
        let rotation = (pb.y - p0.y).atan2(pb.x - p0.x);
        let diameter = 2.0 * major_r.max(minor_r);
        let segments = if diameter < size_threshold { 16 } else { 36 };
        let pts = tessellate_ellipse(center, major_r, minor_r, rotation, segments);
        let layer = e.common.layer.clone();
        let color = e.common.color;

        let source_def = d.attribute_table(&source_table).cloned().unwrap_or_else(|| {
            let fields = fields.iter().map(|(name, _)| AttributeField { name: name.clone(), kind: AttributeFieldKind::Character }).collect();
            AttributeTableDef { name: source_table.clone(), fields }
        });
        let dest_def = ensure_dest_table(s.doc_mut()?, &dest_table, &source_def);
        let mut dest_fields: Vec<(String, String)> = fields
            .iter()
            .map(|(k, v)| if k.eq_ignore_ascii_case(&field_nature) { (k.clone(), remapped(v, &remap)) } else { (k.clone(), v.clone()) })
            .collect();
        for f in &dest_def.fields {
            if !dest_fields.iter().any(|(k, _)| k.eq_ignore_ascii_case(&f.name)) {
                dest_fields.push((f.name.clone(), String::new()));
            }
        }
        let xdata = cadcraft_doc::import_attrs::tag_fields(ATTR_TABLE_APPID, &dest_def.name, &dest_fields);

        let poly = cadcraft_doc::LwPolyline {
            vertices: pts.into_iter().map(|p| PolyVertex { p, ..Default::default() }).collect(),
            closed: true,
            const_width: 0.0,
            elevation: 0.0,
            plinegen: false,
        };
        let new_h = s.add_entity(EntityKind::LwPolyline(poly))?;
        s.doc_mut()?
            .modify_entity(new_h, move |e| {
                e.common.layer = layer;
                e.common.color = color;
                e.common.xdata = xdata;
            })
            .map_err(|e| bad(CMD, e.to_string()))?;
        created += 1;

        if nature_value.eq_ignore_ascii_case(&delete_trigger) {
            s.doc_mut()?.remove_entity(h);
            deleted += 1;
        } else {
            let orig_xdata = cadcraft_doc::import_attrs::tag_fields(ATTR_TABLE_APPID, &table, &fields);
            s.doc_mut()?
                .modify_entity(h, move |e| {
                    e.common.xdata = orig_xdata;
                    match &mut e.kind {
                        EntityKind::LwPolyline(pl) => {
                            pl.vertices = vec![PolyVertex { p: p0, ..PolyVertex::default() }, PolyVertex { p: pb, ..PolyVertex::default() }];
                            pl.closed = false;
                        }
                        EntityKind::Line(l) => {
                            l.a = Vec3::new(p0.x, p0.y, l.a.z);
                            l.b = Vec3::new(pb.x, pb.y, l.b.z);
                        }
                        _ => {}
                    }
                })
                .map_err(|e| bad(CMD, e.to_string()))?;
            simplified += 1;
        }
    }

    if created == 0 {
        return Err(bad(CMD, format!("no selected entity carries table `{source_table}` with a non-empty `{field_nature}` value")));
    }
    s.touch();
    Ok(json!({ "created": created, "deleted": deleted, "simplified": simplified }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadcraft_doc::{Attrib, AttribModes, AttribProps, Insert, Line, LwPolyline, Text};
    use cadcraft_geom::PolyVertex as PV;

    fn tcpoint(x: f64, y: f64, alt: &str) -> Insert {
        let mut i = Insert {
            block: "TCPOINT".to_string(),
            insert: Vec3::new(x, y, 0.0),
            scale: Vec3::new(1.0, 1.0, 1.0),
            rotation: 0.0,
            attribs: Vec::new(),
            cols: 1,
            rows: 1,
            col_spacing: 0.0,
            row_spacing: 0.0,
        };
        i.attribs.push(Attrib {
            tag: "ALT".to_string(),
            text: Text {
                insert: Vec3::new(x, y, 0.0),
                align_pt: None,
                height: 1.0,
                value: alt.to_string(),
                rotation: 0.0,
                width_factor: 1.0,
                oblique: 0.0,
                style: "Standard".to_string(),
                halign: Default::default(),
                valign: Default::default(),
            },
            invisible: false,
            constant: false,
            prompt: String::new(),
            props: AttribProps::default(),
            multiline: None,
            modes: AttribModes::default(),
        });
        i
    }

    fn pv(x: f64, y: f64) -> PV {
        PV { p: Vec2::new(x, y), ..PV::default() }
    }

    #[test]
    fn altitude_to_poly_sets_elevation_from_nearest_tcpoint() {
        let mut s = Session::new();
        s.add_entity(EntityKind::Insert(tcpoint(0.0, 0.0, "123.5"))).unwrap();
        s.add_entity(EntityKind::Insert(tcpoint(1000.0, 1000.0, "9.0"))).unwrap();

        let pl = LwPolyline { vertices: vec![pv(0.1, 0.0), pv(5.0, 0.0)], closed: false, const_width: 0.0, elevation: 0.0, plinegen: false };
        let h = s.add_entity(EntityKind::LwPolyline(pl)).unwrap();

        let out = s.execute("data.altitude_to_poly", &json!({ "handles": [h.hex()] })).unwrap();
        assert_eq!(out["changed"], 1);
        let EntityKind::LwPolyline(pl) = &s.doc().unwrap().entity(h).unwrap().kind else { panic!() };
        assert!((pl.elevation - 123.5).abs() < 1e-9);
    }

    #[test]
    fn altitude_to_poly_errors_with_no_tcpoint() {
        let mut s = Session::new();
        let l = Line { a: Vec3::new(0.0, 0.0, 0.0), b: Vec3::new(1.0, 0.0, 0.0) };
        let h = s.add_entity(EntityKind::Line(l)).unwrap();
        assert!(s.execute("data.altitude_to_poly", &json!({ "handles": [h.hex()] })).is_err());
    }

    #[test]
    fn axe_to_ellipse_creates_ellipse_and_simplifies_source() {
        let mut s = Session::new();
        s.doc_mut().unwrap().attribute_tables.push(AttributeTableDef {
            name: "Axe".to_string(),
            fields: vec![
                AttributeField { name: "Num".to_string(), kind: AttributeFieldKind::Character },
                AttributeField { name: "Nature".to_string(), kind: AttributeFieldKind::Character },
            ],
        });
        let pl =
            LwPolyline { vertices: vec![pv(0.0, 0.0), pv(1.0, 0.0), pv(0.5, 0.3)], closed: false, const_width: 0.0, elevation: 0.0, plinegen: false };
        let h = s.add_entity(EntityKind::LwPolyline(pl)).unwrap();
        let xdata = cadcraft_doc::import_attrs::tag_fields(
            ATTR_TABLE_APPID,
            "Axe",
            &[("Num".to_string(), "42".to_string()), ("Nature".to_string(), "FOSSE".to_string())],
        );
        s.doc_mut().unwrap().modify_entity(h, move |e| e.common.xdata = xdata).unwrap();

        let out = s.execute("data.axe_to_ellipse", &json!({ "handles": [h.hex()] })).unwrap();
        assert_eq!(out["created"], 1);
        assert_eq!(out["simplified"], 1);
        assert_eq!(out["deleted"], 0);
        assert!(s.doc().unwrap().attribute_table("Structure").is_some());
        let EntityKind::LwPolyline(pl) = &s.doc().unwrap().entity(h).unwrap().kind else { panic!() };
        assert_eq!(pl.vertices.len(), 2);
    }

    #[test]
    fn axe_to_ellipse_deletes_source_when_nature_matches_trigger() {
        let mut s = Session::new();
        s.doc_mut().unwrap().attribute_tables.push(AttributeTableDef {
            name: "Axe".to_string(),
            fields: vec![AttributeField { name: "Nature".to_string(), kind: AttributeFieldKind::Character }],
        });
        let pl =
            LwPolyline { vertices: vec![pv(0.0, 0.0), pv(1.0, 0.0), pv(0.5, 0.3)], closed: false, const_width: 0.0, elevation: 0.0, plinegen: false };
        let h = s.add_entity(EntityKind::LwPolyline(pl)).unwrap();
        let xdata = cadcraft_doc::import_attrs::tag_fields(ATTR_TABLE_APPID, "Axe", &[("Nature".to_string(), "PLAN".to_string())]);
        s.doc_mut().unwrap().modify_entity(h, move |e| e.common.xdata = xdata).unwrap();

        let out = s.execute("data.axe_to_ellipse", &json!({ "handles": [h.hex()] })).unwrap();
        assert_eq!(out["deleted"], 1);
        assert!(s.doc().unwrap().entity(h).is_none());
    }
}
