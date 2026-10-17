//! Topography commands built on `cadcraft-topo-calc`: orienting a station from known
//! references (zero value, *VO*) and computing new points from polar observations
//! (*rayonnement*), inserting the results directly into the active document as POINT
//! entities (with an optional TEXT label carrying each point's code).

use cadcraft_doc::{EntityKind, Point};
use cadcraft_geom::Vec3;
use cadcraft_topo_calc::{RadiationObservation, ReferenceObservation, radiation, zero_value};
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
    ]
}

fn vec3_req(cmd: &str, p: &Value, key: &str) -> Result<Vec3> {
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

fn run_zero_value(_s: &mut Session, p: &Value) -> Result<Value> {
    let station = vec3_req("topo.zero_value", p, "station")?;
    let refs = references_param("topo.zero_value", p, "references")?;
    let result = zero_value(station, &refs).map_err(|e| topo_err("topo.zero_value", e))?;
    Ok(json!({ "vo": result.vo_gon, "sigma": result.sigma_gon, "count": result.count }))
}

fn run_radiation(s: &mut Session, p: &Value) -> Result<Value> {
    let station = vec3_req("topo.radiation", p, "station")?;

    let (vo_gon, sigma) = match p.get("vo").and_then(Value::as_f64) {
        Some(vo) => (vo, None),
        None => {
            let refs = references_param("topo.radiation", p, "references")?;
            if refs.is_empty() {
                return Err(bad("topo.radiation", "either `vo` or a non-empty `references` is required"));
            }
            let r = zero_value(station, &refs).map_err(|e| topo_err("topo.radiation", e))?;
            (r.vo_gon, Some(r.sigma_gon))
        }
    };

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

        let handle = s.add_entity(EntityKind::Point(Point { p: coord, angle: 0.0 }))?;
        let label_handle = match (&code, label) {
            (Some(c), true) if !c.is_empty() => {
                let xy = cadcraft_geom::Vec2::new(coord.x, coord.y);
                let k = make_text(s, xy, c, text_height, 0.0, (cadcraft_doc::HAlign::Left, cadcraft_doc::VAlign::Baseline));
                Some(s.add_entity(k)?.hex())
            }
            _ => None,
        };

        points.push(json!({
            "handle": handle.hex(),
            "x": coord.x, "y": coord.y, "z": coord.z,
            "code": code,
            "labelHandle": label_handle,
        }));
    }

    Ok(json!({ "vo": vo_gon, "sigma": sigma, "points": points }))
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
