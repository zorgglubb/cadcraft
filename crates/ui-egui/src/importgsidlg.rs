//! The Import GSI Carnet dialog: TOPO.IMPORT_GSI from a menu or the command line (no parameters)
//! picks the file first (same picker OPEN/IMPORT use), then inspects it with TOPO.GSI_INSPECT -
//! a coordinate-only carnet (WI 81/82/83) needs nothing further and imports as soon as this
//! opens the same way the old one-shot behaviour did; a carnet with angle+distance shots and no
//! ready-made coordinates needs a station occupied and an orientation (a known VO, or references
//! to compute one from) first, so this shows the same station/VO fields [`crate::stationdlg`]
//! uses for TOPO.RADIATION/TOPO.ZERO_VALUE before enabling Import.

use egui::vec2;
use serde_json::{Value, json};

use crate::CadApp;
use crate::stationdlg::ReferenceRow;

#[derive(Clone)]
pub struct State {
    path: String,
    needs_station: bool,
    point_count: u64,
    station_x: String,
    station_y: String,
    station_z: String,
    from_references: bool,
    vo: String,
    references: Vec<ReferenceRow>,
    label: bool,
}

impl State {
    fn new(path: String) -> Self {
        State {
            path,
            needs_station: false,
            point_count: 0,
            station_x: String::new(),
            station_y: String::new(),
            station_z: "0".into(),
            from_references: false,
            vo: "0".into(),
            references: vec![ReferenceRow::default()],
            label: true,
        }
    }
}

pub fn open(app: &mut CadApp, path: String) {
    let mut st = State::new(path);
    if let Ok(v) = app.run("topo.gsi_inspect", json!({ "path": st.path })) {
        st.needs_station = v["needsStation"].as_bool().unwrap_or(false);
        st.point_count = v["pointCount"].as_u64().unwrap_or(0);
    }
    // A coordinate-only carnet has nothing left to ask for - import right away, same as before
    // this dialog existed, and report the result rather than opening an empty-looking window.
    if !st.needs_station {
        run_import(app, &st);
        return;
    }
    app.import_gsi_dlg = Some(st);
    app.ui.dialog = Some("importgsi".into());
}

fn num(s: &str) -> Option<f64> {
    let t = s.trim().replace(',', ".");
    if t.is_empty() { None } else { t.parse::<f64>().ok() }
}

fn ready(st: &State) -> bool {
    let station = num(&st.station_x).is_some() && num(&st.station_y).is_some();
    station
        && if st.from_references { st.references.iter().any(|r| !r.x.trim().is_empty() && !r.hz.trim().is_empty()) } else { num(&st.vo).is_some() }
}

fn run_import(app: &mut CadApp, st: &State) {
    let mut params = serde_json::Map::new();
    params.insert("path".into(), json!(st.path));
    params.insert("label".into(), json!(st.label));
    if st.needs_station {
        let Some(x) = num(&st.station_x) else { return };
        let Some(y) = num(&st.station_y) else { return };
        params.insert("station".into(), json!([x, y, num(&st.station_z).unwrap_or(0.0)]));
        if st.from_references {
            let refs: Vec<Value> = st
                .references
                .iter()
                .filter(|r| !r.x.trim().is_empty() && !r.hz.trim().is_empty())
                .filter_map(|r| Some(json!({ "point": [num(&r.x)?, num(&r.y)?, num(&r.z).unwrap_or(0.0)], "hzReading": num(&r.hz)? })))
                .collect();
            params.insert("references".into(), Value::Array(refs));
        } else if let Some(vo) = num(&st.vo) {
            params.insert("vo".into(), json!(vo));
        }
    }

    let r = app.run("topo.import_gsi", Value::Object(params));
    app.session.echo(match &r {
        Ok(v) => crate::tf!("Imported {n} point(s)", n = v.get("importedPoints").and_then(Value::as_u64).unwrap_or(0)),
        Err(e) => e.clone(),
    });
    if r.is_ok() {
        app.canvas.zoom_pending = true;
    }
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let Some(mut st) = app.import_gsi_dlg.clone() else {
        *open = false;
        return;
    };

    let (mut import, mut close) = (false, false);
    egui::Window::new(crate::tl!("Import GSI Carnet")).open(open).resizable(true).collapsible(false).default_size(vec2(480.0, 380.0)).show(
        ctx,
        |ui| {
            ui.horizontal(|ui| {
                ui.label(crate::tl!("File"));
                ui.label(&st.path);
            });
            ui.label(crate::tf!("{n} point(s) read; a station is needed for its angle/distance shots", n = st.point_count));
            ui.separator();
            ui.label(crate::tl!("Station"));
            ui.horizontal(|ui| {
                ui.label("X");
                ui.text_edit_singleline(&mut st.station_x);
                ui.label("Y");
                ui.text_edit_singleline(&mut st.station_y);
                ui.label("Z");
                ui.text_edit_singleline(&mut st.station_z);
            });
            ui.horizontal(|ui| {
                if ui.selectable_label(!st.from_references, crate::tl!("Known VO value")).clicked() {
                    st.from_references = false;
                }
                if ui.selectable_label(st.from_references, crate::tl!("Compute from references")).clicked() {
                    st.from_references = true;
                }
            });
            if st.from_references {
                egui::Grid::new("cc_gsi_refs").striped(true).show(ui, |ui| {
                    ui.label("X");
                    ui.label("Y");
                    ui.label("Z");
                    ui.label(crate::tl!("Hz reading (gon)"));
                    ui.end_row();
                    let mut remove: Option<usize> = None;
                    for (i, r) in st.references.iter_mut().enumerate() {
                        ui.text_edit_singleline(&mut r.x);
                        ui.text_edit_singleline(&mut r.y);
                        ui.text_edit_singleline(&mut r.z);
                        ui.text_edit_singleline(&mut r.hz);
                        if ui.button("✕").clicked() {
                            remove = Some(i);
                        }
                        ui.end_row();
                    }
                    if let Some(i) = remove {
                        st.references.remove(i);
                    }
                });
                if ui.button(crate::tl!("Add reference")).clicked() {
                    st.references.push(ReferenceRow::default());
                }
            } else {
                ui.horizontal(|ui| {
                    ui.label(crate::tl!("VO (gon)"));
                    ui.text_edit_singleline(&mut st.vo);
                });
            }
            ui.checkbox(&mut st.label, crate::tl!("Show code label"));
            ui.separator();
            ui.horizontal(|ui| {
                import = ui.add_enabled(ready(&st), egui::Button::new(crate::tl!("Import"))).clicked();
                close = ui.button(crate::tl!("Close")).clicked();
            });
        },
    );
    if close {
        *open = false;
    }
    if import {
        run_import(app, &st);
        *open = false;
    }
    if *open {
        app.import_gsi_dlg = Some(st);
    } else {
        app.import_gsi_dlg = None;
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

    #[test]
    fn a_coordinate_only_carnet_imports_immediately_without_opening_the_dialog() {
        let mut app = app_with_doc();
        let path = std::env::temp_dir().join(format!("cadcraft-ui-gsi-coord-{}.gsi", std::process::id()));
        std::fs::write(&path, "110001+00000P1 81..00+00001234 82..00+00005678 83..00+00000100\n").unwrap();

        open(&mut app, path.to_string_lossy().to_string());
        let _ = std::fs::remove_file(&path);

        assert!(app.import_gsi_dlg.is_none(), "coordinate-only carnet should not need the dialog");
    }

    #[test]
    fn a_carnet_with_angle_distance_shots_opens_the_dialog_needing_a_station() {
        let mut app = app_with_doc();
        let path = std::env::temp_dir().join(format!("cadcraft-ui-gsi-radial-{}.gsi", std::process::id()));
        std::fs::write(&path, "110001+00000P1 21.324+00005000 22.324+00010000 31..00+00010000\n").unwrap();

        open(&mut app, path.to_string_lossy().to_string());
        let _ = std::fs::remove_file(&path);

        let st = app.import_gsi_dlg.expect("dialog should open");
        assert!(st.needs_station);
    }
}
