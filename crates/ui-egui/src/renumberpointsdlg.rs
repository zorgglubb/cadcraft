//! Topo 2D ▸ Points ▸ Renumber Points... (TOPO.RENUMBER_POINTS): rewrites the matricule (MAT
//! attribute) of every matching topo point block - sequentially from a given number,
//! incrementing every existing numeric index by a constant, replacing the radical (the matricule's
//! leading non-digit prefix), or prepending text before the existing radical and index.
//!
//! Which points are in scope is picked the same way [`crate::editaltitudesdlg`] does (its own
//! `Selection` enum is reused here directly: block name(s) + layer(s), a manual selection, or
//! every point in the drawing - "every point in the list" has the same placeholder, disabled
//! radio, since nothing here keeps a list to draw from yet either), optionally narrowed further
//! by a matricule range. The read-only "existing matricules" table above the operations comes
//! from `topo.point_radicals`, grouping what's already in the drawing by radical so a person can
//! see what range to renumber into before doing it.

use egui::vec2;
use serde_json::{Value, json};

use crate::CadApp;
use crate::editaltitudesdlg::Selection;

#[derive(Clone, Copy, PartialEq, Default)]
pub enum Op {
    #[default]
    RenumberFrom,
    IncrementIndices,
    ChangeRadical,
    PrefixRadical,
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
    pub op: Op,
    /// The number ("Renumber from"/"Increment indices by") or text ("Change radicals to"/"Add
    /// before radicals") the chosen operation takes - kept as one field since only one is ever
    /// shown at a time.
    pub value: String,
    pub handles: Vec<cadcraft_doc::Handle>,
    picking: bool,
    pub last_error: Option<String>,
}

pub fn open(app: &mut CadApp) {
    app.renumber_points_dlg = Some(State { op: Op::RenumberFrom, value: "1000".into(), ..State::default() });
    app.ui.dialog = Some("renumberpoints".into());
}

fn start_pick(app: &mut CadApp) {
    let Some(st) = app.renumber_points_dlg.as_mut() else { return };
    st.picking = true;
    app.session.table_pick = None;
    app.ui.dialog = None;
    app.start("data.pick_entities");
}

pub fn resume(app: &mut CadApp) {
    if !app.renumber_points_dlg.as_ref().is_some_and(|s| s.picking) {
        return;
    }
    if app.session.running.is_some() {
        return;
    }
    let picked = app.session.table_pick.take();
    let Some(st) = app.renumber_points_dlg.as_mut() else { return };
    st.picking = false;
    if let Some(hs) = picked {
        st.handles = hs;
    }
    app.ui.dialog = Some("renumberpoints".into());
}

fn ready(st: &State) -> bool {
    let selection_ready = match st.selection {
        Selection::Blocks => !st.block_names.is_empty(),
        Selection::Manual => !st.handles.is_empty(),
        Selection::All => true,
        Selection::List => false,
    };
    if !selection_ready {
        return false;
    }
    match st.op {
        Op::RenumberFrom | Op::IncrementIndices => st.value.trim().parse::<f64>().is_ok(),
        Op::ChangeRadical | Op::PrefixRadical => !st.value.trim().is_empty(),
    }
}

fn params(st: &State) -> Option<Value> {
    let mut p = serde_json::Map::new();
    p.insert(
        "selection".into(),
        json!(match st.selection {
            Selection::Blocks => "blocks",
            Selection::Manual => "manual",
            Selection::All => "all",
            Selection::List => "list",
        }),
    );
    p.insert("blockNames".into(), json!(st.block_names));
    p.insert("layers".into(), json!(st.layers));
    p.insert("useFilterForManual".into(), json!(st.use_filter_for_manual));
    if st.selection == Selection::Manual {
        p.insert("handles".into(), json!(st.handles.iter().map(|h| h.hex()).collect::<Vec<_>>()));
    }
    if st.matricule_filter {
        p.insert("matriculeFirst".into(), json!(st.matricule_first.trim()));
        p.insert("matriculeLast".into(), json!(st.matricule_last.trim()));
    }
    p.insert(
        "mode".into(),
        json!(match st.op {
            Op::RenumberFrom => "renumberFrom",
            Op::IncrementIndices => "incrementIndices",
            Op::ChangeRadical => "changeRadical",
            Op::PrefixRadical => "prefixRadical",
        }),
    );
    match st.op {
        Op::RenumberFrom | Op::IncrementIndices => {
            p.insert("value".into(), json!(st.value.trim().parse::<f64>().ok()?));
        }
        Op::ChangeRadical | Op::PrefixRadical => {
            p.insert("value".into(), json!(st.value.trim()));
        }
    }
    Some(Value::Object(p))
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let Some(mut st) = app.renumber_points_dlg.clone() else {
        *open = false;
        return;
    };

    let block_choices = crate::editaltitudesdlg::block_choices(app);
    let layer_choices = crate::editaltitudesdlg::layer_choices(app);
    let radicals: Vec<Value> =
        app.run("topo.point_radicals", json!({})).ok().and_then(|v| v.get("radicals").and_then(Value::as_array).cloned()).unwrap_or_default();

    let mut pick = false;
    let (mut run, mut close) = (false, false);
    egui::Window::new(crate::tl!("Renumber Points...")).open(open).resizable(true).collapsible(false).default_size(vec2(600.0, 620.0)).show(
        ctx,
        |ui| {
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
                        crate::editaltitudesdlg::checklist(ui, "cc_renumber_blocks", &block_choices, &mut st.block_names);
                        ui.add_space(6.0);
                        ui.label(crate::tl!("Block insertion layer(s)"));
                        crate::editaltitudesdlg::checklist(ui, "cc_renumber_layers", &layer_choices, &mut st.layers);
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

                ui.label(crate::tl!("Existing matricules in the drawing"));
                egui_extras::TableBuilder::new(ui)
                    .resizable(true)
                    .striped(true)
                    .min_scrolled_height(90.0)
                    .column(egui_extras::Column::initial(140.0).at_least(60.0))
                    .column(egui_extras::Column::initial(140.0).at_least(60.0))
                    .column(egui_extras::Column::initial(80.0).at_least(50.0))
                    .header(20.0, |mut header| {
                        header.col(|ui| {
                            ui.strong(crate::tl!("Radical"));
                        });
                        header.col(|ui| {
                            ui.strong(crate::tl!("Index range"));
                        });
                        header.col(|ui| {
                            ui.strong(crate::tl!("Count"));
                        });
                    })
                    .body(|body| {
                        body.rows(18.0, radicals.len(), |mut row| {
                            let r = &radicals[row.index()];
                            row.col(|ui| {
                                let radical = r["radical"].as_str().unwrap_or("");
                                ui.label(if radical.is_empty() { "—" } else { radical });
                            });
                            row.col(|ui| {
                                ui.label(format!("{} - {}", r["minIndex"].as_i64().unwrap_or(0), r["maxIndex"].as_i64().unwrap_or(0)));
                            });
                            row.col(|ui| {
                                ui.label(r["count"].as_u64().unwrap_or(0).to_string());
                            });
                        });
                    });
                ui.separator();

                ui.label(crate::tl!("Processing to perform"));
                ui.radio_value(&mut st.op, Op::RenumberFrom, crate::tl!("Renumber starting at"));
                ui.radio_value(&mut st.op, Op::IncrementIndices, crate::tl!("Increment indices by"));
                ui.radio_value(&mut st.op, Op::ChangeRadical, crate::tl!("Change radicals to"));
                ui.radio_value(&mut st.op, Op::PrefixRadical, crate::tl!("Add before radicals"));
                ui.horizontal(|ui| {
                    ui.label(crate::tl!("Value"));
                    ui.text_edit_singleline(&mut st.value);
                });
                if let Some(err) = &st.last_error {
                    ui.colored_label(ui.visuals().warn_fg_color, err);
                }
            });

            ui.separator();
            ui.horizontal(|ui| {
                run = ui.add_enabled(ready(&st), egui::Button::new(crate::tl!("OK"))).clicked();
                close = ui.button(crate::tl!("Cancel")).clicked();
            });
        },
    );

    if close {
        *open = false;
    }

    if pick {
        app.renumber_points_dlg = Some(st);
        start_pick(app);
        return;
    }

    if run && let Some(p) = params(&st) {
        match app.run("topo.renumber_points", p) {
            Ok(v) => {
                let n = v["updated"].as_u64().unwrap_or(0);
                app.session.echo(crate::tf!("{n} point(s) renumbered", n = n));
                st.last_error = None;
                *open = false;
            }
            Err(e) => st.last_error = Some(e),
        }
    }

    if *open {
        app.renumber_points_dlg = Some(st);
    } else {
        app.renumber_points_dlg = None;
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

    fn import(app: &mut CadApp, mat: &str) {
        let path = std::env::temp_dir().join(format!("cadcraft-renumber-ui-test-{}-{mat}.csv", std::process::id()));
        std::fs::write(&path, format!("ID;X;Y;Z;CODE\n{mat};0;0;10;X\n")).unwrap();
        app.run("topo.import_points", json!({ "path": path.to_string_lossy(), "label": false })).unwrap();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn ready_needs_a_selection_and_a_valid_value_for_the_chosen_operation() {
        let st = State { selection: Selection::All, op: Op::RenumberFrom, value: "1000".into(), ..State::default() };
        assert!(ready(&st));

        let st = State { selection: Selection::All, op: Op::RenumberFrom, value: String::new(), ..State::default() };
        assert!(!ready(&st), "renumberFrom needs a numeric value");

        let st = State { selection: Selection::All, op: Op::ChangeRadical, value: String::new(), ..State::default() };
        assert!(!ready(&st), "changeRadical needs non-empty text");

        let st = State { selection: Selection::Blocks, op: Op::RenumberFrom, value: "1".into(), ..State::default() };
        assert!(!ready(&st), "blocks selection needs at least one block name");
    }

    #[test]
    fn params_sends_a_number_for_renumber_from_and_text_for_change_radical() {
        let st = State { selection: Selection::All, op: Op::RenumberFrom, value: "1000".into(), ..State::default() };
        let p = params(&st).unwrap();
        assert_eq!(p["value"].as_f64(), Some(1000.0));
        assert_eq!(p["mode"].as_str(), Some("renumberFrom"));

        let st = State { selection: Selection::All, op: Op::ChangeRadical, value: "B".into(), ..State::default() };
        let p = params(&st).unwrap();
        assert_eq!(p["value"].as_str(), Some("B"));
        assert_eq!(p["mode"].as_str(), Some("changeRadical"));
    }

    #[test]
    fn running_renumbers_every_point_and_reports_how_many() {
        let mut app = app_with_doc();
        import(&mut app, "5");
        import(&mut app, "2");

        open(&mut app);
        let st = State { selection: Selection::All, op: Op::RenumberFrom, value: "100".into(), ..app.renumber_points_dlg.clone().unwrap() };
        let out = app.run("topo.renumber_points", params(&st).unwrap()).unwrap();
        assert_eq!(out["updated"].as_u64(), Some(2));
    }

    #[test]
    fn select_button_picks_entities_and_resume_brings_the_handles_back() {
        let mut app = app_with_doc();
        import(&mut app, "1");
        let handle = app.session.doc().unwrap().model.iter().next().map(|e| e.handle).unwrap();

        open(&mut app);
        start_pick(&mut app);
        assert_eq!(app.ui.dialog, None, "dialog hides while picking");
        assert!(app.renumber_points_dlg.as_ref().unwrap().picking);

        let _ = app.session.start("data.pick_entities");
        app.session.input(cadcraft_engine::Input::Pick(vec![handle])).unwrap();
        app.session.input(cadcraft_engine::Input::Enter).unwrap();

        resume(&mut app);
        assert_eq!(app.ui.dialog, Some("renumberpoints".into()));
        let st = app.renumber_points_dlg.unwrap();
        assert!(!st.picking);
        assert_eq!(st.handles, vec![handle]);
    }
}
