//! "Data" commands: user-defined attribute tables ("table d'attributs", Covadis's term for
//! them) - a named schema of typed fields the person authors by hand (DATA.NEW_TABLE), then
//! attaches to entities (DATA.ASSIGN_TABLE) or detaches (DATA.REMOVE_TABLE).
//!
//! The *definitions* live on `Drawing::attribute_tables` (`cadcraft_doc::tables.rs`'s
//! `AttributeTableDef`); the per-entity assignment reuses the Shapefile import's own XDATA
//! convention (`cadcraft_doc::import_attrs`), under its own application id
//! (`ATTR_TABLE_APPID`), so it survives save/load and undo like any other entity data without a
//! second storage mechanism. An entity carries at most one table at a time (same one-table-per-
//! entity limit the Shapefile mechanism has) - assigning a new one replaces whatever was there.
//!
//! DATA.SET_ATTRIBUTE is the Properties palette's own way in (no menu entry), mirroring
//! `plan.set_shp_attribute` but checking the new value against the field's declared type first.
//! DATA.PICK_ENTITIES is the Assign Table dialog's "Select entities" button (no menu entry
//! either): it never touches the drawing, only leaves handles in [`Session::table_pick`].

use cadcraft_doc::{AttributeField, AttributeFieldKind, AttributeTableDef, Handle};
use serde_json::{Value, json};

use super::machines::SelectPhase;
use super::*;
use crate::{Input, Interactive, Prompt, Result, Session, Step};

pub const ATTR_TABLE_APPID: &str = cadcraft_doc::import_attrs::ATTR_TABLE_APPID;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("data.new_table", "Attribute Table...", run_new_table).menu(&["Data", "Attribute Table..."]).params(
            "{name: string, fields: [{name: string, kind: \"character\"|\"integer\"|\"decimal\"|\"date\"}]} \
                 → {name, fields}. Creates a new attribute-table schema (no entity carries it until \
                 `data.assign_table` attaches it).",
        ),
        CommandSpec::new("data.list_tables", "List Attribute Tables", run_list_tables)
            .params("{} → {tables: [{name, fields: [{name, kind}]}]}")
            .noundo(),
        CommandSpec::new("data.assign_table", "Assign Table...", run_assign_table).menu(&["Data", "Assign Table..."]).params(
            "{name: string (an existing attribute table), handles?: [string] (default: current \
             selection)} → {table, assigned}. Replaces any table already on the targeted entities, \
             with every field starting empty.",
        ),
        CommandSpec::new("data.remove_table", "Remove Table", run_remove_table)
            .menu(&["Data", "Remove Table"])
            .params("{handles?: [string] (default: current selection)} → {removed}")
            .interactive(|_| Ok(Box::new(super::machines::SelectRun::new("data.remove_table", "DATAREMOVETABLE")))),
        CommandSpec::new("data.set_attribute", "Set Table Attribute", run_set_attribute).params(
            "{handle: string, key: string, value: string} → {} (Properties palette only; errs if `value` doesn't match the field's declared type)",
        ),
        // No menu entry: only the Assign Table dialog's "Select entities" button starts this (it
        // hides the dialog, runs the pick, and reads the result back from `Session::table_pick`).
        CommandSpec::new("data.pick_entities", "Pick Entities", run_pick_entities)
            .params("{} (typed/menu only: DATA.PICK_ENTITIES prompts \"Select objects\") → sets Session::table_pick")
            .interactive(|_| Ok(Box::new(TablePickM::default()))),
        CommandSpec::new("data.search", "Search...", run_search).menu(&["Data", "Search..."]).noundo().params(
            "{query?: string, table?: string, field?: string} → {rows: [{handle, table, fields: \
             [{name, kind, value}]}]}. With no `table`, searches every tagged entity's fields; with \
             `table` but no `field`, searches that table's fields; with both, only that field. An \
             empty or missing `query` lists every row that matches the `table`/`field` filter.",
        ),
    ]
}

fn kind_from_str(cmd: &str, s: &str) -> Result<AttributeFieldKind> {
    match s {
        "character" => Ok(AttributeFieldKind::Character),
        "integer" => Ok(AttributeFieldKind::Integer),
        "decimal" => Ok(AttributeFieldKind::Decimal),
        "date" => Ok(AttributeFieldKind::Date),
        other => Err(bad(cmd, format!("unknown field type `{other}` (want character, integer, decimal or date)"))),
    }
}

fn kind_label(k: AttributeFieldKind) -> &'static str {
    match k {
        AttributeFieldKind::Character => "character",
        AttributeFieldKind::Integer => "integer",
        AttributeFieldKind::Decimal => "decimal",
        AttributeFieldKind::Date => "date",
    }
}

fn table_json(t: &AttributeTableDef) -> Value {
    json!({
        "name": t.name,
        "fields": t.fields.iter().map(|f| json!({ "name": f.name, "kind": kind_label(f.kind) })).collect::<Vec<_>>(),
    })
}

fn run_new_table(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "data.new_table";
    let name = str_param(p, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(CMD, "`name` is required"))?;
    if name.chars().count() > 255 || name.chars().any(|c| "<>/\\\":;?*|,=`".contains(c) || c.is_control()) {
        return Err(bad(CMD, "invalid table name"));
    }
    let raw_fields = p.get("fields").and_then(Value::as_array).ok_or_else(|| bad(CMD, "`fields` ([{name, kind}]) is required"))?;
    if raw_fields.is_empty() {
        return Err(bad(CMD, "a table needs at least one field"));
    }
    let mut fields = Vec::with_capacity(raw_fields.len());
    for f in raw_fields {
        let fname = str_param(f, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(CMD, "a field is missing its `name`"))?;
        if fields.iter().any(|existing: &AttributeField| existing.name.eq_ignore_ascii_case(fname)) {
            return Err(bad(CMD, format!("duplicate field name `{fname}`")));
        }
        let kind = kind_from_str(CMD, str_param(f, "kind").unwrap_or_default())?;
        fields.push(AttributeField { name: fname.to_string(), kind });
    }

    let d = s.doc_mut()?;
    if d.attribute_tables.iter().any(|t| t.name.eq_ignore_ascii_case(name)) {
        return Err(bad(CMD, format!("an attribute table named `{name}` already exists")));
    }
    let def = AttributeTableDef { name: name.to_string(), fields };
    let out = table_json(&def);
    d.attribute_tables.push(def);
    s.touch();
    Ok(out)
}

fn run_list_tables(s: &mut Session, _p: &Value) -> Result<Value> {
    let d = s.doc()?;
    Ok(json!({ "tables": d.attribute_tables.iter().map(table_json).collect::<Vec<_>>() }))
}

fn run_assign_table(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "data.assign_table";
    let name = str_param(p, "name").ok_or_else(|| bad(CMD, "`name` (an existing attribute table) is required"))?.to_string();
    let hs = targets(s, p)?;
    if hs.is_empty() {
        return Err(bad(CMD, "nothing selected"));
    }
    let d = s.doc()?;
    let def = d.attribute_table(&name).ok_or_else(|| bad(CMD, format!("no attribute table named `{name}`")))?.clone();
    let field_values: Vec<(String, String)> = def.fields.iter().map(|f| (f.name.clone(), String::new())).collect();
    let xdata = cadcraft_doc::import_attrs::tag_fields(ATTR_TABLE_APPID, &def.name, &field_values);

    let d = s.doc_mut()?;
    let mut assigned = 0usize;
    for h in &hs {
        if d.modify_entity(*h, |e| e.common.xdata = xdata.clone()).is_ok() {
            assigned += 1;
        }
    }
    s.touch();
    Ok(json!({ "table": def.name, "assigned": assigned }))
}

fn run_remove_table(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "data.remove_table";
    let hs = targets(s, p)?;
    if hs.is_empty() {
        return Err(bad(CMD, "nothing selected"));
    }
    let has_table: Vec<Handle> = {
        let d = s.doc()?;
        hs.iter().copied().filter(|h| d.entity(*h).is_some_and(|e| cadcraft_doc::import_attrs::fields(e, ATTR_TABLE_APPID).is_some())).collect()
    };
    let d = s.doc_mut()?;
    let mut removed = 0usize;
    for h in &has_table {
        if d.modify_entity(*h, |e| e.common.xdata.clear()).is_ok() {
            removed += 1;
        }
    }
    s.touch();
    Ok(json!({ "removed": removed }))
}

fn run_set_attribute(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "data.set_attribute";
    let handle = str_param(p, "handle").and_then(Handle::parse_hex).ok_or_else(|| bad(CMD, "`handle` is required"))?;
    let key = str_param(p, "key").ok_or_else(|| bad(CMD, "`key` is required"))?.to_string();
    let value = str_param(p, "value").unwrap_or_default().to_string();

    let d = s.doc()?;
    let e = d.entity(handle).ok_or_else(|| bad(CMD, "no such entity"))?;
    let (table, mut field_values) =
        cadcraft_doc::import_attrs::fields(e, ATTR_TABLE_APPID).ok_or_else(|| bad(CMD, "entity has no attribute table"))?;
    if let Some(def) = d.attribute_table(&table)
        && let Some(f) = def.fields.iter().find(|f| f.name == key)
        && !f.kind.validate(&value)
    {
        return Err(bad(CMD, format!("`{value}` is not a valid {} for `{key}`", kind_label(f.kind))));
    }
    match field_values.iter_mut().find(|(k, _)| *k == key) {
        Some((_, v)) => *v = value,
        None => field_values.push((key, value)),
    }
    let xdata = cadcraft_doc::import_attrs::tag_fields(ATTR_TABLE_APPID, &table, &field_values);

    s.doc_mut()?.modify_entity(handle, move |e| e.common.xdata = xdata).map_err(|e| bad(CMD, e.to_string()))?;
    s.touch();
    ok()
}

/// Caps how many rows `data.search` ever returns, so a query with no filter on a huge drawing
/// can't blow up the response (or the spreadsheet-like dialog rendering it).
const MAX_SEARCH_ROWS: usize = 5000;

fn run_search(s: &mut Session, p: &Value) -> Result<Value> {
    let table_filter = str_param(p, "table").map(str::to_string);
    let field_filter = str_param(p, "field").map(str::to_string);
    let query = str_param(p, "query").unwrap_or_default().trim().to_ascii_lowercase();

    let d = s.doc()?;
    let mut rows = Vec::new();
    let stores = std::iter::once(&d.model).chain(d.layouts.iter().map(|l| &l.entities));
    'entities: for store in stores {
        for e in store.iter() {
            let Some((table, field_values)) = cadcraft_doc::import_attrs::fields(e, ATTR_TABLE_APPID) else { continue };
            if table_filter.as_ref().is_some_and(|t| !t.eq_ignore_ascii_case(&table)) {
                continue;
            }
            let def = d.attribute_table(&table);
            let matches = query.is_empty()
                || field_values.iter().any(|(k, v)| {
                    (field_filter.is_none() || field_filter.as_deref().is_some_and(|f| f.eq_ignore_ascii_case(k)))
                        && v.to_ascii_lowercase().contains(&query)
                });
            if !matches {
                continue;
            }
            let fields: Vec<Value> = field_values
                .iter()
                .filter(|(k, _)| field_filter.is_none() || field_filter.as_deref().is_some_and(|f| f.eq_ignore_ascii_case(k)))
                .map(|(k, v)| {
                    let kind = def.and_then(|d| d.fields.iter().find(|f| f.name == *k)).map_or("character", |f| kind_label(f.kind));
                    json!({ "name": k, "kind": kind, "value": v })
                })
                .collect();
            rows.push(json!({ "handle": e.handle.hex(), "table": table, "fields": fields }));
            if rows.len() >= MAX_SEARCH_ROWS {
                break 'entities;
            }
        }
    }
    Ok(json!({ "rows": rows }))
}

/// DATA.PICK_ENTITIES's non-interactive form: nothing to do headlessly.
fn run_pick_entities(_s: &mut Session, _p: &Value) -> Result<Value> {
    Err(bad("data.pick_entities", "pick entities on screen (typed/menu only)"))
}

/// DATA.PICK_ENTITIES: a plain "Select objects" phase, left in [`Session::table_pick`] for the
/// Assign Table dialog to take (never touches the drawing).
#[derive(Default)]
struct TablePickM {
    sel: SelectPhase,
}

impl Interactive for TablePickM {
    fn name(&self) -> &'static str {
        "DATAPICKENTITIES"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        self.sel = SelectPhase::begin(s);
        if self.sel.done {
            s.table_pick = Some(self.sel.picked.clone());
            return Ok(Step::Done);
        }
        Ok(Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        self.sel.prompt()
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if matches!(i, Input::Cancel) {
            return Ok(Step::Cancel);
        }
        match self.sel.feed(s, &i)? {
            super::machines::SelOutcome::More => Ok(Step::Continue),
            super::machines::SelOutcome::Done(hs) => {
                s.table_pick = Some(hs);
                Ok(Step::Done)
            }
            super::machines::SelOutcome::Empty => Ok(Step::Cancel),
        }
    }
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

    fn make_table(s: &mut Session) {
        s.execute(
            "data.new_table",
            &json!({ "name": "Parcelles", "fields": [{ "name": "Proprietaire", "kind": "character" }, { "name": "Surface", "kind": "decimal" }] }),
        )
        .expect("should create the table");
    }

    #[test]
    fn new_table_rejects_a_duplicate_name_or_an_empty_field_list() {
        let mut s = session_with_doc();
        make_table(&mut s);
        assert!(s.execute("data.new_table", &json!({ "name": "Parcelles", "fields": [{ "name": "X", "kind": "character" }] })).is_err());
        assert!(s.execute("data.new_table", &json!({ "name": "Autre", "fields": [] })).is_err());
        assert!(s.execute("data.new_table", &json!({ "name": "Autre", "fields": [{ "name": "A" }] })).is_err(), "missing kind");
    }

    #[test]
    fn list_tables_reports_what_was_created() {
        let mut s = session_with_doc();
        make_table(&mut s);
        let out = s.execute("data.list_tables", &json!({})).unwrap();
        let tables = out["tables"].as_array().unwrap();
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0]["name"], "Parcelles");
        assert_eq!(tables[0]["fields"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn assign_table_tags_the_selected_entities_with_empty_fields() {
        let mut s = session_with_doc();
        make_table(&mut s);
        let h = s.execute("point", &json!({ "at": [0.0, 0.0] })).unwrap()["handles"][0].as_str().unwrap().to_string();
        let out = s.execute("data.assign_table", &json!({ "name": "parcelles", "handles": [h.clone()] })).expect("should assign");
        assert_eq!(out["assigned"].as_u64(), Some(1));

        let e = s.doc().unwrap().entity(cadcraft_doc::Handle::parse_hex(&h).unwrap()).unwrap();
        let (table, fields) = cadcraft_doc::import_attrs::fields(e, ATTR_TABLE_APPID).expect("should be tagged");
        assert_eq!(table, "Parcelles");
        assert_eq!(fields, vec![("Proprietaire".to_string(), String::new()), ("Surface".to_string(), String::new())]);
    }

    #[test]
    fn assign_table_rejects_an_unknown_table_or_an_empty_selection() {
        let mut s = session_with_doc();
        make_table(&mut s);
        let h = s.execute("point", &json!({ "at": [0.0, 0.0] })).unwrap()["handles"][0].as_str().unwrap().to_string();
        assert!(s.execute("data.assign_table", &json!({ "name": "Nope", "handles": [h] })).is_err());
        assert!(s.execute("data.assign_table", &json!({ "name": "Parcelles", "handles": [] })).is_err());
    }

    #[test]
    fn set_attribute_validates_the_fields_declared_type() {
        let mut s = session_with_doc();
        make_table(&mut s);
        let h = s.execute("point", &json!({ "at": [0.0, 0.0] })).unwrap()["handles"][0].as_str().unwrap().to_string();
        s.execute("data.assign_table", &json!({ "name": "Parcelles", "handles": [h.clone()] })).unwrap();

        assert!(s.execute("data.set_attribute", &json!({ "handle": h, "key": "Surface", "value": "not a number" })).is_err());
        s.execute("data.set_attribute", &json!({ "handle": h.clone(), "key": "Surface", "value": "12.5" })).expect("valid decimal");
        s.execute("data.set_attribute", &json!({ "handle": h.clone(), "key": "Proprietaire", "value": "Dupont" }))
            .expect("character accepts anything");

        let e = s.doc().unwrap().entity(cadcraft_doc::Handle::parse_hex(&h).unwrap()).unwrap();
        let (_, fields) = cadcraft_doc::import_attrs::fields(e, ATTR_TABLE_APPID).unwrap();
        assert!(fields.contains(&("Surface".to_string(), "12.5".to_string())));
        assert!(fields.contains(&("Proprietaire".to_string(), "Dupont".to_string())));
    }

    #[test]
    fn remove_table_clears_only_entities_that_had_one() {
        let mut s = session_with_doc();
        make_table(&mut s);
        let h1 = s.execute("point", &json!({ "at": [0.0, 0.0] })).unwrap()["handles"][0].as_str().unwrap().to_string();
        let h2 = s.execute("point", &json!({ "at": [1.0, 1.0] })).unwrap()["handles"][0].as_str().unwrap().to_string();
        s.execute("data.assign_table", &json!({ "name": "Parcelles", "handles": [h1.clone()] })).unwrap();

        let out = s.execute("data.remove_table", &json!({ "handles": [h1.clone(), h2.clone()] })).expect("should run");
        assert_eq!(out["removed"].as_u64(), Some(1));
        let e1 = s.doc().unwrap().entity(cadcraft_doc::Handle::parse_hex(&h1).unwrap()).unwrap();
        assert!(cadcraft_doc::import_attrs::fields(e1, ATTR_TABLE_APPID).is_none());
    }

    #[test]
    fn pick_entities_sets_the_session_table_pick_from_a_selection() {
        let mut s = session_with_doc();
        let h = s.execute("point", &json!({ "at": [0.0, 0.0] })).unwrap()["handles"][0].as_str().unwrap().to_string();
        let handle = cadcraft_doc::Handle::parse_hex(&h).unwrap();
        assert!(s.execute("data.pick_entities", &json!({})).is_err(), "headless call can't pick");
        s.start("data.pick_entities").expect("should start");
        s.input(crate::Input::Pick(vec![handle])).unwrap();
        s.input(crate::Input::Enter).unwrap();
        assert!(s.running.is_none());
        assert_eq!(s.table_pick, Some(vec![handle]));
    }
}
