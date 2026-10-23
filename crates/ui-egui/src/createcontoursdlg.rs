//! Topo 3D ▸ Create Contours... (TERRAIN.CREATE_CONTOURS): same point-source picker as
//! [`crate::createmntdlg`] (auto by block/layer, or a manual pick), an "Estimate" readout of the
//! points' own elevation range (so a step can be chosen with the real min/max in view, the way
//! [`crate::createmntdlg`]'s own Estimate works), a resizable table of "orders" - each its own
//! step, its own target layer, and a colour picked from a gradient swatch - and a light-smoothing
//! toggle, since a real terrain usually wants several nested contour intervals (a fine one every
//! 0.25m, a bolder one every metre) each drawn where it can be toggled on its own.

use egui::{Color32, vec2};
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
    pub color: Color32,
}

impl OrderRow {
    fn new(step: &str, layer: &str, color: Color32) -> Self {
        OrderRow { step: step.into(), layer: layer.into(), color }
    }
}

/// The last `terrain.create_mnt` dry run (`draw: false`) over the same point selection, just to
/// show the real elevation range before the person picks a step - not itself a MNT/TIN creation.
#[derive(Clone)]
pub struct Estimate {
    pub point_count: u64,
    pub elevation_range: Option<(f64, f64)>,
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct State {
    pub selection: Selection,
    pub block_names: Vec<String>,
    pub layers: Vec<String>,
    pub ignore_null_altitude: bool,
    pub clear_layers: bool,
    /// One light corner-cutting smoothing pass over each contour (checked by default).
    pub smooth: bool,
    pub orders: Vec<OrderRow>,
    pub handles: Vec<cadcraft_doc::Handle>,
    picking: bool,
    pub estimate: Option<Estimate>,
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
            smooth: true,
            // Our own default spread (not copied from any reference application): a fine, a
            // middle and a bold interval, warming from yellow to brown as the step grows.
            orders: vec![
                OrderRow::new("0.25", "CONTOUR_025", Color32::from_rgb(255, 221, 0)),
                OrderRow::new("0.5", "CONTOUR_050", Color32::from_rgb(255, 140, 0)),
                OrderRow::new("1", "CONTOUR_100", Color32::from_rgb(139, 69, 19)),
            ],
            handles: Vec::new(),
            picking: false,
            estimate: None,
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

/// The point-selection fields shared by the real run and the elevation-range estimate.
fn selection_params(st: &State, draw: bool) -> serde_json::Map<String, Value> {
    let mut p = serde_json::Map::new();
    p.insert("selection".into(), json!(if st.selection == Selection::Manual { "manual" } else { "auto" }));
    p.insert("blockNames".into(), json!(st.block_names));
    p.insert("layers".into(), json!(st.layers));
    p.insert("ignoreNullAltitude".into(), json!(st.ignore_null_altitude));
    p.insert("draw".into(), json!(draw));
    if st.selection == Selection::Manual {
        p.insert("handles".into(), json!(st.handles.iter().map(|h| h.hex()).collect::<Vec<_>>()));
    }
    p
}

fn params(st: &State, draw: bool) -> Value {
    let mut p = selection_params(st, draw);
    p.insert("clearLayers".into(), json!(st.clear_layers));
    p.insert("smooth".into(), json!(st.smooth));
    let orders: Vec<Value> = st
        .orders
        .iter()
        .filter(|o| !o.step.trim().is_empty() && !o.layer.trim().is_empty())
        .filter_map(|o| {
            let step: f64 = o.step.trim().parse().ok()?;
            let mut row = serde_json::Map::new();
            row.insert("step".into(), json!(step));
            row.insert("layer".into(), json!(o.layer.trim()));
            row.insert("colorRgb".into(), json!([o.color.r(), o.color.g(), o.color.b()]));
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
    let mut estimate = false;
    let mut add_row = false;
    let mut remove_row: Option<usize> = None;
    let mut run = false;
    let mut close = false;
    egui::Window::new(crate::tl!("Create Contours...")).open(open).resizable(true).collapsible(false).default_size(vec2(640.0, 560.0)).show(
        ctx,
        |ui| {
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
                    if ui.button(crate::tl!("Estimate...")).clicked() {
                        estimate = true;
                    }
                    if let Some(e) = &st.estimate {
                        match (&e.error, e.elevation_range) {
                            (Some(err), _) => {
                                ui.colored_label(ui.visuals().warn_fg_color, err);
                            }
                            (None, Some((lo, hi))) => {
                                ui.label(crate::tf!(
                                    "{n} point(s), altitude {lo} to {hi}",
                                    n = e.point_count,
                                    lo = format!("{lo:.2}"),
                                    hi = format!("{hi:.2}")
                                ));
                            }
                            (None, None) => {
                                ui.label(crate::tf!("{n} point(s), no elevation range yet", n = e.point_count));
                            }
                        }
                    }
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
            ui.checkbox(&mut st.smooth, crate::tl!("Slightly smooth the contours"));
            ui.separator();

            ui.label(crate::tl!("Contour orders (each its own step, layer and colour)"));
            // Resizable columns, and no outer `ScrollArea` wrapping the table itself, so
            // resizing the window actually grows the rows shown instead of just adding a
            // scrollbar - see the same fix on the table in `crate::searchpointsdlg`.
            use egui_extras::{Column, TableBuilder};
            let mut table_remove: Option<usize> = None;
            TableBuilder::new(ui)
                .striped(true)
                .resizable(true)
                .min_scrolled_height(90.0)
                .column(Column::initial(70.0).at_least(50.0))
                .column(Column::initial(160.0).at_least(60.0))
                .column(Column::initial(70.0).at_least(50.0).resizable(false))
                .column(Column::remainder().at_least(70.0).resizable(false))
                .header(20.0, |mut header| {
                    header.col(|ui| {
                        ui.label(crate::tl!("Step"));
                    });
                    header.col(|ui| {
                        ui.label(crate::tl!("Layer"));
                    });
                    header.col(|ui| {
                        ui.label(crate::tl!("Colour"));
                    });
                    header.col(|_| {});
                })
                .body(|body| {
                    body.rows(22.0, st.orders.len(), |mut row| {
                        let i = row.index();
                        row.col(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut st.orders[i].step));
                        });
                        row.col(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut st.orders[i].layer));
                        });
                        row.col(|ui| {
                            // Clicking opens egui's own hue/saturation gradient picker.
                            ui.color_edit_button_srgba(&mut st.orders[i].color);
                        });
                        row.col(|ui| {
                            if ui.button(crate::tl!("Remove")).clicked() {
                                table_remove = Some(i);
                            }
                        });
                    });
                });
            if let Some(i) = table_remove {
                remove_row = Some(i);
            }
            if ui.button(crate::tl!("Add order")).clicked() {
                add_row = true;
            }
            if let Some(err) = &st.last_error {
                ui.colored_label(ui.visuals().warn_fg_color, err);
            }

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
        st.orders.push(OrderRow::new("", "", Color32::WHITE));
    }
    if let Some(i) = remove_row {
        st.orders.remove(i);
    }

    if pick {
        app.create_contours_dlg = Some(st);
        start_pick(app);
        return;
    }

    if estimate {
        let p = Value::Object(selection_params(&st, false));
        match app.run("terrain.create_mnt", p) {
            Ok(v) => {
                let range = v.get("elevationRange").and_then(|r| r.as_array()).and_then(|a| Some((a.first()?.as_f64()?, a.get(1)?.as_f64()?)));
                st.estimate = Some(Estimate { point_count: v["pointCount"].as_u64().unwrap_or(0), elevation_range: range, error: None });
            }
            Err(e) => st.estimate = Some(Estimate { point_count: 0, elevation_range: None, error: Some(e) }),
        }
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

    fn row(step: &str, layer: &str) -> OrderRow {
        OrderRow::new(step, layer, Color32::WHITE)
    }

    #[test]
    fn default_orders_are_the_fine_middle_bold_spread() {
        let st = State::default();
        assert_eq!(st.orders.len(), 3);
        assert_eq!(st.orders[0].step, "0.25");
        assert_eq!(st.orders[1].step, "0.5");
        assert_eq!(st.orders[2].step, "1");
        assert!(st.smooth, "smoothing is on by default");
    }

    #[test]
    fn ready_requires_at_least_one_complete_order_and_a_real_selection() {
        let st = State { orders: vec![row("1", "C1")], ..State::default() };
        assert!(ready(&st));
        let st = State { orders: vec![row("", "")], ..State::default() };
        assert!(!st.orders.iter().any(|o| !o.step.trim().is_empty() && !o.layer.trim().is_empty()));
        assert!(!ready(&st));
        let st = State { selection: Selection::Manual, orders: vec![row("1", "C1")], ..State::default() };
        assert!(!ready(&st));
    }

    #[test]
    fn params_only_includes_complete_order_rows_and_carries_the_picked_colour() {
        let mut st = State { orders: vec![row("2", "C2"), row("", "EMPTY_STEP"), row("5", "")], ..State::default() };
        st.orders[0].color = Color32::from_rgb(10, 20, 30);
        let p = params(&st, true);
        let orders = p["orders"].as_array().unwrap();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0]["layer"].as_str(), Some("C2"));
        assert_eq!(orders[0]["colorRgb"], json!([10, 20, 30]));
        assert_eq!(p["smooth"].as_bool(), Some(true));
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
    fn estimate_reports_the_points_own_elevation_range() {
        let mut app = app_with_doc();
        import_tcpoint(&mut app, "1", 0.0, 0.0, 1.0);
        import_tcpoint(&mut app, "2", 10.0, 0.0, 10.0);
        import_tcpoint(&mut app, "3", 10.0, 10.0, 10.0);

        open(&mut app);
        let st = app.create_contours_dlg.clone().unwrap();
        let p = Value::Object(selection_params(&st, false));
        let out = app.run("terrain.create_mnt", p).unwrap();
        let range = out["elevationRange"].as_array().map(|a| (a[0].as_f64().unwrap(), a[1].as_f64().unwrap()));
        assert_eq!(range, Some((1.0, 10.0)));
    }

    #[test]
    fn running_draws_contours_for_every_complete_order() {
        let mut app = app_with_doc();
        import_tcpoint(&mut app, "1", 0.0, 0.0, 1.0);
        import_tcpoint(&mut app, "2", 10.0, 0.0, 10.0);
        import_tcpoint(&mut app, "3", 10.0, 10.0, 10.0);
        import_tcpoint(&mut app, "4", 0.0, 10.0, 1.0);

        open(&mut app);
        let st = State { orders: vec![row("2", "C2")], ..app.create_contours_dlg.clone().unwrap() };
        let out = app.run("terrain.create_contours", params(&st, true)).unwrap();
        assert!(out["orders"][0]["polylineCount"].as_u64().unwrap_or(0) > 0);
        assert!(app.session.doc().unwrap().model.iter().any(|e| e.common.layer.eq_ignore_ascii_case("C2")));
    }
}
