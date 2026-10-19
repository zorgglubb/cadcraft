//! The Radiation and Zero Value (VO) dialogs: TOPO.RADIATION/TOPO.ZERO_VALUE from a menu or the
//! command line (no parameters) open this instead of erring on a missing `station`/`vo` - they
//! need a station, an orientation (a known VO or a set of backsight references to compute one
//! from) and, for Radiation, at least one observation entered first.
//!
//! Both commands share the same station/orientation fields, so one `State` with a `mode` serves
//! both: Zero Value hides the observations table and its button just computes and reports VO/σ;
//! Radiation shows the table and its button computes every row's coordinate and inserts it as a
//! POINT (TOPO.RADIATION itself).

use egui::vec2;
use serde_json::{Value, json};

use crate::CadApp;

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    Radiation,
    ZeroValue,
}

/// One backsight used to compute VO from `references` instead of typing a known value.
#[derive(Clone, Default)]
pub struct ReferenceRow {
    pub x: String,
    pub y: String,
    pub z: String,
    pub hz: String,
}

/// One polar shot (Radiation only): angle + distance from the station to a new point.
#[derive(Clone, Default)]
pub struct ObservationRow {
    pub hz: String,
    pub zenith: String,
    pub dist: String,
    pub instrument_height: String,
    pub target_height: String,
    pub code: String,
}

#[derive(Clone)]
pub struct State {
    pub mode: Mode,
    pub station_x: String,
    pub station_y: String,
    pub station_z: String,
    /// `true`: VO comes from `references` below; `false`: the typed `vo` value is used as-is.
    pub from_references: bool,
    pub vo: String,
    pub references: Vec<ReferenceRow>,
    pub observations: Vec<ObservationRow>,
    pub label: bool,
    /// The last computed VO/σ, shown after a Zero Value run or a references-based Radiation run.
    pub last_vo: Option<(f64, Option<f64>)>,
}

impl State {
    fn new(mode: Mode) -> Self {
        State {
            mode,
            station_x: String::new(),
            station_y: String::new(),
            station_z: "0".into(),
            from_references: true,
            vo: "0".into(),
            references: vec![ReferenceRow::default()],
            observations: vec![ObservationRow::default()],
            label: true,
            last_vo: None,
        }
    }
}

pub fn open(app: &mut CadApp, mode: Mode) {
    app.station_dlg = Some(State::new(mode));
    app.ui.dialog = Some("station".into());
}

fn num(s: &str) -> Option<f64> {
    let t = s.trim().replace(',', ".");
    if t.is_empty() { None } else { t.parse::<f64>().ok() }
}

fn station_json(st: &State) -> Option<Value> {
    Some(json!([num(&st.station_x)?, num(&st.station_y)?, num(&st.station_z).unwrap_or(0.0)]))
}

fn references_json(st: &State) -> Option<Value> {
    let rows: Option<Vec<Value>> = st
        .references
        .iter()
        .filter(|r| !r.x.trim().is_empty() || !r.y.trim().is_empty() || !r.hz.trim().is_empty())
        .map(|r| Some(json!({ "point": [num(&r.x)?, num(&r.y)?, num(&r.z).unwrap_or(0.0)], "hzReading": num(&r.hz)? })))
        .collect();
    rows.map(Value::Array)
}

fn observations_json(st: &State) -> Option<Value> {
    let rows: Option<Vec<Value>> = st
        .observations
        .iter()
        .filter(|o| !o.hz.trim().is_empty() || !o.zenith.trim().is_empty() || !o.dist.trim().is_empty())
        .map(|o| {
            Some(json!({
                "hzReading": num(&o.hz)?,
                "zenith": num(&o.zenith)?,
                "slopeDistance": num(&o.dist)?,
                "instrumentHeight": num(&o.instrument_height).unwrap_or(0.0),
                "targetHeight": num(&o.target_height).unwrap_or(0.0),
                "code": (!o.code.trim().is_empty()).then(|| o.code.trim().to_string()),
            }))
        })
        .collect();
    rows.map(Value::Array)
}

/// `true` once the station and an orientation (known VO or at least one usable reference) are
/// filled in - independent of whether there's anything to compute from them yet.
fn orientation_ready(st: &State) -> bool {
    station_json(st).is_some()
        && if st.from_references { references_json(st).is_some_and(|v| !v.as_array().is_some_and(Vec::is_empty)) } else { num(&st.vo).is_some() }
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let Some(mut st) = app.station_dlg.clone() else {
        *open = false;
        return;
    };

    let title = match st.mode {
        Mode::Radiation => crate::tl!("Radiation"),
        Mode::ZeroValue => crate::tl!("Zero Value (VO)"),
    };
    let (mut run, mut close) = (false, false);
    egui::Window::new(title).open(open).resizable(true).collapsible(false).default_size(vec2(480.0, 420.0)).show(ctx, |ui| {
        ui.label(crate::tl!("Station"));
        ui.horizontal(|ui| {
            ui.label("X");
            ui.text_edit_singleline(&mut st.station_x);
            ui.label("Y");
            ui.text_edit_singleline(&mut st.station_y);
            ui.label("Z");
            ui.text_edit_singleline(&mut st.station_z);
        });
        ui.separator();
        ui.horizontal(|ui| {
            if ui.selectable_label(!st.from_references, crate::tl!("Known VO value")).clicked() {
                st.from_references = false;
            }
            if ui.selectable_label(st.from_references, crate::tl!("Compute from references")).clicked() {
                st.from_references = true;
            }
        });
        if st.from_references {
            egui::Grid::new("cc_station_refs").striped(true).show(ui, |ui| {
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
        if let Some((vo, sigma)) = st.last_vo {
            ui.separator();
            ui.label(match sigma {
                Some(s) => crate::tf!("Computed VO: {vo} gon (σ {sigma})", vo = format!("{vo:.4}"), sigma = format!("{s:.4}")),
                None => crate::tf!("VO: {vo} gon", vo = format!("{vo:.4}")),
            });
        }

        if st.mode == Mode::Radiation {
            ui.separator();
            ui.label(crate::tl!("Observations"));
            egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                egui::Grid::new("cc_station_obs").striped(true).show(ui, |ui| {
                    ui.label(crate::tl!("Hz reading (gon)"));
                    ui.label(crate::tl!("Zenith (gon)"));
                    ui.label(crate::tl!("Slope distance"));
                    ui.label(crate::tl!("Instrument height"));
                    ui.label(crate::tl!("Target height"));
                    ui.label(crate::tl!("Code"));
                    ui.end_row();
                    let mut remove: Option<usize> = None;
                    for (i, o) in st.observations.iter_mut().enumerate() {
                        ui.text_edit_singleline(&mut o.hz);
                        ui.text_edit_singleline(&mut o.zenith);
                        ui.text_edit_singleline(&mut o.dist);
                        ui.text_edit_singleline(&mut o.instrument_height);
                        ui.text_edit_singleline(&mut o.target_height);
                        ui.text_edit_singleline(&mut o.code);
                        if ui.button("✕").clicked() {
                            remove = Some(i);
                        }
                        ui.end_row();
                    }
                    if let Some(i) = remove {
                        st.observations.remove(i);
                    }
                });
            });
            if ui.button(crate::tl!("Add observation")).clicked() {
                st.observations.push(ObservationRow::default());
            }
            ui.checkbox(&mut st.label, crate::tl!("Show code label"));
        }

        ui.separator();
        ui.horizontal(|ui| {
            let label = match st.mode {
                Mode::Radiation => crate::tl!("Compute"),
                Mode::ZeroValue => crate::tl!("Compute VO"),
            };
            run = ui.add_enabled(orientation_ready(&st), egui::Button::new(label)).clicked();
            close = ui.button(crate::tl!("Close")).clicked();
        });
    });
    if close {
        *open = false;
    }

    if run {
        let Some(station) = station_json(&st) else {
            app.session.echo(crate::i18n::t("Station X/Y are required"));
            app.station_dlg = Some(st);
            return;
        };
        let mut params = serde_json::Map::new();
        params.insert("station".into(), station);
        if st.from_references {
            if let Some(refs) = references_json(&st) {
                params.insert("references".into(), refs);
            }
        } else if let Some(vo) = num(&st.vo) {
            params.insert("vo".into(), json!(vo));
        }

        match st.mode {
            Mode::ZeroValue => {
                let r = app.run("topo.zero_value", Value::Object(params));
                match &r {
                    Ok(v) => st.last_vo = Some((v["vo"].as_f64().unwrap_or(0.0), v["sigma"].as_f64())),
                    Err(e) => app.session.echo(e.clone()),
                }
            }
            Mode::Radiation => {
                let Some(obs) = observations_json(&st) else {
                    app.session.echo(crate::i18n::t("At least one observation is required"));
                    app.station_dlg = Some(st);
                    return;
                };
                params.insert("observations".into(), obs);
                params.insert("label".into(), json!(st.label));
                let r = app.run("topo.radiation", Value::Object(params));
                match &r {
                    Ok(v) => {
                        st.last_vo = Some((v["vo"].as_f64().unwrap_or(0.0), v["sigma"].as_f64()));
                        app.session.echo(crate::tf!("Inserted {n} point(s)", n = v["points"].as_array().map(Vec::len).unwrap_or(0)));
                        app.canvas.zoom_pending = true;
                    }
                    Err(e) => app.session.echo(e.clone()),
                }
            }
        }
    }

    if *open {
        app.station_dlg = Some(st);
    } else {
        app.station_dlg = None;
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
    fn zero_value_with_references_computes_vo() {
        let mut app = app_with_doc();
        open(&mut app, Mode::ZeroValue);
        let mut st = app.station_dlg.clone().unwrap();
        st.station_x = "1000".into();
        st.station_y = "1000".into();
        st.references[0] = ReferenceRow { x: "1100".into(), y: "1000".into(), z: "0".into(), hz: "0".into() };
        app.station_dlg = Some(st);

        let st2 = app.station_dlg.clone().unwrap();
        assert!(orientation_ready(&st2));
        let r = app
            .run("topo.zero_value", json!({ "station": [1000.0, 1000.0, 0.0], "references": [{"point": [1100.0, 1000.0, 0.0], "hzReading": 0.0}] }));
        assert!(r.is_ok(), "{r:?}");
    }

    #[test]
    fn radiation_with_known_vo_inserts_a_point() {
        let mut app = app_with_doc();
        open(&mut app, Mode::Radiation);
        let mut st = app.station_dlg.clone().unwrap();
        st.station_x = "1000".into();
        st.station_y = "1000".into();
        st.from_references = false;
        st.vo = "0".into();
        st.observations[0] = ObservationRow {
            hz: "50".into(),
            zenith: "100".into(),
            dist: "10".into(),
            instrument_height: "0".into(),
            target_height: "0".into(),
            code: "P1".into(),
        };
        app.station_dlg = Some(st.clone());

        assert!(orientation_ready(&st));
        let r = app.run(
            "topo.radiation",
            json!({
                "station": [1000.0, 1000.0, 0.0],
                "vo": 0.0,
                "observations": [{"hzReading": 50.0, "zenith": 100.0, "slopeDistance": 10.0, "code": "P1"}],
            }),
        );
        assert!(r.is_ok(), "{r:?}");
        assert_eq!(r.unwrap()["points"].as_array().map(Vec::len), Some(1));
    }
}
