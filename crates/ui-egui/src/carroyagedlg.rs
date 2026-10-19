//! The Carroyage dialog: PLAN.CARROYAGE from a menu or the command line (no parameters) opens
//! this instead of erring on a missing `window`/`scale` - it needs a window picked and a scale
//! chosen first. "Select window" hides the dialog, runs PLAN.CARROYAGE_PICK on screen, and
//! [`resume`] brings the dialog back with the two corners it picked (unchanged if cancelled).
//!
//! The live preview (paper size, cross count, fit/doesn't-fit) is read back every frame from
//! PLAN.CARROYAGE itself, called with `preview: true` so it never touches the drawing - the
//! dialog never duplicates the engine's own fit/grid math.

use egui::vec2;
use serde_json::{Value, json};

use crate::CadApp;

#[derive(Clone)]
pub struct State {
    /// The window picked on screen (model-space corners), if any yet.
    window: Option<(cadcraft_geom::Vec2, cadcraft_geom::Vec2)>,
    paper: String,
    /// `None`: whichever orientation fits the picked window (the engine's own default).
    landscape: Option<bool>,
    scale_text: String,
    spacing_text: String,
    layer: String,
    layout: String,
    /// Set while PLAN.CARROYAGE_PICK is running on screen ([`resume`] clears it).
    picking: bool,
}

impl Default for State {
    fn default() -> Self {
        State {
            window: None,
            paper: "A4".into(),
            landscape: None,
            scale_text: "500".into(),
            spacing_text: "10".into(),
            layer: "CARROYAGE".into(),
            layout: String::new(),
            picking: false,
        }
    }
}

const PAPERS: [&str; 5] = ["A4", "A3", "A2", "A1", "A0"];

/// Normalized map scales (the 1:N series a drafter actually picks from), ascending.
const STANDARD_SCALES: [u32; 20] = [5, 10, 20, 40, 50, 100, 150, 200, 250, 500, 750, 1000, 1250, 1500, 2000, 2500, 4000, 5000, 10000, 20000];

/// The smallest normalized scale (least zoomed out) at which `window` fits on `paper_name`
/// (`landscape`: `None` auto-picks per the window's own aspect, same default PLAN.CARROYAGE
/// itself uses) - found by trying each standard scale through the engine's own preview, so the
/// fit check is exactly the one PLAN.CARROYAGE applies when actually run, never a second,
/// possibly-drifting copy of its margin/unit math.
fn suggest_scale(app: &mut CadApp, window: (cadcraft_geom::Vec2, cadcraft_geom::Vec2), paper: &str, landscape: Option<bool>) -> Option<u32> {
    let (a, b) = window;
    for &n in &STANDARD_SCALES {
        let mut v = json!({
            "window": { "min": [a.x, a.y], "max": [b.x, b.y] },
            "scale": n,
            "paper": paper,
            "spacingCm": 10.0,
            "preview": true,
        });
        if let Some(l) = landscape {
            v["landscape"] = json!(l);
        }
        if app.run("plan.carroyage", v).is_ok() {
            return Some(n);
        }
    }
    None
}

/// Typed, menu or toolbar invocation of PLAN.CARROYAGE (no parameters) opens this.
pub fn open(app: &mut CadApp) {
    app.carroyage_dlg = Some(State::default());
    app.ui.dialog = Some("carroyage".into());
}

/// Hide the dialog and run PLAN.CARROYAGE_PICK; [`resume`] brings the dialog back.
fn start_pick(app: &mut CadApp) {
    let Some(st) = app.carroyage_dlg.as_mut() else { return };
    st.picking = true;
    app.session.carroyage_pick = None;
    app.ui.dialog = None;
    app.start("plan.carroyage_pick");
}

/// Every frame: once a PLAN.CARROYAGE_PICK the dialog started has ended, take its window and
/// bring the dialog back (unchanged when the pick was cancelled).
pub fn resume(app: &mut CadApp) {
    if !app.carroyage_dlg.as_ref().is_some_and(|s| s.picking) {
        return;
    }
    if app.session.running.is_some() {
        return;
    }
    let picked = app.session.carroyage_pick.take();
    let Some(st) = app.carroyage_dlg.as_mut() else { return };
    st.picking = false;
    if let Some((a, b)) = picked {
        st.window = Some((a, b));
    }
    app.ui.dialog = Some("carroyage".into());
}

fn params(st: &State, preview: bool) -> Option<Value> {
    let (a, b) = st.window?;
    let scale: f64 = st.scale_text.trim().parse().ok()?;
    let spacing: f64 = st.spacing_text.trim().parse().ok()?;
    let mut v = json!({
        "window": { "min": [a.x, a.y], "max": [b.x, b.y] },
        "scale": scale,
        "paper": st.paper,
        "spacingCm": spacing,
        "preview": preview,
    });
    if let Some(l) = st.landscape {
        v["landscape"] = json!(l);
    }
    let layer = st.layer.trim();
    if !layer.is_empty() {
        v["layer"] = json!(layer);
    }
    let layout = st.layout.trim();
    if !layout.is_empty() {
        v["layout"] = json!(layout);
    }
    Some(v)
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let Some(mut st) = app.carroyage_dlg.clone() else {
        *open = false;
        return;
    };

    let (mut select, mut create, mut close, mut suggest) = (false, false, false, false);
    let old_paper = st.paper.clone();
    egui::Window::new(crate::tl!("Carroyage")).open(open).resizable(true).collapsible(false).default_size(vec2(380.0, 460.0)).show(ctx, |ui| {
        ui.horizontal(|ui| {
            select = ui.button(crate::tl!("Select window")).clicked();
            match st.window {
                Some((a, b)) => {
                    ui.label(format!("({:.2}, {:.2}) → ({:.2}, {:.2})", a.x, a.y, b.x, b.y));
                }
                None => {
                    ui.label(crate::tl!("No window selected yet."));
                }
            }
        });
        ui.separator();
        egui::Grid::new("cc_carroyage_grid").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
            ui.label(crate::tl!("Paper"));
            ui.horizontal(|ui| {
                let changed = egui::ComboBox::from_id_salt("cc_carroyage_paper")
                    .selected_text(st.paper.clone())
                    .show_ui(ui, |ui| {
                        for p in PAPERS {
                            ui.selectable_value(&mut st.paper, p.to_string(), p);
                        }
                    })
                    .inner
                    .is_some();
                // Re-suggesting right away when the paper changes means the frame/scale shown
                // always matches the sheet actually picked, instead of silently going stale
                // (or not fitting) until the person remembers to click Suggest themselves.
                suggest = ui.button(crate::tl!("Suggest scale")).clicked() || (changed && st.paper != old_paper);
            });
            ui.end_row();

            ui.label(crate::tl!("Orientation"));
            ui.horizontal(|ui| {
                ui.selectable_value(&mut st.landscape, None, crate::tl!("Auto"));
                ui.selectable_value(&mut st.landscape, Some(false), crate::tl!("Portrait"));
                ui.selectable_value(&mut st.landscape, Some(true), crate::tl!("Landscape"));
            });
            ui.end_row();

            ui.label(crate::tl!("Scale 1:"));
            ui.horizontal(|ui| {
                ui.text_edit_singleline(&mut st.scale_text);
                let current: Option<u32> = st.scale_text.trim().parse().ok();
                egui::ComboBox::from_id_salt("cc_carroyage_scale").selected_text(crate::tl!("Standard")).width(90.0).show_ui(ui, |ui| {
                    for n in STANDARD_SCALES {
                        if ui.selectable_label(current == Some(n), format!("1:{n}")).clicked() {
                            st.scale_text = n.to_string();
                        }
                    }
                });
            });
            ui.end_row();

            ui.label(crate::tl!("Cross spacing (paper cm)"));
            ui.text_edit_singleline(&mut st.spacing_text);
            ui.end_row();

            // The same spacing restated on the ground, in metres - what "N cm on paper" actually
            // means once the scale is applied, so the person doesn't have to do that math by hand.
            if let (Ok(spacing_cm), Ok(scale)) = (st.spacing_text.trim().parse::<f64>(), st.scale_text.trim().parse::<f64>()) {
                ui.label("");
                ui.label(crate::tf!("= {m} m on the ground", m = format!("{:.2}", spacing_cm / 100.0 * scale)));
                ui.end_row();
            }

            ui.label(crate::tl!("Layer"));
            ui.text_edit_singleline(&mut st.layer);
            ui.end_row();

            ui.label(crate::tl!("Layout (blank: next free name)"));
            ui.text_edit_singleline(&mut st.layout);
            ui.end_row();
        });
        ui.separator();

        match params(&st, true) {
            None if st.window.is_none() => {
                ui.label(crate::tl!("Select a window first."));
            }
            None => {
                ui.colored_label(ui.visuals().error_fg_color, crate::tl!("Enter a valid scale and spacing."));
            }
            Some(p) => match app.run("plan.carroyage", p) {
                Ok(v) => {
                    let crosses = v.get("crosses").and_then(Value::as_u64).unwrap_or(0);
                    let w = v.get("viewportWidth").and_then(Value::as_f64).unwrap_or(0.0);
                    let h = v.get("viewportHeight").and_then(Value::as_f64).unwrap_or(0.0);
                    ui.label(crate::tf!(
                        "Preview: {w}×{h} mm on paper, {crosses} cross(es)",
                        w = format!("{w:.0}"),
                        h = format!("{h:.0}"),
                        crosses = crosses
                    ));
                }
                Err(e) => {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
            },
        }
        ui.separator();
        ui.horizontal(|ui| {
            create = ui.add_enabled(params(&st, true).is_some(), egui::Button::new(crate::tl!("Create"))).clicked();
            close = ui.button(crate::tl!("Close")).clicked();
        });
    });

    if select {
        app.carroyage_dlg = Some(st.clone());
        start_pick(app);
        return;
    }
    if suggest && let Some(window) = st.window {
        match suggest_scale(app, window, &st.paper, st.landscape) {
            Some(n) => st.scale_text = n.to_string(),
            None => app.session.echo(crate::tf!(
                "No standard scale up to 1:{max} fits {paper} - pick a larger paper or a smaller window",
                max = STANDARD_SCALES[STANDARD_SCALES.len() - 1],
                paper = st.paper
            )),
        }
    }
    if create && let Some(p) = params(&st, false) {
        let r = app.run("plan.carroyage", p);
        app.session.echo(match &r {
            Ok(v) => crate::tf!(
                "Carroyage created in layout {layout} on layer {layer}",
                layout = v.get("layout").and_then(Value::as_str).unwrap_or_default(),
                layer = v.get("layer").and_then(Value::as_str).unwrap_or_default()
            ),
            Err(e) => e.clone(),
        });
        if r.is_ok() {
            close = true;
        }
    }
    if close {
        *open = false;
    }
    if *open {
        app.carroyage_dlg = Some(st);
    } else {
        app.carroyage_dlg = None;
    }
}

#[cfg(test)]
mod tests {
    use cadcraft_engine::geom::Vec2;
    use cadcraft_engine::{Input, Session};

    use super::*;
    use crate::{CadApp, Services};

    fn app() -> CadApp {
        CadApp::new(Session::new(), Services::default())
    }

    #[test]
    fn select_window_hides_the_dialog_and_brings_it_back_with_the_corners() {
        let mut app = app();
        open(&mut app);
        start_pick(&mut app);
        assert_eq!(app.ui.dialog, None, "hidden while picking");
        assert!(app.session.prompt_text().contains("first corner"));

        resume(&mut app);
        assert_eq!(app.ui.dialog, None, "still picking");

        app.session.input(Input::Point(Vec2::new(1.0, 2.0))).unwrap();
        app.session.input(Input::Point(Vec2::new(5.0, 6.0))).unwrap();
        resume(&mut app);
        assert_eq!(app.ui.dialog, Some("carroyage".into()), "dialog comes back");
        let st = app.carroyage_dlg.expect("state kept");
        assert_eq!(st.window, Some((Vec2::new(1.0, 2.0), Vec2::new(5.0, 6.0))));
    }

    #[test]
    fn cancelling_the_pick_keeps_the_dialog_without_a_window() {
        let mut app = app();
        open(&mut app);
        start_pick(&mut app);
        app.session.input(Input::Cancel).unwrap();
        resume(&mut app);
        assert_eq!(app.ui.dialog, Some("carroyage".into()));
        assert_eq!(app.carroyage_dlg.expect("state kept").window, None);
    }

    #[test]
    fn params_is_none_without_a_window_or_with_a_bad_number() {
        let mut st = State::default();
        assert!(params(&st, true).is_none(), "no window yet");
        st.window = Some((Vec2::ZERO, Vec2::new(10.0, 10.0)));
        st.scale_text = "not a number".into();
        assert!(params(&st, true).is_none(), "bad scale");
    }

    #[test]
    fn create_runs_the_command_and_closes_on_success() {
        let mut app = app();
        open(&mut app);
        let mut st = app.carroyage_dlg.clone().unwrap();
        st.window = Some((Vec2::new(0.0, 0.0), Vec2::new(4000.0, 4000.0)));
        st.scale_text = "1000".into();
        st.spacing_text = "1".into();
        app.carroyage_dlg = Some(st.clone());
        let p = params(&st, false).expect("params should be ready");
        let r = app.run("plan.carroyage", p);
        assert!(r.is_ok(), "{r:?}");
    }
}
