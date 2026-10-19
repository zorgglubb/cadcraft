//! The Edit Altitudes dialog: TOPO.EDIT_ALTITUDES opened with no parameters - batch-adds a
//! constant to every matching topo point block's elevation, the way a total-station survey often
//! needs after discovering a benchmark offset (the constant is the field that matters most: most
//! other fields just narrow which points it applies to).

use egui::vec2;
use serde_json::{Value, json};

use crate::CadApp;

#[derive(Clone, Copy, PartialEq)]
pub enum Selection {
    Blocks,
    Manual,
    All,
    List,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    Keep2D,
    Keep3D,
}

#[derive(Clone)]
pub struct State {
    pub selection: Selection,
    pub block_names: Vec<String>,
    pub layers: Vec<String>,
    pub use_filter_for_manual: bool,
    pub matricule_filter: bool,
    pub matricule_first: String,
    pub matricule_last: String,
    pub mode: Mode,
    pub constant: String,
    pub decimals: u32,
}

impl Default for State {
    fn default() -> Self {
        State {
            selection: Selection::Manual,
            block_names: Vec::new(),
            layers: Vec::new(),
            use_filter_for_manual: false,
            matricule_filter: false,
            matricule_first: String::new(),
            matricule_last: String::new(),
            mode: Mode::Keep3D,
            constant: "0.0000".into(),
            decimals: 2,
        }
    }
}

pub fn open(app: &mut CadApp) {
    app.edit_altitudes_dlg = Some(State::default());
    app.ui.dialog = Some("editaltitudes".into());
}

fn num(s: &str) -> Option<f64> {
    let t = s.trim().replace(',', ".");
    if t.is_empty() { None } else { t.parse::<f64>().ok() }
}

/// `true` once the selection mode has what it needs (manual needs a prior pick-set, which this
/// dialog doesn't yet gather itself - see the module doc comment) and the constant parses.
fn ready(st: &State) -> bool {
    let selection_ready = match st.selection {
        Selection::Blocks => !st.block_names.is_empty(),
        Selection::Manual => false, // no selection-gathering service wired up yet
        Selection::All => true,
        Selection::List => false, // not implemented - always disabled, see `Selection::List`'s radio button
    };
    selection_ready && num(&st.constant).is_some()
}

fn string_list(ui: &mut egui::Ui, id: &str, items: &mut Vec<String>) {
    let mut remove: Option<usize> = None;
    egui::ScrollArea::vertical().id_salt(id).max_height(70.0).show(ui, |ui| {
        for (i, item) in items.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(item.as_str());
                if ui.small_button("✕").clicked() {
                    remove = Some(i);
                }
            });
        }
    });
    if let Some(i) = remove {
        items.remove(i);
    }
    let text_id = ui.id().with(id).with("add");
    let mut text = ui.data_mut(|d| d.get_temp::<String>(text_id)).unwrap_or_default();
    ui.horizontal(|ui| {
        ui.text_edit_singleline(&mut text);
        if ui.small_button("+").clicked() && !text.trim().is_empty() {
            items.push(text.trim().to_string());
            text.clear();
        }
    });
    ui.data_mut(|d| d.insert_temp(text_id, text));
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let Some(mut st) = app.edit_altitudes_dlg.clone() else {
        *open = false;
        return;
    };

    let (mut run, mut close) = (false, false);
    egui::Window::new(crate::tl!("Edit Altitudes")).open(open).resizable(true).collapsible(false).default_size(vec2(560.0, 520.0)).show(ctx, |ui| {
        ui.label(crate::tl!("Points to take into account"));
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.radio_value(&mut st.selection, Selection::Blocks, crate::tl!("Point block name(s) + layer(s)"));
                ui.radio_value(&mut st.selection, Selection::Manual, crate::tl!("Manual selection"));
                ui.radio_value(&mut st.selection, Selection::All, crate::tl!("Every point in the drawing"));
                ui.add_enabled_ui(false, |ui| {
                    ui.radio_value(&mut st.selection, Selection::List, crate::tl!("Every point in the list"));
                });
            });
            ui.separator();
            ui.vertical(|ui| {
                ui.label(crate::tl!("Point block name(s)"));
                string_list(ui, "cc_altitudes_blocks", &mut st.block_names);
                ui.add_space(6.0);
                ui.label(crate::tl!("Block insertion layer(s)"));
                string_list(ui, "cc_altitudes_layers", &mut st.layers);
                ui.add_enabled_ui(st.selection == Selection::Manual, |ui| {
                    ui.checkbox(&mut st.use_filter_for_manual, crate::tl!("Use to filter the manual selection"));
                });
            });
        });
        if st.selection == Selection::Manual {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                crate::tl!("Manual selection isn't wired up in this dialog yet - pick \"Point block name(s) + layer(s)\" or \"Every point in the drawing\" instead."),
            );
        }
        ui.separator();

        ui.checkbox(&mut st.matricule_filter, crate::tl!("Filter by matricule"));
        ui.add_enabled_ui(st.matricule_filter, |ui| {
            ui.horizontal(|ui| {
                ui.label(crate::tl!("First matricule"));
                ui.text_edit_singleline(&mut st.matricule_first);
                ui.label(crate::tl!("Last matricule"));
                ui.text_edit_singleline(&mut st.matricule_last);
            });
        });
        ui.separator();

        ui.label(crate::tl!("Operations to perform"));
        egui::Grid::new("cc_altitudes_ops").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            ui.vertical(|ui| {
                ui.radio_value(&mut st.mode, Mode::Keep2D, crate::tl!("Set/keep points in 2D"));
                ui.radio_value(&mut st.mode, Mode::Keep3D, crate::tl!("Set/keep points in 3D"));
            });
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(crate::tl!("Altimetric constant"));
                    ui.text_edit_singleline(&mut st.constant);
                    ui.label("m");
                });
                ui.horizontal(|ui| {
                    ui.label(crate::tl!("New number of decimals"));
                    egui::ComboBox::from_id_salt("cc_altitudes_decimals").selected_text(st.decimals.to_string()).show_ui(ui, |ui| {
                        for d in 0..=6u32 {
                            ui.selectable_value(&mut st.decimals, d, d.to_string());
                        }
                    });
                });
            });
            ui.end_row();
        });

        ui.separator();
        ui.horizontal(|ui| {
            run = ui.add_enabled(ready(&st), egui::Button::new(crate::tl!("OK"))).clicked();
            close = ui.button(crate::tl!("Cancel")).clicked();
        });
    });
    if close {
        *open = false;
    }

    if run {
        let Some(constant) = num(&st.constant) else {
            app.edit_altitudes_dlg = Some(st);
            return;
        };
        let mut params = serde_json::Map::new();
        params.insert(
            "selection".into(),
            json!(match st.selection {
                Selection::Blocks => "blocks",
                Selection::Manual => "manual",
                Selection::All => "all",
                Selection::List => "list",
            }),
        );
        params.insert("blockNames".into(), json!(st.block_names));
        params.insert("layers".into(), json!(st.layers));
        params.insert("useFilterForManual".into(), json!(st.use_filter_for_manual));
        if st.matricule_filter {
            params.insert("matriculeFirst".into(), json!(st.matricule_first.trim()));
            params.insert("matriculeLast".into(), json!(st.matricule_last.trim()));
        }
        params.insert("mode".into(), json!(if st.mode == Mode::Keep3D { "3d" } else { "2d" }));
        params.insert("constant".into(), json!(constant));
        params.insert("decimals".into(), json!(st.decimals));

        let r = app.run("topo.edit_altitudes", Value::Object(params));
        match &r {
            Ok(v) => {
                let n = v["updated"].as_u64().unwrap_or(0) as usize;
                app.session.echo(crate::tf!("{n} point(s) updated", n = n));
                // OK closes the dialog on success - no re-zoom: every edited point was already on
                // screen (only its elevation changed, never its X/Y), so keeping the current view
                // is less disruptive than recentring on them. `close` was already handled above
                // this frame, so closing here sets `*open` directly instead.
                *open = false;
            }
            Err(e) => app.session.echo(e.clone()),
        }
    }

    if *open {
        app.edit_altitudes_dlg = Some(st);
    } else {
        app.edit_altitudes_dlg = None;
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
    fn all_selection_is_ready_once_the_constant_parses() {
        let mut st = State { selection: Selection::All, constant: String::new(), ..State::default() };
        assert!(!ready(&st), "empty constant should block Compute");
        st.constant = "1.50".into();
        assert!(ready(&st));
    }

    #[test]
    fn blocks_selection_needs_at_least_one_block_name() {
        let mut st = State { selection: Selection::Blocks, constant: "0".into(), ..State::default() };
        assert!(!ready(&st));
        st.block_names.push("CADCRAFT_TCPOINT".into());
        assert!(ready(&st));
    }

    #[test]
    fn runs_the_command_and_reports_how_many_points_were_updated() {
        let mut app = app_with_doc();
        // One point block in scope, inserted the same way `topo.import_points` would.
        let path = std::env::temp_dir().join(format!("cadcraft-editaltitudes-ui-test-{}.csv", std::process::id()));
        std::fs::write(&path, "ID;X;Y;Z;CODE\nP1;0;0;10;X\n").unwrap();
        app.run("topo.import_points", json!({ "path": path.to_string_lossy(), "label": false })).unwrap();
        let _ = std::fs::remove_file(&path);

        open(&mut app);
        let mut st = app.edit_altitudes_dlg.clone().unwrap();
        st.selection = Selection::All;
        st.constant = "2.0".into();
        app.edit_altitudes_dlg = Some(st.clone());
        assert!(ready(&st));

        let r = app.run("topo.edit_altitudes", json!({ "selection": "all", "mode": "3d", "constant": 2.0, "decimals": 2 }));
        assert_eq!(r.unwrap()["updated"].as_u64(), Some(1));
    }
}
