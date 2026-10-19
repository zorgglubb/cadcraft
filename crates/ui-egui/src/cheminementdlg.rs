//! The Traverse ("cheminement") dialog: TOPO.CHEMINEMENT opened with no parameters - chains a
//! sequence of legs (gisement + distance) from a known start point, optionally adjusting every
//! intermediate station by the compass rule when the traverse closes back onto a second known
//! point.

use egui::vec2;
use serde_json::{Value, json};

use crate::CadApp;

/// One traverse leg: the gisement and distance from the previous station, plus an optional code
/// for the point it lands on.
#[derive(Clone, Default)]
pub struct LegRow {
    pub gisement: String,
    pub distance: String,
    pub code: String,
}

#[derive(Clone)]
pub struct State {
    pub start_x: String,
    pub start_y: String,
    pub start_z: String,
    pub legs: Vec<LegRow>,
    pub has_closing_point: bool,
    pub closing_x: String,
    pub closing_y: String,
    pub closing_z: String,
    pub label: bool,
    /// The last run's closing error, if a closing point was given.
    pub last_closing_error: Option<(f64, f64)>,
}

impl Default for State {
    fn default() -> Self {
        State {
            start_x: String::new(),
            start_y: String::new(),
            start_z: "0".into(),
            legs: vec![LegRow::default()],
            has_closing_point: false,
            closing_x: String::new(),
            closing_y: String::new(),
            closing_z: "0".into(),
            label: true,
            last_closing_error: None,
        }
    }
}

pub fn open(app: &mut CadApp) {
    app.cheminement_dlg = Some(State::default());
    app.ui.dialog = Some("cheminement".into());
}

fn num(s: &str) -> Option<f64> {
    let t = s.trim().replace(',', ".");
    if t.is_empty() { None } else { t.parse::<f64>().ok() }
}

fn start_json(st: &State) -> Option<Value> {
    Some(json!([num(&st.start_x)?, num(&st.start_y)?, num(&st.start_z).unwrap_or(0.0)]))
}

fn legs_json(st: &State) -> Option<Value> {
    let rows: Option<Vec<Value>> = st
        .legs
        .iter()
        .filter(|l| !l.gisement.trim().is_empty() || !l.distance.trim().is_empty())
        .map(|l| {
            Some(json!({
                "gisement": num(&l.gisement)?,
                "distance": num(&l.distance)?,
                "code": (!l.code.trim().is_empty()).then(|| l.code.trim().to_string()),
            }))
        })
        .collect();
    rows.filter(|v: &Vec<Value>| !v.is_empty()).map(Value::Array)
}

fn closing_json(st: &State) -> Option<Value> {
    if !st.has_closing_point {
        return None;
    }
    Some(json!([num(&st.closing_x)?, num(&st.closing_y)?, num(&st.closing_z).unwrap_or(0.0)]))
}

fn ready(st: &State) -> bool {
    start_json(st).is_some() && legs_json(st).is_some() && (!st.has_closing_point || closing_json(st).is_some())
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let Some(mut st) = app.cheminement_dlg.clone() else {
        *open = false;
        return;
    };

    let (mut run, mut close) = (false, false);
    egui::Window::new(crate::tl!("Traverse (Cheminement)")).open(open).resizable(true).collapsible(false).default_size(vec2(560.0, 480.0)).show(
        ctx,
        |ui| {
            ui.label(crate::tl!("Start point"));
            ui.horizontal(|ui| {
                ui.label("X");
                ui.text_edit_singleline(&mut st.start_x);
                ui.label("Y");
                ui.text_edit_singleline(&mut st.start_y);
                ui.label("Z");
                ui.text_edit_singleline(&mut st.start_z);
            });
            ui.separator();
            ui.label(crate::tl!("Legs (gisement + distance from the previous station, in order)"));
            egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                egui::Grid::new("cc_cheminement_legs").striped(true).show(ui, |ui| {
                    ui.label(crate::tl!("Gisement (gon)"));
                    ui.label(crate::tl!("Distance"));
                    ui.label(crate::tl!("Code"));
                    ui.end_row();
                    let mut remove: Option<usize> = None;
                    for (i, l) in st.legs.iter_mut().enumerate() {
                        ui.text_edit_singleline(&mut l.gisement);
                        ui.text_edit_singleline(&mut l.distance);
                        ui.text_edit_singleline(&mut l.code);
                        if ui.button("✕").clicked() {
                            remove = Some(i);
                        }
                        ui.end_row();
                    }
                    if let Some(i) = remove {
                        st.legs.remove(i);
                    }
                });
            });
            if ui.button(crate::tl!("Add leg")).clicked() {
                st.legs.push(LegRow::default());
            }
            ui.separator();
            ui.checkbox(&mut st.has_closing_point, crate::tl!("Close onto a known point (adjust by the compass rule)"));
            ui.add_enabled_ui(st.has_closing_point, |ui| {
                ui.horizontal(|ui| {
                    ui.label("X");
                    ui.text_edit_singleline(&mut st.closing_x);
                    ui.label("Y");
                    ui.text_edit_singleline(&mut st.closing_y);
                    ui.label("Z");
                    ui.text_edit_singleline(&mut st.closing_z);
                });
            });
            ui.checkbox(&mut st.label, crate::tl!("Show code label"));
            if let Some((ex, ey)) = st.last_closing_error {
                ui.separator();
                ui.label(crate::tf!(
                    "Raw closing error: {ex}, {ey} (distributed across the traverse)",
                    ex = format!("{ex:.4}"),
                    ey = format!("{ey:.4}")
                ));
            }
            ui.separator();
            ui.horizontal(|ui| {
                run = ui.add_enabled(ready(&st), egui::Button::new(crate::tl!("Compute"))).clicked();
                close = ui.button(crate::tl!("Close")).clicked();
            });
        },
    );
    if close {
        *open = false;
    }

    if run {
        let (Some(start), Some(legs)) = (start_json(&st), legs_json(&st)) else {
            app.cheminement_dlg = Some(st);
            return;
        };
        let mut params = serde_json::Map::new();
        params.insert("start".into(), start);
        params.insert("legs".into(), legs);
        if let Some(c) = closing_json(&st) {
            params.insert("closingPoint".into(), c);
        }
        params.insert("label".into(), json!(st.label));

        let r = app.run("topo.cheminement", Value::Object(params));
        match &r {
            Ok(v) => {
                st.last_closing_error = v["closingError"].as_array().map(|a| (a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0)));
                app.session.echo(crate::tf!("Inserted {n} point(s)", n = v["points"].as_array().map(Vec::len).unwrap_or(0)));
                app.canvas.zoom_pending = true;
            }
            Err(e) => app.session.echo(e.clone()),
        }
    }

    if *open {
        app.cheminement_dlg = Some(st);
    } else {
        app.cheminement_dlg = None;
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
    fn runs_a_traverse_and_inserts_each_leg() {
        let mut app = app_with_doc();
        open(&mut app);
        let mut st = app.cheminement_dlg.clone().unwrap();
        st.start_x = "0".into();
        st.start_y = "0".into();
        st.legs[0] = LegRow { gisement: "100".into(), distance: "10".into(), code: "P1".into() };
        st.legs.push(LegRow { gisement: "0".into(), distance: "10".into(), code: "P2".into() });
        app.cheminement_dlg = Some(st.clone());
        assert!(ready(&st));

        let r = app.run(
            "topo.cheminement",
            json!({
                "start": [0.0, 0.0, 0.0],
                "legs": [{"gisement": 100.0, "distance": 10.0, "code": "P1"}, {"gisement": 0.0, "distance": 10.0, "code": "P2"}],
            }),
        );
        assert!(r.is_ok(), "{r:?}");
        assert_eq!(r.unwrap()["points"].as_array().map(Vec::len), Some(2));
    }

    #[test]
    fn needs_a_start_point_and_at_least_one_leg() {
        let mut st = State::default();
        assert!(!ready(&st));
        st.start_x = "0".into();
        st.start_y = "0".into();
        assert!(!ready(&st), "a leg with no gisement/distance isn't ready yet");
        st.legs[0] = LegRow { gisement: "0".into(), distance: "10".into(), code: String::new() };
        assert!(ready(&st));
    }
}
