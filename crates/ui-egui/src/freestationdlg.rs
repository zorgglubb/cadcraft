//! The Free Station ("station libre") dialog: TOPO.FREE_STATION opened with no parameters -
//! resections an unknown station from two or more known reference points (each with a measured
//! horizontal distance and Hz reading), independent of the instrument's own orientation.

use egui::vec2;
use serde_json::{Value, json};

use crate::CadApp;

/// One known reference point observed from the unknown station.
#[derive(Clone, Default)]
pub struct ReferenceRow {
    pub x: String,
    pub y: String,
    pub z: String,
    pub distance: String,
    pub hz: String,
}

#[derive(Clone)]
pub struct State {
    pub references: Vec<ReferenceRow>,
    pub approx_x: String,
    pub approx_y: String,
    pub code: String,
    pub label: bool,
    pub insert_station: bool,
    /// The last computed result, shown after a successful run.
    pub last_result: Option<(f64, f64, f64, f64, Option<f64>, usize)>, // x, y, z, vo, sigma, iterations
}

impl Default for State {
    fn default() -> Self {
        State {
            references: vec![ReferenceRow::default(), ReferenceRow::default()],
            approx_x: String::new(),
            approx_y: String::new(),
            code: String::new(),
            label: true,
            insert_station: true,
            last_result: None,
        }
    }
}

pub fn open(app: &mut CadApp) {
    app.free_station_dlg = Some(State::default());
    app.ui.dialog = Some("freestation".into());
}

fn num(s: &str) -> Option<f64> {
    let t = s.trim().replace(',', ".");
    if t.is_empty() { None } else { t.parse::<f64>().ok() }
}

fn references_json(st: &State) -> Option<Value> {
    let rows: Vec<Value> = st
        .references
        .iter()
        .filter(|r| !r.x.trim().is_empty() || !r.y.trim().is_empty() || !r.distance.trim().is_empty() || !r.hz.trim().is_empty())
        .filter_map(|r| {
            Some(json!({
                "point": [num(&r.x)?, num(&r.y)?, num(&r.z).unwrap_or(0.0)],
                "distance": num(&r.distance)?,
                "hzReading": num(&r.hz)?,
            }))
        })
        .collect();
    (rows.len() >= 2).then_some(Value::Array(rows))
}

fn ready(st: &State) -> bool {
    references_json(st).is_some()
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let Some(mut st) = app.free_station_dlg.clone() else {
        *open = false;
        return;
    };

    let (mut run, mut close) = (false, false);
    egui::Window::new(crate::tl!("Free Station")).open(open).resizable(true).collapsible(false).default_size(vec2(560.0, 440.0)).show(ctx, |ui| {
        ui.label(crate::tl!("Known reference points (distance + Hz reading to each, at least 2)"));
        egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
            egui::Grid::new("cc_freestation_refs").striped(true).show(ui, |ui| {
                ui.label("X");
                ui.label("Y");
                ui.label("Z");
                ui.label(crate::tl!("Distance"));
                ui.label(crate::tl!("Hz reading (gon)"));
                ui.end_row();
                let mut remove: Option<usize> = None;
                for (i, r) in st.references.iter_mut().enumerate() {
                    ui.text_edit_singleline(&mut r.x);
                    ui.text_edit_singleline(&mut r.y);
                    ui.text_edit_singleline(&mut r.z);
                    ui.text_edit_singleline(&mut r.distance);
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
        });
        if ui.button(crate::tl!("Add reference")).clicked() {
            st.references.push(ReferenceRow::default());
        }
        ui.separator();
        ui.label(crate::tl!("Approximate station position (optional, helps convergence)"));
        ui.horizontal(|ui| {
            ui.label("X");
            ui.text_edit_singleline(&mut st.approx_x);
            ui.label("Y");
            ui.text_edit_singleline(&mut st.approx_y);
        });
        ui.separator();
        ui.checkbox(&mut st.insert_station, crate::tl!("Insert the computed station as a point"));
        ui.add_enabled_ui(st.insert_station, |ui| {
            ui.horizontal(|ui| {
                ui.label(crate::tl!("Code"));
                ui.text_edit_singleline(&mut st.code);
            });
            ui.checkbox(&mut st.label, crate::tl!("Show code label"));
        });
        if let Some((x, y, z, vo, sigma, iterations)) = st.last_result {
            ui.separator();
            ui.label(crate::tf!(
                "Station: {x}, {y}, {z} - VO {vo} gon{sigma} ({iterations} iteration(s))",
                x = format!("{x:.4}"),
                y = format!("{y:.4}"),
                z = format!("{z:.4}"),
                vo = format!("{vo:.4}"),
                sigma = sigma.map(|s| format!(" (σ {s:.4})")).unwrap_or_default(),
                iterations = iterations.to_string(),
            ));
        }
        ui.separator();
        ui.horizontal(|ui| {
            run = ui.add_enabled(ready(&st), egui::Button::new(crate::tl!("Compute"))).clicked();
            close = ui.button(crate::tl!("Close")).clicked();
        });
    });
    if close {
        *open = false;
    }

    if run {
        let Some(refs) = references_json(&st) else {
            app.free_station_dlg = Some(st);
            return;
        };
        let mut params = serde_json::Map::new();
        params.insert("references".into(), refs);
        if let (Some(x), Some(y)) = (num(&st.approx_x), num(&st.approx_y)) {
            params.insert("approx".into(), json!([x, y, 0.0]));
        }
        params.insert("insertStation".into(), json!(st.insert_station));
        if !st.code.trim().is_empty() {
            params.insert("code".into(), json!(st.code.trim()));
        }
        params.insert("label".into(), json!(st.label));

        let r = app.run("topo.free_station", Value::Object(params));
        match &r {
            Ok(v) => {
                let station = v["station"].as_array().cloned().unwrap_or_default();
                let gx = |i: usize| station.get(i).and_then(Value::as_f64).unwrap_or(0.0);
                st.last_result =
                    Some((gx(0), gx(1), gx(2), v["vo"].as_f64().unwrap_or(0.0), v["sigma"].as_f64(), v["iterations"].as_u64().unwrap_or(0) as usize));
                if st.insert_station {
                    app.session.echo(crate::tl!("Inserted the free station"));
                    app.canvas.zoom_pending = true;
                }
            }
            Err(e) => app.session.echo(e.clone()),
        }
    }

    if *open {
        app.free_station_dlg = Some(st);
    } else {
        app.free_station_dlg = None;
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
    fn resects_a_station_from_three_references() {
        let mut app = app_with_doc();
        open(&mut app);
        let mut st = app.free_station_dlg.clone().unwrap();
        // True station at (1000, 1000); three references around it with exact distance/gisement.
        let truth: (f64, f64) = (1000.0, 1000.0);
        let refs: [(f64, f64); 3] = [(1100.0, 1000.0), (1000.0, 1120.0), (900.0, 950.0)];
        st.references = refs
            .iter()
            .map(|(x, y)| {
                let dx = x - truth.0;
                let dy = y - truth.1;
                let dist = dx.hypot(dy);
                let gisement = dx.atan2(dy).to_degrees() * (400.0 / 360.0);
                let gisement = if gisement < 0.0 { gisement + 400.0 } else { gisement };
                ReferenceRow { x: x.to_string(), y: y.to_string(), z: "0".into(), distance: dist.to_string(), hz: gisement.to_string() }
            })
            .collect();
        app.free_station_dlg = Some(st.clone());
        assert!(ready(&st));

        let r = app.run("topo.free_station", json!({ "references": references_json(&st).unwrap() }));
        assert!(r.is_ok(), "{r:?}");
        let v = r.unwrap();
        assert!((v["station"][0].as_f64().unwrap() - 1000.0).abs() < 1e-3);
        assert!((v["station"][1].as_f64().unwrap() - 1000.0).abs() < 1e-3);
    }

    #[test]
    fn needs_at_least_two_references_to_be_ready() {
        let mut st = State::default();
        st.references[0] = ReferenceRow { x: "0".into(), y: "0".into(), z: "0".into(), distance: "10".into(), hz: "0".into() };
        assert!(!ready(&st));
        st.references[1] = ReferenceRow { x: "10".into(), y: "0".into(), z: "0".into(), distance: "10".into(), hz: "100".into() };
        assert!(ready(&st));
    }
}
