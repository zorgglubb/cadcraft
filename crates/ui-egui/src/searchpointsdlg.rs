//! Topo ▸ Search Points...: `topo.search_points` opened with no parameters - a spreadsheet-like
//! lookup across every block reference carrying MAT/COD/ALT attributes anywhere in the drawing
//! (any layer, any block name - CADCRAFT_TCPOINT by default, but a custom name from
//! `topo.import_points`'s own `block` parameter or a point symbol counts too). Mirrors
//! [`crate::tabledlg::search_dialog`] (Data ▸ Search...), which only covers user-defined attribute
//! tables and never sees a topo point's own block attributes.

use egui::vec2;
use serde_json::{Value, json};

use crate::CadApp;

/// One matched TCPOINT reference, already resolved from `topo.search_points`'s JSON rows.
#[derive(Clone)]
struct Row {
    handle: cadcraft_doc::Handle,
    layer: String,
    mat: String,
    cod: String,
    alt: String,
    x: f64,
    y: f64,
    z: f64,
}

#[derive(Clone, Default)]
pub struct State {
    query: String,
    /// "" (any field), "MAT", "COD" or "ALT".
    field: String,
    rows: Vec<Row>,
    checked: std::collections::HashSet<cadcraft_doc::Handle>,
}

pub fn open(app: &mut CadApp) {
    let mut st = State::default();
    refresh(app, &mut st);
    app.search_points_dlg = Some(st);
    app.ui.dialog = Some("searchpoints".into());
}

fn refresh(app: &mut CadApp, st: &mut State) {
    let params = json!({ "query": st.query, "field": if st.field.is_empty() { Value::Null } else { Value::String(st.field.clone()) } });
    st.rows = app
        .run("topo.search_points", params)
        .ok()
        .and_then(|v| v.get("rows").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|r| {
            let str_field = |k: &str| r.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
            let num_field = |k: &str| r.get(k).and_then(Value::as_f64).unwrap_or(0.0);
            Some(Row {
                handle: cadcraft_doc::Handle::parse_hex(r.get("handle")?.as_str()?)?,
                layer: str_field("layer"),
                mat: str_field("mat"),
                cod: str_field("cod"),
                alt: str_field("alt"),
                x: num_field("x"),
                y: num_field("y"),
                z: num_field("z"),
            })
        })
        .collect();
    st.checked.retain(|h| st.rows.iter().any(|r| r.handle == *h));
}

/// Pushes `st.checked` into the drawing's current selection, so checking result rows selects the
/// matching points exactly like clicking them on screen would.
fn sync_selection(app: &mut CadApp, st: &State) {
    let handles: Vec<String> = st.checked.iter().map(|h| h.hex()).collect();
    let _ = app.run("select", json!({ "handles": handles }));
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let Some(mut st) = app.search_points_dlg.clone() else {
        *open = false;
        return;
    };

    let mut changed = false;
    let mut edits: Vec<(cadcraft_doc::Handle, &'static str, String)> = Vec::new();
    let mut close = false;
    egui::Window::new(crate::tl!("Search Points...")).open(open).resizable(true).collapsible(false).default_size(vec2(680.0, 420.0)).show(
        ctx,
        |ui| {
            ui.horizontal(|ui| {
                ui.label(crate::tl!("Search"));
                changed |= ui.text_edit_singleline(&mut st.query).changed();
                ui.label(crate::tl!("Field"));
                let field_label: String = if st.field.is_empty() { crate::i18n::t("All fields").to_string() } else { st.field.clone() };
                egui::ComboBox::from_id_salt("cc_search_points_field").selected_text(field_label).show_ui(ui, |ui| {
                    if ui.selectable_label(st.field.is_empty(), crate::tl!("All fields")).clicked() && !st.field.is_empty() {
                        st.field.clear();
                        changed = true;
                    }
                    for name in ["MAT", "COD", "ALT"] {
                        if ui.selectable_label(st.field == name, name).clicked() && st.field != name {
                            st.field = name.to_string();
                            changed = true;
                        }
                    }
                });
            });
            ui.separator();
            // Resizable columns, with Z soaking up whatever width a window resize adds or
            // removes - see the longer rationale on the equivalent table in
            // `crate::tabledlg::search_dialog`.
            use egui_extras::{Column, TableBuilder};
            TableBuilder::new(ui)
                .striped(true)
                .resizable(true)
                .max_scroll_height(300.0)
                .column(Column::auto().at_least(24.0).resizable(false))
                .column(Column::initial(100.0).at_least(50.0))
                .column(Column::initial(90.0).at_least(40.0))
                .column(Column::initial(90.0).at_least(40.0))
                .column(Column::initial(80.0).at_least(40.0))
                .column(Column::initial(90.0).at_least(40.0))
                .column(Column::initial(90.0).at_least(40.0))
                .column(Column::remainder().at_least(40.0).clip(true))
                .header(20.0, |mut header| {
                    header.col(|ui| {
                        ui.label(crate::tl!("Select"));
                    });
                    header.col(|ui| {
                        ui.label(crate::tl!("Layer"));
                    });
                    header.col(|ui| {
                        ui.label("MAT");
                    });
                    header.col(|ui| {
                        ui.label("COD");
                    });
                    header.col(|ui| {
                        ui.label("ALT");
                    });
                    header.col(|ui| {
                        ui.label("X");
                    });
                    header.col(|ui| {
                        ui.label("Y");
                    });
                    header.col(|ui| {
                        ui.label("Z");
                    });
                })
                .body(|body| {
                    body.rows(22.0, st.rows.len(), |mut table_row| {
                        let row = st.rows[table_row.index()].clone();
                        let mut checked = st.checked.contains(&row.handle);
                        table_row.col(|ui| {
                            if ui.checkbox(&mut checked, "").changed() {
                                if checked {
                                    st.checked.insert(row.handle);
                                } else {
                                    st.checked.remove(&row.handle);
                                }
                                changed = true;
                            }
                        });
                        table_row.col(|ui| {
                            ui.label(&row.layer);
                        });
                        for (tag, v) in [("MAT", &row.mat), ("COD", &row.cod), ("ALT", &row.alt)] {
                            table_row.col(|ui| {
                                let id = ui.id().with(("cc_search_points_val", row.handle, tag));
                                if let Some(nv) = crate::palettes::edit_field(ui, id, v.clone()) {
                                    edits.push((row.handle, tag, nv));
                                }
                            });
                        }
                        table_row.col(|ui| {
                            ui.label(format!("{:.3}", row.x));
                        });
                        table_row.col(|ui| {
                            ui.label(format!("{:.3}", row.y));
                        });
                        table_row.col(|ui| {
                            ui.label(format!("{:.3}", row.z));
                        });
                    });
                });
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(crate::tf!("{n} point(s)", n = st.rows.len()));
                close = ui.button(crate::tl!("Close")).clicked();
            });
        },
    );
    if close {
        *open = false;
    }

    for (handle, tag, value) in edits {
        let mut values = serde_json::Map::new();
        values.insert(tag.to_string(), Value::String(value));
        let r = app.run("attedit", json!({ "handle": handle.hex(), "values": Value::Object(values) }));
        if let Err(e) = r {
            app.session.echo(e);
        }
        changed = true;
    }
    if changed {
        refresh(app, &mut st);
        sync_selection(app, &st);
    }
    if *open {
        app.search_points_dlg = Some(st);
    } else {
        app.search_points_dlg = None;
    }
}

#[cfg(test)]
mod tests {
    use cadcraft_engine::Session;
    use serde_json::json;

    use super::*;
    use crate::Services;

    fn with_two_points() -> CadApp {
        let mut s = Session::new();
        s.execute("new", &json!({})).unwrap();
        // `std::process::id()` alone collides between tests running concurrently as separate
        // threads of the same test binary (same pid); a thread-local counter on top keeps every
        // call's paths unique even when `with_two_points` runs on two threads at once.
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = |n: &str| std::env::temp_dir().join(format!("cadcraft-search-points-{n}-{}-{unique}.csv", std::process::id()));
        let (p1, p2) = (path("a"), path("b"));
        std::fs::write(&p1, "ID;X;Y;Z;CODE\nP1;10.0;20.0;5.0;BORNE\n").unwrap();
        std::fs::write(&p2, "ID;X;Y;Z;CODE\nP2;30.0;40.0;6.0;REGARD\n").unwrap();
        s.execute("topo.import_points", &json!({ "path": p1.to_string_lossy(), "layer": "BORNES" })).unwrap();
        s.execute("topo.import_points", &json!({ "path": p2.to_string_lossy() })).unwrap();
        let _ = std::fs::remove_file(&p1);
        let _ = std::fs::remove_file(&p2);
        CadApp::new(s, Services::default())
    }

    #[test]
    fn opening_lists_every_tcpoint_across_layers() {
        let mut app = with_two_points();
        open(&mut app);
        let st = app.search_points_dlg.as_ref().unwrap();
        assert_eq!(st.rows.len(), 2, "{:?}", st.rows.iter().map(|r| &r.mat).collect::<Vec<_>>());
        assert!(st.rows.iter().any(|r| r.layer == "BORNES" && r.mat == "P1" && r.cod == "BORNE"));
        assert!(st.rows.iter().any(|r| r.layer == "0" && r.mat == "P2" && r.cod == "REGARD"));
    }

    #[test]
    fn a_query_narrows_the_rows_and_checking_one_selects_it() {
        let mut app = with_two_points();
        open(&mut app);
        let mut st = app.search_points_dlg.clone().unwrap();
        st.query = "regard".into();
        app.search_points_dlg = Some(st);
        let mut st = app.search_points_dlg.clone().unwrap();
        refresh(&mut app, &mut st);
        assert_eq!(st.rows.len(), 1);
        assert_eq!(st.rows[0].mat, "P2");

        st.checked.insert(st.rows[0].handle);
        sync_selection(&mut app, &st);
        let sel = app.session.state().unwrap().selection.clone();
        assert_eq!(sel, vec![st.rows[0].handle]);
    }
}
