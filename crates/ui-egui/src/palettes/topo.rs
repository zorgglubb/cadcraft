//! The "Topo" Tool Sets tab: a thin face on `crates/engine/src/cmd/foncier.rs`'s
//! Foncier/Parcellaire commands and the Land Records (EDIGEO) importer, grouped the way
//! Covadis groups its own ribbon (one tab per subject, not one menu item per command).
//!
//! **Scope, stated plainly**: this is a face for the commands that already exist, not a new
//! workflow of its own. `topo.merge_parcels` needs an object selection the same way MOVE or
//! ERASE do, so "Merge Selection" simply reads the canvas's current pickset
//! (`Session::selection`) rather than opening its own picker. `topo.import_edigeo` needs every
//! `.VEC` file of one lot at once, and there is no multi-file picker service yet (`Services`
//! only exposes a single-file `pick_open`) - so files are added one at a time to a small list
//! kept here, which is a real limitation worth outgrowing rather than a worked-around one.

use egui::{RichText, vec2};
use serde_json::{Value, json};

use crate::CadApp;
use crate::theme::Tokens;

#[derive(Clone, Default)]
struct State {
    /// `.VEC` files queued for the next Land Records import, one lot's worth.
    edigeo_paths: Vec<String>,
    last_message: Option<String>,
    last_error: Option<String>,
}

fn id() -> egui::Id {
    egui::Id::new("cadcraft-topo-tab")
}

fn report(state: &mut State, result: Result<String, String>) {
    match result {
        Ok(msg) => {
            state.last_message = Some(msg);
            state.last_error = None;
        }
        Err(e) => {
            state.last_error = Some(e);
            state.last_message = None;
        }
    }
}

fn run_merge_parcels(app: &mut CadApp, state: &mut State) {
    let handles: Vec<Value> = app.session.selection().into_iter().map(|h| Value::String(h.hex())).collect();
    if handles.len() < 2 {
        report(state, Err(crate::tl!("Select at least two closed polylines or regions first.").to_string()));
        return;
    }
    let result = app.run("topo.merge_parcels", json!({ "handles": handles })).map(|r| {
        let area = r.get("area").and_then(Value::as_f64).unwrap_or(0.0);
        format!("{} ({area:.2})", crate::tl!("Merged"))
    });
    report(state, result);
}

fn run_parcel_state(app: &mut CadApp, state: &mut State) {
    let result = app.run("topo.parcel_state", Value::Null).map(|r| {
        let n = r.get("parcels").and_then(Value::as_array).map(Vec::len).unwrap_or(0);
        format!("{n} {}", crate::tl!("parcel(s)"))
    });
    report(state, result);
}

fn run_inspect_edigeo(app: &mut CadApp, state: &mut State, path: &str) {
    let result = app.run("topo.inspect_edigeo", json!({ "path": path })).map(|r| {
        let n = r.get("totalRecords").and_then(Value::as_u64).unwrap_or(0);
        format!("{path}: {n} {}", crate::tl!("record(s)"))
    });
    report(state, result);
}

fn run_import_edigeo(app: &mut CadApp, state: &mut State) {
    if state.edigeo_paths.is_empty() {
        report(state, Err(crate::tl!("Add at least one .VEC file first.").to_string()));
        return;
    }
    let paths: Vec<Value> = state.edigeo_paths.iter().cloned().map(Value::String).collect();
    let result = app.run("topo.import_edigeo", json!({ "paths": paths })).map(|r| {
        let imported = r.get("imported").and_then(Value::as_u64).unwrap_or(0);
        let skipped = r.get("skipped").and_then(Value::as_array).map(Vec::len).unwrap_or(0);
        format!("{imported} {}, {skipped} {}", crate::tl!("imported"), crate::tl!("skipped"))
    });
    report(state, result);
}

fn section(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    let t = Tokens::get();
    ui.add_space(6.0);
    ui.label(RichText::new(crate::i18n::t(title)).size(12.5).color(t.text));
    ui.add_space(4.0);
    egui::Frame::NONE.inner_margin(6.0).fill(t.chrome).show(ui, body);
    ui.add_space(4.0);
}

pub fn show(app: &mut CadApp, ui: &mut egui::Ui) {
    let t = Tokens::get();
    let mut state = ui.ctx().memory(|m| m.data.get_temp::<State>(id())).unwrap_or_default();

    ui.spacing_mut().item_spacing = vec2(0.0, 2.0);

    section(ui, "Foncier", |ui| {
        ui.horizontal_wrapped(|ui| {
            if ui.button(crate::tl!("Merge Selection")).on_hover_text(crate::tl!("topo.merge_parcels - current selection")).clicked() {
                run_merge_parcels(app, &mut state);
            }
            if ui.button(crate::tl!("Parcel State")).on_hover_text(crate::tl!("topo.parcel_state")).clicked() {
                run_parcel_state(app, &mut state);
            }
        });
    });

    section(ui, "Land Records (EDIGEO)", |ui| {
        ui.horizontal_wrapped(|ui| {
            if ui.button(crate::tl!("Add .VEC File…")).clicked()
                && let Some(path) = app.services.pick_open.as_ref().and_then(|f| f())
            {
                state.edigeo_paths.push(path);
            }
            if ui.button(crate::tl!("Clear List")).clicked() {
                state.edigeo_paths.clear();
            }
        });
        if state.edigeo_paths.is_empty() {
            ui.label(RichText::new(crate::tl!("No file queued.")).color(t.text_dim).size(11.0));
        } else {
            let mut remove: Option<usize> = None;
            for (i, path) in state.edigeo_paths.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(path).size(11.0));
                    if ui.small_button("✕").clicked() {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                state.edigeo_paths.remove(i);
            }
        }
        ui.horizontal_wrapped(|ui| {
            let first = state.edigeo_paths.first().cloned();
            ui.add_enabled_ui(first.is_some(), |ui| {
                if ui.button(crate::tl!("Inspect First File")).clicked()
                    && let Some(path) = first
                {
                    run_inspect_edigeo(app, &mut state, &path);
                }
            });
            ui.add_enabled_ui(!state.edigeo_paths.is_empty(), |ui| {
                if ui.button(crate::tl!("Import Lot")).clicked() {
                    run_import_edigeo(app, &mut state);
                }
            });
        });
    });

    if let Some(msg) = &state.last_message {
        ui.add_space(4.0);
        ui.label(RichText::new(msg).color(t.text).size(11.5));
    }
    if let Some(err) = &state.last_error {
        ui.add_space(4.0);
        ui.label(RichText::new(err).color(t.warn).size(11.5));
    }

    ui.ctx().memory_mut(|m| m.data.insert_temp(id(), state));
}
