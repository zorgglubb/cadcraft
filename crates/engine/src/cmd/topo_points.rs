//! "Constructions de points" - point-construction tools from the Points & Constructions
//! géométriques part of the topography module: line/line intersection, a point at a given
//! distance and bearing, the perpendicular projection of a point onto an alignment, and dividing
//! a segment into N equal parts. Each behaves like an extended OSNAP, usable at the command
//! line, and inserts its result(s) as POINT entities in the active document.

use cadcraft_geom::{Line as GLine, Vec2, Vec3, line_line_infinite};
use cadcraft_topo_calc::{gisement, point_at_bearing};
use serde_json::{Value, json};

use super::topo::{insert_point, vec3_req};
use super::*;
use crate::{Result, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("topo.point_intersection", "Point: Line/Line Intersection", run_point_intersection)
            .menu(&["Topo", "Points", "Intersection"])
            .params("{a1: [x,y,z], a2: [x,y,z], b1: [x,y,z], b2: [x,y,z], code?: string, label?: bool, textHeight?: number}"),
        CommandSpec::new("topo.point_at", "Point: Distance & Bearing", run_point_at)
            .menu(&["Topo", "Points", "Distance & Bearing"])
            .params("{from: [x,y,z], gisement: gon, distance: number, code?: string, label?: bool, textHeight?: number}"),
        CommandSpec::new("topo.point_projection", "Point: Projection on Alignment", run_point_projection)
            .menu(&["Topo", "Points", "Projection"])
            .params("{point: [x,y,z], a: [x,y,z], b: [x,y,z], code?: string, label?: bool, textHeight?: number}"),
        CommandSpec::new("topo.divide_segment", "Point: Divide Segment", run_divide_segment)
            .menu(&["Topo", "Points", "Divide Segment"])
            .params("{a: [x,y,z], b: [x,y,z], count: integer (>=2), code?: string, label?: bool, textHeight?: number}"),
        CommandSpec::new("topo.angle_transfer", "Report d'angle (Angle Transfer)", run_angle_transfer)
            .menu(&["Topo", "Points", "Angle Transfer"])
            .alias(&["report_angle"])
            .params("{vertex: [x,y,z], reference: [x,y,z], angle: gon, distance: number, code?: string, label?: bool, textHeight?: number}"),
    ]
}

fn topo_err(cmd: &str, e: cadcraft_topo_calc::TopoCalcError) -> crate::EngineError {
    bad(cmd, e.to_string())
}

/// Reads the shared `code`/`label`/`textHeight` params every command in this file takes, with
/// the active document's text size as the default height, matching `topo.rs`'s convention.
fn common_params(s: &Session, p: &Value) -> Result<(Option<String>, bool, f64)> {
    let code = str_param(p, "code").map(str::to_string);
    let label = bool_or(p, "label", true);
    let text_height = f64_or(p, "textHeight", s.doc()?.header.f64("TEXTSIZE", 0.2));
    Ok((code, label, text_height))
}

fn run_point_intersection(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "topo.point_intersection";
    let a1 = vec3_req(CMD, p, "a1")?;
    let a2 = vec3_req(CMD, p, "a2")?;
    let b1 = vec3_req(CMD, p, "b1")?;
    let b2 = vec3_req(CMD, p, "b2")?;
    let (code, label, text_height) = common_params(s, p)?;

    let (hit, _, _) = line_line_infinite(Vec2::new(a1.x, a1.y), Vec2::new(a2.x, a2.y), Vec2::new(b1.x, b1.y), Vec2::new(b2.x, b2.y))
        .ok_or_else(|| bad(CMD, "the two lines are parallel (or coincident): no single intersection"))?;
    let coord = Vec3::new(hit.x, hit.y, f64_or(p, "z", 0.0));
    let point = insert_point(s, coord, code, label, text_height)?;
    Ok(json!({ "point": point }))
}

fn run_point_at(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "topo.point_at";
    let from = vec3_req(CMD, p, "from")?;
    let gisement_gon = f64_req(CMD, p, "gisement")?;
    let distance = f64_req(CMD, p, "distance")?;
    let (code, label, text_height) = common_params(s, p)?;

    let coord = point_at_bearing(from, gisement_gon, distance).map_err(|e| topo_err(CMD, e))?;
    let point = insert_point(s, coord, code, label, text_height)?;
    Ok(json!({ "point": point }))
}

fn run_point_projection(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "topo.point_projection";
    let point = vec3_req(CMD, p, "point")?;
    let a = vec3_req(CMD, p, "a")?;
    let b = vec3_req(CMD, p, "b")?;
    let (code, label, text_height) = common_params(s, p)?;

    let line = GLine::new(Vec2::new(a.x, a.y), Vec2::new(b.x, b.y));
    if line.len() < 1e-9 {
        return Err(bad(CMD, "`a` and `b` must not coincide: the alignment has no direction"));
    }
    let foot = line.project(Vec2::new(point.x, point.y));
    let coord = Vec3::new(foot.x, foot.y, point.z);
    let result = insert_point(s, coord, code, label, text_height)?;
    Ok(json!({ "point": result }))
}

fn run_divide_segment(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "topo.divide_segment";
    let a = vec3_req(CMD, p, "a")?;
    let b = vec3_req(CMD, p, "b")?;
    let count = p.get("count").and_then(Value::as_u64).ok_or_else(|| bad(CMD, "`count` (integer >= 2) is required"))?;
    if !(2..=100_000).contains(&count) {
        return Err(bad(CMD, "`count` must be between 2 and 100000"));
    }
    let (code, label, text_height) = common_params(s, p)?;

    // N parts needs (N - 1) interior division points; the segment's own endpoints are not
    // re-inserted.
    let mut points = Vec::with_capacity((count - 1) as usize);
    for i in 1..count {
        let t = i as f64 / count as f64;
        let coord = Vec3::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t, a.z + (b.z - a.z) * t);
        points.push(insert_point(s, coord, code.clone(), label, text_height)?);
    }
    Ok(json!({ "points": points }))
}

/// *Report d'angle*: reproduces a given angle (turned from the direction `vertex -> reference`)
/// at `vertex`, at the given distance - the classic drafting construction for laying out a new
/// direction from a known one without first computing its absolute bearing by hand.
fn run_angle_transfer(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "topo.angle_transfer";
    let vertex = vec3_req(CMD, p, "vertex")?;
    let reference = vec3_req(CMD, p, "reference")?;
    let angle_gon = f64_req(CMD, p, "angle")?;
    let distance = f64_req(CMD, p, "distance")?;
    let (code, label, text_height) = common_params(s, p)?;

    let base_gisement = gisement(vertex, reference).map_err(|e| topo_err(CMD, e))?;
    let coord = point_at_bearing(vertex, base_gisement + angle_gon, distance).map_err(|e| topo_err(CMD, e))?;
    let point = insert_point(s, coord, code, label, text_height)?;
    Ok(json!({ "point": point, "gisement": cadcraft_topo_calc::norm_gon(base_gisement + angle_gon) }))
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
    fn point_intersection_of_two_perpendicular_lines() {
        let mut s = session_with_doc();
        let out = s
            .execute(
                "topo.point_intersection",
                &json!({
                    "a1": [0.0, 5.0, 0.0], "a2": [10.0, 5.0, 0.0],
                    "b1": [5.0, 0.0, 0.0], "b2": [5.0, 10.0, 0.0]
                }),
            )
            .expect("should succeed");
        let pt = &out["point"];
        assert!((pt["x"].as_f64().unwrap_or(f64::NAN) - 5.0).abs() < 1e-9);
        assert!((pt["y"].as_f64().unwrap_or(f64::NAN) - 5.0).abs() < 1e-9);
    }

    #[test]
    fn point_intersection_rejects_parallel_lines() {
        let mut s = session_with_doc();
        let out = s.execute(
            "topo.point_intersection",
            &json!({
                "a1": [0.0, 0.0, 0.0], "a2": [10.0, 0.0, 0.0],
                "b1": [0.0, 5.0, 0.0], "b2": [10.0, 5.0, 0.0]
            }),
        );
        assert!(out.is_err());
    }

    #[test]
    fn point_at_distance_and_bearing_due_east() {
        let mut s = session_with_doc();
        let out =
            s.execute("topo.point_at", &json!({ "from": [1000.0, 1000.0, 50.0], "gisement": 100.0, "distance": 25.0 })).expect("should succeed");
        let pt = &out["point"];
        assert!((pt["x"].as_f64().unwrap_or(f64::NAN) - 1025.0).abs() < 1e-9);
        assert!((pt["y"].as_f64().unwrap_or(f64::NAN) - 1000.0).abs() < 1e-9);
    }

    #[test]
    fn point_at_rejects_negative_distance() {
        let mut s = session_with_doc();
        let out = s.execute("topo.point_at", &json!({ "from": [0.0, 0.0, 0.0], "gisement": 0.0, "distance": -5.0 }));
        assert!(out.is_err());
    }

    #[test]
    fn point_projection_onto_a_horizontal_alignment() {
        let mut s = session_with_doc();
        let out = s
            .execute("topo.point_projection", &json!({ "point": [5.0, 5.0, 12.0], "a": [0.0, 0.0, 0.0], "b": [10.0, 0.0, 0.0] }))
            .expect("should succeed");
        let pt = &out["point"];
        assert!((pt["x"].as_f64().unwrap_or(f64::NAN) - 5.0).abs() < 1e-9);
        assert!((pt["y"].as_f64().unwrap_or(f64::NAN) - 0.0).abs() < 1e-9);
        assert!((pt["z"].as_f64().unwrap_or(f64::NAN) - 12.0).abs() < 1e-9, "elevation should carry over from the source point");
    }

    #[test]
    fn point_projection_rejects_a_degenerate_alignment() {
        let mut s = session_with_doc();
        let out = s.execute("topo.point_projection", &json!({ "point": [5.0, 5.0, 0.0], "a": [1.0, 1.0, 0.0], "b": [1.0, 1.0, 0.0] }));
        assert!(out.is_err());
    }

    #[test]
    fn divide_segment_into_four_parts_inserts_three_points() {
        let mut s = session_with_doc();
        let out = s.execute("topo.divide_segment", &json!({ "a": [0.0, 0.0, 0.0], "b": [100.0, 0.0, 0.0], "count": 4 })).expect("should succeed");
        let pts = out["points"].as_array().expect("points array");
        assert_eq!(pts.len(), 3);
        assert!((pts[0]["x"].as_f64().unwrap_or(f64::NAN) - 25.0).abs() < 1e-9);
        assert!((pts[1]["x"].as_f64().unwrap_or(f64::NAN) - 50.0).abs() < 1e-9);
        assert!((pts[2]["x"].as_f64().unwrap_or(f64::NAN) - 75.0).abs() < 1e-9);
    }

    #[test]
    fn divide_segment_rejects_count_below_two() {
        let mut s = session_with_doc();
        let out = s.execute("topo.divide_segment", &json!({ "a": [0.0, 0.0, 0.0], "b": [10.0, 0.0, 0.0], "count": 1 }));
        assert!(out.is_err());
    }

    #[test]
    fn angle_transfer_turns_a_right_angle_from_a_known_direction() {
        let mut s = session_with_doc();
        // Reference due north of vertex (gisement 0 gon); turning +100 gon from it is due east.
        let out = s
            .execute("topo.angle_transfer", &json!({ "vertex": [0.0, 0.0, 0.0], "reference": [0.0, 10.0, 0.0], "angle": 100.0, "distance": 10.0 }))
            .expect("should succeed");
        let pt = &out["point"];
        assert!((pt["x"].as_f64().unwrap_or(f64::NAN) - 10.0).abs() < 1e-9);
        assert!((pt["y"].as_f64().unwrap_or(f64::NAN) - 0.0).abs() < 1e-9);
        assert!((out["gisement"].as_f64().unwrap_or(f64::NAN) - 100.0).abs() < 1e-9);
    }

    #[test]
    fn angle_transfer_rejects_a_coincident_vertex_and_reference() {
        let mut s = session_with_doc();
        let out =
            s.execute("topo.angle_transfer", &json!({ "vertex": [5.0, 5.0, 0.0], "reference": [5.0, 5.0, 0.0], "angle": 50.0, "distance": 10.0 }));
        assert!(out.is_err());
    }
}
