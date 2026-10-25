//! The Numérotation auto dialog: PLAN.AUTONUM from a menu or the command line (no parameters)
//! opens this instead of making the user type seven prompt answers before the first click lands.
//! "Start" feeds each field to the already-existing PLAN.AUTONUM prompt chain as if typed
//! (`Session::input`), then leaves the command running so clicking the canvas places the
//! incrementing text exactly like typing it at the command line would - the dialog is a front
//! end for that chain, not a second implementation of it.

use cadcraft_engine::Input;
use egui::vec2;

use crate::CadApp;

#[derive(Clone)]
pub struct State {
    start_text: String,
    increment_text: String,
    prefix: String,
    suffix: String,
    layer: String,
    height_text: String,
    rotation_text: String,
}

impl Default for State {
    fn default() -> Self {
        State {
            start_text: "1".into(),
            increment_text: "1".into(),
            prefix: String::new(),
            suffix: String::new(),
            layer: String::new(),
            height_text: String::new(),
            rotation_text: "0".into(),
        }
    }
}

pub fn open(app: &mut CadApp) {
    app.autonum_dlg = Some(State::default());
    app.ui.dialog = Some("autonum".into());
}

fn ready(st: &State) -> bool {
    st.start_text.trim().parse::<i64>().is_ok()
        && (st.increment_text.trim().is_empty() || st.increment_text.trim().parse::<i64>().is_ok())
        && (st.height_text.trim().is_empty() || st.height_text.trim().parse::<f64>().is_ok_and(|h| h > 0.0))
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let Some(mut st) = app.autonum_dlg.clone() else {
        *open = false;
        return;
    };

    let (mut start, mut close) = (false, false);
    egui::Window::new(crate::tl!("Numérotation auto")).open(open).resizable(true).collapsible(false).default_size(vec2(340.0, 300.0)).show(
        ctx,
        |ui| {
            egui::Grid::new("cc_autonum_grid").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                ui.label(crate::tl!("Starting value"));
                ui.text_edit_singleline(&mut st.start_text);
                ui.end_row();

                ui.label(crate::tl!("Index increment"));
                ui.text_edit_singleline(&mut st.increment_text);
                ui.end_row();

                ui.label(crate::tl!("Prefix string"));
                ui.text_edit_singleline(&mut st.prefix);
                ui.end_row();

                ui.label(crate::tl!("Suffix string"));
                ui.text_edit_singleline(&mut st.suffix);
                ui.end_row();

                ui.label(crate::tl!("Target layer (blank = current)"));
                ui.text_edit_singleline(&mut st.layer);
                ui.end_row();

                ui.label(crate::tl!("Text height (blank = drawing default)"));
                ui.text_edit_singleline(&mut st.height_text);
                ui.end_row();

                ui.label(crate::tl!("Text rotation"));
                ui.text_edit_singleline(&mut st.rotation_text);
                ui.end_row();
            });
            if !ready(&st) {
                ui.colored_label(ui.visuals().error_fg_color, crate::tl!("The starting value, increment and height must be numbers."));
            }
            ui.label(crate::tl!("After Start, click repeatedly on the drawing to place the numbering; press Enter or Escape to stop."));
            ui.separator();
            ui.horizontal(|ui| {
                start = ui.add_enabled(ready(&st), egui::Button::new(crate::tl!("Start"))).clicked();
                close = ui.button(crate::tl!("Close")).clicked();
            });
        },
    );
    if close {
        *open = false;
    }

    if start && ready(&st) {
        *open = false;
        if app.session.start("plan.autonum").is_ok() {
            let text_or_enter = |s: &str| if s.trim().is_empty() { Input::Enter } else { Input::Text(s.trim().to_string()) };
            // Same order as PLAN.AUTONUM's own prompt chain: start, increment, prefix, suffix,
            // layer, height, rotation - each fed as if the user had typed it, so a step this
            // dialog gets wrong surfaces the exact same error the command line would show.
            for input in [
                text_or_enter(&st.start_text),
                text_or_enter(&st.increment_text),
                text_or_enter(&st.prefix),
                text_or_enter(&st.suffix),
                text_or_enter(&st.layer),
                text_or_enter(&st.height_text),
                text_or_enter(&st.rotation_text),
            ] {
                if app.session.input(input).is_err() {
                    break;
                }
            }
        }
    }
    if *open {
        app.autonum_dlg = Some(st);
    } else {
        app.autonum_dlg = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_requires_numeric_start_increment_and_height() {
        let mut st = State::default();
        assert!(ready(&st), "defaults are already valid");
        st.start_text = "abc".into();
        assert!(!ready(&st));
        st.start_text = "5".into();
        st.height_text = "not a number".into();
        assert!(!ready(&st));
        st.height_text = "2.5".into();
        assert!(ready(&st));
    }
}
