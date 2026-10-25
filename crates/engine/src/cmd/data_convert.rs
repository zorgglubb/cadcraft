//! Two Data-menu commands derived from a user's AutoLISP toolbox, generalised away from their
//! original hardcoded table/field choices into ordinary JSON parameters. Menu/toolbar invocation
//! still never opens an egui dialog (see `CLAUDE.md`'s "Programmatic calls never open dialogs"):
//! instead each one is `Interactive`, gathering its required table/field/layer names through a
//! short chain of command-line prompts - the same mechanism PLAN.AUTONUM uses - before running
//! the plain JSON form with the answers.
//!
//! - DATA.COD_TO_NUM ("Point cod->num"): for each selected polyline, finds every block insert
//!   touching it, tallies one of the block's attribute values (default tag `COD`), and writes
//!   the most common one into a chosen attribute-table field on the polyline.
//! - DATA.EXTRACT_TO_TEXT ("Extraire donnée->texte"): for each selected entity, reads one or
//!   more attribute-table fields it carries and writes each value as a new TEXT entity near the
//!   entity's bounding-box centre, stacking multiple lines vertically.

use cadcraft_color::Color;
use cadcraft_doc::{AttributeField, AttributeFieldKind, AttributeTableDef, EntityKind, Handle};
use cadcraft_geom::{Polyline, Vec2, Vec3};
use serde_json::{Value, json};

use super::machines::{SelOutcome, SelectPhase};
use super::*;
use crate::{Accept, EngineError, Input, Interactive, Prompt, Result, Session, Step};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("data.cod_to_num", "Point cod->num", run_cod_to_num)
            .menu(&["Data", "Point cod->num"])
            .interactive(|_| Ok(Box::new(CodToNumM::default())))
            .params(
                "{table: string, field: string, attr?: string (default \"COD\"), tolerance?: number (default 1e-6), \
             createIfMissing?: boolean (default false), handles?: [string] (default: current selection, must be \
             polylines)} → {updated}. For each targeted polyline, finds every block insert whose insertion point \
             touches it (within `tolerance`), reads that block's `attr` attribute, and writes the most frequent \
             value into `field` of the polyline's `table` (an existing attribute table, unless `createIfMissing` \
             is true, in which case a new one-field Character table is created on the fly; the polyline is \
             tagged with it if it didn't already carry one).",
            ),
        CommandSpec::new("data.extract_to_text", "Extraire donnée->texte", run_extract_to_text)
            .menu(&["Data", "Extraire donnée->texte"])
            .interactive(|_| Ok(Box::new(ExtractToTextM::default())))
            .params(
                "{fields: [{table: string, field: string}], layer: string, color?: integer (ACI, default 7), \
             height?: number (default 2.5), dx?: number (default 0), dy?: number (default 0), \
             handles?: [string] (default: current selection)} → {created}. For each targeted entity that \
             carries one of the requested tables, writes that field's value as a new TEXT entity at the \
             entity's bounding-box centre (offset by dx/dy), on `layer` (created if missing) in `color`. An \
             entity matching several requested fields gets one stacked text per match.",
            ),
    ]
}

/// "Select objects", "Table to write into", "Field to write into", "Block attribute to read",
/// then runs DATA.COD_TO_NUM.
#[derive(Default)]
struct CodToNumM {
    sel: SelectPhase,
    picked: Option<Vec<Handle>>,
    table: Option<String>,
    field: Option<String>,
}

impl Interactive for CodToNumM {
    fn name(&self) -> &'static str {
        "COD_TO_NUM"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        self.sel = SelectPhase::begin(s);
        if self.sel.done {
            self.picked = Some(self.sel.picked.clone());
        }
        Ok(Step::Continue)
    }
    fn prompt(&self, s: &Session) -> Prompt {
        if self.picked.is_none() {
            return self.sel.prompt();
        }
        if self.table.is_none() {
            let names: Vec<String> = s.doc().map(|d| d.attribute_tables.iter().map(|t| t.name.clone()).collect()).unwrap_or_default();
            let kws: Vec<&str> = names.iter().map(String::as_str).collect();
            return Prompt::new("Attribute table to write into", Accept::TEXT).kw(&kws);
        }
        if self.field.is_none() {
            let table = self.table.as_deref().unwrap_or("");
            let names: Vec<String> = s
                .doc()
                .ok()
                .and_then(|d| {
                    d.attribute_tables.iter().find(|t| t.name.eq_ignore_ascii_case(table)).map(|t| t.fields.iter().map(|f| f.name.clone()).collect())
                })
                .unwrap_or_default();
            let kws: Vec<&str> = names.iter().map(String::as_str).collect();
            return Prompt::new("Field to write the value into", Accept::TEXT).kw(&kws);
        }
        Prompt::new("Block attribute to read (Enter for COD)", Accept::TEXT).default("COD")
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if self.picked.is_none() {
            return match self.sel.feed(s, &i)? {
                SelOutcome::More => Ok(Step::Continue),
                SelOutcome::Empty => Ok(Step::Done),
                SelOutcome::Done(hs) => {
                    self.picked = Some(hs);
                    Ok(Step::Continue)
                }
            };
        }
        if self.table.is_none() {
            let t = match i {
                Input::Text(t) | Input::Keyword(t) => t.trim().to_string(),
                Input::Cancel => return Ok(Step::Cancel),
                _ => return Ok(Step::Continue),
            };
            if t.is_empty() {
                return Err(bad("data.cod_to_num", "enter a table name"));
            }
            self.table = Some(t);
            return Ok(Step::Continue);
        }
        if self.field.is_none() {
            let f = match i {
                Input::Text(t) | Input::Keyword(t) => t.trim().to_string(),
                Input::Cancel => return Ok(Step::Cancel),
                _ => return Ok(Step::Continue),
            };
            if f.is_empty() {
                return Err(bad("data.cod_to_num", "enter a field name"));
            }
            self.field = Some(f);
            return Ok(Step::Continue);
        }
        let attr = match i {
            Input::Enter => "COD".to_string(),
            Input::Text(t) => {
                let t = t.trim().to_string();
                if t.is_empty() { "COD".to_string() } else { t }
            }
            Input::Cancel => return Ok(Step::Cancel),
            _ => return Ok(Step::Continue),
        };
        let hs = self.picked.clone().unwrap_or_default();
        let spec = super::find_command("data.cod_to_num").ok_or_else(|| EngineError::UnknownCommand("data.cod_to_num".into()))?;
        let r = (spec.run)(
            s,
            &json!({
                "handles": hs.iter().map(|h| h.hex()).collect::<Vec<_>>(),
                "table": self.table.clone().unwrap_or_default(),
                "field": self.field.clone().unwrap_or_default(),
                "attr": attr,
            }),
        )?;
        if let Some(m) = r.get("message").and_then(Value::as_str) {
            s.echo(m.to_string());
        }
        s.set_selection(Vec::new());
        Ok(Step::Done)
    }
}

/// "Select objects", then repeated "Table to extract from"/"Field to extract" pairs (Enter on an
/// empty table name ends the list once at least one pair was entered), then "Destination layer",
/// "Text colour", "Text height", "X offset", "Y offset", then runs DATA.EXTRACT_TO_TEXT.
#[derive(Default)]
struct ExtractToTextM {
    sel: SelectPhase,
    picked: Option<Vec<Handle>>,
    fields: Vec<(String, String)>,
    pending_table: Option<String>,
    fields_done: bool,
    layer: Option<String>,
    color: Option<i32>,
    height: Option<f64>,
    dx: Option<f64>,
}

impl Interactive for ExtractToTextM {
    fn name(&self) -> &'static str {
        "EXTRACT_TO_TEXT"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        self.sel = SelectPhase::begin(s);
        if self.sel.done {
            self.picked = Some(self.sel.picked.clone());
        }
        Ok(Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        if self.picked.is_none() {
            return self.sel.prompt();
        }
        if !self.fields_done {
            return match &self.pending_table {
                None => {
                    let msg = if self.fields.is_empty() {
                        "Table to extract from".to_string()
                    } else {
                        format!("Table to extract from (Enter to finish, {} field(s) added)", self.fields.len())
                    };
                    Prompt { raw: true, ..Prompt::new(msg, Accept::TEXT) }
                }
                Some(t) => Prompt { raw: true, ..Prompt::new(format!("Field in '{t}' to extract"), Accept::TEXT) },
            };
        }
        if self.layer.is_none() {
            return Prompt { raw: true, ..Prompt::new("Destination layer", Accept::TEXT) };
        }
        if self.color.is_none() {
            return Prompt::new("Text colour (ACI)", Accept::TEXT).default("7");
        }
        if self.height.is_none() {
            return Prompt::new("Text height", Accept::TEXT).default("2.5");
        }
        if self.dx.is_none() {
            return Prompt::new("X offset from the shape centre", Accept::TEXT).default("0");
        }
        Prompt::new("Y offset from the shape centre", Accept::TEXT).default("0")
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if self.picked.is_none() {
            return match self.sel.feed(s, &i)? {
                SelOutcome::More => Ok(Step::Continue),
                SelOutcome::Empty => Ok(Step::Done),
                SelOutcome::Done(hs) => {
                    self.picked = Some(hs);
                    Ok(Step::Continue)
                }
            };
        }
        if !self.fields_done {
            return match self.pending_table.clone() {
                None => {
                    let t = match i {
                        Input::Enter => String::new(),
                        Input::Text(t) => t.trim().to_string(),
                        Input::Cancel => return Ok(Step::Cancel),
                        _ => return Ok(Step::Continue),
                    };
                    if t.is_empty() {
                        if self.fields.is_empty() {
                            return Err(bad("data.extract_to_text", "enter at least one table name"));
                        }
                        self.fields_done = true;
                    } else {
                        self.pending_table = Some(t);
                    }
                    Ok(Step::Continue)
                }
                Some(t) => {
                    let f = match i {
                        Input::Text(f) => f.trim().to_string(),
                        Input::Cancel => return Ok(Step::Cancel),
                        _ => return Ok(Step::Continue),
                    };
                    if f.is_empty() {
                        return Err(bad("data.extract_to_text", "enter a field name"));
                    }
                    self.fields.push((t, f));
                    self.pending_table = None;
                    Ok(Step::Continue)
                }
            };
        }
        if self.layer.is_none() {
            let l = match i {
                Input::Text(t) => t.trim().to_string(),
                Input::Cancel => return Ok(Step::Cancel),
                _ => return Ok(Step::Continue),
            };
            if l.is_empty() {
                return Err(bad("data.extract_to_text", "enter a destination layer name"));
            }
            self.layer = Some(l);
            return Ok(Step::Continue);
        }
        if self.color.is_none() {
            let c = match i {
                Input::Enter => 7,
                Input::Text(t) => t.trim().parse::<i32>().map_err(|_| bad("data.extract_to_text", "the colour must be a whole ACI number"))?,
                Input::Cancel => return Ok(Step::Cancel),
                _ => return Ok(Step::Continue),
            };
            self.color = Some(c);
            return Ok(Step::Continue);
        }
        if self.height.is_none() {
            let h = match i {
                Input::Enter => 2.5,
                Input::Text(t) => t.trim().parse::<f64>().map_err(|_| bad("data.extract_to_text", "the height must be a number"))?,
                Input::Cancel => return Ok(Step::Cancel),
                _ => return Ok(Step::Continue),
            };
            if h <= 0.0 {
                return Err(bad("data.extract_to_text", "the height must be positive"));
            }
            self.height = Some(h);
            return Ok(Step::Continue);
        }
        if self.dx.is_none() {
            let v = match i {
                Input::Enter => 0.0,
                Input::Text(t) => t.trim().parse::<f64>().map_err(|_| bad("data.extract_to_text", "the X offset must be a number"))?,
                Input::Cancel => return Ok(Step::Cancel),
                _ => return Ok(Step::Continue),
            };
            self.dx = Some(v);
            return Ok(Step::Continue);
        }
        let dy = match i {
            Input::Enter => 0.0,
            Input::Text(t) => t.trim().parse::<f64>().map_err(|_| bad("data.extract_to_text", "the Y offset must be a number"))?,
            Input::Cancel => return Ok(Step::Cancel),
            _ => return Ok(Step::Continue),
        };
        let hs = self.picked.clone().unwrap_or_default();
        let fields_json: Vec<Value> = self.fields.iter().map(|(t, f)| json!({ "table": t, "field": f })).collect();
        let spec = super::find_command("data.extract_to_text").ok_or_else(|| EngineError::UnknownCommand("data.extract_to_text".into()))?;
        let r = (spec.run)(
            s,
            &json!({
                "handles": hs.iter().map(|h| h.hex()).collect::<Vec<_>>(),
                "fields": fields_json,
                "layer": self.layer.clone().unwrap_or_default(),
                "color": self.color.unwrap_or(7),
                "height": self.height.unwrap_or(2.5),
                "dx": self.dx.unwrap_or(0.0),
                "dy": dy,
            }),
        )?;
        if let Some(m) = r.get("message").and_then(Value::as_str) {
            s.echo(m.to_string());
        }
        s.set_selection(Vec::new());
        Ok(Step::Done)
    }
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

    let create_if_missing = bool_or(p, "createIfMissing", false);
    if s.doc()?.attribute_table(&table).is_none() {
        if !create_if_missing {
            return Err(bad(CMD, format!("no attribute table named `{table}`")));
        }
        let def =
            AttributeTableDef { name: table.clone(), fields: vec![AttributeField { name: field.clone(), kind: AttributeFieldKind::Character }] };
        s.doc_mut()?.attribute_tables.push(def);
    }
    let d = s.doc()?;
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

#[cfg(test)]
mod tests {
    use cadcraft_doc::{Attrib, AttribModes, AttribProps, AttributeField, AttributeFieldKind, AttributeTableDef, Insert, LwPolyline, Text};

    use super::*;
    use crate::Input;

    fn pointtopo(x: f64, y: f64, cod: &str) -> Insert {
        let mut i = Insert {
            block: "PointTopo".to_string(),
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
            tag: "COD".to_string(),
            text: Text {
                insert: Vec3::new(x, y, 0.0),
                align_pt: None,
                height: 1.0,
                value: cod.to_string(),
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

    fn pv(x: f64, y: f64) -> cadcraft_geom::PolyVertex {
        cadcraft_geom::PolyVertex { p: Vec2::new(x, y), ..cadcraft_geom::PolyVertex::default() }
    }

    #[test]
    fn cod_to_num_interactive_picks_table_field_then_writes_the_majority_value() {
        let mut s = Session::new();
        s.doc_mut().unwrap().attribute_tables.push(AttributeTableDef {
            name: "Structure".to_string(),
            fields: vec![AttributeField { name: "Num".to_string(), kind: AttributeFieldKind::Character }],
        });
        s.add_entity(EntityKind::Insert(pointtopo(0.0, 0.0, "A"))).unwrap();
        let pl = LwPolyline { vertices: vec![pv(0.0, 0.0), pv(5.0, 0.0)], closed: false, const_width: 0.0, elevation: 0.0, plinegen: false };
        let h = s.add_entity(EntityKind::LwPolyline(pl)).unwrap();

        s.start("data.cod_to_num").unwrap();
        assert!(s.prompt_text().contains("Select"), "{}", s.prompt_text());
        s.input(Input::Pick(vec![h])).unwrap();
        s.input(Input::Enter).unwrap();
        assert!(s.prompt_text().contains("table"), "{}", s.prompt_text());
        s.input(Input::Text("Structure".into())).unwrap();
        assert!(s.prompt_text().contains("Field"), "{}", s.prompt_text());
        s.input(Input::Text("Num".into())).unwrap();
        assert!(s.prompt_text().contains("attribute"), "{}", s.prompt_text());
        s.input(Input::Enter).unwrap(); // default COD
        assert!(s.running.is_none(), "should finish");

        let (table, fields) = cadcraft_doc::import_attrs::fields(s.doc().unwrap().entity(h).unwrap(), super::attribute_table_appid()).unwrap();
        assert_eq!(table, "Structure");
        assert_eq!(fields.iter().find(|(k, _)| k == "Num").map(|(_, v)| v.as_str()), Some("A"));
    }

    #[test]
    fn extract_to_text_interactive_adds_one_field_then_creates_a_text() {
        let mut s = Session::new();
        s.doc_mut().unwrap().attribute_tables.push(AttributeTableDef {
            name: "Structure".to_string(),
            fields: vec![AttributeField { name: "Num".to_string(), kind: AttributeFieldKind::Character }],
        });
        let pl = LwPolyline {
            vertices: vec![pv(0.0, 0.0), pv(2.0, 0.0), pv(2.0, 2.0), pv(0.0, 2.0)],
            closed: true,
            const_width: 0.0,
            elevation: 0.0,
            plinegen: false,
        };
        let h = s.add_entity(EntityKind::LwPolyline(pl)).unwrap();
        s.execute("data.assign_table", &json!({ "handles": [h.hex()], "name": "Structure" })).unwrap();
        s.execute("data.set_attribute", &json!({ "handle": h.hex(), "key": "Num", "value": "7" })).unwrap();

        s.start("data.extract_to_text").unwrap();
        s.input(Input::Pick(vec![h])).unwrap();
        s.input(Input::Enter).unwrap(); // finish selection
        s.input(Input::Text("Structure".into())).unwrap(); // table
        s.input(Input::Text("Num".into())).unwrap(); // field
        s.input(Input::Enter).unwrap(); // finish fields list
        s.input(Input::Text("4TX".into())).unwrap(); // layer
        s.input(Input::Enter).unwrap(); // default color
        s.input(Input::Enter).unwrap(); // default height
        s.input(Input::Enter).unwrap(); // default dx
        s.input(Input::Enter).unwrap(); // default dy
        assert!(s.running.is_none(), "should finish");

        let texts: Vec<_> = s.doc().unwrap().model.iter().filter(|e| matches!(e.kind, EntityKind::Text(_))).collect();
        assert_eq!(texts.len(), 1);
        let EntityKind::Text(t) = &texts[0].kind else { unreachable!() };
        assert_eq!(t.value, "7");
        assert_eq!(texts[0].common.layer, "4TX");
    }
}
