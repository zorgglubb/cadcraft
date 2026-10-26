//! The "Importer cadastre DXF" dialog: typed, menu or toolbar invocation of
//! PLAN.IMPORT_CADASTRE_DXF (no parameters) opens this instead of erring on a missing `insee`,
//! since it needs a commune code (and optionally one or more section codes) typed in first. A
//! JSON call (script, control channel, MCP) with real parameters still runs the plain command.

use egui::vec2;
use serde_json::{Value, json};

use crate::CadApp;

#[derive(Clone, Default)]
pub struct State {
    insee: String,
    /// Comma/space-separated section codes (e.g. "AD, AB"); empty = every section found.
    sections: String,
    busy: bool,
    last_result: Option<Result<String, String>>,
}

pub fn open(app: &mut CadApp) {
    app.cadastre_dlg = Some(State::default());
    app.ui.dialog = Some("cadastre".into());
}

fn parsed_sections(text: &str) -> Vec<String> {
    text.split([',', ' ', ';']).map(str::trim).filter(|s| !s.is_empty()).map(str::to_ascii_uppercase).collect()
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let Some(mut st) = app.cadastre_dlg.clone() else {
        *open = false;
        return;
    };

    let insee_ok = st.insee.trim().len() >= 5 && st.insee.trim().chars().all(|c| c.is_ascii_alphanumeric());
    let mut import = false;
    let mut close = false;

    egui::Window::new(crate::tl!("Importer cadastre DXF")).open(open).resizable(false).collapsible(false).default_size(vec2(380.0, 220.0)).show(
        ctx,
        |ui| {
            egui::Grid::new("cc_cadastre_grid").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                ui.label(crate::tl!("Code INSEE (commune)"));
                ui.text_edit_singleline(&mut st.insee);
                ui.end_row();

                ui.label(crate::tl!("Sections"));
                ui.text_edit_singleline(&mut st.sections);
                ui.end_row();
            });
            ui.label(crate::tl!("Sections : laisser vide pour toutes les sections de la commune."));
            if !insee_ok {
                ui.colored_label(ui.visuals().error_fg_color, crate::tl!("Entrez un code INSEE d'au moins 5 caractères."));
            }
            if st.busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(crate::tl!("Téléchargement en cours…"));
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
                import = ui.add_enabled(insee_ok && !st.busy, egui::Button::new(crate::tl!("Importer"))).clicked();
                close = ui.button(crate::tl!("Fermer")).clicked();
            });
        },
    );
    if close {
        *open = false;
    }

    if import {
        let mut params = json!({ "insee": st.insee.trim() });
        let sections = parsed_sections(&st.sections);
        if !sections.is_empty() {
            params["sections"] = json!(sections);
        }
        st.busy = true;
        let r = app.run("plan.import_cadastre_dxf", params);
        st.busy = false;
        st.last_result = Some(match &r {
            Ok(v) => {
                let imported = v.get("imported").and_then(Value::as_u64).unwrap_or(0);
                let errors = v.get("errors").and_then(Value::as_array).map_or(0, Vec::len);
                Ok(if errors == 0 {
                    crate::tf!("{n} feuille(s) importée(s).", n = imported)
                } else {
                    crate::tf!("{n} feuille(s) importée(s), {e} erreur(s) - voir la ligne de commande.", n = imported, e = errors)
                })
            }
            Err(e) => Err(e.clone()),
        });
    }

    if *open {
        app.cadastre_dlg = Some(st);
    } else {
        app.cadastre_dlg = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_are_split_trimmed_and_uppercased() {
        assert_eq!(parsed_sections("ad, ab ; ac"), vec!["AD", "AB", "AC"]);
        assert_eq!(parsed_sections("  "), Vec::<String>::new());
        assert_eq!(parsed_sections("AD"), vec!["AD"]);
    }
}
