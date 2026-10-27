//! The "Supprimer calques" dialog: LAYER.DELETE_MANY from the Edit menu (no parameters) opens
//! this instead of erring on a missing `names`, since picking layers by checkbox is the point -
//! LAYDEL itself keeps its own canvas-pick/type-a-name prompt chain under Format ▸ Layer Tools,
//! unchanged.

use egui::vec2;
use serde_json::{Value, json};

use crate::CadApp;

#[derive(Clone, Default)]
pub struct State {
    /// Every layer this dialog lets the user pick, with its checked state. Layer 0, Defpoints,
    /// the current layer and xref-dependent layers (name containing `|`) are left out entirely -
    /// LAYDEL always refuses all of them, so there's nothing useful to offer for them here.
    items: Vec<(String, bool)>,
    busy: bool,
    last_result: Option<Result<String, String>>,
}

/// The deletable layer names, freshly read from the drawing (current, protected and xref layers
/// excluded - see [`State::items`]'s doc comment).
fn deletable_layers(app: &mut CadApp) -> Vec<String> {
    let Ok(v) = app.run("layer", Value::Null) else { return Vec::new() };
    v.get("layers")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter(|l| !l.get("current").and_then(Value::as_bool).unwrap_or(false))
                .filter_map(|l| l.get("name").and_then(Value::as_str))
                .filter(|n| *n != "0" && !n.eq_ignore_ascii_case("Defpoints") && !n.contains('|'))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

pub fn open(app: &mut CadApp) {
    let items = deletable_layers(app).into_iter().map(|n| (n, false)).collect();
    app.delete_layers_dlg = Some(State { items, busy: false, last_result: None });
    app.ui.dialog = Some("deletelayers".into());
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let Some(mut st) = app.delete_layers_dlg.clone() else {
        *open = false;
        return;
    };

    let any_checked = st.items.iter().any(|(_, c)| *c);
    let mut delete = false;
    let (mut check_all, mut uncheck_all) = (false, false);
    let mut close = false;

    egui::Window::new(crate::tl!("Supprimer calques")).open(open).resizable(true).collapsible(false).default_size(vec2(320.0, 420.0)).show(
        ctx,
        |ui| {
            if st.items.is_empty() {
                ui.label(crate::tl!("Aucun calque supprimable (hors calque 0, Defpoints, le calque courant et les calques de référence externe)."));
            } else {
                ui.horizontal(|ui| {
                    check_all = ui.button(crate::tl!("Tout cocher")).clicked();
                    uncheck_all = ui.button(crate::tl!("Tout décocher")).clicked();
                });
                egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                    for (name, checked) in &mut st.items {
                        ui.checkbox(checked, name.as_str());
                    }
                });
            }
            if st.busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(crate::tl!("Suppression en cours…"));
                });
            }
            if let Some(r) = &st.last_result {
                match r {
                    Ok(msg) => {
                        ui.colored_label(egui::Color32::from_rgb(0x2e, 0xa0, 0x4c), msg);
                    }
                    Err(msg) => {
                        ui.colored_label(ui.visuals().error_fg_color, msg);
                    }
                }
            }
            ui.separator();
            ui.horizontal(|ui| {
                delete = ui.add_enabled(any_checked && !st.busy, egui::Button::new(crate::tl!("Supprimer"))).clicked();
                close = ui.button(crate::tl!("Fermer")).clicked();
            });
        },
    );
    if close {
        *open = false;
    }
    if check_all {
        for (_, c) in &mut st.items {
            *c = true;
        }
    }
    if uncheck_all {
        for (_, c) in &mut st.items {
            *c = false;
        }
    }

    if delete {
        let names: Vec<&str> = st.items.iter().filter(|(_, c)| *c).map(|(n, _)| n.as_str()).collect();
        st.busy = true;
        let r = app.run("layer.delete_many", json!({ "names": names }));
        st.busy = false;
        match r {
            Ok(_) => {
                let n = names.len();
                st.last_result =
                    Some(Ok(if n == 1 { crate::tl!("1 calque supprimé.").to_string() } else { crate::tf!("{n} calques supprimés.", n = n) }));
                st.items = deletable_layers(app).into_iter().map(|name| (name, false)).collect();
            }
            Err(e) => st.last_result = Some(Err(e)),
        }
    }

    if *open {
        app.delete_layers_dlg = Some(st);
    } else {
        app.delete_layers_dlg = None;
    }
}
