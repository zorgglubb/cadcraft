//! Dialogs for the Data menu's conversion commands ported from the user's AutoLISP toolbox:
//! DATA.COD_TO_NUM, DATA.EXTRACT_TO_TEXT, DATA.ALTITUDE_TO_POLY and DATA.AXE_TO_ELLIPSE. Typed,
//! menu or toolbar invocation (no parameters) opens the matching dialog instead of erring on a
//! missing required field (`table`/`fields`/`layer`) - a JSON call (script, control channel,
//! MCP) with real parameters still just runs the plain command. All four act on the CURRENT
//! SELECTION, like the plain commands already do when `handles` is omitted - none of these
//! dialogs need their own entity-picking UI.

use egui::vec2;
use serde_json::{Value, json};

use crate::CadApp;

/// `(table name, field names)` for every attribute table DATA.LIST_TABLES currently knows about,
/// read straight from the engine so a dropdown never drifts from what the drawing actually has.
fn tables(app: &mut CadApp) -> Vec<(String, Vec<String>)> {
    app.run("data.list_tables", json!({}))
        .ok()
        .and_then(|v| v.get("tables").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|t| {
            let name = t.get("name").and_then(Value::as_str)?.to_string();
            let fields = t.get("fields").and_then(Value::as_array).cloned().unwrap_or_default();
            let fields = fields.iter().filter_map(|f| f.get("name").and_then(Value::as_str).map(str::to_string)).collect();
            Some((name, fields))
        })
        .collect()
}

fn table_combo(ui: &mut egui::Ui, id: impl std::hash::Hash + std::fmt::Debug, tables: &[(String, Vec<String>)], selected: &mut String) {
    egui::ComboBox::from_id_salt(id).selected_text(if selected.is_empty() { crate::tl!("(choose)").to_string() } else { selected.clone() }).show_ui(
        ui,
        |ui| {
            for (name, _) in tables {
                ui.selectable_value(selected, name.clone(), name);
            }
        },
    );
}

fn field_combo(ui: &mut egui::Ui, id: impl std::hash::Hash + std::fmt::Debug, tables: &[(String, Vec<String>)], table: &str, selected: &mut String) {
    let fields: &[String] = tables.iter().find(|(n, _)| n == table).map_or(&[], |(_, f)| f.as_slice());
    egui::ComboBox::from_id_salt(id).selected_text(if selected.is_empty() { crate::tl!("(choose)").to_string() } else { selected.clone() }).show_ui(
        ui,
        |ui| {
            for f in fields {
                ui.selectable_value(selected, f.clone(), f);
            }
        },
    );
}

// ---------------------------------------------------------------------------------------------
// Point cod->num (DATA.COD_TO_NUM)
// ---------------------------------------------------------------------------------------------

#[derive(Clone)]
pub struct CodToNumState {
    table: String,
    field: String,
    attr: String,
    tolerance_text: String,
}

impl Default for CodToNumState {
    fn default() -> Self {
        CodToNumState { table: String::new(), field: String::new(), attr: "COD".into(), tolerance_text: "1e-6".into() }
    }
}

pub fn open_cod_to_num(app: &mut CadApp) {
    app.cod_to_num_dlg = Some(CodToNumState::default());
    app.ui.dialog = Some("codtonum".into());
}

pub fn cod_to_num_dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let Some(mut st) = app.cod_to_num_dlg.clone() else {
        *open = false;
        return;
    };
    let tables = tables(app);

    let (mut run, mut close) = (false, false);
    egui::Window::new(crate::tl!("Point cod->num")).open(open).resizable(true).collapsible(false).default_size(vec2(340.0, 220.0)).show(ctx, |ui| {
        egui::Grid::new("cc_codtonum_grid").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
            ui.label(crate::tl!("Table"));
            table_combo(ui, "cc_codtonum_table", &tables, &mut st.table);
            ui.end_row();

            ui.label(crate::tl!("Field"));
            field_combo(ui, "cc_codtonum_field", &tables, &st.table, &mut st.field);
            ui.end_row();

            ui.label(crate::tl!("Block attribute"));
            ui.text_edit_singleline(&mut st.attr);
            ui.end_row();

            ui.label(crate::tl!("Tolerance"));
            ui.text_edit_singleline(&mut st.tolerance_text);
            ui.end_row();
        });
        if tables.is_empty() {
            ui.colored_label(ui.visuals().error_fg_color, crate::tl!("No attribute table yet - create one first (Data > Attribute Table...)."));
        } else if st.tolerance_text.trim().parse::<f64>().is_err() {
            ui.colored_label(ui.visuals().error_fg_color, crate::tl!("Tolerance must be a number."));
        }
        ui.separator();
        let ready = !st.table.is_empty() && !st.field.is_empty() && !st.attr.trim().is_empty() && st.tolerance_text.trim().parse::<f64>().is_ok();
        ui.horizontal(|ui| {
            run = ui.add_enabled(ready, egui::Button::new(crate::tl!("Run"))).clicked();
            close = ui.button(crate::tl!("Close")).clicked();
        });
    });
    if close {
        *open = false;
    }

    if run {
        let tol: f64 = st.tolerance_text.trim().parse().unwrap_or(1e-6);
        let r = app.run("data.cod_to_num", json!({ "table": st.table, "field": st.field, "attr": st.attr.trim(), "tolerance": tol }));
        app.session.echo(match &r {
            Ok(v) => crate::tf!("{n} polyline(s) updated", n = v.get("updated").and_then(Value::as_u64).unwrap_or(0)),
            Err(e) => e.clone(),
        });
        if r.is_ok() {
            *open = false;
        }
    }
    if *open {
        app.cod_to_num_dlg = Some(st);
    } else {
        app.cod_to_num_dlg = None;
    }
}

// ---------------------------------------------------------------------------------------------
// Extraire donnée->texte (DATA.EXTRACT_TO_TEXT)
// ---------------------------------------------------------------------------------------------

#[derive(Clone)]
pub struct ExtractToTextState {
    /// (table, field) rows to extract, in order.
    fields: Vec<(String, String)>,
    layer: String,
    color_text: String,
    height_text: String,
    dx_text: String,
    dy_text: String,
}

impl Default for ExtractToTextState {
    fn default() -> Self {
        ExtractToTextState {
            fields: Vec::new(),
            layer: String::new(),
            color_text: "7".into(),
            height_text: "2.5".into(),
            dx_text: "0".into(),
            dy_text: "0".into(),
        }
    }
}

pub fn open_extract_to_text(app: &mut CadApp) {
    app.extract_to_text_dlg = Some(ExtractToTextState::default());
    app.ui.dialog = Some("extracttotext".into());
}

fn extract_ready(st: &ExtractToTextState) -> bool {
    !st.fields.is_empty()
        && st.fields.iter().all(|(t, f)| !t.is_empty() && !f.is_empty())
        && !st.layer.trim().is_empty()
        && st.color_text.trim().parse::<i64>().is_ok()
        && st.height_text.trim().parse::<f64>().is_ok_and(|h| h > 0.0)
        && st.dx_text.trim().parse::<f64>().is_ok()
        && st.dy_text.trim().parse::<f64>().is_ok()
}

pub fn extract_to_text_dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let Some(mut st) = app.extract_to_text_dlg.clone() else {
        *open = false;
        return;
    };
    let tables_list = tables(app);

    let (mut run, mut close) = (false, false);
    let mut remove = None;
    egui::Window::new(crate::tl!("Extraire donnée->texte")).open(open).resizable(true).collapsible(false).default_size(vec2(420.0, 440.0)).show(
        ctx,
        |ui| {
            ui.label(crate::tl!("Fields to extract"));
            egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                for (i, (table, field)) in st.fields.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        table_combo(ui, ("cc_extract_table", i), &tables_list, table);
                        field_combo(ui, ("cc_extract_field", i), &tables_list, table, field);
                        if ui.button("✕").clicked() {
                            remove = Some(i);
                        }
                    });
                }
            });
            if ui.button(crate::tl!("Add field")).clicked() {
                st.fields.push((String::new(), String::new()));
            }
            ui.separator();
            egui::Grid::new("cc_extract_grid").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                ui.label(crate::tl!("Layer"));
                ui.text_edit_singleline(&mut st.layer);
                ui.end_row();

                ui.label(crate::tl!("Colour (ACI)"));
                ui.text_edit_singleline(&mut st.color_text);
                ui.end_row();

                ui.label(crate::tl!("Text height"));
                ui.text_edit_singleline(&mut st.height_text);
                ui.end_row();

                ui.label(crate::tl!("X offset"));
                ui.text_edit_singleline(&mut st.dx_text);
                ui.end_row();

                ui.label(crate::tl!("Y offset"));
                ui.text_edit_singleline(&mut st.dy_text);
                ui.end_row();
            });
            if !extract_ready(&st) {
                ui.colored_label(ui.visuals().error_fg_color, crate::tl!("Add at least one table/field, a layer, and valid numbers."));
            }
            ui.separator();
            ui.horizontal(|ui| {
                run = ui.add_enabled(extract_ready(&st), egui::Button::new(crate::tl!("Run"))).clicked();
                close = ui.button(crate::tl!("Close")).clicked();
            });
        },
    );
    if let Some(i) = remove {
        st.fields.remove(i);
    }
    if close {
        *open = false;
    }

    if run && extract_ready(&st) {
        let fields: Vec<Value> = st.fields.iter().map(|(t, f)| json!({ "table": t, "field": f })).collect();
        let params = json!({
            "fields": fields,
            "layer": st.layer.trim(),
            "color": st.color_text.trim().parse::<i64>().unwrap_or(7),
            "height": st.height_text.trim().parse::<f64>().unwrap_or(2.5),
            "dx": st.dx_text.trim().parse::<f64>().unwrap_or(0.0),
            "dy": st.dy_text.trim().parse::<f64>().unwrap_or(0.0),
        });
        let r = app.run("data.extract_to_text", params);
        app.session.echo(match &r {
            Ok(v) => crate::tf!("{n} text(s) created", n = v.get("created").and_then(Value::as_u64).unwrap_or(0)),
            Err(e) => e.clone(),
        });
        if r.is_ok() {
            *open = false;
        }
    }
    if *open {
        app.extract_to_text_dlg = Some(st);
    } else {
        app.extract_to_text_dlg = None;
    }
}

// ---------------------------------------------------------------------------------------------
// Altitude->poly (DATA.ALTITUDE_TO_POLY)
// ---------------------------------------------------------------------------------------------

#[derive(Clone)]
pub struct AltitudeToPolyState {
    block_name: String,
    alt_attr: String,
}

impl Default for AltitudeToPolyState {
    fn default() -> Self {
        AltitudeToPolyState { block_name: "TCPOINT".into(), alt_attr: "ALT".into() }
    }
}

pub fn open_altitude_to_poly(app: &mut CadApp) {
    app.altitude_to_poly_dlg = Some(AltitudeToPolyState::default());
    app.ui.dialog = Some("altitudetopoly".into());
}

pub fn altitude_to_poly_dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let Some(mut st) = app.altitude_to_poly_dlg.clone() else {
        *open = false;
        return;
    };

    let (mut run, mut close) = (false, false);
    egui::Window::new(crate::tl!("Altitude->poly")).open(open).resizable(false).collapsible(false).default_size(vec2(320.0, 160.0)).show(ctx, |ui| {
        egui::Grid::new("cc_alttopoly_grid").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
            ui.label(crate::tl!("Block name"));
            ui.text_edit_singleline(&mut st.block_name);
            ui.end_row();

            ui.label(crate::tl!("Altitude attribute"));
            ui.text_edit_singleline(&mut st.alt_attr);
            ui.end_row();
        });
        let ready = !st.block_name.trim().is_empty() && !st.alt_attr.trim().is_empty();
        if !ready {
            ui.colored_label(ui.visuals().error_fg_color, crate::tl!("Both fields are required."));
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

    if run {
        let r = app.run("data.altitude_to_poly", json!({ "blockName": st.block_name.trim(), "altAttr": st.alt_attr.trim() }));
        app.session.echo(match &r {
            Ok(v) => crate::tf!("{n} entity(ies) updated", n = v.get("changed").and_then(Value::as_u64).unwrap_or(0)),
            Err(e) => e.clone(),
        });
        if r.is_ok() {
            *open = false;
        }
    }
    if *open {
        app.altitude_to_poly_dlg = Some(st);
    } else {
        app.altitude_to_poly_dlg = None;
    }
}

// ---------------------------------------------------------------------------------------------
// Axe->Ellipse (DATA.AXE_TO_ELLIPSE)
// ---------------------------------------------------------------------------------------------

#[derive(Clone)]
pub struct AxeToEllipseState {
    source_table: String,
    field_nature: String,
    dest_table: String,
    threshold_text: String,
    size_threshold_text: String,
    delete_trigger: String,
}

impl Default for AxeToEllipseState {
    fn default() -> Self {
        AxeToEllipseState {
            source_table: "Axe".into(),
            field_nature: "Nature".into(),
            dest_table: "Structure".into(),
            threshold_text: "0.08".into(),
            size_threshold_text: "0.30".into(),
            delete_trigger: "PLAN".into(),
        }
    }
}

pub fn open_axe_to_ellipse(app: &mut CadApp) {
    app.axe_to_ellipse_dlg = Some(AxeToEllipseState::default());
    app.ui.dialog = Some("axetoellipse".into());
}

fn axe_ready(st: &AxeToEllipseState) -> bool {
    !st.source_table.trim().is_empty()
        && !st.field_nature.trim().is_empty()
        && !st.dest_table.trim().is_empty()
        && !st.delete_trigger.trim().is_empty()
        && st.threshold_text.trim().parse::<f64>().is_ok_and(|v| v >= 0.0)
        && st.size_threshold_text.trim().parse::<f64>().is_ok_and(|v| v >= 0.0)
}

pub fn axe_to_ellipse_dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let Some(mut st) = app.axe_to_ellipse_dlg.clone() else {
        *open = false;
        return;
    };
    let tables_list = tables(app);
    let table_names: Vec<String> = tables_list.iter().map(|(n, _)| n.clone()).collect();

    let (mut run, mut close) = (false, false);
    egui::Window::new(crate::tl!("Axe->Ellipse")).open(open).resizable(true).collapsible(false).default_size(vec2(360.0, 320.0)).show(ctx, |ui| {
        egui::Grid::new("cc_axetoellipse_grid").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
            ui.label(crate::tl!("Source table"));
            ui.horizontal(|ui| {
                ui.text_edit_singleline(&mut st.source_table);
                egui::ComboBox::from_id_salt("cc_axe_src").selected_text("").show_ui(ui, |ui| {
                    for n in &table_names {
                        if ui.selectable_label(false, n).clicked() {
                            st.source_table = n.clone();
                        }
                    }
                });
            });
            ui.end_row();

            ui.label(crate::tl!("Nature field"));
            ui.text_edit_singleline(&mut st.field_nature);
            ui.end_row();

            ui.label(crate::tl!("Destination table"));
            ui.horizontal(|ui| {
                ui.text_edit_singleline(&mut st.dest_table);
                egui::ComboBox::from_id_salt("cc_axe_dest").selected_text("").show_ui(ui, |ui| {
                    for n in &table_names {
                        if ui.selectable_label(false, n).clicked() {
                            st.dest_table = n.clone();
                        }
                    }
                });
            });
            ui.end_row();

            ui.label(crate::tl!("Point-merge threshold (m)"));
            ui.text_edit_singleline(&mut st.threshold_text);
            ui.end_row();

            ui.label(crate::tl!("Small-ellipse threshold (m)"));
            ui.text_edit_singleline(&mut st.size_threshold_text);
            ui.end_row();

            ui.label(crate::tl!("Delete when Nature ="));
            ui.text_edit_singleline(&mut st.delete_trigger);
            ui.end_row();
        });
        if !axe_ready(&st) {
            ui.colored_label(ui.visuals().error_fg_color, crate::tl!("All fields are required, and both thresholds must be numbers."));
        }
        ui.label(crate::tl!("Value remapping isn't available from this window yet - use the JSON `remap` parameter from a script if needed."));
        ui.separator();
        ui.horizontal(|ui| {
            run = ui.add_enabled(axe_ready(&st), egui::Button::new(crate::tl!("Run"))).clicked();
            close = ui.button(crate::tl!("Close")).clicked();
        });
    });
    if close {
        *open = false;
    }

    if run && axe_ready(&st) {
        let threshold: f64 = st.threshold_text.trim().parse().unwrap_or(0.08);
        let size_threshold: f64 = st.size_threshold_text.trim().parse().unwrap_or(0.30);
        let params = json!({
            "sourceTable": st.source_table.trim(),
            "fieldNature": st.field_nature.trim(),
            "destTable": st.dest_table.trim(),
            "threshold": threshold,
            "sizeThreshold": size_threshold,
            "deleteTrigger": st.delete_trigger.trim(),
        });
        let r = app.run("data.axe_to_ellipse", params);
        app.session.echo(match &r {
            Ok(v) => crate::tf!(
                "{created} ellipse(s) created, {deleted} deleted, {simplified} simplified",
                created = v.get("created").and_then(Value::as_u64).unwrap_or(0),
                deleted = v.get("deleted").and_then(Value::as_u64).unwrap_or(0),
                simplified = v.get("simplified").and_then(Value::as_u64).unwrap_or(0)
            ),
            Err(e) => e.clone(),
        });
        if r.is_ok() {
            *open = false;
        }
    }
    if *open {
        app.axe_to_ellipse_dlg = Some(st);
    } else {
        app.axe_to_ellipse_dlg = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_ready_requires_fields_layer_and_valid_numbers() {
        let mut st = ExtractToTextState::default();
        assert!(!extract_ready(&st), "no fields yet");
        st.fields.push(("Structure".into(), "Num".into()));
        assert!(!extract_ready(&st), "no layer yet");
        st.layer = "4TX_St".into();
        assert!(extract_ready(&st));
        st.height_text = "not a number".into();
        assert!(!extract_ready(&st));
    }

    #[test]
    fn axe_ready_requires_every_field_and_nonnegative_thresholds() {
        let mut st = AxeToEllipseState::default();
        assert!(axe_ready(&st), "defaults are already valid");
        st.threshold_text = "-1".into();
        assert!(!axe_ready(&st));
        st.threshold_text = "0.08".into();
        st.dest_table = "  ".into();
        assert!(!axe_ready(&st));
    }
}
