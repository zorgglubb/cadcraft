//! Topo 3D ▸ Create TIN... (TERRAIN.CREATE_MNT) opened with no parameters: a small wizard over
//! that command, mirroring `topo.edit_altitudes`'s own point-source picker (block name(s) +
//! layer(s), or a manual selection via the same `data.pick_entities` pick/resume flow) rather
//! than inventing a second one, since both ultimately gather the same kind of topo points. What's
//! specific to a MNT here is the altitude filter and the live "Estimate" preview before actually
//! triangulating and drawing.

use egui::vec2;
use serde_json::{Value, json};

use crate::CadApp;

#[derive(Clone, Copy, PartialEq, Default)]
pub enum Selection {
    #[default]
    Auto,
    Manual,
}

/// The last `terrain.create_mnt` dry run (`draw: false`), shown as a recap before the real one.
#[derive(Clone)]
pub struct Estimate {
    pub point_count: u64,
    pub skipped_null_altitude: u64,
    pub error: Option<String>,
}

#[derive(Clone, Default)]
pub struct State {
    pub selection: Selection,
    pub block_names: Vec<String>,
    pub layers: Vec<String>,
    pub ignore_null_altitude: bool,
    pub handles: Vec<cadcraft_doc::Handle>,
    picking: bool,
    pub estimate: Option<Estimate>,
}

pub fn open(app: &mut CadApp) {
    app.create_mnt_dlg = Some(State { ignore_null_altitude: true, ..State::default() });
    app.ui.dialog = Some("createmnt".into());
}

fn start_pick(app: &mut CadApp) {
    let Some(st) = app.create_mnt_dlg.as_mut() else { return };
    st.picking = true;
    app.session.table_pick = None;
    app.ui.dialog = None;
    app.start("data.pick_entities");
}

/// Every frame: once a DATA.PICK_ENTITIES this dialog started has ended, take its handles and
/// bring the dialog back (same pattern as `editaltitudesdlg::resume`/`tabledlg::resume_assign`).
pub fn resume(app: &mut CadApp) {
    if !app.create_mnt_dlg.as_ref().is_some_and(|s| s.picking) {
        return;
    }
    if app.session.running.is_some() {
        return;
    }
    let picked = app.session.table_pick.take();
    let Some(st) = app.create_mnt_dlg.as_mut() else { return };
    st.picking = false;
    if let Some(hs) = picked {
        st.handles = hs;
    }
    app.ui.dialog = Some("createmnt".into());
}

fn params(st: &State, draw: bool) -> Value {
    let mut p = serde_json::Map::new();
    p.insert("selection".into(), json!(if st.selection == Selection::Manual { "manual" } else { "auto" }));
    p.insert("blockNames".into(), json!(st.block_names));
    p.insert("layers".into(), json!(st.layers));
    p.insert("ignoreNullAltitude".into(), json!(st.ignore_null_altitude));
    p.insert("draw".into(), json!(draw));
    if st.selection == Selection::Manual {
        p.insert("handles".into(), json!(st.handles.iter().map(|h| h.hex()).collect::<Vec<_>>()));
    }
    Value::Object(p)
}

fn ready(st: &State) -> bool {
    match st.selection {
        Selection::Auto => true,
        Selection::Manual => !st.handles.is_empty(),
    }
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let Some(mut st) = app.create_mnt_dlg.clone() else {
        *open = false;
        return;
    };

    let block_choices = crate::editaltitudesdlg::block_choices(app);
    let layer_choices = crate::editaltitudesdlg::layer_choices(app);

    let mut pick = false;
    let mut estimate = false;
    let mut run = false;
    let mut close = false;
    egui::Window::new(crate::tl!("Create TIN...")).open(open).resizable(true).collapsible(false).default_size(vec2(520.0, 460.0)).show(ctx, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.label(crate::tl!("Points to take into account"));
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.radio_value(&mut st.selection, Selection::Auto, crate::tl!("Point block name(s) + layer(s)"));
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut st.selection, Selection::Manual, crate::tl!("Manual selection"));
                        if ui.button(crate::tl!("Select...")).clicked() {
                            st.selection = Selection::Manual;
                            pick = true;
                        }
                        ui.label(crate::tf!("{n} point(s)", n = st.handles.len()));
                    });
                });
                ui.separator();
                ui.vertical(|ui| {
                    ui.label(crate::tl!("Point block name(s)"));
                    crate::editaltitudesdlg::checklist(ui, "cc_mnt_blocks", &block_choices, &mut st.block_names);
                    ui.add_space(6.0);
                    ui.label(crate::tl!("Block insertion layer(s)"));
                    crate::editaltitudesdlg::checklist(ui, "cc_mnt_layers", &layer_choices, &mut st.layers);
                });
            });
            ui.separator();

            ui.checkbox(&mut st.ignore_null_altitude, crate::tl!("Ignore points with a null altitude"));
            ui.separator();

            if ui.button(crate::tl!("Estimate...")).clicked() {
                estimate = true;
            }
            if let Some(e) = &st.estimate {
                egui::Grid::new("cc_mnt_estimate").num_columns(2).show(ui, |ui| {
                    ui.label(crate::tl!("Points usable"));
                    ui.label(e.point_count.to_string());
                    ui.end_row();
                    ui.label(crate::tl!("Skipped (null altitude)"));
                    ui.label(e.skipped_null_altitude.to_string());
                    ui.end_row();
                });
                if let Some(err) = &e.error {
                    ui.colored_label(ui.visuals().warn_fg_color, err);
                }
            }
        });

        ui.separator();
        ui.horizontal(|ui| {
            run = ui.add_enabled(ready(&st), egui::Button::new(crate::tl!("Create"))).clicked();
            close = ui.button(crate::tl!("Cancel")).clicked();
        });
    });

    if close {
        *open = false;
    }

    if pick {
        app.create_mnt_dlg = Some(st);
        start_pick(app);
        return;
    }

    if estimate {
        let p = params(&st, false);
        match app.run("terrain.create_mnt", p) {
            Ok(v) => {
                st.estimate = Some(Estimate {
                    point_count: v["pointCount"].as_u64().unwrap_or(0),
                    skipped_null_altitude: v["skippedNullAltitude"].as_u64().unwrap_or(0),
                    error: None,
                });
            }
            Err(e) => st.estimate = Some(Estimate { point_count: 0, skipped_null_altitude: 0, error: Some(e) }),
        }
    }

    if run {
        let p = params(&st, true);
        match app.run("terrain.create_mnt", p) {
            Ok(v) => {
                let n = v["pointCount"].as_u64().unwrap_or(0);
                let t = v["triangleCount"].as_u64().unwrap_or(0);
                app.session.echo(crate::tf!("TIN created: {n} point(s), {t} triangle(s)", n = n, t = t));
                *open = false;
            }
            Err(e) => app.session.echo(e),
        }
    }

    if *open {
        app.create_mnt_dlg = Some(st);
    } else {
        app.create_mnt_dlg = None;
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

    fn import_tcpoint(app: &mut CadApp, id: &str, x: f64, y: f64, z: f64) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("cadcraft-createmnt-ui-test-{}-{id}-{n}.csv", std::process::id()));
        std::fs::write(&path, format!("ID;X;Y;Z;CODE\n{id};{x};{y};{z};P\n")).unwrap();
        app.run("topo.import_points", json!({ "path": path.to_string_lossy(), "label": false })).unwrap();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn auto_selection_is_always_ready_manual_needs_a_pick() {
        let st = State { selection: Selection::Auto, ..State::default() };
        assert!(ready(&st));
        let st = State { selection: Selection::Manual, ..State::default() };
        assert!(!ready(&st));
        let st = State { handles: vec![cadcraft_doc::Handle(1)], ..st };
        assert!(ready(&st));
    }

    #[test]
    fn select_button_picks_entities_and_resume_brings_the_handles_back() {
        let mut app = app_with_doc();
        let r = app.session.execute("point", &json!({ "at": [0.0, 0.0] })).unwrap();
        let h = r["handles"][0].as_str().unwrap().to_string();
        let handle = cadcraft_doc::Handle::parse_hex(&h).unwrap();

        open(&mut app);
        start_pick(&mut app);
        assert_eq!(app.ui.dialog, None);
        assert!(app.create_mnt_dlg.as_ref().unwrap().picking);

        app.session.input(cadcraft_engine::Input::Pick(vec![handle])).unwrap();
        app.session.input(cadcraft_engine::Input::Enter).unwrap();
        resume(&mut app);

        assert_eq!(app.ui.dialog, Some("createmnt".into()));
        let st = app.create_mnt_dlg.expect("state kept");
        assert!(!st.picking);
        assert_eq!(st.handles, vec![handle]);
    }

    #[test]
    fn estimate_and_create_run_the_command_with_the_right_draw_flag() {
        let mut app = app_with_doc();
        import_tcpoint(&mut app, "1", 0.0, 0.0, 10.0);
        import_tcpoint(&mut app, "2", 10.0, 0.0, 12.0);
        import_tcpoint(&mut app, "3", 5.0, 10.0, 15.0);

        open(&mut app);
        let st = State { ignore_null_altitude: true, ..app.create_mnt_dlg.clone().unwrap() };
        app.create_mnt_dlg = Some(st);

        let est = app.run("terrain.create_mnt", params(&app.create_mnt_dlg.clone().unwrap(), false)).unwrap();
        assert_eq!(est["pointCount"].as_u64(), Some(3));
        assert_eq!(est["edgeCount"].as_u64(), Some(0), "estimate must not draw");

        let before = app.session.doc().unwrap().model.iter().count();
        let created = app.run("terrain.create_mnt", params(&app.create_mnt_dlg.clone().unwrap(), true)).unwrap();
        assert_eq!(created["triangleCount"].as_u64(), Some(1));
        assert!(app.session.doc().unwrap().model.iter().count() > before, "create should draw the TIN");
    }
}
