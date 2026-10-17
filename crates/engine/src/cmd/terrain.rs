//! Terrain commands built on `cadcraft-terrain`: triangulating a point cloud into a TIN,
//! querying its elevation at a point, and extracting contour-line segments - each as a stateless
//! calculation over the points given in the call (no persistent terrain object yet), consistent
//! with "programmatic calls never open dialogs".

use cadcraft_doc::{EntityKind, Line};
use cadcraft_geom::Vec3;
use cadcraft_terrain::{TerrainError, contours, triangulate};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("terrain.triangulate", "Triangulate (TIN)", run_triangulate)
            .menu(&["Terrain", "TIN", "Triangulate"])
            .params("{points: [[x,y,z], ...], draw?: bool}"),
        CommandSpec::new("terrain.elevation_at", "Elevation At Point", run_elevation_at)
            .menu(&["Terrain", "TIN", "Elevation At Point"])
            .params("{points: [[x,y,z], ...], x: number, y: number}"),
        CommandSpec::new("terrain.contours", "Contour Lines", run_contours)
            .menu(&["Terrain", "TIN", "Contour Lines"])
            .alias(&["courbes"])
            .params("{points: [[x,y,z], ...], interval: number, draw?: bool}"),
    ]
}

fn points_param(cmd: &str, p: &Value) -> Result<Vec<Vec3>> {
    let arr = p.get("points").and_then(Value::as_array).ok_or_else(|| bad(cmd, "`points` (array of [x, y, z]) is required"))?;
    arr.iter()
        .enumerate()
        .map(|(i, v)| {
            let a = v.as_array().ok_or_else(|| bad(cmd, format!("points[{i}] must be an array [x, y, z]")))?;
            let get = |k: usize| a.get(k).and_then(Value::as_f64).filter(|n| n.is_finite());
            match (get(0), get(1)) {
                (Some(x), Some(y)) => Ok(Vec3::new(x, y, get(2).unwrap_or(0.0))),
                _ => Err(bad(cmd, format!("points[{i}] must be [x, y] or [x, y, z] of finite numbers"))),
            }
        })
        .collect()
}

fn terrain_err(cmd: &str, e: TerrainError) -> crate::EngineError {
    bad(cmd, e.to_string())
}

fn run_triangulate(s: &mut Session, p: &Value) -> Result<Value> {
    let pts = points_param("terrain.triangulate", p)?;
    let tin = triangulate(&pts).map_err(|e| terrain_err("terrain.triangulate", e))?;
    let draw = bool_or(p, "draw", true);

    let mut edge_count = 0usize;
    if draw {
        // Each undirected edge drawn once, even though it borders two triangles.
        let mut drawn: std::collections::HashSet<(usize, usize)> = std::collections::HashSet::new();
        for tri in &tin.triangles {
            for (i, j) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                let key = if i < j { (i, j) } else { (j, i) };
                if !drawn.insert(key) {
                    continue;
                }
                let (Some(a), Some(b)) = (tin.points.get(i), tin.points.get(j)) else { continue };
                s.add_entity(EntityKind::Line(Line { a: *a, b: *b }))?;
                edge_count += 1;
            }
        }
    }

    Ok(json!({
        "pointCount": tin.points.len(),
        "triangleCount": tin.triangles.len(),
        "edgeCount": edge_count,
        "elevationRange": tin.elevation_range(),
    }))
}

fn run_elevation_at(_s: &mut Session, p: &Value) -> Result<Value> {
    let pts = points_param("terrain.elevation_at", p)?;
    let x = f64_req("terrain.elevation_at", p, "x")?;
    let y = f64_req("terrain.elevation_at", p, "y")?;
    let tin = triangulate(&pts).map_err(|e| terrain_err("terrain.elevation_at", e))?;
    Ok(json!({ "elevation": tin.elevation_at(x, y) }))
}

/// Extracts and optionally draws contour-line segments. **Limitation, stated plainly**: each
/// segment is emitted per-triangle and is not yet chained into continuous polylines across
/// triangles - that stitching is a later increment.
fn run_contours(s: &mut Session, p: &Value) -> Result<Value> {
    let pts = points_param("terrain.contours", p)?;
    let interval = f64_req("terrain.contours", p, "interval")?;
    if !interval.is_finite() || interval <= 0.0 {
        return Err(bad("terrain.contours", "`interval` must be a positive finite number"));
    }
    let tin = triangulate(&pts).map_err(|e| terrain_err("terrain.contours", e))?;
    let segments = contours(&tin, interval);
    let draw = bool_or(p, "draw", true);

    if draw {
        for seg in &segments {
            s.add_entity(EntityKind::Line(Line { a: seg.start.to3(seg.elevation), b: seg.end.to3(seg.elevation) }))?;
        }
    }

    let mut levels: Vec<f64> = segments.iter().map(|sg| sg.elevation).collect();
    levels.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    levels.dedup();

    Ok(json!({ "segmentCount": segments.len(), "levels": levels }))
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::Session;

    fn session_with_doc() -> Session {
        let mut s = Session::new();
        s.execute("new", &json!({})).expect("new drawing should succeed");
        s
    }

    fn square_pts() -> Value {
        json!([[0.0, 0.0, 0.0], [10.0, 0.0, 10.0], [10.0, 10.0, 10.0], [0.0, 10.0, 0.0]])
    }

    #[test]
    fn triangulate_draws_edges_and_reports_counts() {
        let mut s = session_with_doc();
        let out = s.execute("terrain.triangulate", &json!({ "points": square_pts() })).expect("triangulate should succeed");
        assert_eq!(out["triangleCount"].as_u64(), Some(2));
        assert_eq!(out["pointCount"].as_u64(), Some(4));
        // A convex quad split into 2 triangles has 5 undirected edges (4 sides + 1 diagonal).
        assert_eq!(out["edgeCount"].as_u64(), Some(5));
    }

    #[test]
    fn triangulate_without_draw_inserts_nothing() {
        let mut s = session_with_doc();
        let out = s.execute("terrain.triangulate", &json!({ "points": square_pts(), "draw": false })).expect("should succeed");
        assert_eq!(out["edgeCount"].as_u64(), Some(0));
    }

    #[test]
    fn triangulate_rejects_too_few_points_as_an_error_not_a_panic() {
        let mut s = session_with_doc();
        let out = s.execute("terrain.triangulate", &json!({ "points": [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]] }));
        assert!(out.is_err());
    }

    #[test]
    fn elevation_at_interpolates_on_the_tilted_plane() {
        let mut s = session_with_doc();
        let out = s.execute("terrain.elevation_at", &json!({ "points": square_pts(), "x": 5.0, "y": 5.0 })).expect("should succeed");
        let z = out["elevation"].as_f64().expect("elevation should be present inside the hull");
        assert!((z - 5.0).abs() < 1e-6);
    }

    #[test]
    fn elevation_at_outside_the_hull_is_null() {
        let mut s = session_with_doc();
        let out = s.execute("terrain.elevation_at", &json!({ "points": square_pts(), "x": -50.0, "y": -50.0 })).expect("should succeed");
        assert!(out["elevation"].is_null());
    }

    #[test]
    fn contours_draws_segments_and_reports_levels() {
        let mut s = session_with_doc();
        let out = s.execute("terrain.contours", &json!({ "points": square_pts(), "interval": 2.0 })).expect("should succeed");
        assert_eq!(out["levels"].as_array().map(|a| a.len()), Some(4));
        assert!(out["segmentCount"].as_u64().unwrap_or(0) > 0);
    }

    #[test]
    fn contours_rejects_a_non_positive_interval() {
        let mut s = session_with_doc();
        let out = s.execute("terrain.contours", &json!({ "points": square_pts(), "interval": 0.0 }));
        assert!(out.is_err());
    }
}
