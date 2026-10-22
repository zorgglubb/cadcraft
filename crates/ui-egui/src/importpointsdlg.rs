//! The Import Points (CSV/TXT) dialog: TOPO.IMPORT_POINTS from a menu or the command line (no
//! parameters) picks the file first (same picker OPEN/IMPORT use), then opens this instead of
//! running headlessly - it needs a layer to create/use, a block to insert each point as, and a
//! delimiter chosen first. The preview grid (the file's first 10 lines, split by the chosen
//! delimiter) is read back every frame from TOPO.PREVIEW_POINTS_CSV, which never touches the
//! drawing, so it updates as soon as the delimiter changes.

use egui::vec2;
use serde_json::{Value, json};

use crate::CadApp;

const DELIMITERS: [(&str, &str); 6] =
    [("auto", "Auto"), ("comma", "Comma (,)"), ("semicolon", "Semicolon (;)"), ("colon", "Colon (:)"), ("space", "Space"), ("tab", "Tab")];

#[derive(Clone)]
pub struct State {
    path: String,
    layer: String,
    block: String,
    delimiter: String,
    label: bool,
    /// Whether each point's own CSV Z also becomes the block reference's insertion Z (true 3D
    /// points), or every point stays flat on the XY plane with its Z only in the ALT attribute.
    three_d: bool,
    preview: Vec<Vec<String>>,
}

impl State {
    fn new(path: String) -> Self {
        // A default layer named after the file, the same convention `plan.import_shp` uses -
        // the person can still type a different name before importing.
        let layer = std::path::Path::new(&path).file_stem().and_then(std::ffi::OsStr::to_str).unwrap_or("").to_string();
        State { path, layer, block: "CADCRAFT_TCPOINT".into(), delimiter: "auto".into(), label: true, three_d: true, preview: Vec::new() }
    }
}

pub fn open(app: &mut CadApp, path: String) {
    let mut st = State::new(path);
    refresh_preview(app, &mut st);
    app.import_points_dlg = Some(st);
    app.ui.dialog = Some("importpoints".into());
}

fn refresh_preview(app: &mut CadApp, st: &mut State) {
    st.preview = app
        .run("topo.preview_points_csv", json!({ "path": st.path, "delimiter": st.delimiter, "lines": 10 }))
        .ok()
        .and_then(|v| v.get("rows").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .iter()
        .map(|row| row.as_array().map(|a| a.iter().filter_map(|c| c.as_str().map(str::to_string)).collect()).unwrap_or_default())
        .collect();
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let Some(mut st) = app.import_points_dlg.clone() else {
        *open = false;
        return;
    };

    let mut delimiter_changed = false;
    let (mut import, mut close) = (false, false);
    egui::Window::new(crate::tl!("Import Points (CSV/TXT)...")).open(open).resizable(true).collapsible(false).default_size(vec2(540.0, 440.0)).show(
        ctx,
        |ui| {
            ui.horizontal(|ui| {
                ui.label(crate::tl!("File"));
                ui.label(&st.path);
            });
            ui.horizontal(|ui| {
                ui.label(crate::tl!("Layer"));
                ui.text_edit_singleline(&mut st.layer);
            });
            ui.horizontal(|ui| {
                ui.label(crate::tl!("Block"));
                ui.text_edit_singleline(&mut st.block);
            });
            ui.horizontal(|ui| {
                ui.label(crate::tl!("Delimiter"));
                for (key, label) in DELIMITERS {
                    if ui.selectable_label(st.delimiter == key, crate::i18n::t(label)).clicked() && st.delimiter != key {
                        st.delimiter = key.to_string();
                        delimiter_changed = true;
                    }
                }
            });
            ui.checkbox(&mut st.label, crate::tl!("Show code label"));
            ui.horizontal(|ui| {
                ui.label(crate::tl!("Points"));
                ui.radio_value(&mut st.three_d, false, crate::tl!("2D (flat)"));
                ui.radio_value(&mut st.three_d, true, crate::tl!("3D (use Z)"));
            });
            ui.separator();
            ui.label(crate::tl!("Preview"));
            egui::ScrollArea::both().max_height(220.0).show(ui, |ui| {
                let cols = st.preview.iter().map(Vec::len).max().unwrap_or(1).max(1);
                egui::Grid::new("cc_points_preview").striped(true).num_columns(cols).show(ui, |ui| {
                    for row in &st.preview {
                        for cell in row {
                            ui.label(cell);
                        }
                        ui.end_row();
                    }
                });
            });
            ui.separator();
            ui.horizontal(|ui| {
                import =
                    ui.add_enabled(!st.layer.trim().is_empty() && !st.block.trim().is_empty(), egui::Button::new(crate::tl!("Import"))).clicked();
                close = ui.button(crate::tl!("Close")).clicked();
            });
        },
    );
    if close {
        *open = false;
    }
    if delimiter_changed {
        refresh_preview(app, &mut st);
    }

    if import {
        let r = app.run(
            "topo.import_points",
            json!({
                "path": st.path,
                "layer": st.layer.trim(),
                "block": st.block.trim(),
                "delimiter": st.delimiter,
                "label": st.label,
                "threeD": st.three_d,
            }),
        );
        app.session.echo(match &r {
            Ok(v) => crate::tf!("Imported {n} point(s)", n = v.get("importedPoints").and_then(Value::as_u64).unwrap_or(0)),
            Err(e) => e.clone(),
        });
        if r.is_ok() {
            app.canvas.zoom_pending = true;
            *open = false;
        }
    }
    if *open {
        app.import_points_dlg = Some(st);
    } else {
        app.import_points_dlg = None;
    }
}

#[cfg(test)]
mod tests {
    use cadcraft_engine::Session;
    use serde_json::json;

    use super::*;
    use crate::Services;

    fn app_with_doc() -> CadApp {
        let mut s = Session::new();
        s.execute("new", &json!({})).unwrap();
        CadApp::new(s, Services::default())
    }

    #[test]
    fn open_defaults_the_layer_to_the_files_stem_and_fills_the_preview() {
        let mut app = app_with_doc();
        let path = std::env::temp_dir().join(format!("cadcraft-ui-points-{}.csv", std::process::id()));
        std::fs::write(&path, "ID;X;Y;Z\nP1;100.0;200.0;10.0\n").unwrap();

        open(&mut app, path.to_string_lossy().to_string());
        let _ = std::fs::remove_file(&path);

        assert_eq!(app.ui.dialog, Some("importpoints".into()));
        let st = app.import_points_dlg.expect("state kept");
        assert_eq!(st.layer, path.file_stem().and_then(std::ffi::OsStr::to_str).unwrap());
        assert_eq!(st.block, "CADCRAFT_TCPOINT");
        assert_eq!(st.preview.len(), 2, "header row + one data row");
    }

    #[test]
    fn preview_updates_when_the_delimiter_changes() {
        let mut app = app_with_doc();
        let path = std::env::temp_dir().join(format!("cadcraft-ui-points-delim-{}.csv", std::process::id()));
        std::fs::write(&path, "ID;X;Y;Z\nP1;100.0;200.0;10.0\n").unwrap();
        open(&mut app, path.to_string_lossy().to_string());

        let mut st = app.import_points_dlg.clone().unwrap();
        st.delimiter = "comma".into();
        refresh_preview(&mut app, &mut st);
        let _ = std::fs::remove_file(&path);

        // Forced to split on commas, the whole semicolon-delimited line is one field.
        assert_eq!(st.preview[0].len(), 1);
    }

    #[test]
    fn importing_runs_the_command_with_the_dialogs_chosen_layer_and_block() {
        let mut app = app_with_doc();
        let path = std::env::temp_dir().join(format!("cadcraft-ui-points-import-{}.csv", std::process::id()));
        std::fs::write(&path, "ID;X;Y;Z;CODE\nP1;0.0;0.0;10.0;BORNE\n").unwrap();
        open(&mut app, path.to_string_lossy().to_string());
        let st = app.import_points_dlg.clone().unwrap();
        assert!(st.three_d, "3D is the default");

        let r = app.run(
            "topo.import_points",
            json!({ "path": st.path, "layer": st.layer, "block": st.block, "delimiter": st.delimiter, "label": st.label, "threeD": st.three_d }),
        );
        let _ = std::fs::remove_file(&path);
        assert!(r.is_ok(), "{r:?}");
        let v = r.unwrap();
        assert_eq!(v["importedPoints"].as_u64(), Some(1));
        assert_eq!(v["threeD"].as_bool(), Some(true));
    }

    #[test]
    fn importing_in_2d_mode_passes_three_d_false() {
        let mut app = app_with_doc();
        let path = std::env::temp_dir().join(format!("cadcraft-ui-points-import2d-{}.csv", std::process::id()));
        std::fs::write(&path, "ID;X;Y;Z;CODE\nP1;0.0;0.0;10.0;BORNE\n").unwrap();
        open(&mut app, path.to_string_lossy().to_string());
        let st = State { three_d: false, ..app.import_points_dlg.clone().unwrap() };

        let r = app.run(
            "topo.import_points",
            json!({ "path": st.path, "layer": st.layer, "block": st.block, "delimiter": st.delimiter, "label": st.label, "threeD": st.three_d }),
        );
        let _ = std::fs::remove_file(&path);
        assert_eq!(r.unwrap()["threeD"].as_bool(), Some(false));
    }
}
