//! Small dialogs for the Plan menu's conversion commands that take one optional parameter:
//! CONVERT.ELLIPSE2PL and CONVERT.SPLINE2PL both just need a segment count, so typed, menu or
//! toolbar invocation (no parameters) opens one of these instead of silently using the default
//! with no way to change it. Both run on the CURRENT SELECTION - no entity picking here, the
//! same default the plain command already falls back to when `handles` is omitted.

use egui::vec2;
use serde_json::{Value, json};

use crate::CadApp;

#[derive(Clone)]
pub struct EllipseToPolyState {
    segments_text: String,
}

impl Default for EllipseToPolyState {
    fn default() -> Self {
        EllipseToPolyState { segments_text: "36".into() }
    }
}

pub fn open_ellipse2pl(app: &mut CadApp) {
    app.ellipse2pl_dlg = Some(EllipseToPolyState::default());
    app.ui.dialog = Some("ellipse2pl".into());
}

fn segments(text: &str) -> Option<u64> {
    text.trim().parse::<u64>().ok().filter(|&n| n >= 3)
}

pub fn ellipse2pl_dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let Some(mut st) = app.ellipse2pl_dlg.clone() else {
        *open = false;
        return;
    };

    let (mut convert, mut close) = (false, false);
    egui::Window::new(crate::tl!("Ellipse to Polyline")).open(open).resizable(false).collapsible(false).default_size(vec2(320.0, 140.0)).show(
        ctx,
        |ui| {
            egui::Grid::new("cc_ellipse2pl_grid").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                ui.label(crate::tl!("Segments (full ellipse)"));
                ui.text_edit_singleline(&mut st.segments_text);
                ui.end_row();
            });
            if segments(&st.segments_text).is_none() {
                ui.colored_label(ui.visuals().error_fg_color, crate::tl!("Enter a whole number of 3 or more."));
            }
            ui.separator();
            ui.horizontal(|ui| {
                convert = ui.add_enabled(segments(&st.segments_text).is_some(), egui::Button::new(crate::tl!("Convert"))).clicked();
                close = ui.button(crate::tl!("Close")).clicked();
            });
        },
    );
    if close {
        *open = false;
    }

    if convert && let Some(n) = segments(&st.segments_text) {
        let r = app.run("convert.ellipse2pl", json!({ "segments": n }));
        app.session.echo(match &r {
            Ok(v) => crate::tf!("{n} ellipse(s) converted", n = v.get("handles").and_then(Value::as_array).map_or(0, Vec::len)),
            Err(e) => e.clone(),
        });
        if r.is_ok() {
            *open = false;
        }
    }
    if *open {
        app.ellipse2pl_dlg = Some(st);
    } else {
        app.ellipse2pl_dlg = None;
    }
}

#[derive(Clone)]
pub struct SplineToPolyState {
    segments_text: String,
}

impl Default for SplineToPolyState {
    fn default() -> Self {
        SplineToPolyState { segments_text: "50".into() }
    }
}

pub fn open_spline2pl(app: &mut CadApp) {
    app.spline2pl_dlg = Some(SplineToPolyState::default());
    app.ui.dialog = Some("spline2pl".into());
}

pub fn spline2pl_dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let Some(mut st) = app.spline2pl_dlg.clone() else {
        *open = false;
        return;
    };

    let (mut convert, mut close) = (false, false);
    egui::Window::new(crate::tl!("Spline to Polyline")).open(open).resizable(false).collapsible(false).default_size(vec2(320.0, 140.0)).show(
        ctx,
        |ui| {
            egui::Grid::new("cc_spline2pl_grid").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                ui.label(crate::tl!("Segments"));
                ui.text_edit_singleline(&mut st.segments_text);
                ui.end_row();
            });
            if segments(&st.segments_text).is_none() {
                ui.colored_label(ui.visuals().error_fg_color, crate::tl!("Enter a whole number of 3 or more."));
            }
            ui.separator();
            ui.horizontal(|ui| {
                convert = ui.add_enabled(segments(&st.segments_text).is_some(), egui::Button::new(crate::tl!("Convert"))).clicked();
                close = ui.button(crate::tl!("Close")).clicked();
            });
        },
    );
    if close {
        *open = false;
    }

    if convert && let Some(n) = segments(&st.segments_text) {
        let r = app.run("convert.spline2pl", json!({ "segments": n }));
        app.session.echo(match &r {
            Ok(v) => crate::tf!("{n} spline(s) converted", n = v.get("handles").and_then(Value::as_array).map_or(0, Vec::len)),
            Err(e) => e.clone(),
        });
        if r.is_ok() {
            *open = false;
        }
    }
    if *open {
        app.spline2pl_dlg = Some(st);
    } else {
        app.spline2pl_dlg = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_rejects_zero_negative_and_non_numbers() {
        assert_eq!(segments("36"), Some(36));
        assert_eq!(segments(" 50 "), Some(50));
        assert_eq!(segments("0"), None);
        assert_eq!(segments("2"), None);
        assert_eq!(segments("abc"), None);
        assert_eq!(segments(""), None);
    }
}
