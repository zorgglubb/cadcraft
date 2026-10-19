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
