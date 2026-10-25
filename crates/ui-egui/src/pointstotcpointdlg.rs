//! The Point->TCPOINT dialog: DATA.POINTS_TO_TCPOINT from a menu or the command line opens this
//! real popup instead of silently acting on a stale pre-dialog selection. It asks the user to
//! select the source POINT entities explicitly (same pick/[`resume`] pattern as
//! [`crate::tabledlg::resume_assign`] and [`crate::dataconvdlg::resume_altitude_to_poly`]), then
//! lets them choose which attribute-table field feeds each of COD/ALT/MAT - the fields offered
//! come from every attribute table DATA.LIST_TABLES knows about (including Shapefile/DWG Map
//! tables), since the picked points' own table isn't known until the engine runs the conversion.

use egui::vec2;
use serde_json::json;

use crate::CadApp;

#[derive(Clone)]
pub struct State {
    handles: Vec<cadcraft_doc::Handle>,
    picking: bool,
    cod_field: String,
    alt_field: String,
    mat_field: String,
    block_name: String,
    last_result: Option<Result<String, String>>,
}

impl Default for State {
    fn default() -> Self {
        State {
            handles: Vec::new(),
            picking: false,
            cod_field: "COD".into(),
            alt_field: "ALT".into(),
            mat_field: "MAT".into(),
            block_name: "CADCRAFT_TCPOINT".into(),
            last_result: None,
        }
    }
}

pub fn open(app: &mut CadApp) {
    app.points_to_tcpoint_dlg = Some(State::default());
    app.ui.dialog = Some("pointstotcpoint".into());
}

fn start_pick(app: &mut CadApp) {
    let Some(st) = app.points_to_tcpoint_dlg.as_mut() else { return };
    st.picking = true;
    app.session.table_pick = None;
    app.ui.dialog = None;
    app.start("data.pick_entities");
}

/// Every frame: once the DATA.PICK_ENTITIES this dialog started has ended, take its handles and
/// bring the dialog back (unchanged when the pick was cancelled).
pub fn resume(app: &mut CadApp) {
    if !app.points_to_tcpoint_dlg.as_ref().is_some_and(|s| s.picking) {
        return;
    }
    if app.session.running.is_some() {
        return;
    }
    let picked = app.session.table_pick.take();
    let Some(st) = app.points_to_tcpoint_dlg.as_mut() else { return };
    st.picking = false;
    if let Some(hs) = picked {
        st.handles = hs;
    }
    app.ui.dialog = Some("pointstotcpoint".into());
}

/// All field names known across every attribute table (DATA.LIST_TABLES, including Shapefile and
/// DWG Map tables), deduplicated - the picked points' own table isn't known from here, so the
/// dropdowns offer the union rather than a strict per-table choice.
fn all_field_names(app: &mut CadApp) -> Vec<String> {
    let mut names: Vec<String> = crate::dataconvdlg::tables(app).into_iter().flat_map(|(_, fields)| fields).collect();
    names.sort();
    names.dedup();
    names
}

fn field_text_combo(ui: &mut egui::Ui, id: impl std::hash::Hash + std::fmt::Debug, names: &[String], selected: &mut String) {
    ui.horizontal(|ui| {
        ui.text_edit_singleline(selected);
        egui::ComboBox::from_id_salt(id).selected_text(crate::tl!("(fields)")).show_ui(ui, |ui| {
            if names.is_empty() {
                ui.label(crate::tl!("No attribute table yet."));
            }
            for n in names {
                if ui.selectable_label(selected.as_str() == n.as_str(), n).clicked() {
                    *selected = n.clone();
                }
            }
        });
    });
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let Some(mut st) = app.points_to_tcpoint_dlg.clone() else {
        *open = false;
        return;
    };
    let field_names = all_field_names(app);

    let (mut select, mut run, mut close) = (false, false, false);
    egui::Window::new(crate::tl!("Point->TCPOINT")).open(open).resizable(true).collapsible(false).default_size(vec2(380.0, 260.0)).show(ctx, |ui| {
        ui.horizontal(|ui| {
            select = ui.button(crate::tl!("Select objects")).clicked();
            let count_label: String = if st.handles.is_empty() {
                crate::tl!("No point selected yet.").to_string()
            } else {
                crate::tf!("{n} entity(ies) selected", n = st.handles.len())
            };
            ui.label(count_label);
        });
        ui.separator();
        egui::Grid::new("cc_pointstotcpoint_grid").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
            ui.label(crate::tl!("Block name"));
            ui.text_edit_singleline(&mut st.block_name);
            ui.end_row();

            ui.label(crate::tl!("COD field"));
            field_text_combo(ui, "cc_p2t_cod", &field_names, &mut st.cod_field);
            ui.end_row();

            ui.label(crate::tl!("ALT field"));
            field_text_combo(ui, "cc_p2t_alt", &field_names, &mut st.alt_field);
            ui.end_row();

            ui.label(crate::tl!("MAT field"));
            field_text_combo(ui, "cc_p2t_mat", &field_names, &mut st.mat_field);
            ui.end_row();
        });
        let ready = !st.handles.is_empty() && !st.block_name.trim().is_empty();
        if !ready {
            ui.colored_label(ui.visuals().error_fg_color, crate::tl!("Select at least one point, and fill the block name."));
        }
        ui.label(crate::tl!("Each selected point keeps its own attribute-table data; a point with no attribute table is skipped."));
        match &st.last_result {
            Some(Ok(msg)) => {
                ui.colored_label(egui::Color32::from_rgb(0x2e, 0xa0, 0x4c), msg.clone());
            }
            Some(Err(msg)) => {
                ui.colored_label(ui.visuals().error_fg_color, msg.clone());
            }
            None => {}
        }
        ui.separator();
        ui.horizontal(|ui| {
            run = ui.add_enabled(ready, egui::Button::new(crate::tl!("Run"))).clicked();
            close = ui.button(crate::tl!("Close")).clicked();
        });
    });
    if close {
        *open = false;
    }

    if select {
        app.points_to_tcpoint_dlg = Some(st.clone());
        start_pick(app);
        return;
    }

    if run && !st.handles.is_empty() {
        let handles: Vec<String> = st.handles.iter().map(|h| h.hex()).collect();
        let r = app.run(
            "data.points_to_tcpoint",
            json!({
                "handles": handles,
                "blockName": st.block_name.trim(),
                "codField": st.cod_field.trim(),
                "altField": st.alt_field.trim(),
                "matField": st.mat_field.trim(),
            }),
        );
        st.last_result = Some(match &r {
            Ok(v) => Ok(crate::tf!(
                "{created} TCPOINT(s) created, {skipped} skipped",
                created = v.get("created").and_then(serde_json::Value::as_u64).unwrap_or(0),
                skipped = v.get("skipped").and_then(serde_json::Value::as_u64).unwrap_or(0)
            )),
            Err(e) => Err(e.clone()),
        });
        app.session.echo(match &st.last_result {
            Some(Ok(msg)) => msg.clone(),
            Some(Err(msg)) => msg.clone(),
            None => String::new(),
        });
    }
    if *open {
        app.points_to_tcpoint_dlg = Some(st);
    } else {
        app.points_to_tcpoint_dlg = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_block_name_matches_engine_default() {
        assert_eq!(State::default().block_name, "CADCRAFT_TCPOINT");
    }
}
