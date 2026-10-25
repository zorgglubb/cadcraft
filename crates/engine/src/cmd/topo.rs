//! Topography commands built on `cadcraft-topo-calc`: orienting a station from known
//! references (zero value, *VO*) and computing new points from polar observations
//! (*rayonnement*), inserting the results directly into the active document as POINT
//! entities (with an optional TEXT label carrying each point's code).
//!
//! The file-import commands ([`run_import_gsi`], [`run_import_points`]) go one step further:
//! each imported point becomes a [`CADCRAFT_TCPOINT`](TCPOINT_BLOCK) block reference carrying its
//! matricule/id, altitude and code as MAT/ALT/COD attributes - real entity data that survives in
//! the drawing and shows up in Properties - rather than a bare POINT plus a loose text label.
//!
//! The point symbol (circle + cross) sits on layer `0`, so it floats to whichever layer the
//! block reference itself is inserted on - `topo.import_points`'s own `layer` param, created if
//! it doesn't exist yet. COD/MAT/ALT are drawn from each one's own attribute *definition*, laid
//! out around the point (COD top-left, MAT top-right, ALT bottom-right) and each on its own
//! dedicated layer (`TOPOCOD`/`TOPOMAT`/`TOPOALT`) rather than layer `0`: AutoCAD/CADCraft only
//! lets an attribute float to the reference's layer when its definition is on layer `0`, so
//! giving each one a real layer of its own is what makes, say, turning `TOPOMAT` off hide every
//! point's MAT text and nothing else (`crates/render/src/lib.rs`'s `insert` resolves each
//! attribute against its own definition's properties, not the reference's).

use cadcraft_color::Color;
use cadcraft_doc::{EntityKind, HAlign, Handle, Point, VAlign};
use cadcraft_geom::{Vec2, Vec3};
use cadcraft_topo_calc::{
    FreeStationObservation, RadiationObservation, ReferenceObservation, TraverseLeg, free_station, radiation, traverse, zero_value,
};
use cadcraft_topo_io::{csv_points, gsi, text_encoding};
use serde_json::{Value, json};

use super::draw::make_text;
use super::*;
use crate::{Result, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("topo.radiation", "Radiation", run_radiation)
            .menu(&["Topo Calculations", "Calculations", "Radiation"])
            .alias(&["rayonnement"])
            .params(
                "{station: [x,y,z], vo?: gon, references?: [{point: [x,y,z], hzReading: gon}], \
                 observations: [{hzReading, zenith, slopeDistance, instrumentHeight?, targetHeight?, code?}], label?: bool}",
            ),
        CommandSpec::new("topo.zero_value", "Zero Value (VO)", run_zero_value)
            .menu(&["Topo Calculations", "Calculations", "Zero Value"])
            .alias(&["vo"])
            .params("{station: [x,y,z], references: [{point: [x,y,z], hzReading: gon}]}"),
        CommandSpec::new("topo.free_station", "Free Station", run_free_station)
            .menu(&["Topo Calculations", "Calculations", "Free Station"])
            .alias(&["station_libre"])
            .params(
                "{references: [{point: [x,y,z], distance: number, hzReading: gon}] (>=2), approx?: [x,y,z], \
                 insertStation?: bool (default true), code?: string, label?: bool} → \
                 {station: [x,y,z], vo, sigma, iterations, point?}. Resections the station's X/Y from the \
                 horizontal distances to >=2 known points (independent of orientation), then derives its VO \
                 from their Hz readings.",
            ),
        CommandSpec::new("topo.cheminement", "Traverse (Cheminement)", run_cheminement)
            .menu(&["Topo Calculations", "Calculations", "Traverse (Cheminement)"])
            .alias(&["traverse"])
            .params(
                "{start: [x,y,z], legs: [{gisement: gon, distance: number, code?: string}], \
                 closingPoint?: [x,y,z], label?: bool} → {points: [...], closingError?: [ex,ey]}. Chains \
                 each leg from `start` and, when `closingPoint` is given, distributes the raw closing error \
                 across every station by the compass (Bowditch) rule so the last point lands exactly on it. \
                 Each computed station (not `start`, already in the drawing) is inserted as a point.",
            ),
        CommandSpec::new("topo.import_gsi", "Import GSI Carnet", run_import_gsi).menu(&["Topo Calculations", "Import", "GSI Carnet"]).params(
            "{path: string, station?: [x,y,z], vo?: gon, references?: [{point: [x,y,z], hzReading: gon}], \
                 label?: bool} - station/vo (or references) are required only if the \
                 carnet has angle+distance shots with no ready-made coordinates; a coordinate-only carnet \
                 (WI 81/82/83) imports with neither. Each point becomes a CADCRAFT_TCPOINT block reference \
                 with its id/matricule, altitude and code as MAT/ALT/COD attributes (`label: false` hides \
                 the visible code text; the attributes themselves are kept either way).",
        ),
        CommandSpec::new("topo.import_points", "Import Points (CSV/TXT)...", run_import_points)
            .menu(&["Topo Calculations", "Import", "Points CSV/TXT..."])
            .params(
                "{path: string, label?: bool, layer?: string (created if missing; default: the \
                 current layer), block?: string (the block definition to insert each point as - \
                 created the first time it's used, default \"CADCRAFT_TCPOINT\"), \
                 delimiter?: \"auto\"|\"comma\"|\"semicolon\"|\"colon\"|\"space\"|\"tab\" (default \"auto\"), \
                 threeD?: bool (default true: the reference's own insertion point carries the CSV's \
                 Z too, not just the ALT attribute, so the point sits at its real height; false \
                 keeps every point flat on the XY plane)} - each point becomes a `block` reference \
                 with its id/matricule, altitude and code as MAT/ALT/COD attributes.",
            ),
        // No menu entry: the Import Points dialog's own preview grid, recomputed as the person
        // changes the delimiter - never touches the drawing.
        CommandSpec::new("topo.preview_points_csv", "Preview Points CSV/TXT", run_preview_points_csv)
            .params(
                "{path: string, delimiter?: \"auto\"|\"comma\"|\"semicolon\"|\"colon\"|\"space\"|\"tab\" \
                 (default \"auto\"), lines?: number (default 10)} → {rows: [[string]]}",
            )
            .noundo(),
        // No menu entry: the Import GSI Carnet dialog's own first step, read before anything is
        // asked of the person - it decides whether the dialog needs to show the station/VO
        // fields at all (a coordinate-only carnet imports with neither).
        CommandSpec::new("topo.gsi_inspect", "Inspect GSI Carnet", run_gsi_inspect)
            .params("{path: string} → {pointCount, needsStation: bool, skippedLines, points: [{id, code, hz, vz, dist, x, y, z}]}")
            .noundo(),
        // No menu entry: the Import GSI dialog's "known references" file picker, matched by id
        // against the carnet's own rows rather than inserted on its own.
        CommandSpec::new("topo.parse_points_csv", "Parse Points CSV/TXT", run_parse_points_csv)
            .params(
                "{path: string, delimiter?: \"auto\"|\"comma\"|\"semicolon\"|\"colon\"|\"space\"|\"tab\" \
                 (default \"auto\")} → {points: [{id, x, y, z, code}], skippedLines}",
            )
            .noundo(),
        // No menu entry: the Import GSI dialog's "Compute" button, previewing every angle+distance
        // row's coordinates (once a station/orientation is known) before anything is imported -
        // touches nothing in the drawing, unlike `topo.radiation` itself.
        CommandSpec::new("topo.compute_radiation", "Compute Radiation (preview)", run_compute_radiation)
            .params(
                "{station: [x,y,z], vo?: gon, references?: [{point: [x,y,z], hzReading: gon}], \
                 observations: [{hzReading, zenith, slopeDistance, instrumentHeight?, targetHeight?, code?}]} \
                 → {vo, sigma, points: [{x, y, z, code}]}",
            )
            .noundo(),
        CommandSpec::new("topo.search_points", "Search Points...", run_search_points)
            .menu(&["Topo Calculations", "Search Points..."])
            .params(
                "{query?: string, field?: \"MAT\"|\"COD\"|\"ALT\", block?: string} \
             → {rows: [{handle, layer, mat, cod, alt, x, y, z}]}. Looks across every layer for every block \
             reference carrying at least one of MAT/COD/ALT (any block name - CADCRAFT_TCPOINT, a custom \
             name from `topo.import_points`'s own `block` param, a point symbol, ...), or only references \
             named `block` if given, optionally keeping only the ones whose MAT/COD/ALT (or just `field`, \
             if given) contains `query` (case-insensitively); with no `query`, every point. Capped at \
             5000 rows.",
            )
            .noundo(),
    ]
}

/// Maps the import dialog's delimiter choice (also the `delimiter` JSON param) to
/// `cadcraft_topo_io`'s own enum; unrecognised text falls back to `Auto` rather than erroring,
/// since this only ever changes how a file is split, never whether the import succeeds.
fn csv_delimiter(s: Option<&str>) -> csv_points::CsvDelimiter {
    match s.unwrap_or("auto").to_ascii_lowercase().as_str() {
        "comma" | "," => csv_points::CsvDelimiter::Comma,
        "semicolon" | ";" => csv_points::CsvDelimiter::Semicolon,
        "colon" | ":" => csv_points::CsvDelimiter::Colon,
        "space" => csv_points::CsvDelimiter::Space,
        "tab" | "\t" => csv_points::CsvDelimiter::Tab,
        _ => csv_points::CsvDelimiter::Auto,
    }
}

fn run_preview_points_csv(_s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "topo.preview_points_csv";
    let path = str_param(p, "path").ok_or_else(|| bad(CMD, "`path` is required"))?;
    let lines = p.get("lines").and_then(Value::as_u64).map_or(10, |n| n.clamp(1, 100) as usize);
    let text = text_encoding::read_to_string(path).map_err(|e| bad(CMD, format!("{path}: {e}")))?;
    let rows = csv_points::preview(&text, csv_delimiter(str_param(p, "delimiter")), lines);
    Ok(json!({ "rows": rows }))
}

/// Whether a GSI carnet needs a station/VO before it can import - same test `run_import_gsi`
/// itself uses, exposed read-only so the Import GSI dialog can decide which fields to show
/// before the person has entered anything.
fn gsi_needs_station(import: &gsi::GsiImport) -> bool {
    import.points.iter().any(|gp| gp.x.is_none() && gp.hz_gon.is_some() && gp.vz_gon.is_some() && gp.slope_distance.is_some())
}

fn run_gsi_inspect(_s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "topo.gsi_inspect";
    let path = str_param(p, "path").ok_or_else(|| bad(CMD, "`path` is required"))?;
    let text = text_encoding::read_to_string(path).map_err(|e| bad(CMD, format!("{path}: {e}")))?;
    let import = gsi::parse(&text);
    Ok(json!({
        "pointCount": import.points.len(),
        "needsStation": gsi_needs_station(&import),
        "skippedLines": import.skipped_lines,
        // Every point the carnet decoded, for the Import GSI dialog's own points list - the
        // person tags each row Station/Reference/Measure there (right-click), rather than
        // retyping coordinates CADCraft already read from the file.
        "points": import.points.iter().map(|gp| json!({
            "id": gp.id,
            "code": gp.code,
            "hz": gp.hz_gon,
            "vz": gp.vz_gon,
            "dist": gp.slope_distance,
            "x": gp.x, "y": gp.y, "z": gp.z,
        })).collect::<Vec<_>>(),
    }))
}

/// Parses a plain ID/X/Y/Z(/code) points file without touching the drawing - the Import GSI
/// dialog's "known references" file, matched by id against the carnet's own rows so a
/// reference's Hz reading (only the carnet has it) pairs with coordinates from a separate,
/// trusted control-point file rather than requiring both in one place.
fn run_parse_points_csv(_s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "topo.parse_points_csv";
    let path = str_param(p, "path").ok_or_else(|| bad(CMD, "`path` is required"))?;
    let text = text_encoding::read_to_string(path).map_err(|e| bad(CMD, format!("{path}: {e}")))?;
    let import = csv_points::parse_with(&text, csv_delimiter(str_param(p, "delimiter")));
    Ok(json!({
        "points": import.points.iter().map(|cp| json!({
            "id": cp.id, "x": cp.x, "y": cp.y, "z": cp.z, "code": cp.code,
        })).collect::<Vec<_>>(),
        "skippedLines": import.skipped_lines,
    }))
}

/// Rows capped the same way `data.search` caps its own (large drawings, hostile input).
const MAX_SEARCH_ROWS: usize = 5000;

/// Data > Search... (`data.search`) only finds entities carrying a *user-defined* attribute
/// table; TCPOINT's MAT/COD/ALT are plain block ATTRIB values instead, so this is its topo
/// counterpart: every `block` reference (CADCRAFT_TCPOINT by default) on every layer, searched by
/// those three fields and shown with layer + 3D position for a spreadsheet-like review.
fn run_search_points(s: &mut Session, p: &Value) -> Result<Value> {
    // `block` narrows to one block name, matched case-insensitively (handy to split a mixed
    // drawing); left out, every INSERT carrying at least one of MAT/COD/ALT counts as a topo
    // point, whatever its block is actually called - `topo.import_points`/`topo.import_gsi`'s own
    // `block` parameter, a point symbol (`topo.insert_symbol`), or a block authored by hand can
    // all use a name other than CADCRAFT_TCPOINT, and requiring the default name here would show
    // nothing for any of them.
    let block = str_param(p, "block").map(str::trim).filter(|n| !n.is_empty());
    let field = str_param(p, "field").map(str::to_ascii_uppercase);
    let query = str_param(p, "query").unwrap_or_default().trim().to_ascii_lowercase();
    let d = s.doc()?;
    let mut rows = Vec::new();
    // TCPOINT references always live in model space (nothing inserts one into a layout or
    // another block), but a block definition's own entities are scanned too so a point copied
    // into a block still turns up.
    let stores = std::iter::once(&d.model).chain(d.layouts.iter().map(|l| &l.entities)).chain(d.blocks.values().map(|b| &b.entities));
    'entities: for store in stores {
        for e in store.iter() {
            let EntityKind::Insert(ins) = &e.kind else { continue };
            if block.is_some_and(|b| !ins.block.eq_ignore_ascii_case(b)) {
                continue;
            }
            let tag = |t: &str| ins.attribs.iter().find(|a| a.tag.eq_ignore_ascii_case(t)).map(|a| a.text.value.clone()).unwrap_or_default();
            let (mat, cod, alt) = (tag("MAT"), tag("COD"), tag("ALT"));
            if block.is_none() && !ins.attribs.iter().any(|a| matches!(a.tag.to_ascii_uppercase().as_str(), "MAT" | "COD" | "ALT")) {
                continue;
            }
            let matches = query.is_empty()
                || match field.as_deref() {
                    Some("MAT") => mat.to_ascii_lowercase().contains(&query),
                    Some("COD") => cod.to_ascii_lowercase().contains(&query),
                    Some("ALT") => alt.to_ascii_lowercase().contains(&query),
                    _ => [&mat, &cod, &alt].iter().any(|v| v.to_ascii_lowercase().contains(&query)),
                };
            if !matches {
                continue;
            }
            rows.push(json!({
                "handle": e.handle.hex(),
                "layer": e.common.layer,
                "mat": mat, "cod": cod, "alt": alt,
                "x": ins.insert.x, "y": ins.insert.y, "z": ins.insert.z,
            }));
            if rows.len() >= MAX_SEARCH_ROWS {
                break 'entities;
            }
        }
    }
    Ok(json!({ "rows": rows }))
}

pub(crate) fn vec3_req(cmd: &str, p: &Value, key: &str) -> Result<Vec3> {
    let v = p.get(key).ok_or_else(|| bad(cmd, format!("`{key}` ([x, y, z]) is required")))?;
    let a = v.as_array().ok_or_else(|| bad(cmd, format!("`{key}` must be an array [x, y, z]")))?;
    let get = |i: usize| a.get(i).and_then(Value::as_f64).filter(|n| n.is_finite());
    match (get(0), get(1)) {
        (Some(x), Some(y)) => Ok(Vec3::new(x, y, get(2).unwrap_or(0.0))),
        _ => Err(bad(cmd, format!("`{key}` must be [x, y] or [x, y, z] of finite numbers"))),
    }
}

fn references_param(cmd: &str, p: &Value, key: &str) -> Result<Vec<ReferenceObservation>> {
    let arr = p.get(key).and_then(Value::as_array).ok_or_else(|| bad(cmd, format!("`{key}` (array) is required")))?;
    arr.iter()
        .map(|r| {
            let reference = vec3_req(cmd, r, "point")?;
            let hz_reading_gon = f64_req(cmd, r, "hzReading")?;
            Ok(ReferenceObservation { reference, hz_reading_gon })
        })
        .collect()
}

fn topo_err(cmd: &str, e: cadcraft_topo_calc::TopoCalcError) -> crate::EngineError {
    bad(cmd, e.to_string())
}

/// Resolves the station's orientation: an explicit `vo` wins; otherwise it is computed from
/// `references`, whose (circular) standard deviation is then reported alongside it.
fn resolve_vo(cmd: &str, station: Vec3, p: &Value) -> Result<(f64, Option<f64>)> {
    match p.get("vo").and_then(Value::as_f64) {
        Some(vo) => Ok((vo, None)),
        None => {
            let refs = references_param(cmd, p, "references")?;
            if refs.is_empty() {
                return Err(bad(cmd, "either `vo` or a non-empty `references` is required"));
            }
            let r = zero_value(station, &refs).map_err(|e| topo_err(cmd, e))?;
            Ok((r.vo_gon, Some(r.sigma_gon)))
        }
    }
}

/// Inserts one computed point as a POINT entity, with an optional TEXT label carrying its code,
/// and returns the JSON summary used in every command's `points` array.
pub(crate) fn insert_point(s: &mut Session, coord: Vec3, code: Option<String>, label: bool, text_height: f64) -> Result<Value> {
    let handle = s.add_entity(EntityKind::Point(Point { p: coord, angle: 0.0 }))?;
    let label_handle = match (&code, label) {
        (Some(c), true) if !c.is_empty() => {
            let xy = Vec2::new(coord.x, coord.y);
            let k = make_text(s, xy, c, text_height, 0.0, (HAlign::Left, VAlign::Baseline));
            Some(s.add_entity(k)?.hex())
        }
        _ => None,
    };
    Ok(json!({
        "handle": handle.hex(),
        "x": coord.x, "y": coord.y, "z": coord.z,
        "code": code,
        "labelHandle": label_handle,
    }))
}

/// Block name for imported topo points, prefixed to avoid colliding with a user's own block of
/// the same short name (matches the convention in `topo_symbols.rs`).
pub(crate) const TCPOINT_BLOCK: &str = "CADCRAFT_TCPOINT";

/// Pulls a result's `"handle"` (single) or `"handles"` (array) field into `out` - same helper as
/// `topo_symbols.rs`'s `collect_handles`, duplicated locally to avoid a cross-module dependency
/// for one tiny function.
fn collect_handles(out: &mut Vec<Value>, result: &Value) {
    match result.get("handles").and_then(Value::as_array) {
        Some(hs) => out.extend(hs.iter().cloned()),
        None => {
            if let Some(h) = result.get("handle") {
                out.push(h.clone());
            }
        }
    }
}

/// The dedicated layers COD/MAT/ALT are defined on (never layer `0`, so each stays on its own
/// layer rather than floating to whatever layer a given point block is inserted on - see the
/// module doc comment).
const TOPOCOD_LAYER: &str = "TOPOCOD";
const TOPOMAT_LAYER: &str = "TOPOMAT";
const TOPOALT_LAYER: &str = "TOPOALT";

/// Builds the point block named `block` the first time it's needed (a no-op if a block by that
/// name already exists - letting a person reuse one of their own instead of always getting
/// `CADCRAFT_TCPOINT`): a small cross inside a circle on layer `0` (so it floats to whichever
/// layer the reference itself is inserted on), plus COD (top-left), MAT (top-right) and ALT
/// (bottom-right) attribute definitions, each on its own layer (created if missing) and each
/// visible except ALT, whose value is also shown as the point's elevation via COD/MAT - kept
/// anyway so it survives as real, editable entity data.
/// Public alias of [`ensure_tcpoint_block`] for callers outside this module (e.g.
/// `points_to_tcpoint.rs`) that just need the block definition to exist, without inserting a
/// point themselves.
pub(crate) fn insert_tcpoint_block_only(s: &mut Session, block: &str) -> Result<()> {
    ensure_tcpoint_block(s, block)
}

fn ensure_tcpoint_block(s: &mut Session, block: &str) -> Result<()> {
    if s.doc()?.blocks.keys().any(|k| k.eq_ignore_ascii_case(block)) {
        return Ok(());
    }
    // ACI 1 (red)/3 (green)/4 (cyan) - the same standard AutoCAD colour index a person would
    // pick by eye, so COD/ALT/MAT are visually distinct the instant the layers exist, before
    // anyone thinks to colour them by hand.
    for (layer, aci) in [(TOPOCOD_LAYER, 1u8), (TOPOALT_LAYER, 3), (TOPOMAT_LAYER, 4)] {
        s.doc_mut()?.ensure_layer(layer);
        if let Some(l) = s.doc_mut()?.layer_mut(layer) {
            l.color = Color::Index(aci);
        }
    }
    let mut handles: Vec<Value> = Vec::new();
    let mut draw = |s: &mut Session, cmd: &str, params: Value, layer: Option<&str>| -> Result<()> {
        let r = s.execute(cmd, &params)?;
        collect_handles(&mut handles, &r);
        if let Some(layer) = layer
            && let Some(h) = r.get("handle").and_then(Value::as_str).and_then(Handle::parse_hex)
        {
            let layer = layer.to_string();
            s.doc_mut()?.modify_entity(h, move |e| e.common.layer = layer)?;
        }
        Ok(())
    };
    draw(s, "circle", json!({ "center": [0.0, 0.0], "radius": 0.2 }), None)?;
    draw(s, "line", json!({ "points": [[-0.3, 0.0], [0.3, 0.0]] }), None)?;
    draw(s, "line", json!({ "points": [[0.0, -0.3], [0.0, 0.3]] }), None)?;
    draw(s, "attdef", json!({ "tag": "MAT", "prompt": "Matricule", "default": "", "at": [-0.4, 0.3], "height": 0.15 }), Some(TOPOMAT_LAYER))?;
    // Right-justified: the anchor at `at` stays put, and the text body is laid out to its left,
    // so COD reads toward the point instead of away from it like MAT/ALT.
    draw(
        s,
        "attdef",
        json!({ "tag": "COD", "prompt": "Code", "default": "", "at": [0.4, 0.3], "height": 0.15, "justify": "R" }),
        Some(TOPOCOD_LAYER),
    )?;
    draw(s, "attdef", json!({ "tag": "ALT", "prompt": "Altitude", "default": "", "at": [0.4, -0.3], "height": 0.15 }), Some(TOPOALT_LAYER))?;
    s.execute("block", &json!({ "name": block, "base": [0.0, 0.0], "handles": handles, "keep": "delete" }))?;
    Ok(())
}

/// Inserts one imported topo point as a CADCRAFT_TCPOINT block reference carrying its matricule
/// (`mat` - the point's own id/number from the source file), altitude (`coord.z`) and code as
/// MAT/ALT/COD attributes, so every field survives as real entity data - visible and editable in
/// Properties - instead of the plain POINT + loose text label [`insert_point`] uses. Mirrors
/// `insert_point`'s JSON result shape so callers' `points` arrays don't have to change; `mat` is
/// added alongside the existing fields.
pub(crate) fn insert_tcpoint(
    s: &mut Session,
    coord: Vec3,
    mat: Option<String>,
    code: Option<String>,
    label: bool,
    block: &str,
    layer: Option<&str>,
) -> Result<Value> {
    ensure_tcpoint_block(s, block)?;
    if let Some(layer) = layer {
        s.doc_mut()?.ensure_layer(layer);
    }

    let mut attribs = serde_json::Map::new();
    if let Some(c) = &code {
        attribs.insert("COD".into(), Value::String(c.clone()));
    }
    if let Some(m) = &mat {
        attribs.insert("MAT".into(), Value::String(m.clone()));
    }
    attribs.insert("ALT".into(), Value::String(format!("{:.3}", coord.z)));

    let out = s.execute("insert", &json!({ "name": block, "at": [coord.x, coord.y], "attribs": Value::Object(attribs) }))?;
    let handle = out.get("handle").and_then(Value::as_str).map(str::to_string);
    let hd = handle.as_deref().and_then(Handle::parse_hex);

    // `insert` always uses the current layer (CLAYER), so the reference's own layer - which the
    // point symbol (on layer 0) then floats to - is set here instead of asking `insert` for it.
    if let (Some(layer), Some(hd)) = (layer, hd) {
        let layer = layer.to_string();
        s.doc_mut()?.modify_entity(hd, move |e| e.common.layer = layer)?;
    }

    // COD is visible by default (matching the old label's behaviour); `label: false` hides it,
    // but MAT/ALT stay in the entity's data either way - this only affects what's drawn.
    if !label && let Some(hd) = hd {
        let _ = s.doc_mut()?.modify_entity(hd, |e| {
            if let EntityKind::Insert(ins) = &mut e.kind
                && let Some(a) = ins.attribs.iter_mut().find(|a| a.tag == "COD")
            {
                a.invisible = true;
            }
        });
    }

    Ok(json!({
        "handle": handle,
        "x": coord.x, "y": coord.y, "z": coord.z,
        "code": code,
        "mat": mat,
        "labelHandle": Value::Null,
    }))
}

fn run_zero_value(_s: &mut Session, p: &Value) -> Result<Value> {
    let station = vec3_req("topo.zero_value", p, "station")?;
    let refs = references_param("topo.zero_value", p, "references")?;
    let result = zero_value(station, &refs).map_err(|e| topo_err("topo.zero_value", e))?;
    Ok(json!({ "vo": result.vo_gon, "sigma": result.sigma_gon, "count": result.count }))
}

fn run_radiation(s: &mut Session, p: &Value) -> Result<Value> {
    let station = vec3_req("topo.radiation", p, "station")?;
    let (vo_gon, sigma) = resolve_vo("topo.radiation", station, p)?;

    let obs_arr = p.get("observations").and_then(Value::as_array).ok_or_else(|| bad("topo.radiation", "`observations` (array) is required"))?;
    if obs_arr.is_empty() {
        return Err(bad("topo.radiation", "`observations` must not be empty"));
    }
    let label = bool_or(p, "label", true);
    let text_height = f64_or(p, "textHeight", s.doc()?.header.f64("TEXTSIZE", 0.2));

    let mut points = Vec::with_capacity(obs_arr.len());
    for o in obs_arr {
        let obs = RadiationObservation {
            hz_reading_gon: f64_req("topo.radiation", o, "hzReading")?,
            zenith_gon: f64_req("topo.radiation", o, "zenith")?,
            slope_distance: f64_req("topo.radiation", o, "slopeDistance")?,
            instrument_height: f64_or(o, "instrumentHeight", 0.0),
            target_height: f64_or(o, "targetHeight", 0.0),
        };
        let coord = radiation(station, vo_gon, obs).map_err(|e| topo_err("topo.radiation", e))?;
        let code = str_param(o, "code").map(str::to_string);
        points.push(insert_point(s, coord, code, label, text_height)?);
    }

    Ok(json!({ "vo": vo_gon, "sigma": sigma, "points": points }))
}

/// Resections an unknown station from >=2 known reference points (distance + Hz reading to
/// each), independent of the instrument's own orientation - see `cadcraft_topo_calc::free_station`.
/// The station itself is inserted as a TCPOINT by default (`insertStation: false` to only report
/// the computed coordinates/VO without touching the drawing).
fn run_free_station(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "topo.free_station";
    let arr = p.get("references").and_then(Value::as_array).ok_or_else(|| bad(CMD, "`references` (array) is required"))?;
    if arr.len() < 2 {
        return Err(bad(CMD, "`references` needs at least 2 points"));
    }
    let obs: Vec<FreeStationObservation> = arr
        .iter()
        .map(|r| {
            Ok(FreeStationObservation {
                reference: vec3_req(CMD, r, "point")?,
                horizontal_distance: f64_req(CMD, r, "distance")?,
                hz_reading_gon: f64_req(CMD, r, "hzReading")?,
            })
        })
        .collect::<Result<_>>()?;
    let approx = p.get("approx").map(|_| vec3_req(CMD, p, "approx")).transpose()?;

    let result = free_station(&obs, approx).map_err(|e| topo_err(CMD, e))?;
    let label = bool_or(p, "label", true);
    let code = str_param(p, "code").map(str::to_string);
    let insert_station = bool_or(p, "insertStation", true);
    let point = if insert_station { Some(insert_tcpoint(s, result.station, None, code, label, TCPOINT_BLOCK, None)?) } else { None };

    Ok(json!({
        "station": [result.station.x, result.station.y, result.station.z],
        "vo": result.vo.vo_gon,
        "sigma": result.vo.sigma_gon,
        "iterations": result.iterations,
        "point": point,
    }))
}

/// Runs a traverse ("cheminement") from `start`, chaining each leg's gisement/distance and, when
/// a known `closingPoint` is given, adjusting every intermediate station by the compass rule -
/// see `cadcraft_topo_calc::traverse`. Every computed station but `start` (already a point in the
/// drawing) is inserted as a TCPOINT, in order.
fn run_cheminement(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "topo.cheminement";
    let start = vec3_req(CMD, p, "start")?;
    let legs_arr = p.get("legs").and_then(Value::as_array).ok_or_else(|| bad(CMD, "`legs` (array) is required"))?;
    if legs_arr.is_empty() {
        return Err(bad(CMD, "`legs` must not be empty"));
    }
    let legs: Vec<TraverseLeg> = legs_arr
        .iter()
        .map(|l| Ok(TraverseLeg { gisement_gon: f64_req(CMD, l, "gisement")?, distance: f64_req(CMD, l, "distance")? }))
        .collect::<Result<_>>()?;
    let closing_point = p.get("closingPoint").map(|_| vec3_req(CMD, p, "closingPoint")).transpose()?;

    let result = traverse(start, &legs, closing_point).map_err(|e| topo_err(CMD, e))?;
    let label = bool_or(p, "label", true);

    let mut points = Vec::with_capacity(legs.len());
    for (i, leg) in legs_arr.iter().enumerate() {
        let coord = result.points[i + 1];
        let code = str_param(leg, "code").map(str::to_string);
        points.push(insert_tcpoint(s, coord, None, code, label, TCPOINT_BLOCK, None)?);
    }

    Ok(json!({
        "points": points,
        "closingError": result.closing_error.map(|(ex, ey)| json!([ex, ey])),
    }))
}

/// Previews what `topo.radiation` would compute for each observation - same station/VO
/// resolution, same per-observation `radiation()` call - but returns bare coordinates instead of
/// inserting anything, for a dialog that wants to show the result before committing to it.
fn run_compute_radiation(_s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "topo.compute_radiation";
    let station = vec3_req(CMD, p, "station")?;
    let (vo_gon, sigma) = resolve_vo(CMD, station, p)?;

    let obs_arr = p.get("observations").and_then(Value::as_array).ok_or_else(|| bad(CMD, "`observations` (array) is required"))?;
    let mut points = Vec::with_capacity(obs_arr.len());
    for o in obs_arr {
        let obs = RadiationObservation {
            hz_reading_gon: f64_req(CMD, o, "hzReading")?,
            zenith_gon: f64_req(CMD, o, "zenith")?,
            slope_distance: f64_req(CMD, o, "slopeDistance")?,
            instrument_height: f64_or(o, "instrumentHeight", 0.0),
            target_height: f64_or(o, "targetHeight", 0.0),
        };
        let coord = radiation(station, vo_gon, obs).map_err(|e| topo_err(CMD, e))?;
        points.push(json!({ "x": coord.x, "y": coord.y, "z": coord.z, "code": str_param(o, "code") }));
    }

    Ok(json!({ "vo": vo_gon, "sigma": sigma, "points": points }))
}

/// Imports a Leica GSI-8/16 carnet from disk. Two kinds of line insert directly, no station
/// needed: a line already carrying X/Y/Z (WI 81/82/83 or the GeoMax station words 84/85/86) goes
/// straight in as a coordinate; a line with a full radiation shot (Hz, Vz and slope distance all
/// present) is turned into a coordinate via `station`/`vo` - required only when at least one
/// such line exists, so a pure coordinate carnet (the common case for building a point geobase)
/// never has to supply a station it doesn't have. A line with neither (say, a references-only
/// angle with no distance) is counted in `skippedPoints` rather than silently dropped.
fn run_import_gsi(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_param(p, "path").ok_or_else(|| bad("topo.import_gsi", "`path` is required"))?;
    let label = bool_or(p, "label", true);

    let text = text_encoding::read_to_string(path).map_err(|e| bad("topo.import_gsi", format!("{path}: {e}")))?;
    let import = gsi::parse(&text);

    let needs_radiation = import.points.iter().any(|gp| gp.x.is_none() && gp.hz_gon.is_some() && gp.vz_gon.is_some() && gp.slope_distance.is_some());
    let station_vo = if needs_radiation {
        let station = vec3_req("topo.import_gsi", p, "station")?;
        let (vo_gon, sigma) = resolve_vo("topo.import_gsi", station, p)?;
        Some((station, vo_gon, sigma))
    } else {
        None
    };

    let mut points = Vec::new();
    let mut skipped_points = 0usize;
    for gp in &import.points {
        let code = gp.code.clone().or_else(|| Some(gp.id.clone()));
        let coord = match (gp.x, gp.y) {
            (Some(x), Some(y)) => Vec3::new(x, y, gp.z.unwrap_or(0.0)),
            _ => {
                let (Some((station, vo_gon, _)), Some(hz), Some(vz), Some(dist)) = (station_vo, gp.hz_gon, gp.vz_gon, gp.slope_distance) else {
                    skipped_points += 1;
                    continue;
                };
                let obs = RadiationObservation {
                    hz_reading_gon: hz,
                    zenith_gon: vz,
                    slope_distance: dist,
                    instrument_height: gp.instrument_height.unwrap_or(0.0),
                    target_height: gp.target_height.unwrap_or(0.0),
                };
                radiation(station, vo_gon, obs).map_err(|e| topo_err("topo.import_gsi", e))?
            }
        };
        points.push(insert_tcpoint(s, coord, Some(gp.id.clone()), code, label, TCPOINT_BLOCK, None)?);
    }

    Ok(json!({
        "vo": station_vo.map(|(_, vo, _)| vo),
        "sigma": station_vo.and_then(|(_, _, sigma)| sigma),
        "importedPoints": points.len(),
        "skippedPoints": skipped_points,
        "skippedLines": import.skipped_lines,
        "points": points,
    }))
}

/// Imports a plain CSV/TXT point list (already-computed coordinates: `ID, X, Y, Z[, code]`, with
/// or without a header, `;`/`,`/tab/space delimited) and inserts each point as-is into the
/// active document - no station or polar calculation involved, unlike [`run_import_gsi`].
fn run_import_points(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "topo.import_points";
    let path = str_param(p, "path").ok_or_else(|| bad(CMD, "`path` is required"))?;
    let label = bool_or(p, "label", true);
    let block = str_param(p, "block").map(str::trim).filter(|n| !n.is_empty()).unwrap_or(TCPOINT_BLOCK).to_string();
    let layer = str_param(p, "layer").map(str::trim).filter(|n| !n.is_empty() && *n != "0").map(str::to_string);
    // The block reference always carries its Z in the ALT attribute; `threeD` additionally puts
    // it in the reference's own insertion point, so the point sits at its real height in space
    // rather than flat on the XY plane. Default true: a CSV with a Z column is usually meant as
    // a real 3D point.
    let three_d = bool_or(p, "threeD", true);

    let text = text_encoding::read_to_string(path).map_err(|e| bad(CMD, format!("{path}: {e}")))?;
    let import = csv_points::parse_with(&text, csv_delimiter(str_param(p, "delimiter")));

    let mut points = Vec::with_capacity(import.points.len());
    for cp in &import.points {
        let coord = Vec3::new(cp.x, cp.y, cp.z.unwrap_or(0.0));
        let code = cp.code.clone().or_else(|| Some(cp.id.clone()));
        let out = insert_tcpoint(s, coord, Some(cp.id.clone()), code, label, &block, layer.as_deref())?;
        if three_d && coord.z != 0.0 {
            let h = out.get("handle").and_then(Value::as_str).and_then(Handle::parse_hex);
            if let Some(h) = h {
                let z = coord.z;
                s.doc_mut()?.modify_entity(h, move |e| {
                    if let EntityKind::Insert(ins) = &mut e.kind {
                        ins.insert.z = z;
                    }
                })?;
            }
        }
        points.push(out);
    }

    Ok(json!({
        "importedPoints": points.len(),
        "skippedLines": import.skipped_lines,
        "layer": layer,
        "block": block,
        "threeD": three_d,
        "points": points,
    }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::Session;

    fn session_with_doc() -> Session {
        let mut s = Session::new();
        s.execute("new", &json!({})).expect("new drawing should succeed");
        s
    }

    #[test]
    fn radiation_with_explicit_vo_inserts_points() {
        let mut s = session_with_doc();
        let out = s
            .execute(
                "topo.radiation",
                &json!({
                    "station": [1000.0, 1000.0, 100.0],
                    "vo": 0.0,
                    "observations": [
                        { "hzReading": 100.0, "zenith": 100.0, "slopeDistance": 50.0, "instrumentHeight": 1.5, "targetHeight": 1.5, "code": "P1" }
                    ]
                }),
            )
            .expect("radiation should succeed");
        let pts = out["points"].as_array().expect("points array");
        assert_eq!(pts.len(), 1);
        assert!((pts[0]["x"].as_f64().unwrap_or(f64::NAN) - 1050.0).abs() < 1e-6);
        assert!((pts[0]["y"].as_f64().unwrap_or(f64::NAN) - 1000.0).abs() < 1e-6);
        assert!(pts[0]["labelHandle"].is_string(), "a coded point should get a label");
    }

    #[test]
    fn compute_radiation_matches_radiation_but_inserts_nothing() {
        let mut s = session_with_doc();
        let before = s.doc().expect("doc").model.iter().count();

        let out = s
            .execute(
                "topo.compute_radiation",
                &json!({
                    "station": [1000.0, 1000.0, 100.0],
                    "vo": 0.0,
                    "observations": [{ "hzReading": 100.0, "zenith": 100.0, "slopeDistance": 50.0, "code": "P1" }]
                }),
            )
            .expect("compute_radiation should succeed");
        let pts = out["points"].as_array().expect("points array");
        assert_eq!(pts.len(), 1);
        assert!((pts[0]["x"].as_f64().unwrap_or(f64::NAN) - 1050.0).abs() < 1e-6);
        assert!((pts[0]["y"].as_f64().unwrap_or(f64::NAN) - 1000.0).abs() < 1e-6);
        assert_eq!(pts[0]["code"].as_str(), Some("P1"));

        let after = s.doc().expect("doc").model.iter().count();
        assert_eq!(before, after, "a preview must not add anything to the drawing");
    }

    #[test]
    fn radiation_orients_from_references_when_vo_omitted() {
        let mut s = session_with_doc();
        let out = s
            .execute(
                "topo.radiation",
                &json!({
                    "station": [500.0, 500.0, 50.0],
                    "references": [
                        { "point": [500.0, 600.0, 50.0], "hzReading": 10.0 }
                    ],
                    "observations": [
                        { "hzReading": 110.0, "zenith": 100.0, "slopeDistance": 20.0, "instrumentHeight": 1.6, "targetHeight": 1.6 }
                    ]
                }),
            )
            .expect("radiation should succeed");
        assert!((out["vo"].as_f64().unwrap_or(f64::NAN) - 390.0).abs() < 1e-6);
        let pts = out["points"].as_array().expect("points array");
        assert!((pts[0]["x"].as_f64().unwrap_or(f64::NAN) - 520.0).abs() < 1e-6);
    }

    #[test]
    fn radiation_requires_vo_or_references() {
        let mut s = session_with_doc();
        let err = s.execute(
            "topo.radiation",
            &json!({
                "station": [0.0, 0.0, 0.0],
                "observations": [{ "hzReading": 0.0, "zenith": 100.0, "slopeDistance": 5.0 }]
            }),
        );
        assert!(err.is_err(), "missing vo and references should be refused");
    }

    #[test]
    fn import_gsi_reads_a_real_carnet_and_inserts_points() {
        let mut s = session_with_doc();
        // One ready-made coordinate line (inserted directly, no station needed) and one real
        // radiation shot (Hz 100 gon = due east, Vz 100 gon = horizontal, slope dist 50 m), P1.
        let gsi = "110001+0000STA1 81..00+00100000000 82..00+00100000000 83..00+00000010000\n\
                   110001+0000P001 21.324+10000000 22.324+10000000 31..00+00500000 41..00+0000P1\n";
        let path = std::env::temp_dir().join(format!("cadcraft-topo-test-{}.gsi", std::process::id()));
        std::fs::write(&path, gsi).expect("writing the temp carnet should succeed");

        let out = s.execute(
            "topo.import_gsi",
            &json!({
                "path": path.to_string_lossy(),
                "station": [1000.0, 1000.0, 100.0],
                "vo": 0.0
            }),
        );
        let _ = std::fs::remove_file(&path);
        let out = out.expect("import should succeed");

        assert_eq!(out["importedPoints"].as_u64(), Some(2));
        assert_eq!(out["skippedPoints"].as_u64(), Some(0));
        let pts = out["points"].as_array().expect("points array");
        // STA1's own coordinate (digits 00100000000 / 1000 = 100000.0), untouched by station/vo.
        assert!((pts[0]["x"].as_f64().unwrap_or(f64::NAN) - 100000.0).abs() < 1e-6, "{pts:?}");
        assert!((pts[1]["x"].as_f64().unwrap_or(f64::NAN) - 1050.0).abs() < 1e-6);
        assert!((pts[1]["y"].as_f64().unwrap_or(f64::NAN) - 1000.0).abs() < 1e-6);
        assert_eq!(pts[1]["code"].as_str(), Some("P1"));
    }

    #[test]
    fn import_gsi_coordinate_only_carnet_needs_no_station() {
        let mut s = session_with_doc();
        // X=200.0 → digits 00000200000 (÷1000), same encoding `cadcraft_topo_io::gsi` tests use.
        let gsi = "110001+0000P001 81..00+00000200000 82..00+00000300000 83..00+00000050000 41..00+0000A\n\
                   110001+0000P002 81..00+00000210000 82..00+00000310000 83..00+00000051000 41..00+0000B\n";
        let path = std::env::temp_dir().join(format!("cadcraft-topo-coords-test-{}.gsi", std::process::id()));
        std::fs::write(&path, gsi).expect("writing the temp carnet should succeed");

        // No `station`/`vo` at all: a pure coordinate carnet must not need them.
        let out = s.execute("topo.import_gsi", &json!({ "path": path.to_string_lossy() }));
        let _ = std::fs::remove_file(&path);
        let out = out.expect("a coordinate-only carnet should import without a station");

        assert_eq!(out["importedPoints"].as_u64(), Some(2));
        assert_eq!(out["skippedPoints"].as_u64(), Some(0));
        assert!(out["vo"].is_null());
        let pts = out["points"].as_array().expect("points array");
        assert!((pts[0]["x"].as_f64().unwrap_or(f64::NAN) - 200.0).abs() < 1e-6, "{pts:?}");
        assert!((pts[0]["y"].as_f64().unwrap_or(f64::NAN) - 300.0).abs() < 1e-6);
        assert_eq!(pts[0]["code"].as_str(), Some("A"));
    }

    #[test]
    fn import_gsi_without_station_still_requires_it_for_angle_shots() {
        let mut s = session_with_doc();
        let gsi = "110001+0000P001 21.324+10000000 22.324+10000000 31..00+00500000 41..00+0000P1\n";
        let path = std::env::temp_dir().join(format!("cadcraft-topo-noangle-test-{}.gsi", std::process::id()));
        std::fs::write(&path, gsi).expect("writing the temp carnet should succeed");

        let out = s.execute("topo.import_gsi", &json!({ "path": path.to_string_lossy() }));
        let _ = std::fs::remove_file(&path);
        assert!(out.is_err(), "an angle+distance shot with no station/vo must be refused, not silently skipped");
    }

    #[test]
    fn import_gsi_reports_a_missing_file_as_an_error_not_a_panic() {
        let mut s = session_with_doc();
        let out = s.execute(
            "topo.import_gsi",
            &json!({
                "path": "/nonexistent/path/does-not-exist.gsi",
                "station": [0.0, 0.0, 0.0],
                "vo": 0.0
            }),
        );
        assert!(out.is_err(), "a missing file should be refused, not panic");
    }

    #[test]
    fn import_points_reads_a_csv_file_and_inserts_points() {
        let mut s = session_with_doc();
        let path = std::env::temp_dir().join(format!("cadcraft-topo-points-test-{}.csv", std::process::id()));
        std::fs::write(&path, "ID;X;Y;Z\nP1;100.0;200.0;10.0\nP2;not-a-number;200.0;10.0\nP3;110.0;210.0;11.0\n")
            .expect("writing the temp points file should succeed");

        let out = s.execute("topo.import_points", &json!({ "path": path.to_string_lossy() }));
        let _ = std::fs::remove_file(&path);
        let out = out.expect("import should succeed");

        assert_eq!(out["importedPoints"].as_u64(), Some(2));
        assert_eq!(out["skippedLines"].as_u64(), Some(1));
        let pts = out["points"].as_array().expect("points array");
        assert_eq!(pts[0]["code"].as_str(), Some("P1"));
        assert!((pts[1]["x"].as_f64().unwrap_or(f64::NAN) - 110.0).abs() < 1e-6);
    }

    #[test]
    fn import_points_inserts_a_tcpoint_block_with_mat_alt_cod_attributes() {
        let mut s = session_with_doc();
        let path = std::env::temp_dir().join(format!("cadcraft-topo-tcpoint-test-{}.csv", std::process::id()));
        std::fs::write(&path, "ID;X;Y;Z;CODE\nP1;100.0;200.0;12.5;BORNE\n").expect("writing the temp points file should succeed");

        let out = s.execute("topo.import_points", &json!({ "path": path.to_string_lossy() }));
        let _ = std::fs::remove_file(&path);
        let out = out.expect("import should succeed");
        let pts = out["points"].as_array().expect("points array");
        assert_eq!(pts[0]["mat"].as_str(), Some("P1"));

        assert!(s.doc().expect("doc").blocks.keys().any(|k| k.eq_ignore_ascii_case(super::TCPOINT_BLOCK)), "the TCPOINT block should now exist");
        let handle = super::Handle::parse_hex(pts[0]["handle"].as_str().expect("handle")).expect("valid handle");
        let doc = s.doc().expect("doc");
        let e = doc.entity(handle).expect("entity");
        let super::EntityKind::Insert(ins) = &e.kind else { panic!("expected an INSERT entity, got {:?}", e.kind) };
        assert_eq!(ins.block, super::TCPOINT_BLOCK);
        let tag = |t: &str| ins.attribs.iter().find(|a| a.tag == t).map(|a| a.text.value.clone());
        assert_eq!(tag("MAT"), Some("P1".to_string()));
        assert_eq!(tag("COD"), Some("BORNE".to_string()));
        assert_eq!(tag("ALT"), Some("12.500".to_string()));
    }

    #[test]
    fn search_points_finds_tcpoints_across_layers_filtered_by_any_field() {
        let mut s = session_with_doc();
        s.execute("layer.new", &json!({ "name": "BORNES" })).unwrap();
        s.execute("layer.current", &json!({ "name": "BORNES" })).unwrap();
        super::insert_tcpoint(
            &mut s,
            cadcraft_geom::Vec3::new(100.0, 200.0, 12.5),
            Some("P1".into()),
            Some("BORNE".into()),
            true,
            super::TCPOINT_BLOCK,
            None,
        )
        .unwrap();
        s.execute("layer.current", &json!({ "name": "0" })).unwrap();
        super::insert_tcpoint(
            &mut s,
            cadcraft_geom::Vec3::new(50.0, 60.0, 3.0),
            Some("P2".into()),
            Some("REGARD".into()),
            true,
            super::TCPOINT_BLOCK,
            None,
        )
        .unwrap();
        // Not a TCPOINT - must never show up.
        s.execute("circle", &json!({ "center": [0, 0], "radius": 1 })).unwrap();

        let all = s.execute("topo.search_points", &json!({})).unwrap();
        let rows = all["rows"].as_array().expect("rows");
        assert_eq!(rows.len(), 2, "{rows:?}");

        let by_cod = s.execute("topo.search_points", &json!({ "query": "borne" })).unwrap();
        let rows = by_cod["rows"].as_array().expect("rows");
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0]["mat"].as_str(), Some("P1"));
        assert_eq!(rows[0]["layer"].as_str(), Some("BORNES"));
        assert!((rows[0]["x"].as_f64().unwrap_or(f64::NAN) - 100.0).abs() < 1e-6);
        assert!((rows[0]["z"].as_f64().unwrap_or(f64::NAN) - 0.0).abs() < 1e-6, "z is the geometric insertion point, not ALT: {rows:?}");
        assert_eq!(rows[0]["alt"].as_str(), Some("12.500"));

        // A `field` narrows which column `query` is matched against.
        let by_mat_only = s.execute("topo.search_points", &json!({ "query": "borne", "field": "MAT" })).unwrap();
        assert_eq!(by_mat_only["rows"].as_array().expect("rows").len(), 0, "BORNE is the code, not the matricule");
    }

    #[test]
    fn import_points_with_label_false_hides_the_code_but_keeps_the_attributes() {
        let mut s = session_with_doc();
        let path = std::env::temp_dir().join(format!("cadcraft-topo-tcpoint-nolabel-test-{}.csv", std::process::id()));
        std::fs::write(&path, "ID;X;Y;Z;CODE\nP1;0.0;0.0;0.0;BORNE\n").expect("writing the temp points file should succeed");

        let out = s.execute("topo.import_points", &json!({ "path": path.to_string_lossy(), "label": false }));
        let _ = std::fs::remove_file(&path);
        let out = out.expect("import should succeed");
        let pts = out["points"].as_array().expect("points array");

        let handle = super::Handle::parse_hex(pts[0]["handle"].as_str().expect("handle")).expect("valid handle");
        let doc = s.doc().expect("doc");
        let e = doc.entity(handle).expect("entity");
        let super::EntityKind::Insert(ins) = &e.kind else { panic!("expected an INSERT entity, got {:?}", e.kind) };
        let cod = ins.attribs.iter().find(|a| a.tag == "COD").expect("COD attribute should still be attached");
        assert!(cod.invisible, "label: false should hide the code text");
        assert_eq!(cod.text.value, "BORNE", "but the value itself should be kept");
    }

    #[test]
    fn import_points_uses_a_chosen_layer_and_block_and_creates_both() {
        let mut s = session_with_doc();
        let path = std::env::temp_dir().join(format!("cadcraft-topo-tcpoint-layer-{}.csv", std::process::id()));
        std::fs::write(&path, "ID;X;Y;Z;CODE\nP1;0.0;0.0;10.0;BORNE\n").expect("writing the temp points file should succeed");

        let out = s.execute("topo.import_points", &json!({ "path": path.to_string_lossy(), "layer": "LEVE1", "block": "MYPOINT" }));
        let _ = std::fs::remove_file(&path);
        let out = out.expect("import should succeed");
        let pts = out["points"].as_array().expect("points array");

        let doc = s.doc().expect("doc");
        assert!(doc.layer("LEVE1").is_some(), "the chosen layer should have been created");
        assert!(doc.blocks.keys().any(|k| k.eq_ignore_ascii_case("MYPOINT")), "the chosen block should have been created");
        let handle = super::Handle::parse_hex(pts[0]["handle"].as_str().expect("handle")).expect("valid handle");
        let e = doc.entity(handle).expect("entity");
        assert_eq!(e.common.layer, "LEVE1");
        let super::EntityKind::Insert(ins) = &e.kind else { panic!("expected an INSERT entity") };
        assert_eq!(ins.block, "MYPOINT");
    }

    #[test]
    fn import_points_defaults_to_putting_z_in_the_insertion_point_too() {
        let mut s = session_with_doc();
        let path = std::env::temp_dir().join(format!("cadcraft-topo-tcpoint-3d-{}.csv", std::process::id()));
        std::fs::write(&path, "ID;X;Y;Z;CODE\nP1;0.0;0.0;12.5;BORNE\n").expect("writing the temp points file should succeed");

        let out = s.execute("topo.import_points", &json!({ "path": path.to_string_lossy() }));
        let _ = std::fs::remove_file(&path);
        let out = out.expect("import should succeed");
        assert_eq!(out["threeD"].as_bool(), Some(true));
        let pts = out["points"].as_array().expect("points array");
        let handle = super::Handle::parse_hex(pts[0]["handle"].as_str().expect("handle")).expect("valid handle");
        let doc = s.doc().expect("doc");
        let e = doc.entity(handle).expect("entity");
        let super::EntityKind::Insert(ins) = &e.kind else { panic!("expected an INSERT entity") };
        assert!((ins.insert.z - 12.5).abs() < 1e-6, "3D import should carry Z into the reference's own position");
        let alt = ins.attribs.iter().find(|a| a.tag == "ALT").expect("ALT attribute");
        assert_eq!(alt.text.value, "12.500");
    }

    #[test]
    fn import_points_with_three_d_false_keeps_every_point_flat() {
        let mut s = session_with_doc();
        let path = std::env::temp_dir().join(format!("cadcraft-topo-tcpoint-2d-{}.csv", std::process::id()));
        std::fs::write(&path, "ID;X;Y;Z;CODE\nP1;0.0;0.0;12.5;BORNE\n").expect("writing the temp points file should succeed");

        let out = s.execute("topo.import_points", &json!({ "path": path.to_string_lossy(), "threeD": false }));
        let _ = std::fs::remove_file(&path);
        let out = out.expect("import should succeed");
        assert_eq!(out["threeD"].as_bool(), Some(false));
        let pts = out["points"].as_array().expect("points array");
        let handle = super::Handle::parse_hex(pts[0]["handle"].as_str().expect("handle")).expect("valid handle");
        let doc = s.doc().expect("doc");
        let e = doc.entity(handle).expect("entity");
        let super::EntityKind::Insert(ins) = &e.kind else { panic!("expected an INSERT entity") };
        assert_eq!(ins.insert.z, 0.0, "2D import should keep the reference flat on the XY plane");
        let alt = ins.attribs.iter().find(|a| a.tag == "ALT").expect("ALT attribute");
        assert_eq!(alt.text.value, "12.500", "the altitude is still recorded in the ALT attribute either way");
    }

    #[test]
    fn import_points_puts_cod_mat_alt_definitions_each_on_their_own_layer() {
        let mut s = session_with_doc();
        let path = std::env::temp_dir().join(format!("cadcraft-topo-tcpoint-sublayers-{}.csv", std::process::id()));
        std::fs::write(&path, "ID;X;Y;Z;CODE\nP1;0.0;0.0;10.0;BORNE\n").expect("writing the temp points file should succeed");
        s.execute("topo.import_points", &json!({ "path": path.to_string_lossy() })).expect("import should succeed");
        let _ = std::fs::remove_file(&path);

        let doc = s.doc().expect("doc");
        let blk = doc.block(super::TCPOINT_BLOCK).expect("the TCPOINT block should exist");
        let layer_of = |tag: &str| {
            blk.entities
                .iter()
                .find_map(|e| if let super::EntityKind::AttDef(a) = &e.kind { (a.tag == tag).then(|| e.common.layer.clone()) } else { None })
        };
        assert_eq!(layer_of("COD"), Some("TOPOCOD".to_string()));
        assert_eq!(layer_of("MAT"), Some("TOPOMAT".to_string()));
        assert_eq!(layer_of("ALT"), Some("TOPOALT".to_string()));
        for layer in ["TOPOCOD", "TOPOMAT", "TOPOALT"] {
            assert!(doc.layer(layer).is_some(), "{layer} should have been created");
        }
        // The point symbol itself stays on layer 0, so it floats to the reference's own layer.
        let (circles, others): (Vec<_>, Vec<_>) =
            blk.entities.iter().partition(|e| matches!(e.kind, super::EntityKind::Circle(_) | super::EntityKind::Line(_)));
        assert!(!circles.is_empty());
        assert!(circles.iter().all(|e| e.common.layer == "0"));
        assert_eq!(others.len(), 3, "COD/MAT/ALT attribute definitions");
    }

    #[test]
    fn import_points_respects_an_explicit_delimiter_even_when_auto_would_differ() {
        let mut s = session_with_doc();
        let path = std::env::temp_dir().join(format!("cadcraft-topo-points-delim-{}.csv", std::process::id()));
        // Comma-separated, but with semicolons inside a trailing comment-like field; forcing
        // comma avoids any ambiguity auto-detection might otherwise have to resolve.
        std::fs::write(&path, "P1,100.0,200.0,10.0\n").expect("writing the temp points file should succeed");

        let out = s.execute("topo.import_points", &json!({ "path": path.to_string_lossy(), "delimiter": "comma" }));
        let _ = std::fs::remove_file(&path);
        let out = out.expect("import should succeed");
        assert_eq!(out["importedPoints"].as_u64(), Some(1));
    }

    #[test]
    fn preview_points_csv_reflects_the_chosen_delimiter() {
        let mut s = session_with_doc();
        let path = std::env::temp_dir().join(format!("cadcraft-topo-points-preview-{}.csv", std::process::id()));
        std::fs::write(&path, "ID;X;Y;Z\nP1;100.0;200.0;10.0\nP2;110.0;210.0;11.0\n").expect("writing the temp points file should succeed");

        let out = s.execute("topo.preview_points_csv", &json!({ "path": path.to_string_lossy(), "delimiter": "semicolon", "lines": 10 }));
        let _ = std::fs::remove_file(&path);
        let out = out.expect("preview should succeed");
        let rows = out["rows"].as_array().expect("rows array");
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect::<Vec<_>>(), vec!["ID", "X", "Y", "Z"]);
    }

    #[test]
    fn preview_points_csv_reports_a_missing_file_as_an_error_not_a_panic() {
        let mut s = session_with_doc();
        assert!(s.execute("topo.preview_points_csv", &json!({ "path": "/nonexistent/does-not-exist.csv" })).is_err());
    }

    #[test]
    fn zero_value_command_reports_sigma() {
        let mut s = session_with_doc();
        let out = s
            .execute(
                "topo.zero_value",
                &json!({
                    "station": [0.0, 0.0, 0.0],
                    "references": [
                        { "point": [10.0, 0.0, 0.0], "hzReading": 20.0 },
                        { "point": [0.0, 10.0, 0.0], "hzReading": 320.0 }
                    ]
                }),
            )
            .expect("zero_value should succeed");
        assert!((out["vo"].as_f64().unwrap_or(f64::NAN) - 80.0).abs() < 1e-6);
        assert!(out["sigma"].as_f64().unwrap_or(f64::NAN) < 1e-6);
    }
}
