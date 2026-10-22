//! Topo 3D ▸ Create Contours... (TERRAIN.CREATE_CONTOURS): same point-source picker as
//! [`crate::createmntdlg`] (auto by block/layer, or a manual pick), plus a short editable table
//! of "orders" - each its own step, its own target layer, and an optional colour - since a real
//! terrain usually wants several nested contour intervals (a fine one every metre, a bolder one
//! every five) each drawn where it can be toggled on its own.

use egui::vec2;
use serde_json::{Value, json};

use crate::CadApp;

#[derive(Clone, Copy, PartialEq, Default)]
pub enum Selection {
    #[default]
    Auto,
    Manual,
}

#[derive(Clone)]
pub struct OrderRow {
    pub step: String,
    pub layer: String,
    /// ACI colour index as typed text (empty: ByLayer).
    pub color: String,
}

impl OrderRow {
    fn new(step: &str, layer: &str) -> Self {
        OrderRow { step: step.into(), layer: layer.into(), color: String::new() }
    }
}

#[derive(Clone)]
pub struct State {
    pub selection: Selection,
    pub block_names: Vec<String>,
    pub layers: Vec<String>,
    pub ignore_null_altitude: bool,
    pub clear_layers: bool,
    pub orders: Vec<OrderRow>,
    pub handles: Vec<cadcraft_doc::Handle>,
    picking: bool,
    pub last_error: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        State {
            selection: Selection::default(),
            block_names: Vec::new(),
            layers: Vec::new(),
            ignore_null_altitude: true,
            clear_layers: true,
            // Our own default spread, not copied from any reference application's naming.
            orders: vec![OrderRow::new("1", "CONTOUR_1"), OrderRow::new("5", "CONTOUR_5")],
            handles: Vec::new(),
            picking: false,
            last_error: None,
        }
    }
}

pub fn open(app: &mut CadApp) {
    app.create_contours_dlg = Some(State::default());
    app.ui.dialog = Some("createcontours".into());
}

fn start_pick(app: &mut CadApp) {
    let Some(st) = app.create_contours_dlg.as_mut() else { return };
    st.picking = true;
    app.session.table_pick = None;
    app.ui.dialog = None;
    app.start("data.pick_entities");
}

pub fn resume(app: &mut CadApp) {
    if !app.create_contours_dlg.as_ref().is_some_and(|s| s.picking) {
        return;
    }
    if app.session.running.is_some() {
        return;
    }
    let picked = app.session.table_pick.take();
    let Some(st) = app.create_contours_dlg.as_mut() else { return };
    st.picking = false;
    if let Some(hs) = picked {
        st.handles = hs;
    }
    app.ui.dialog = Some("createcontours".into());
}

fn ready(st: &State) -> bool {
    let selection_ready = match st.selection {
        Selection::Auto => true,
        Selection::Manual => !st.handles.is_empty(),
    };
    selection_ready && st.orders.iter().any(|o| !o.step.trim().is_empty() && !o.layer.trim().is_empty())
}

fn params(st: &State, draw: bool) -> Value {
    let mut p = serde_json::Map::new();
    p.insert("selection".into(), json!(if st.selection == Selection::Manual { "manual" } else { "auto" }));
    p.insert("blockNames".into(), json!(st.block_names));
    p.insert("layers".into(), json!(st.layers));
    p.insert("ignoreNullAltitude".into(), json!(st.ignore_null_altitude));
    p.insert("clearLayers".into(), json!(st.clear_layers));
    p.insert("draw".into(), json!(draw));
    if st.selection == Selection::Manual {
        p.insert("handles".into(), json!(st.handles.iter().map(|h| h.hex()).collect::<Vec<_>>()));
    }
    let orders: Vec<Value> = st
        .orders
        .iter()
        .filter(|o| !o.step.trim().is_empty() && !o.layer.trim().is_empty())
        .filter_map(|o| {
            let step: f64 = o.step.trim().parse().ok()?;
            let mut row = serde_json::Map::new();
            row.insert("step".into(), json!(step));
            row.insert("layer".into(), json!(o.layer.trim()));
            if let Ok(c) = o.color.trim().parse::<u64>() {
                row.insert("color".into(), json!(c));
            }
            Some(Value::Object(row))
        })
        .collect();
    p.insert("orders".into(), json!(orders));
    Value::Object(p)
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let Some(mut st) = app.create_contours_dlg.clone() else {
        *open = false;
        return;
    };

    let block_choices = crate::editaltitudesdlg::block_choices(app);
    let layer_choices = crate::editaltitudesdlg::layer_choices(app);

    let mut pick = false;
    let mut add_row = false;
    let mut remove_row: Option<usize> = None;
    let mut run = false;
    let mut close = false;
    egui::Window::new(crate::tl!("Create Contours...")).open(open).resizable(true).collapsible(false).default_size(vec2(560.0, 520.0)).show(
        ctx,
        |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.label(crate::tl!("Points to take into account"));
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.radio_value(&mut st.selection, Selection::Auto, crate::tl!("Point block name(s) + layer(s)"));
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut st.selection, Selection::Manual, crate::tl!("Manual selection"));
                            if ui.button(crate::tl!("Select...")).clicked() {
                                st.selection = Selection::Manual;
                                pick = true;
                            }
                            ui.label(crate::tf!("{n} point(s)", n = st.handles.len()));
                        });
                    });
                    ui.separator();
                    ui.vertical(|ui| {
                        ui.label(crate::tl!("Point block name(s)"));
                        crate::editaltitudesdlg::checklist(ui, "cc_contours_blocks", &block_choices, &mut st.block_names);
                        ui.add_space(6.0);
                        ui.label(crate::tl!("Block insertion layer(s)"));
                        crate::editaltitudesdlg::checklist(ui, "cc_contours_layers", &layer_choices, &mut st.layers);
                    });
                });
                ui.separator();
                ui.checkbox(&mut st.ignore_null_altitude, crate::tl!("Ignore points with a null altitude"));
                ui.checkbox(&mut st.clear_layers, crate::tl!("Erase the target layer(s) before drawing"));
                ui.separator();

                ui.label(crate::tl!("Contour orders (each its own step and layer)"));
                egui::Grid::new("cc_contours_orders").num_columns(4).striped(true).show(ui, |ui| {
                    ui.label(crate::tl!("Step"));
                    ui.label(crate::tl!("Layer"));
                    ui.label(crate::tl!("Colour (ACI, optional)"));
                    ui.end_row();
                    for (i, row) in st.orders.iter_mut().enumerate() {
                        ui.add(egui::TextEdit::singleline(&mut row.step).desired_width(60.0));
                        ui.add(egui::TextEdit::singleline(&mut row.layer).desired_width(140.0));
                        ui.add(egui::TextEdit::singleline(&mut row.color).desired_width(60.0));
                        if ui.button(crate::tl!("Remove")).clicked() {
                            remove_row = Some(i);
                        }
                        ui.end_row();
                    }
                });
                if ui.button(crate::tl!("Add order")).clicked() {
                    add_row = true;
                }
                if let Some(err) = &st.last_error {
                    ui.colored_label(ui.visuals().warn_fg_color, err);
                }
            });

            ui.separator();
            ui.horizontal(|ui| {
                run = ui.add_enabled(ready(&st), egui::Button::new(crate::tl!("Create"))).clicked();
                close = ui.button(crate::tl!("Cancel")).clicked();
            });
        },
    );

    if close {
        *open = false;
    }
    if add_row {
        st.orders.push(OrderRow::new("", ""));
    }
    if let Some(i) = remove_row {
        st.orders.remove(i);
    }

    if pick {
        app.create_contours_dlg = Some(st);
        start_pick(app);
        return;
    }

    if run {
        let p = params(&st, true);
        match app.run("terrain.create_contours", p) {
            Ok(v) => {
                let n = v["orders"].as_array().map(Vec::len).unwrap_or(0);
                app.session.echo(crate::tf!("Contours created for {n} order(s)", n = n));
                st.last_error = None;
                *open = false;
            }
            Err(e) => st.last_error = Some(e),
        }
    }

    if *open {
        app.create_contours_dlg = Some(st);
    } else {
        app.create_contours_dlg = None;
    }
}

#[cfg(test)]
mod tests {
    use cadcraft_engine::Session;
    use serde_json::json;

    use super::*;
    use crate::Services;

    fn app_with_doc() -> CadApp {
        let mut s = Session::new();
        s.execute("new", &json!({})).unwrap();
        CadApp::new(s, Services::default())
    }

    fn import_tcpoint(app: &mut CadApp, id: &str, x: f64, y: f64, z: f64) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("cadcraft-createcontours-ui-test-{}-{id}-{n}.csv", std::process::id()));
        std::fs::write(&path, format!("ID;X;Y;Z;CODE\n{id};{x};{y};{z};P\n")).unwrap();
        app.run("topo.import_points", json!({ "path": path.to_string_lossy(), "label": false })).unwrap();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn ready_requires_at_least_one_complete_order_and_a_real_selection() {
        let st = State { orders: vec![OrderRow::new("1", "C1")], ..State::default() };
        assert!(ready(&st));
        let st = State { orders: vec![OrderRow::new("", "")], ..State::default() };
        assert!(!st.orders.iter().any(|o| !o.step.trim().is_empty() && !o.layer.trim().is_empty()));
        assert!(!ready(&st));
        let st = State { selection: Selection::Manual, orders: vec![OrderRow::new("1", "C1")], ..State::default() };
        assert!(!ready(&st));
    }

    #[test]
    fn params_only_includes_complete_order_rows() {
        let st = State { orders: vec![OrderRow::new("2", "C2"), OrderRow::new("", "EMPTY_STEP"), OrderRow::new("5", "")], ..State::default() };
        let p = params(&st, true);
        let orders = p["orders"].as_array().unwrap();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0]["layer"].as_str(), Some("C2"));
    }

    #[test]
    fn select_button_picks_entities_and_resume_brings_the_handles_back() {
        let mut app = app_with_doc();
        let r = app.session.execute("point", &json!({ "at": [0.0, 0.0] })).unwrap();
        let handle = cadcraft_doc::Handle::parse_hex(r["handles"][0].as_str().unwrap()).unwrap();

        open(&mut app);
        start_pick(&mut app);
        assert_eq!(app.ui.dialog, None);

        app.session.input(cadcraft_engine::Input::Pick(vec![handle])).unwrap();
        app.session.input(cadcraft_engine::Input::Enter).unwrap();
        resume(&mut app);

        assert_eq!(app.ui.dialog, Some("createcontours".into()));
        assert_eq!(app.create_contours_dlg.unwrap().handles, vec![handle]);
    }

    #[test]
    fn running_draws_contours_for_every_complete_order() {
        let mut app = app_with_doc();
        import_tcpoint(&mut app, "1", 0.0, 0.0, 1.0);
        import_tcpoint(&mut app, "2", 10.0, 0.0, 10.0);
        import_tcpoint(&mut app, "3", 10.0, 10.0, 10.0);
        import_tcpoint(&mut app, "4", 0.0, 10.0, 1.0);

        open(&mut app);
        let st = State { orders: vec![OrderRow::new("2", "C2")], ..app.create_contours_dlg.clone().unwrap() };
        let out = app.run("terrain.create_contours", params(&st, true)).unwrap();
        assert!(out["orders"][0]["segmentCount"].as_u64().unwrap_or(0) > 0);
        assert!(app.session.doc().unwrap().model.iter().any(|e| e.common.layer.eq_ignore_ascii_case("C2")));
    }
}
