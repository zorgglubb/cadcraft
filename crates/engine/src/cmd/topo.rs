//! Topography commands built on `cadcraft-topo-calc`: orienting a station from known
//! references (zero value, *VO*) and computing new points from polar observations
//! (*rayonnement*), inserting the results directly into the active document as POINT
//! entities (with an optional TEXT label carrying each point's code).

use cadcraft_doc::{EntityKind, HAlign, Point, VAlign};
use cadcraft_geom::{Vec2, Vec3};
use cadcraft_topo_calc::{RadiationObservation, ReferenceObservation, radiation, zero_value};
use cadcraft_topo_io::{csv_points, gsi};
use serde_json::{Value, json};

use super::draw::make_text;
use super::*;
use crate::{Result, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("topo.radiation", "Radiation", run_radiation).menu(&["Topo", "Calculations", "Radiation"]).alias(&["rayonnement"]).params(
            "{station: [x,y,z], vo?: gon, references?: [{point: [x,y,z], hzReading: gon}], \
                 observations: [{hzReading, zenith, slopeDistance, instrumentHeight?, targetHeight?, code?}], label?: bool}",
        ),
        CommandSpec::new("topo.zero_value", "Zero Value (VO)", run_zero_value)
            .menu(&["Topo", "Calculations", "Zero Value"])
            .alias(&["vo"])
            .params("{station: [x,y,z], references: [{point: [x,y,z], hzReading: gon}]}"),
        CommandSpec::new("topo.import_gsi", "Import GSI Carnet", run_import_gsi).menu(&["Topo", "Import", "GSI Carnet"]).params(
            "{path: string, station: [x,y,z], vo?: gon, references?: [{point: [x,y,z], hzReading: gon}], \
                 label?: bool, textHeight?: number}",
        ),
        CommandSpec::new("topo.import_points", "Import Points (CSV/TXT)", run_import_points)
            .menu(&["Topo", "Import", "Points CSV/TXT"])
            .params("{path: string, label?: bool, textHeight?: number}"),
    ]
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

/// Imports a Leica GSI-8/16 carnet from disk and inserts every point it can compute a full
/// radiation observation for (Hz, Vz and slope distance all present) into the active document.
/// Points missing one of those (a references-only coordinate line, for instance) are counted in
/// `skippedPoints` rather than silently dropped.
fn run_import_gsi(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_param(p, "path").ok_or_else(|| bad("topo.import_gsi", "`path` is required"))?;
    let station = vec3_req("topo.import_gsi", p, "station")?;
    let (vo_gon, sigma) = resolve_vo("topo.import_gsi", station, p)?;
    let label = bool_or(p, "label", true);
    let text_height = f64_or(p, "textHeight", s.doc()?.header.f64("TEXTSIZE", 0.2));

    let text = std::fs::read_to_string(path).map_err(|e| bad("topo.import_gsi", format!("{path}: {e}")))?;
    let import = gsi::parse(&text);

    let mut points = Vec::new();
    let mut skipped_points = 0usize;
    for gp in &import.points {
        let (Some(hz), Some(vz), Some(dist)) = (gp.hz_gon, gp.vz_gon, gp.slope_distance) else {
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
        let coord = radiation(station, vo_gon, obs).map_err(|e| topo_err("topo.import_gsi", e))?;
        let code = gp.code.clone().or_else(|| Some(gp.id.clone()));
        points.push(insert_point(s, coord, code, label, text_height)?);
    }

    Ok(json!({
        "vo": vo_gon, "sigma": sigma,
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
    let path = str_param(p, "path").ok_or_else(|| bad("topo.import_points", "`path` is required"))?;
    let label = bool_or(p, "label", true);
    let text_height = f64_or(p, "textHeight", s.doc()?.header.f64("TEXTSIZE", 0.2));

    let text = std::fs::read_to_string(path).map_err(|e| bad("topo.import_points", format!("{path}: {e}")))?;
    let import = csv_points::parse(&text);

    let mut points = Vec::with_capacity(import.points.len());
    for cp in &import.points {
        let coord = Vec3::new(cp.x, cp.y, cp.z.unwrap_or(0.0));
        let code = cp.code.clone().or_else(|| Some(cp.id.clone()));
        points.push(insert_point(s, coord, code, label, text_height)?);
    }

    Ok(json!({
        "importedPoints": points.len(),
        "skippedLines": import.skipped_lines,
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
        // One station-coordinate line (skipped: no Hz/Vz/dist) and one real radiation shot
        // (Hz 100 gon = due east, Vz 100 gon = horizontal, slope dist 50 m), code P1.
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

        assert_eq!(out["importedPoints"].as_u64(), Some(1));
        assert_eq!(out["skippedPoints"].as_u64(), Some(1));
        let pts = out["points"].as_array().expect("points array");
        assert!((pts[0]["x"].as_f64().unwrap_or(f64::NAN) - 1050.0).abs() < 1e-6);
        assert!((pts[0]["y"].as_f64().unwrap_or(f64::NAN) - 1000.0).abs() < 1e-6);
        assert_eq!(pts[0]["code"].as_str(), Some("P1"));
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
