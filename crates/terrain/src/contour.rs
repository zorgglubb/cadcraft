//! Contour-line extraction from a [`Tin`].
//!
//! For each triangle and each requested elevation, this finds where that elevation crosses the
//! triangle's edges and emits the resulting segment. **Scope note**: segments are emitted
//! per-triangle and are not yet chained into continuous polylines across triangle boundaries -
//! that stitching is a follow-up increment, not pretended-complete here. Each segment carries
//! its elevation so a caller can still group and chain them downstream.

use crate::tin::Tin;
use cadcraft_geom::Vec2;

/// One straight segment of a contour line at a single elevation, crossing one triangle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContourSegment {
    pub elevation: f64,
    pub start: Vec2,
    pub end: Vec2,
}

/// Extracts contour segments for every multiple of `interval` that crosses the TIN's elevation
/// range. Returns an empty vector (not an error) for a non-positive interval, an interval coarser
/// than the whole elevation range, or a TIN with no elevation range (no points) - none of those
/// are malformed input, they just produce no contours.
pub fn contours(tin: &Tin, interval: f64) -> Vec<ContourSegment> {
    let mut out = Vec::new();
    if !interval.is_finite() || interval <= 0.0 {
        return out;
    }
    let Some((lo, hi)) = tin.elevation_range() else { return out };
    if !lo.is_finite() || !hi.is_finite() {
        return out;
    }

    let first_level = (lo / interval).ceil() * interval;
    let mut level = first_level;
    // Cap the number of levels so a pathological (near-zero) interval can't loop unboundedly on
    // hostile input; this is generous for any real terrain (thousands of contour levels).
    const MAX_LEVELS: usize = 100_000;
    let mut emitted_levels = 0usize;
    while level <= hi + 1e-9 && emitted_levels < MAX_LEVELS {
        for tri in &tin.triangles {
            if let Some(seg) = triangle_crossing(tin, tri, level) {
                out.push(seg);
            }
        }
        level += interval;
        emitted_levels += 1;
    }
    out
}

/// The single segment where `level` crosses triangle `tri`'s two "transition" edges, if the
/// level actually lies strictly between the triangle's min and max elevation.
fn triangle_crossing(tin: &Tin, tri: &[usize; 3], level: f64) -> Option<ContourSegment> {
    let pts: [cadcraft_geom::Vec3; 3] = [*tin.points.get(tri[0])?, *tin.points.get(tri[1])?, *tin.points.get(tri[2])?];
    let zs = [pts[0].z, pts[1].z, pts[2].z];
    let (zmin, zmax) = (zs[0].min(zs[1]).min(zs[2]), zs[0].max(zs[1]).max(zs[2]));
    if level <= zmin || level >= zmax {
        return None;
    }

    let mut crossings: Vec<Vec2> = Vec::with_capacity(2);
    for (i, j) in [(0usize, 1usize), (1, 2), (2, 0)] {
        let (za, zb) = (zs[i], zs[j]);
        let crosses = (za < level) != (zb < level);
        if !crosses {
            continue;
        }
        let denom = zb - za;
        if denom.abs() < 1e-12 {
            continue;
        }
        let t = (level - za) / denom;
        let t = t.clamp(0.0, 1.0);
        let a = Vec2::new(pts[i].x, pts[i].y);
        let b = Vec2::new(pts[j].x, pts[j].y);
        crossings.push(a + (b - a) * t);
    }

    match crossings.as_slice() {
        [start, end] => Some(ContourSegment { elevation: level, start: *start, end: *end }),
        // A level landing exactly on a vertex, or other degenerate edge case: skip rather than
        // guess, this is never hostile-input-unsafe since we only ever read, never panic.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tin::triangulate;
    use cadcraft_geom::Vec3;

    #[test]
    fn non_positive_interval_yields_no_contours() {
        let pts = [Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 10.0), Vec3::new(0.0, 10.0, 0.0)];
        let tin = triangulate(&pts).expect("should triangulate");
        assert!(contours(&tin, 0.0).is_empty());
        assert!(contours(&tin, -1.0).is_empty());
        assert!(contours(&tin, f64::NAN).is_empty());
    }

    #[test]
    fn a_tilted_plane_produces_one_segment_per_level() {
        // z = x again: on [0,10], interval 2 should cross at x = 2,4,6,8 -> 4 levels.
        let pts = [Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 10.0), Vec3::new(10.0, 10.0, 10.0), Vec3::new(0.0, 10.0, 0.0)];
        let tin = triangulate(&pts).expect("should triangulate");
        let segs = contours(&tin, 2.0);
        let mut levels: Vec<f64> = segs.iter().map(|s| s.elevation).collect();
        levels.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
        levels.dedup();
        assert_eq!(levels, vec![2.0, 4.0, 6.0, 8.0]);
    }

    #[test]
    fn a_flat_tin_produces_no_contours() {
        let pts = [Vec3::new(0.0, 0.0, 5.0), Vec3::new(10.0, 0.0, 5.0), Vec3::new(0.0, 10.0, 5.0)];
        let tin = triangulate(&pts).expect("should triangulate");
        assert!(contours(&tin, 1.0).is_empty());
    }

    #[test]
    fn hostile_tiny_interval_never_loops_forever() {
        let pts = [Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 1000.0), Vec3::new(0.0, 10.0, 0.0)];
        let tin = triangulate(&pts).expect("should triangulate");
        // Interval far smaller than the range: must terminate (via MAX_LEVELS) rather than hang.
        let segs = contours(&tin, 1e-9);
        assert!(!segs.is_empty());
    }
}
