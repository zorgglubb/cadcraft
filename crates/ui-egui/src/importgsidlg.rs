//! The Import GSI Carnet dialog: TOPO.IMPORT_GSI from a menu or the command line (no parameters)
//! picks the file first (same picker OPEN/IMPORT use), then inspects it with TOPO.GSI_INSPECT -
//! a coordinate-only carnet (WI 81/82/83) needs nothing further and imports as soon as this
//! opens, the same way the old one-shot behaviour did; a carnet with angle+distance shots and no
//! ready-made coordinates needs a station occupied and an orientation (a known VO, or references
//! to compute one from) first.
//!
//! Organised the way a field carnet actually gets worked up: every point the carnet decoded is
//! listed, and right-clicking a row tags it Station or Reference (everything else is a plain
//! measured point, imported as-is) - rather than retyping coordinates CADCraft already read from
//! the file. A reference often only has its coordinates in a separate control-point list, not in
//! the carnet itself, so "Import known references..." reads one (any delimiter, like
//! TOPO.IMPORT_POINTS's own picker) and fills in any carnet row whose id matches.

use egui::vec2;
use serde_json::{Value, json};

use crate::CadApp;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum Role {
    #[default]
    Measure,
    Station,
    Reference,
}

#[derive(Clone)]
struct GsiRow {
    id: String,
    code: Option<String>,
    hz: Option<f64>,
    vz: Option<f64>,
    dist: Option<f64>,
    x: Option<f64>,
    y: Option<f64>,
    z: Option<f64>,
    role: Role,
}

const DELIMITERS: [(&str, &str); 6] =
    [("auto", "Auto"), ("comma", "Comma (,)"), ("semicolon", "Semicolon (;)"), ("colon", "Colon (:)"), ("space", "Space"), ("tab", "Tab")];

#[derive(Clone)]
pub struct State {
    path: String,
    needs_station: bool,
    rows: Vec<GsiRow>,
    /// Manual station coordinates, used when the row tagged Station carries none of its own (a
    /// pure angle+distance carnet never does) - pre-filled from that row's coordinates when it
    /// has them, but always editable.
    station_x: String,
    station_y: String,
    station_z: String,
    from_references: bool,
    vo: String,
    /// The last "Import known references..." file, kept so changing the delimiter re-merges it.
    ref_path: Option<String>,
    ref_delimiter: String,
    ref_matched: usize,
    label: bool,
    /// The last "Compute" run's VO/σ, shown once coordinates have been previewed.
    last_vo: Option<(f64, Option<f64>)>,
}

fn opt_num(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(Value::as_f64)
}

impl State {
    fn new(path: String) -> Self {
        State {
            path,
            needs_station: false,
            rows: Vec::new(),
            station_x: String::new(),
            station_y: String::new(),
            station_z: "0".into(),
            from_references: false,
            vo: "0".into(),
            ref_path: None,
            ref_delimiter: "auto".into(),
            ref_matched: 0,
            label: true,
            last_vo: None,
        }
    }
}

pub fn open(app: &mut CadApp, path: String) {
    let mut st = State::new(path);
    if let Ok(v) = app.run("topo.gsi_inspect", json!({ "path": st.path })) {
        st.needs_station = v["needsStation"].as_bool().unwrap_or(false);
        st.rows = v["points"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|p| {
                Some(GsiRow {
                    id: p.get("id")?.as_str()?.to_string(),
                    code: p.get("code").and_then(Value::as_str).map(str::to_string),
                    hz: opt_num(p, "hz"),
                    vz: opt_num(p, "vz"),
                    dist: opt_num(p, "dist"),
                    x: opt_num(p, "x"),
                    y: opt_num(p, "y"),
                    z: opt_num(p, "z"),
                    role: Role::Measure,
                })
            })
            .collect();
    }
    // A coordinate-only carnet has nothing left to ask for - import right away, same as before
    // this dialog existed, and report the result rather than opening an empty-looking window.
    if !st.needs_station {
        run_import(app, &st);
        return;
    }
    app.import_gsi_dlg = Some(st);
    app.ui.dialog = Some("importgsi".into());
}

fn num(s: &str) -> Option<f64> {
    let t = s.trim().replace(',', ".");
    if t.is_empty() { None } else { t.parse::<f64>().ok() }
}

fn fmt(v: Option<f64>) -> String {
    v.map(|n| format!("{n:.4}")).unwrap_or_default()
}

/// References usable to compute VO: tagged Reference, with both coordinates (from the carnet or
/// a merged known-references file) and the carnet's own Hz reading for that shot.
fn usable_references(st: &State) -> Vec<&GsiRow> {
    st.rows.iter().filter(|r| r.role == Role::Reference && r.x.is_some() && r.y.is_some() && r.hz.is_some()).collect()
}

fn ready(st: &State) -> bool {
    let station = num(&st.station_x).is_some() && num(&st.station_y).is_some();
    station && if st.from_references { !usable_references(st).is_empty() } else { num(&st.vo).is_some() }
}

fn run_import(app: &mut CadApp, st: &State) {
    let mut params = serde_json::Map::new();
    params.insert("path".into(), json!(st.path));
    params.insert("label".into(), json!(st.label));
    if st.needs_station {
        let Some(x) = num(&st.station_x) else { return };
        let Some(y) = num(&st.station_y) else { return };
        params.insert("station".into(), json!([x, y, num(&st.station_z).unwrap_or(0.0)]));
        if st.from_references {
            let refs: Vec<Value> = usable_references(st)
                .iter()
                .map(|r| json!({ "point": [r.x.unwrap_or(0.0), r.y.unwrap_or(0.0), r.z.unwrap_or(0.0)], "hzReading": r.hz.unwrap_or(0.0) }))
                .collect();
            params.insert("references".into(), Value::Array(refs));
        } else if let Some(vo) = num(&st.vo) {
            params.insert("vo".into(), json!(vo));
        }
    }

    let r = app.run("topo.import_gsi", Value::Object(params));
    app.session.echo(match &r {
        Ok(v) => crate::tf!("Imported {n} point(s)", n = v.get("importedPoints").and_then(Value::as_u64).unwrap_or(0)),
        Err(e) => e.clone(),
    });
    if r.is_ok() {
        app.canvas.zoom_pending = true;
    }
}

/// Re-parses the known-references file at the dialog's chosen delimiter and fills in the
/// coordinates (and tags as Reference) of every carnet row whose id matches one in it -
/// idempotent, so it can be re-run every time the path or delimiter changes.
fn merge_references(app: &mut CadApp, st: &mut State) {
    st.ref_matched = 0;
    let Some(path) = st.ref_path.clone() else { return };
    let Ok(v) = app.run("topo.parse_points_csv", json!({ "path": path, "delimiter": st.ref_delimiter })) else { return };
    let Some(points) = v["points"].as_array().cloned() else { return };
    for p in &points {
        let Some(id) = p.get("id").and_then(Value::as_str) else { continue };
        if let Some(row) = st.rows.iter_mut().find(|r| r.id.eq_ignore_ascii_case(id)) {
            row.x = opt_num(p, "x");
            row.y = opt_num(p, "y");
            row.z = opt_num(p, "z");
            if row.hz.is_some() {
                row.role = Role::Reference;
            }
            st.ref_matched += 1;
        }
    }
}

/// Previews every angle+distance row's coordinates with the station/orientation currently filled
/// in (TOPO.COMPUTE_RADIATION - never touches the drawing), updating their X/Y/Z in place so the
/// person can see and export the result before committing to Import.
fn compute_coordinates(app: &mut CadApp, st: &mut State) {
    let (Some(x), Some(y)) = (num(&st.station_x), num(&st.station_y)) else { return };
    let station = json!([x, y, num(&st.station_z).unwrap_or(0.0)]);

    let mut params = serde_json::Map::new();
    params.insert("station".into(), station);
    if st.from_references {
        let refs: Vec<Value> = usable_references(st)
            .iter()
            .map(|r| json!({ "point": [r.x.unwrap_or(0.0), r.y.unwrap_or(0.0), r.z.unwrap_or(0.0)], "hzReading": r.hz.unwrap_or(0.0) }))
            .collect();
        if refs.is_empty() {
            return;
        }
        params.insert("references".into(), Value::Array(refs));
    } else {
        let Some(vo) = num(&st.vo) else { return };
        params.insert("vo".into(), json!(vo));
    }

    // Indices of every row this can actually compute (a full angle+distance shot), in the same
    // order the observations are sent - so the response's `points[i]` maps straight back to
    // `st.rows[indices[i]]`.
    let indices: Vec<usize> =
        st.rows.iter().enumerate().filter(|(_, r)| r.hz.is_some() && r.vz.is_some() && r.dist.is_some()).map(|(i, _)| i).collect();
    if indices.is_empty() {
        return;
    }
    let observations: Vec<Value> = indices
        .iter()
        .map(|&i| {
            let r = &st.rows[i];
            json!({ "hzReading": r.hz, "zenith": r.vz, "slopeDistance": r.dist })
        })
        .collect();
    params.insert("observations".into(), Value::Array(observations));

    if let Ok(v) = app.run("topo.compute_radiation", Value::Object(params)) {
        st.last_vo = Some((v["vo"].as_f64().unwrap_or(0.0), v["sigma"].as_f64()));
        if let Some(points) = v["points"].as_array() {
            for (&i, p) in indices.iter().zip(points) {
                st.rows[i].x = opt_num(p, "x");
                st.rows[i].y = opt_num(p, "y");
                st.rows[i].z = opt_num(p, "z");
            }
        }
    }
}

/// Writes every row that has coordinates (computed or already in the carnet) to a CSV/TXT file,
/// the same `ID;X;Y;Z;CODE` shape TOPO.IMPORT_POINTS/TOPO.PARSE_POINTS_CSV read back.
fn export_csv(app: &mut CadApp, st: &State) {
    let Some(pick) = app.services.pick_save.as_ref() else { return };
    let Some(path) = pick("points.csv", crate::POINTS_FILTERS) else { return };
    let mut out = String::from("ID;X;Y;Z;CODE\n");
    let mut written = 0usize;
    for r in &st.rows {
        let (Some(x), Some(y)) = (r.x, r.y) else { continue };
        out.push_str(&format!("{};{x};{y};{};{}\n", r.id, r.z.unwrap_or(0.0), r.code.as_deref().unwrap_or("")));
        written += 1;
    }
    match std::fs::write(&path, out) {
        Ok(()) => app.session.echo(crate::tf!("{n} point(s) exported to {path}", n = written, path = path.as_str())),
        Err(e) => app.session.echo(e.to_string()),
    }
}

fn use_as_station(st: &mut State, i: usize) {
    for r in &mut st.rows {
        if r.role == Role::Station {
            r.role = Role::Measure;
        }
    }
    let row = &mut st.rows[i];
    row.role = Role::Station;
    st.station_x = row.x.map(|n| format!("{n}")).unwrap_or_default();
    st.station_y = row.y.map(|n| format!("{n}")).unwrap_or_default();
    st.station_z = row.z.map(|n| format!("{n}")).unwrap_or_else(|| "0".into());
}

fn role_label(role: Role) -> &'static str {
    match role {
        Role::Measure => "",
        Role::Station => "📍",
        Role::Reference => "🎯",
    }
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let Some(mut st) = app.import_gsi_dlg.clone() else {
        *open = false;
        return;
    };

    let (mut import, mut close, mut pick_refs, mut delimiter_changed, mut compute, mut export) = (false, false, false, false, false, false);
    let mut row_action: Option<(usize, Role)> = None;
    egui::Window::new(crate::tl!("Import GSI Carnet")).open(open).resizable(true).collapsible(false).default_size(vec2(640.0, 560.0)).show(
        ctx,
        |ui| {
            ui.horizontal(|ui| {
                ui.label(crate::tl!("File"));
                ui.label(&st.path);
            });
            ui.label(crate::tf!("{n} point(s) read; right-click a row to tag it Station or Reference", n = st.rows.len()));
            ui.separator();
            egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                egui::Grid::new("cc_gsi_rows").striped(true).show(ui, |ui| {
                    ui.label("");
                    ui.label("ID");
                    ui.label(crate::tl!("Code"));
                    ui.label(crate::tl!("Hz reading (gon)"));
                    ui.label(crate::tl!("Zenith (gon)"));
                    ui.label(crate::tl!("Slope distance"));
                    ui.label("X");
                    ui.label("Y");
                    ui.label("Z");
                    ui.end_row();
                    for (i, r) in st.rows.iter().enumerate() {
                        // Union every cell's response into one: an untagged row's icon cell is
                        // an empty string (`role_label(Role::Measure)` is ""), so attaching the
                        // context menu to that first cell alone left almost no clickable area -
                        // right-clicking anywhere else on the row (the ID, a coordinate...) did
                        // nothing. Merging the responses makes the whole row right-clickable.
                        let mut resp = ui.label(role_label(r.role));
                        resp |= ui.label(&r.id);
                        resp |= ui.label(r.code.as_deref().unwrap_or(""));
                        resp |= ui.label(fmt(r.hz));
                        resp |= ui.label(fmt(r.vz));
                        resp |= ui.label(fmt(r.dist));
                        resp |= ui.label(fmt(r.x));
                        resp |= ui.label(fmt(r.y));
                        resp |= ui.label(fmt(r.z));
                        ui.end_row();
                        resp.context_menu(|ui| {
                            if ui.button(crate::tl!("Use as station")).clicked() {
                                row_action = Some((i, Role::Station));
                                ui.close();
                            }
                            if ui.button(crate::tl!("Mark as reference")).clicked() {
                                row_action = Some((i, Role::Reference));
                                ui.close();
                            }
                            if ui.button(crate::tl!("Mark as measured point")).clicked() {
                                row_action = Some((i, Role::Measure));
                                ui.close();
                            }
                        });
                    }
                });
            });
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button(crate::tl!("Import known references...")).clicked() {
                    pick_refs = true;
                }
                if let Some(path) = &st.ref_path {
                    ui.label(path);
                    ui.label(crate::tf!("{n} matched", n = st.ref_matched));
                }
            });
            if st.ref_path.is_some() {
                ui.horizontal(|ui| {
                    ui.label(crate::tl!("Delimiter"));
                    for (key, label) in DELIMITERS {
                        if ui.selectable_label(st.ref_delimiter == key, crate::i18n::t(label)).clicked() && st.ref_delimiter != key {
                            st.ref_delimiter = key.to_string();
                            delimiter_changed = true;
                        }
                    }
                });
            }
            ui.separator();
            ui.label(crate::tl!("Station"));
            ui.horizontal(|ui| {
                ui.label("X");
                ui.text_edit_singleline(&mut st.station_x);
                ui.label("Y");
                ui.text_edit_singleline(&mut st.station_y);
                ui.label("Z");
                ui.text_edit_singleline(&mut st.station_z);
            });
            ui.horizontal(|ui| {
                if ui.selectable_label(!st.from_references, crate::tl!("Known VO value")).clicked() {
                    st.from_references = false;
                }
                if ui.selectable_label(st.from_references, crate::tl!("Compute from references")).clicked() {
                    st.from_references = true;
                }
            });
            if st.from_references {
                let n = usable_references(&st).len();
                ui.label(crate::tf!("{n} tagged reference(s) with coordinates and an Hz reading", n = n));
            } else {
                ui.horizontal(|ui| {
                    ui.label(crate::tl!("VO (gon)"));
                    ui.text_edit_singleline(&mut st.vo);
                });
            }
            ui.horizontal(|ui| {
                compute = ui.add_enabled(ready(&st), egui::Button::new(crate::tl!("Compute"))).clicked();
                if let Some((vo, sigma)) = st.last_vo {
                    ui.label(match sigma {
                        Some(s) => crate::tf!("Computed VO: {vo} gon (σ {sigma})", vo = format!("{vo:.4}"), sigma = format!("{s:.4}")),
                        None => crate::tf!("VO: {vo} gon", vo = format!("{vo:.4}")),
                    });
                }
            });
            ui.checkbox(&mut st.label, crate::tl!("Show code label"));
            ui.separator();
            ui.horizontal(|ui| {
                import = ui.add_enabled(ready(&st), egui::Button::new(crate::tl!("Import"))).clicked();
                export = ui.button(crate::tl!("Export CSV/TXT...")).clicked();
                close = ui.button(crate::tl!("Close")).clicked();
            });
        },
    );
    if close {
        *open = false;
    }
    if let Some((i, role)) = row_action {
        if role == Role::Station {
            use_as_station(&mut st, i);
        } else {
            st.rows[i].role = role;
        }
    }
    if pick_refs {
        let picked = app.services.pick_open.as_ref().and_then(|pick| pick(crate::POINTS_FILTERS));
        if let Some(path) = picked {
            st.ref_path = Some(path);
            merge_references(app, &mut st);
        }
    }
    if delimiter_changed {
        merge_references(app, &mut st);
    }
    if compute {
        compute_coordinates(app, &mut st);
    }
    if export {
        export_csv(app, &st);
    }

    if import {
        run_import(app, &st);
        *open = false;
    }
    if *open {
        app.import_gsi_dlg = Some(st);
    } else {
        app.import_gsi_dlg = None;
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
    fn a_coordinate_only_carnet_imports_immediately_without_opening_the_dialog() {
        let mut app = app_with_doc();
        let path = std::env::temp_dir().join(format!("cadcraft-ui-gsi-coord-{}.gsi", std::process::id()));
        std::fs::write(&path, "110001+00000P1 81..00+00001234 82..00+00005678 83..00+00000100\n").unwrap();

        open(&mut app, path.to_string_lossy().to_string());
        let _ = std::fs::remove_file(&path);

        assert!(app.import_gsi_dlg.is_none(), "coordinate-only carnet should not need the dialog");
    }

    #[test]
    fn a_carnet_with_angle_distance_shots_opens_the_dialog_listing_its_rows() {
        let mut app = app_with_doc();
        let path = std::env::temp_dir().join(format!("cadcraft-ui-gsi-radial-{}.gsi", std::process::id()));
        std::fs::write(&path, "110001+00000P1 21.324+00005000 22.324+00010000 31..00+00010000\n").unwrap();

        open(&mut app, path.to_string_lossy().to_string());
        let _ = std::fs::remove_file(&path);

        let st = app.import_gsi_dlg.expect("dialog should open");
        assert!(st.needs_station);
        assert_eq!(st.rows.len(), 1);
        assert_eq!(st.rows[0].id, "P1");
        assert_eq!(st.rows[0].role, Role::Measure);
    }

    #[test]
    fn using_a_coordinate_row_as_station_prefills_its_coordinates() {
        let mut app = app_with_doc();
        let path = std::env::temp_dir().join(format!("cadcraft-ui-gsi-station-{}.gsi", std::process::id()));
        std::fs::write(
            &path,
            "110001+00000STA 81..00+00001000000 82..00+00001000000 83..00+00000100000\n\
             110002+00000P2 21.324+00005000 22.324+00010000 31..00+00010000\n",
        )
        .unwrap();

        open(&mut app, path.to_string_lossy().to_string());
        let _ = std::fs::remove_file(&path);
        let mut st = app.import_gsi_dlg.expect("dialog should open");

        let i = st.rows.iter().position(|r| r.id == "STA").expect("station row present");
        use_as_station(&mut st, i);

        assert_eq!(st.station_x, "1000");
        assert_eq!(st.station_y, "1000");
        assert_eq!(st.rows[i].role, Role::Station);
    }

    #[test]
    fn merging_a_references_file_fills_matching_rows_and_tags_them() {
        let mut app = app_with_doc();
        let gsi_path = std::env::temp_dir().join(format!("cadcraft-ui-gsi-refmerge-{}.gsi", std::process::id()));
        std::fs::write(&gsi_path, "110001+00000R1 21.324+00005000 22.324+00010000 31..00+00010000\n").unwrap();
        open(&mut app, gsi_path.to_string_lossy().to_string());
        let _ = std::fs::remove_file(&gsi_path);
        let mut st = app.import_gsi_dlg.clone().expect("dialog should open");

        let csv_path = std::env::temp_dir().join(format!("cadcraft-ui-gsi-refs-{}.csv", std::process::id()));
        std::fs::write(&csv_path, "ID;X;Y;Z\nR1;2000.0;3000.0;50.0\n").unwrap();
        st.ref_path = Some(csv_path.to_string_lossy().to_string());
        merge_references(&mut app, &mut st);
        let _ = std::fs::remove_file(&csv_path);

        assert_eq!(st.ref_matched, 1);
        let row = st.rows.iter().find(|r| r.id == "R1").unwrap();
        assert_eq!(row.x, Some(2000.0));
        assert_eq!(row.role, Role::Reference);
        assert_eq!(usable_references(&st).len(), 1);
    }
}
