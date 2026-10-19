//! The Data menu's two dialogs: Attribute Table... (DATA.NEW_TABLE, builds a field list locally
//! before creating the schema in one call) and Assign Table... (DATA.ASSIGN_TABLE, picks a table
//! from the existing ones then an entity selection, the same hide-while-picking/[`resume_assign`]
//! pattern as [`crate::carroyagedlg`]). DATA.REMOVE_TABLE needs neither: it runs directly off
//! the current selection, like LAYOFF or LAYCUR (`cmd::machines::SelectRun`).

use egui::vec2;
use serde_json::{Value, json};

use crate::CadApp;

const KINDS: [(&str, &str); 4] = [("character", "Character"), ("integer", "Integer"), ("decimal", "Decimal"), ("date", "Date")];

fn kind_label(kind: &str) -> &'static str {
    KINDS.iter().find(|(k, _)| *k == kind).map_or("Character", |(_, l)| *l)
}

// ---------------------------------------------------------------------------------------------
// Attribute Table... (DATA.NEW_TABLE)
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Default)]
pub struct NewTableState {
    name: String,
    /// (field name, kind key from [`KINDS`]).
    fields: Vec<(String, String)>,
}

pub fn open_new(app: &mut CadApp) {
    app.new_table_dlg = Some(NewTableState::default());
    app.ui.dialog = Some("newtable".into());
}

pub fn new_table_dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let Some(mut st) = app.new_table_dlg.clone() else {
        *open = false;
        return;
    };

    let (mut create, mut close) = (false, false);
    egui::Window::new(crate::tl!("Attribute Table...")).open(open).resizable(true).collapsible(false).default_size(vec2(360.0, 400.0)).show(
        ctx,
        |ui| {
            ui.horizontal(|ui| {
                ui.label(crate::tl!("Table name"));
                ui.text_edit_singleline(&mut st.name);
            });
            ui.separator();
            ui.label(crate::tl!("Fields"));
            let mut remove = None;
            egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                for (i, (fname, kind)) in st.fields.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(fname);
                        egui::ComboBox::from_id_salt(("cc_table_field_kind", i)).selected_text(crate::i18n::t(kind_label(kind))).show_ui(ui, |ui| {
                            for (k, label) in KINDS {
                                ui.selectable_value(kind, k.to_string(), crate::i18n::t(label));
                            }
                        });
                        if ui.button("✕").clicked() {
                            remove = Some(i);
                        }
                    });
                }
            });
            if let Some(i) = remove {
                st.fields.remove(i);
            }
            if ui.button(crate::tl!("Add field")).clicked() {
                st.fields.push((String::new(), "character".to_string()));
            }
            ui.separator();
            let ready = !st.name.trim().is_empty() && !st.fields.is_empty() && st.fields.iter().all(|(n, _)| !n.trim().is_empty());
            ui.horizontal(|ui| {
                create = ui.add_enabled(ready, egui::Button::new(crate::tl!("Create"))).clicked();
                close = ui.button(crate::tl!("Close")).clicked();
            });
        },
    );
    if close {
        *open = false;
    }

    if create {
        let params = json!({
            "name": st.name.trim(),
            "fields": st.fields.iter().map(|(n, k)| json!({ "name": n.trim(), "kind": k })).collect::<Vec<_>>(),
        });
        let r = app.run("data.new_table", params);
        app.session.echo(match &r {
            Ok(v) => crate::tf!("Attribute table {name} created", name = v.get("name").and_then(Value::as_str).unwrap_or_default()),
            Err(e) => e.clone(),
        });
        if r.is_ok() {
            *open = false;
        }
    }
    if *open {
        app.new_table_dlg = Some(st);
    } else {
        app.new_table_dlg = None;
    }
}

// ---------------------------------------------------------------------------------------------
// Assign Table... (DATA.ASSIGN_TABLE)
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Default)]
pub struct AssignTableState {
    selected: Option<String>,
    handles: Vec<cadcraft_doc::Handle>,
    picking: bool,
}

pub fn open_assign(app: &mut CadApp) {
    app.assign_table_dlg = Some(AssignTableState::default());
    app.ui.dialog = Some("assigntable".into());
}

fn start_pick(app: &mut CadApp) {
    let Some(st) = app.assign_table_dlg.as_mut() else { return };
    st.picking = true;
    app.session.table_pick = None;
    app.ui.dialog = None;
    app.start("data.pick_entities");
}

/// Every frame: once a DATA.PICK_ENTITIES the dialog started has ended, take its handles and
/// bring the dialog back (unchanged when the pick was cancelled).
pub fn resume_assign(app: &mut CadApp) {
    if !app.assign_table_dlg.as_ref().is_some_and(|s| s.picking) {
        return;
    }
    if app.session.running.is_some() {
        return;
    }
    let picked = app.session.table_pick.take();
    let Some(st) = app.assign_table_dlg.as_mut() else { return };
    st.picking = false;
    if let Some(hs) = picked {
        st.handles = hs;
    }
    app.ui.dialog = Some("assigntable".into());
}

pub fn assign_table_dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let Some(mut st) = app.assign_table_dlg.clone() else {
        *open = false;
        return;
    };

    let tables: Vec<String> = app
        .run("data.list_tables", json!({}))
        .ok()
        .and_then(|v| v.get("tables").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|t| t.get("name").and_then(Value::as_str).map(str::to_string))
        .collect();

    let (mut select, mut assign, mut close) = (false, false, false);
    egui::Window::new(crate::tl!("Assign Table...")).open(open).resizable(true).collapsible(false).default_size(vec2(320.0, 360.0)).show(ctx, |ui| {
        ui.label(crate::tl!("Table"));
        egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
            if tables.is_empty() {
                ui.label(crate::tl!("No attribute table yet."));
            }
            for name in &tables {
                if ui.selectable_label(st.selected.as_deref() == Some(name.as_str()), name).clicked() {
                    st.selected = Some(name.clone());
                }
            }
        });
        ui.separator();
        ui.horizontal(|ui| {
            select = ui.button(crate::tl!("Select entities")).clicked();
            let count_label: String = if st.handles.is_empty() {
                crate::tl!("No entity selected yet.").to_string()
            } else {
                crate::tf!("{n} entity(ies) selected", n = st.handles.len())
            };
            ui.label(count_label);
        });
        ui.separator();
        ui.horizontal(|ui| {
            assign = ui.add_enabled(st.selected.is_some() && !st.handles.is_empty(), egui::Button::new(crate::tl!("Assign"))).clicked();
            close = ui.button(crate::tl!("Close")).clicked();
        });
    });
    if close {
        *open = false;
    }

    if select {
        app.assign_table_dlg = Some(st.clone());
        start_pick(app);
        return;
    }
    if assign && let Some(name) = st.selected.clone() {
        let handles: Vec<String> = st.handles.iter().map(|h| h.hex()).collect();
        let r = app.run("data.assign_table", json!({ "name": name, "handles": handles }));
        app.session.echo(match &r {
            Ok(v) => crate::tf!(
                "Table {table} assigned to {n} entity(ies)",
                table = v.get("table").and_then(Value::as_str).unwrap_or_default(),
                n = v.get("assigned").and_then(Value::as_u64).unwrap_or(0)
            ),
            Err(e) => e.clone(),
        });
        if r.is_ok() {
            *open = false;
        }
    }
    if *open {
        app.assign_table_dlg = Some(st);
    } else {
        app.assign_table_dlg = None;
    }
}

// ---------------------------------------------------------------------------------------------
// Search... (DATA.SEARCH)
// ---------------------------------------------------------------------------------------------

/// One matched row: an entity plus the fields DATA.SEARCH returned for it (already narrowed to
/// the chosen table/field, if any).
#[derive(Clone)]
struct SearchRow {
    handle: cadcraft_doc::Handle,
    table: String,
    fields: Vec<(String, String)>,
}

#[derive(Clone, Default)]
pub struct SearchState {
    query: String,
    /// Empty means "every table".
    table: String,
    /// Empty means "every field"; only meaningful once a table is chosen.
    field: String,
    rows: Vec<SearchRow>,
    checked: std::collections::HashSet<cadcraft_doc::Handle>,
}

pub fn open_search(app: &mut CadApp) {
    let mut st = SearchState::default();
    refresh_search(app, &mut st);
    app.search_dlg = Some(st);
    app.ui.dialog = Some("datasearch".into());
}

fn refresh_search(app: &mut CadApp, st: &mut SearchState) {
    let params = json!({
        "query": st.query,
        "table": if st.table.is_empty() { Value::Null } else { Value::String(st.table.clone()) },
        "field": if st.field.is_empty() { Value::Null } else { Value::String(st.field.clone()) },
    });
    st.rows = app
        .run("data.search", params)
        .ok()
        .and_then(|v| v.get("rows").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|r| {
            let handle = cadcraft_doc::Handle::parse_hex(r.get("handle")?.as_str()?)?;
            let table = r.get("table")?.as_str()?.to_string();
            let fields = r
                .get("fields")?
                .as_array()?
                .iter()
                .filter_map(|f| Some((f.get("name")?.as_str()?.to_string(), f.get("value")?.as_str().unwrap_or_default().to_string())))
                .collect();
            Some(SearchRow { handle, table, fields })
        })
        .collect();
    st.checked.retain(|h| st.rows.iter().any(|r| r.handle == *h));
}

/// Pushes `st.checked` into the drawing's current selection, so clicking result rows selects the
/// matching entities exactly like clicking them on screen would.
fn sync_selection(app: &mut CadApp, st: &SearchState) {
    let handles: Vec<String> = st.checked.iter().map(|h| h.hex()).collect();
    let _ = app.run("select", json!({ "handles": handles }));
}

pub fn search_dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let Some(mut st) = app.search_dlg.clone() else {
        *open = false;
        return;
    };

    let tables: Vec<String> = app
        .run("data.list_tables", json!({}))
        .ok()
        .and_then(|v| v.get("tables").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|t| t.get("name").and_then(Value::as_str).map(str::to_string))
        .collect();
    let fields: Vec<String> = app
        .run("data.list_tables", json!({}))
        .ok()
        .and_then(|v| v.get("tables").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .iter()
        .find(|t| t.get("name").and_then(Value::as_str) == Some(st.table.as_str()))
        .and_then(|t| t.get("fields").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|f| f.get("name").and_then(Value::as_str).map(str::to_string))
        .collect();

    let mut changed = false;
    let mut edits: Vec<(cadcraft_doc::Handle, String, String)> = Vec::new();
    let mut close = false;
    egui::Window::new(crate::tl!("Search...")).open(open).resizable(true).collapsible(false).default_size(vec2(560.0, 420.0)).show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.label(crate::tl!("Search"));
            changed |= ui.text_edit_singleline(&mut st.query).changed();
        });
        ui.horizontal(|ui| {
            ui.label(crate::tl!("Table"));
            let table_label: String = if st.table.is_empty() { crate::i18n::t("All tables").to_string() } else { st.table.clone() };
            egui::ComboBox::from_id_salt("cc_search_table").selected_text(table_label).show_ui(ui, |ui| {
                if ui.selectable_label(st.table.is_empty(), crate::tl!("All tables")).clicked() && !st.table.is_empty() {
                    st.table.clear();
                    st.field.clear();
                    changed = true;
                }
                for name in &tables {
                    if ui.selectable_label(&st.table == name, name).clicked() && &st.table != name {
                        st.table = name.clone();
                        st.field.clear();
                        changed = true;
                    }
                }
            });
            ui.add_enabled_ui(!st.table.is_empty(), |ui| {
                ui.label(crate::tl!("Field"));
                let field_label: String = if st.field.is_empty() { crate::i18n::t("All fields").to_string() } else { st.field.clone() };
                egui::ComboBox::from_id_salt("cc_search_field").selected_text(field_label).show_ui(ui, |ui| {
                    if ui.selectable_label(st.field.is_empty(), crate::tl!("All fields")).clicked() && !st.field.is_empty() {
                        st.field.clear();
                        changed = true;
                    }
                    for name in &fields {
                        if ui.selectable_label(&st.field == name, name).clicked() && &st.field != name {
                            st.field = name.clone();
                            changed = true;
                        }
                    }
                });
            });
        });
        ui.separator();
        egui::ScrollArea::both().max_height(280.0).show(ui, |ui| {
            egui::Grid::new("cc_search_grid").striped(true).num_columns(3 + fields.len().max(1)).show(ui, |ui| {
                ui.label(crate::tl!("Select"));
                ui.label(crate::tl!("Table"));
                if st.table.is_empty() || fields.is_empty() {
                    ui.label(crate::tl!("Field"));
                    ui.label(crate::tl!("Value"));
                } else {
                    for name in &fields {
                        ui.label(name);
                    }
                }
                ui.end_row();
                for row in st.rows.clone() {
                    let mut checked = st.checked.contains(&row.handle);
                    if ui.checkbox(&mut checked, "").changed() {
                        if checked {
                            st.checked.insert(row.handle);
                        } else {
                            st.checked.remove(&row.handle);
                        }
                        changed = true;
                    }
                    ui.label(&row.table);
                    if st.table.is_empty() || fields.is_empty() {
                        for (k, v) in &row.fields {
                            ui.label(k);
                            let id = ui.id().with(("cc_search_val", row.handle, k.as_str()));
                            if let Some(nv) = crate::palettes::edit_field(ui, id, v.clone()) {
                                edits.push((row.handle, k.clone(), nv));
                            }
                        }
                    } else {
                        for name in &fields {
                            let v = row.fields.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone()).unwrap_or_default();
                            let id = ui.id().with(("cc_search_val", row.handle, name.as_str()));
                            if let Some(nv) = crate::palettes::edit_field(ui, id, v) {
                                edits.push((row.handle, name.clone(), nv));
                            }
                        }
                    }
                    ui.end_row();
                }
            });
        });
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(crate::tf!("{n} row(s)", n = st.rows.len()));
            close = ui.button(crate::tl!("Close")).clicked();
        });
    });
    if close {
        *open = false;
    }

    for (handle, key, value) in edits {
        let r = app.run("data.set_attribute", json!({ "handle": handle.hex(), "key": key, "value": value }));
        if let Err(e) = r {
            app.session.echo(e);
        }
        changed = true;
    }
    if changed {
        refresh_search(app, &mut st);
        sync_selection(app, &st);
    }
    if *open {
        app.search_dlg = Some(st);
    } else {
        app.search_dlg = None;
    }
}

#[cfg(test)]
mod tests {
    use cadcraft_engine::{Input, Session};
    use serde_json::json;

    use super::*;
    use crate::Services;

    fn app_with_table() -> CadApp {
        let mut s = Session::new();
        s.execute("new", &json!({})).unwrap();
        s.execute("data.new_table", &json!({ "name": "Parcelles", "fields": [{ "name": "Nom", "kind": "character" }] })).unwrap();
        CadApp::new(s, Services::default())
    }

    #[test]
    fn new_table_dialog_creates_the_table() {
        let mut app = app_with_table();
        open_new(&mut app);
        let mut st = app.new_table_dlg.clone().unwrap();
        st.name = "Reseaux".into();
        st.fields.push(("Diametre".into(), "integer".into()));
        app.new_table_dlg = Some(st.clone());
        let params = json!({
            "name": st.name.trim(),
            "fields": st.fields.iter().map(|(n, k)| json!({ "name": n.trim(), "kind": k })).collect::<Vec<_>>(),
        });
        let r = app.run("data.new_table", params);
        assert!(r.is_ok(), "{r:?}");
    }

    #[test]
    fn assign_table_select_entities_hides_the_dialog_and_brings_it_back() {
        let mut app = app_with_table();
        let h = app.session.execute("point", &json!({ "at": [0.0, 0.0] })).unwrap()["handles"][0].as_str().unwrap().to_string();
        let handle = cadcraft_doc::Handle::parse_hex(&h).unwrap();

        open_assign(&mut app);
        start_pick(&mut app);
        assert_eq!(app.ui.dialog, None, "hidden while picking");

        app.session.input(Input::Pick(vec![handle])).unwrap();
        app.session.input(Input::Enter).unwrap();
        resume_assign(&mut app);
        assert_eq!(app.ui.dialog, Some("assigntable".into()));
        let st = app.assign_table_dlg.expect("state kept");
        assert_eq!(st.handles, vec![handle]);
    }

    #[test]
    fn assign_table_runs_the_command_with_the_chosen_table_and_handles() {
        let mut app = app_with_table();
        let h = app.session.execute("point", &json!({ "at": [0.0, 0.0] })).unwrap()["handles"][0].as_str().unwrap().to_string();
        let handle = cadcraft_doc::Handle::parse_hex(&h).unwrap();
        open_assign(&mut app);
        let mut st = app.assign_table_dlg.clone().unwrap();
        st.selected = Some("Parcelles".into());
        st.handles = vec![handle];
        app.assign_table_dlg = Some(st);
        let r = app.run("data.assign_table", json!({ "name": "Parcelles", "handles": [handle.hex()] }));
        assert!(r.is_ok(), "{r:?}");
    }
}
