//! The Coordinate System dialog: PLAN.SET_CRS, PLAN.GET_CRS and PLAN.CONVERT_CRS from a menu or
//! the command line (no parameters) open this instead of erring on a missing `epsg`/`to` - they
//! all need a system picked first. It reads the registry through PLAN.LIST_CRS and the drawing's
//! own system through PLAN.GET_CRS, so the UI never duplicates what the engine already knows.
//!
//! Assign only tags the drawing (PLAN.SET_CRS); Convert actually reprojects every point in it
//! (PLAN.CONVERT_CRS) - see that command's own doc comment for which entity kinds and systems it
//! covers. A JSON call with real parameters (a script, the control channel, MCP) never opens this
//! and runs the plain command as given.

use egui::vec2;
use serde_json::{Value, json};

use crate::CadApp;

#[derive(Clone, Default)]
struct State {
    query: String,
    selected: Option<u32>,
}

fn id() -> egui::Id {
    egui::Id::new("cc_crs_dialog_state")
}

/// Typed, menu or toolbar invocation of any of the three commands (no parameters) opens this.
pub fn open(app: &mut CadApp) {
    app.ui.dialog = Some("crs".into());
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let mut state = ctx.data_mut(|d| d.get_temp::<State>(id())).unwrap_or_default();

    // `json!({})`, not `Value::Null`: a null `plan.get_crs`/`plan.set_crs`/`plan.convert_crs`
    // call is what opens this dialog in the first place (`menus.rs`), so calling back in with
    // `Value::Null` here would just reopen it instead of reading the drawing's system.
    let current = app.run("plan.get_crs", json!({})).unwrap_or(Value::Null);
    let current_epsg = current.get("epsg").and_then(Value::as_u64);
    let current_name = current.get("name").and_then(Value::as_str).unwrap_or_default().to_string();

    let list = app.run("plan.list_crs", json!({ "query": state.query })).unwrap_or(Value::Null);
    let systems: Vec<(u32, String, bool)> = list
        .get("systems")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|s| {
                    Some((u32::try_from(s.get("epsg")?.as_u64()?).ok()?, s.get("name")?.as_str()?.to_string(), s.get("hasDefinition")?.as_bool()?))
                })
                .collect()
        })
        .unwrap_or_default();

    let (mut assign, mut convert, mut close) = (false, false, false);
    egui::Window::new(crate::tl!("Coordinate System")).open(open).resizable(true).collapsible(false).default_size(vec2(400.0, 460.0)).show(
        ctx,
        |ui| {
            match current_epsg {
                Some(e) => {
                    ui.label(format!("{}: EPSG:{e} — {current_name}", crate::tl!("Current system")));
                }
                None => {
                    ui.label(crate::tl!("No coordinate system is assigned yet."));
                }
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(crate::tl!("Search"));
                ui.text_edit_singleline(&mut state.query);
            });
            egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                for (epsg, name, has_def) in &systems {
                    let label = if *has_def {
                        format!("EPSG:{epsg} — {name}")
                    } else {
                        format!("EPSG:{epsg} — {name} ({})", crate::tl!("no definition"))
                    };
                    if ui.selectable_label(state.selected == Some(*epsg), label).clicked() {
                        state.selected = Some(*epsg);
                    }
                }
                if systems.is_empty() {
                    ui.label(crate::tl!("No coordinate system matches that search."));
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                let selected_has_def = state.selected.is_some_and(|e| systems.iter().any(|s| s.0 == e && s.2));
                assign = ui
                    .add_enabled(state.selected.is_some(), egui::Button::new(crate::tl!("Assign")))
                    .on_hover_text(crate::tl!("Tags the drawing with this system; does not move anything."))
                    .clicked();
                convert = ui
                    .add_enabled(selected_has_def && current_epsg.is_some(), egui::Button::new(crate::tl!("Convert...")))
                    .on_hover_text(crate::tl!("Reprojects every point in the drawing from the current system to this one."))
                    .clicked();
                close = ui.button(crate::tl!("Close")).clicked();
            });
        },
    );

    if assign && let Some(epsg) = state.selected {
        let r = app.run("plan.set_crs", json!({ "epsg": epsg }));
        app.session.echo(match &r {
            Ok(v) => crate::tf!("Assigned {name}", name = v.get("name").and_then(Value::as_str).unwrap_or_default()),
            Err(e) => e.clone(),
        });
    }
    if convert && let Some(epsg) = state.selected {
        let r = app.run("plan.convert_crs", json!({ "to": epsg }));
        app.session.echo(match &r {
            Ok(v) => crate::tf!(
                "Converted {n} object(s) to EPSG:{to}",
                n = v.get("converted").and_then(Value::as_u64).unwrap_or(0),
                to = v.get("to").and_then(Value::as_u64).unwrap_or(0)
            ),
            Err(e) => e.clone(),
        });
    }
    if close {
        *open = false;
    }
    if *open {
        ctx.data_mut(|d| d.insert_temp(id(), state));
    } else {
        ctx.data_mut(|d| d.remove::<State>(id()));
    }
}
