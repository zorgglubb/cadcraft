//! The Edit Altitudes dialog: TOPO.EDIT_ALTITUDES opened with no parameters - batch-adds a
//! constant to every matching topo point block's elevation, the way a total-station survey often
//! needs after discovering a benchmark offset (the constant is the field that matters most: most
//! other fields just narrow which points it applies to).

use egui::vec2;
use serde_json::{Value, json};

use crate::CadApp;

#[derive(Clone, Copy, PartialEq, Default)]
pub enum Selection {
    Blocks,
    #[default]
    Manual,
    All,
    List,
}

#[derive(Clone, Copy, PartialEq, Default)]
pub enum Mode {
    Keep2D,
    #[default]
    Keep3D,
}

#[derive(Clone, Default)]
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
    /// A prior `data.pick_entities` pick for `Selection::Manual` (empty until the "Select..."
    /// button has been used at least once).
    pub handles: Vec<cadcraft_doc::Handle>,
    /// `true` between starting `data.pick_entities` and [`resume`] bringing the dialog back.
    picking: bool,
}

pub fn open(app: &mut CadApp) {
    app.edit_altitudes_dlg = Some(State { constant: "0.0000".into(), decimals: 2, ..State::default() });
    app.ui.dialog = Some("editaltitudes".into());
}

fn num(s: &str) -> Option<f64> {
    let t = s.trim().replace(',', ".");
    if t.is_empty() { None } else { t.parse::<f64>().ok() }
}

/// `true` once the selection mode has what it needs and the constant parses.
fn ready(st: &State) -> bool {
    let selection_ready = match st.selection {
        Selection::Blocks => !st.block_names.is_empty(),
        Selection::Manual => !st.handles.is_empty(),
        Selection::All => true,
        Selection::List => false, // not implemented - always disabled, see `Selection::List`'s radio button
    };
    selection_ready && num(&st.constant).is_some()
}

/// Hides the dialog and starts `data.pick_entities`, the same "Select objects" phase/resume
/// pattern `tabledlg::start_pick`/`resume_assign` use for Data ▸ Assign Table....
fn start_pick(app: &mut CadApp) {
    let Some(st) = app.edit_altitudes_dlg.as_mut() else { return };
    st.picking = true;
    app.session.table_pick = None;
    app.ui.dialog = None;
    app.start("data.pick_entities");
}

/// Every frame: once a DATA.PICK_ENTITIES this dialog started has ended, take its handles and
/// bring the dialog back (the handle set is left unchanged when the pick was cancelled).
pub fn resume(app: &mut CadApp) {
    if !app.edit_altitudes_dlg.as_ref().is_some_and(|s| s.picking) {
        return;
    }
    if app.session.running.is_some() {
        return;
    }
    let picked = app.session.table_pick.take();
    let Some(st) = app.edit_altitudes_dlg.as_mut() else { return };
    st.picking = false;
    if let Some(hs) = picked {
        st.handles = hs;
    }
    app.ui.dialog = Some("editaltitudes".into());
}

/// A scrollable checklist over `all` (every name actually usable - a block already present on a
/// topo point, or an existing layer), toggling membership in `selected`. Replaces free-text entry
/// so a name can't be mistyped into matching nothing; `all` empty shows a plain "none yet" hint.
/// Shared with [`crate::createmntdlg`], which picks points for a MNT/TIN the same way.
pub(crate) fn checklist(ui: &mut egui::Ui, id: &str, all: &[String], selected: &mut Vec<String>) {
    if all.is_empty() {
        ui.weak(crate::tl!("(none in the drawing yet)"));
        return;
    }
    egui::ScrollArea::vertical().id_salt(id).max_height(70.0).show(ui, |ui| {
        for name in all {
            let mut checked = selected.iter().any(|s| s.eq_ignore_ascii_case(name));
            if ui.checkbox(&mut checked, name.as_str()).changed() {
                if checked {
                    if !selected.iter().any(|s| s.eq_ignore_ascii_case(name)) {
                        selected.push(name.clone());
                    }
                } else {
                    selected.retain(|s| !s.eq_ignore_ascii_case(name));
                }
            }
        }
    });
}

/// Every block name already used by a topo point - free text would otherwise let a name be
/// mistyped into silently matching nothing.
pub(crate) fn block_choices(app: &mut CadApp) -> Vec<String> {
    app.run("topo.point_block_names", json!({}))
        .ok()
        .and_then(|v| v.get("blocks").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

/// Every layer the drawing actually has.
pub(crate) fn layer_choices(app: &CadApp) -> Vec<String> {
    app.session.doc().map(|d| d.layers.iter().map(|l| l.name.clone()).collect()).unwrap_or_default()
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let Some(mut st) = app.edit_altitudes_dlg.clone() else {
        *open = false;
        return;
    };

    // What a block-name/layer picker can offer.
    let block_choices = block_choices(app);
    let layer_choices = layer_choices(app);

    let mut pick = false;
    let (mut run, mut close) = (false, false);
    egui::Window::new(crate::tl!("Edit Altitudes")).open(open).resizable(true).collapsible(false).default_size(vec2(560.0, 520.0)).show(ctx, |ui| {
        // Everything above the OK/Cancel row scrolls on its own, so shrinking the window (it's
        // resizable) clips the point-selection/matricule/operations groups rather than pushing
        // the button row itself out of view or off the bottom of the window.
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.label(crate::tl!("Points to take into account"));
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.radio_value(&mut st.selection, Selection::Blocks, crate::tl!("Point block name(s) + layer(s)"));
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut st.selection, Selection::Manual, crate::tl!("Manual selection"));
                        if ui.button(crate::tl!("Select...")).clicked() {
                            st.selection = Selection::Manual;
                            pick = true;
                        }
                        ui.label(crate::tf!("{n} point(s)", n = st.handles.len()));
                    });
                    ui.radio_value(&mut st.selection, Selection::All, crate::tl!("Every point in the drawing"));
                    ui.add_enabled_ui(false, |ui| {
                        ui.radio_value(&mut st.selection, Selection::List, crate::tl!("Every point in the list"));
                    });
                });
                ui.separator();
                ui.vertical(|ui| {
                    ui.label(crate::tl!("Point block name(s)"));
                    checklist(ui, "cc_altitudes_blocks", &block_choices, &mut st.block_names);
                    ui.add_space(6.0);
                    ui.label(crate::tl!("Block insertion layer(s)"));
                    checklist(ui, "cc_altitudes_layers", &layer_choices, &mut st.layers);
                    ui.add_enabled_ui(st.selection == Selection::Manual, |ui| {
                        ui.checkbox(&mut st.use_filter_for_manual, crate::tl!("Use to filter the manual selection"));
                    });
                });
            });
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
                    // Without a floor, shrinking the (resizable) window narrows this grid column
                    // past the radio labels' natural width, and egui wraps them one letter per
                    // line instead of leaving the text be - a floor wide enough for the longer
                    // label keeps it on one line at any window size.
                    ui.set_min_width(190.0);
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

    if pick {
        app.edit_altitudes_dlg = Some(st);
        start_pick(app);
        return;
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
        if st.selection == Selection::Manual {
            params.insert("handles".into(), json!(st.handles.iter().map(|h| h.hex()).collect::<Vec<_>>()));
        }
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

    /// Reproduces the reported layout bug: shrinking the (resizable) window narrowed the mode
    /// radios' grid column past their label's width, and egui wrapped "Set/keep points in 2D"
    /// one letter per line instead of leaving it on one line. `set_min_width` on that column
    /// should keep each label as a single `Shape::Text`, and the OK/Cancel row should still be
    /// reachable (not scrolled away with the rest of the content).
    #[test]
    fn mode_labels_and_buttons_stay_readable_in_a_narrow_window() {
        use egui::{Shape, vec2};
        use egui_kittest::Harness;

        fn texts(shape: &Shape, out: &mut Vec<String>) {
            match shape {
                Shape::Text(t) => out.push(t.galley.text().to_string()),
                Shape::Vec(v) => v.iter().for_each(|s| texts(s, out)),
                _ => {}
            }
        }

        let mut app = app_with_doc();
        open(&mut app);
        let mut open_flag = true;
        // Narrower than the dialog's natural content width (560x520 default), the way the
        // user's own manual resize was, but plenty tall - this isolates the horizontal squeeze
        // from any vertical one.
        let mut h = Harness::builder().with_size(vec2(450.0, 700.0)).build_ui_state(
            |ui, app: &mut CadApp| {
                dialog(app, ui.ctx(), &mut open_flag);
            },
            app,
        );
        h.run_steps(3);
        let mut found = Vec::new();
        for s in &h.output().shapes {
            texts(&s.shape, &mut found);
        }
        assert!(found.iter().any(|t| t == "Set/keep points in 2D"), "label wrapped: {found:?}");
        assert!(found.iter().any(|t| t == "Set/keep points in 3D"), "label wrapped: {found:?}");
        assert!(found.iter().any(|t| t == "OK"), "OK button not rendered: {found:?}");
        assert!(found.iter().any(|t| t == "Cancel"), "Cancel button not rendered: {found:?}");
    }

    #[test]
    fn manual_selection_is_ready_only_once_something_was_picked() {
        let st = State { selection: Selection::Manual, constant: "0".into(), ..State::default() };
        assert!(!ready(&st), "no prior pick yet");
        let st = State { handles: vec![cadcraft_doc::Handle(1)], ..st };
        assert!(ready(&st));
    }

    /// The "Select..." button hides the dialog, runs `data.pick_entities`, and `resume` brings
    /// it back with the picked handles in `State::handles` - the same flow `tabledlg`'s Assign
    /// Table uses, now wired up here instead of leaving "Manual selection" permanently disabled.
    #[test]
    fn select_button_picks_entities_and_resume_brings_the_handles_back() {
        let mut app = app_with_doc();
        let r = app.session.execute("point", &json!({ "at": [0.0, 0.0] })).unwrap();
        let h = r["handles"][0].as_str().unwrap().to_string();
        let handle = cadcraft_doc::Handle::parse_hex(&h).unwrap();

        open(&mut app);
        start_pick(&mut app);
        assert_eq!(app.ui.dialog, None, "dialog hides while picking");
        assert!(app.edit_altitudes_dlg.as_ref().unwrap().picking);

        app.session.input(cadcraft_engine::Input::Pick(vec![handle])).unwrap();
        app.session.input(cadcraft_engine::Input::Enter).unwrap();
        resume(&mut app);

        assert_eq!(app.ui.dialog, Some("editaltitudes".into()));
        let st = app.edit_altitudes_dlg.expect("state kept");
        assert!(!st.picking);
        assert_eq!(st.handles, vec![handle]);
    }

    /// Block-name and layer pickers are now a checklist over what the drawing actually has
    /// (`topo.point_block_names` for blocks, `Drawing::layers` for layers) rather than free text
    /// a user could mistype into matching nothing.
    #[test]
    fn block_and_layer_choices_come_from_the_drawing() {
        let mut app = app_with_doc();
        let path = std::env::temp_dir().join(format!("cadcraft-editaltitudes-checklist-test-{}.csv", std::process::id()));
        std::fs::write(&path, "ID;X;Y;Z;CODE\nP1;0;0;10;X\n").unwrap();
        app.run("topo.import_points", json!({ "path": path.to_string_lossy(), "layer": "BORNES", "label": false })).unwrap();
        let _ = std::fs::remove_file(&path);

        // What the dialog's pickers would offer - checked directly against the drawing rather
        // than through a render (the dialog scrolls a lot of content into a modest default
        // window size, so a lower item can be clipped out of a headless harness's painted output
        // well before it would need scrolling for a real person using a normally-sized window).
        assert_eq!(block_choices(&mut app), vec!["CADCRAFT_TCPOINT".to_string()]);
        let layers = layer_choices(&app);
        assert!(layers.iter().any(|n| n == "BORNES"), "{layers:?}");

        // And a light render smoke test: the dialog opens and paints the one block choice
        // without panicking.
        open(&mut app);
        use egui::{Shape, vec2};
        use egui_kittest::Harness;
        fn texts(shape: &Shape, out: &mut Vec<String>) {
            match shape {
                Shape::Text(t) => out.push(t.galley.text().to_string()),
                Shape::Vec(v) => v.iter().for_each(|s| texts(s, out)),
                _ => {}
            }
        }
        let mut open_flag = true;
        let mut h = Harness::builder().with_size(vec2(700.0, 700.0)).build_ui_state(
            |ui, app: &mut CadApp| {
                dialog(app, ui.ctx(), &mut open_flag);
            },
            app,
        );
        h.run_steps(3);
        let mut found = Vec::new();
        for s in &h.output().shapes {
            texts(&s.shape, &mut found);
        }
        assert!(found.iter().any(|t| t == "CADCRAFT_TCPOINT"), "{found:?}");
    }
}
