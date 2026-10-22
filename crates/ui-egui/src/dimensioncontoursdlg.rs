//! Topo 3D ▸ Dimension Contours... (TERRAIN.DIMENSION_CONTOURS): labels the elevation of
//! existing contour lines (whatever [`crate::createcontoursdlg`] or anything else left on the
//! chosen layers), either at regular intervals along each contour's length or wherever a
//! user-given axis crosses them.
//!
//! **Scope, stated plainly**: the axis's two points are typed in as coordinates rather than
//! picked on screen - a real on-screen two-point pick would need its own interactive command and
//! `Session` field (the way [`crate::createmntdlg`]'s manual point selection uses
//! `data.pick_entities`/`Session::table_pick`), which is a larger, separate increment; typed
//! coordinates are fully usable in the meantime for an axis drawn or measured elsewhere.

use egui::vec2;
use serde_json::{Value, json};

use crate::CadApp;

#[derive(Clone, Copy, PartialEq, Default)]
pub enum Mode {
    #[default]
    Regular,
    Axis,
}

#[derive(Clone)]
pub struct State {
    pub layers: Vec<String>,
    pub mode: Mode,
    pub interval: String,
    pub axis_ax: String,
    pub axis_ay: String,
    pub axis_bx: String,
    pub axis_by: String,
    pub decimals: u32,
    pub height: String,
    pub suffix: String,
    pub target_layer: String,
    pub last_error: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        State {
            layers: Vec::new(),
            mode: Mode::default(),
            interval: "10".into(),
            axis_ax: String::new(),
            axis_ay: String::new(),
            axis_bx: String::new(),
            axis_by: String::new(),
            decimals: 2,
            height: String::new(),
            suffix: String::new(),
            // Our own naming, not a reference application's.
            target_layer: "COTES_COURBES".into(),
            last_error: None,
        }
    }
}

pub fn open(app: &mut CadApp) {
    app.dimension_contours_dlg = Some(State::default());
    app.ui.dialog = Some("dimensioncontours".into());
}

fn ready(st: &State) -> bool {
    if st.layers.is_empty() {
        return false;
    }
    match st.mode {
        Mode::Regular => st.interval.trim().parse::<f64>().is_ok_and(|v| v > 0.0),
        Mode::Axis => {
            [&st.axis_ax, &st.axis_ay, &st.axis_bx, &st.axis_by].iter().all(|f| f.trim().parse::<f64>().is_ok())
                && (st.axis_ax.trim() != st.axis_bx.trim() || st.axis_ay.trim() != st.axis_by.trim())
        }
    }
}

fn params(st: &State, draw: bool) -> Value {
    let mut p = serde_json::Map::new();
    p.insert("layers".into(), json!(st.layers));
    p.insert("mode".into(), json!(if st.mode == Mode::Axis { "axis" } else { "regular" }));
    p.insert("decimals".into(), json!(st.decimals));
    p.insert("suffix".into(), json!(st.suffix));
    p.insert("layer".into(), json!(if st.target_layer.trim().is_empty() { "COTES_COURBES" } else { st.target_layer.trim() }));
    p.insert("draw".into(), json!(draw));
    if let Ok(h) = st.height.trim().parse::<f64>() {
        p.insert("height".into(), json!(h));
    }
    match st.mode {
        Mode::Regular => {
            if let Ok(i) = st.interval.trim().parse::<f64>() {
                p.insert("interval".into(), json!(i));
            }
        }
        Mode::Axis => {
            let f = |s: &str| s.trim().parse::<f64>().unwrap_or(0.0);
            p.insert("axis".into(), json!({ "a": [f(&st.axis_ax), f(&st.axis_ay)], "b": [f(&st.axis_bx), f(&st.axis_by)] }));
        }
    }
    Value::Object(p)
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let Some(mut st) = app.dimension_contours_dlg.clone() else {
        *open = false;
        return;
    };

    let layer_choices = crate::editaltitudesdlg::layer_choices(app);

    let mut run = false;
    let mut close = false;
    egui::Window::new(crate::tl!("Dimension Contours...")).open(open).resizable(true).collapsible(false).default_size(vec2(480.0, 480.0)).show(
        ctx,
        |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.label(crate::tl!("Contour layer(s) to label"));
                crate::editaltitudesdlg::checklist(ui, "cc_dimcontours_layers", &layer_choices, &mut st.layers);
                ui.separator();

                ui.label(crate::tl!("Label placement"));
                ui.radio_value(&mut st.mode, Mode::Regular, crate::tl!("Regular intervals along each contour"));
                if st.mode == Mode::Regular {
                    ui.horizontal(|ui| {
                        ui.label(crate::tl!("Interval"));
                        ui.add(egui::TextEdit::singleline(&mut st.interval).desired_width(80.0));
                    });
                }
                ui.radio_value(&mut st.mode, Mode::Axis, crate::tl!("Where an axis crosses them"));
                if st.mode == Mode::Axis {
                    egui::Grid::new("cc_dimcontours_axis").num_columns(3).show(ui, |ui| {
                        ui.label("");
                        ui.label("X");
                        ui.label("Y");
                        ui.end_row();
                        ui.label(crate::tl!("Axis point 1"));
                        ui.add(egui::TextEdit::singleline(&mut st.axis_ax).desired_width(80.0));
                        ui.add(egui::TextEdit::singleline(&mut st.axis_ay).desired_width(80.0));
                        ui.end_row();
                        ui.label(crate::tl!("Axis point 2"));
                        ui.add(egui::TextEdit::singleline(&mut st.axis_bx).desired_width(80.0));
                        ui.add(egui::TextEdit::singleline(&mut st.axis_by).desired_width(80.0));
                        ui.end_row();
                    });
                }
                ui.separator();

                ui.label(crate::tl!("Label text"));
                egui::Grid::new("cc_dimcontours_text").num_columns(2).show(ui, |ui| {
                    ui.label(crate::tl!("Decimals"));
                    ui.add(egui::DragValue::new(&mut st.decimals).range(0..=6));
                    ui.end_row();
                    ui.label(crate::tl!("Text height (blank: drawing default)"));
                    ui.add(egui::TextEdit::singleline(&mut st.height).desired_width(80.0));
                    ui.end_row();
                    ui.label(crate::tl!("Suffix (e.g. \" m\")"));
                    ui.add(egui::TextEdit::singleline(&mut st.suffix).desired_width(80.0));
                    ui.end_row();
                    ui.label(crate::tl!("Target layer"));
                    ui.text_edit_singleline(&mut st.target_layer);
                    ui.end_row();
                });
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

    if run {
        let p = params(&st, true);
        match app.run("terrain.dimension_contours", p) {
            Ok(v) => {
                let n = v["labelCount"].as_u64().unwrap_or(0);
                app.session.echo(crate::tf!("{n} elevation label(s) created", n = n));
                st.last_error = None;
                *open = false;
            }
            Err(e) => st.last_error = Some(e),
        }
    }

    if *open {
        app.dimension_contours_dlg = Some(st);
    } else {
        app.dimension_contours_dlg = None;
    }
}

#[cfg(test)]
mod tests {
    use cadcraft_doc::{EntityKind, Line};
    use cadcraft_engine::Session;
    use cadcraft_geom::Vec3;
    use serde_json::json;

    use super::*;
    use crate::Services;

    fn app_with_doc() -> CadApp {
        let mut s = Session::new();
        s.execute("new", &json!({})).unwrap();
        CadApp::new(s, Services::default())
    }

    #[test]
    fn ready_needs_layers_and_a_complete_placement() {
        let st = State { layers: vec!["C1".into()], ..State::default() };
        assert!(ready(&st), "regular mode with a default interval should be ready");

        let st = State { layers: Vec::new(), ..State::default() };
        assert!(!ready(&st), "no layers picked yet");

        let st = State { layers: vec!["C1".into()], mode: Mode::Axis, ..State::default() };
        assert!(!ready(&st), "axis mode needs its four coordinates");

        let st = State {
            layers: vec!["C1".into()],
            mode: Mode::Axis,
            axis_ax: "0".into(),
            axis_ay: "0".into(),
            axis_bx: "10".into(),
            axis_by: "0".into(),
            ..State::default()
        };
        assert!(ready(&st));
    }

    #[test]
    fn params_builds_the_axis_object_only_in_axis_mode() {
        let st = State {
            layers: vec!["C1".into()],
            mode: Mode::Axis,
            axis_ax: "1".into(),
            axis_ay: "2".into(),
            axis_bx: "3".into(),
            axis_by: "4".into(),
            ..State::default()
        };
        let p = params(&st, true);
        assert_eq!(p["axis"]["a"], json!([1.0, 2.0]));
        assert_eq!(p["axis"]["b"], json!([3.0, 4.0]));
        assert!(p.get("interval").is_none());

        let st = State { layers: vec!["C1".into()], mode: Mode::Regular, interval: "2.5".into(), ..State::default() };
        let p = params(&st, true);
        assert_eq!(p["interval"].as_f64(), Some(2.5));
        assert!(p.get("axis").is_none());
    }

    #[test]
    fn running_regular_mode_labels_a_drawn_contour() {
        let mut app = app_with_doc();
        app.session.execute("setvar", &json!({ "name": "CLAYER", "value": "0" })).ok();
        app.session.add_entity(EntityKind::Line(Line { a: Vec3::new(0.0, 0.0, 5.0), b: Vec3::new(20.0, 0.0, 5.0) })).unwrap();

        open(&mut app);
        let st = State { layers: vec!["0".into()], interval: "5".into(), ..app.dimension_contours_dlg.clone().unwrap() };
        let out = app.run("terrain.dimension_contours", params(&st, true)).unwrap();
        assert!(out["labelCount"].as_u64().unwrap_or(0) > 0);
    }
}
