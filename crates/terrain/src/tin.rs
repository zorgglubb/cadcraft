//! Delaunay triangulation (the TIN itself) and elevation queries on it.
//!
//! Built with the Bowyer-Watson incremental algorithm: a point at a time, remove every
//! triangle whose circumcircle contains it, and retriangulate the hole that leaves. The
//! triangulation works in the plane (X, Y); each point's Z rides along for elevation queries
//! and contour extraction, not for choosing the triangulation itself - the usual convention
//! for a terrain TIN.

use cadcraft_geom::{Vec2, Vec3};
use thiserror::Error;

#[derive(Debug, Error, PartialEq)]
pub enum TerrainError {
    #[error("a triangulation needs at least 3 points, got {0}")]
    NotEnoughPoints(usize),
    #[error("point {index} has a non-finite coordinate")]
    NonFinitePoint { index: usize },
    #[error("the points are degenerate (collinear, or all coincident): no triangle could be formed")]
    Degenerate,
}

type Result<T> = std::result::Result<T, TerrainError>;

/// A triangulated irregular network: the input points plus the triangles connecting them, each
/// stored as the indices of three of those points (counter-clockwise).
#[derive(Clone, Debug, PartialEq)]
pub struct Tin {
    pub points: Vec<Vec3>,
    pub triangles: Vec<[usize; 3]>,
}

impl Tin {
    /// The lowest and highest elevation among the TIN's points, if it has any.
    pub fn elevation_range(&self) -> Option<(f64, f64)> {
        let mut it = self.points.iter().map(|p| p.z);
        let first = it.next()?;
        Some(it.fold((first, first), |(lo, hi), z| (lo.min(z), hi.max(z))))
    }

    /// Linearly interpolates the elevation at `(x, y)` from whichever triangle contains it
    /// (barycentric interpolation of its three corners' Z), or `None` outside the TIN's hull.
    pub fn elevation_at(&self, x: f64, y: f64) -> Option<f64> {
        let p = Vec2::new(x, y);
        for tri in &self.triangles {
            let [ia, ib, ic] = *tri;
            let (a, b, c) = (self.points.get(ia)?, self.points.get(ib)?, self.points.get(ic)?);
            if let Some((u, v, w)) = barycentric(Vec2::new(a.x, a.y), Vec2::new(b.x, b.y), Vec2::new(c.x, c.y), p) {
                return Some(u * a.z + v * b.z + w * c.z);
            }
        }
        None
    }
}

/// Barycentric coordinates of `p` in triangle `(a, b, c)`, or `None` when `p` falls outside it
/// (any coordinate strictly negative beyond a small tolerance) or the triangle is degenerate
/// (zero area).
fn barycentric(a: Vec2, b: Vec2, c: Vec2, p: Vec2) -> Option<(f64, f64, f64)> {
    let v0 = b - a;
    let v1 = c - a;
    let v2 = p - a;
    let d00 = v0.dot(v0);
    let d01 = v0.dot(v1);
    let d11 = v1.dot(v1);
    let d20 = v2.dot(v0);
    let d21 = v2.dot(v1);
    let denom = d00 * d11 - d01 * d01;
    if denom.abs() < 1e-12 {
        return None;
    }
    let v = (d11 * d20 - d01 * d21) / denom;
    let w = (d00 * d21 - d01 * d20) / denom;
    let u = 1.0 - v - w;
    const EPS: f64 = 1e-9;
    (u >= -EPS && v >= -EPS && w >= -EPS).then_some((u, v, w))
}

/// Whether `p` lies strictly inside the circumcircle of the (any-orientation) triangle `(a, b,
/// c)`, via the standard in-circle determinant, normalized to a counter-clockwise triangle first
/// so the determinant's sign is meaningful regardless of how `(a, b, c)` were wound.
fn in_circumcircle(a: Vec2, b: Vec2, c: Vec2, p: Vec2) -> bool {
    let (b, c) = if (b - a).cross(c - a) < 0.0 { (c, b) } else { (b, c) };
    let (ax, ay) = (a.x - p.x, a.y - p.y);
    let (bx, by) = (b.x - p.x, b.y - p.y);
    let (cx, cy) = (c.x - p.x, c.y - p.y);
    let det = (ax * ax + ay * ay) * (bx * cy - cx * by) - (bx * bx + by * by) * (ax * cy - cx * ay) + (cx * cx + cy * cy) * (ax * by - bx * ay);
    det > 1e-9
}

/// Builds the Delaunay triangulation of `points` (their X, Y; Z carries over unused by the
/// triangulation itself). Never panics: malformed input (too few points, a non-finite
/// coordinate, or points too degenerate to form any triangle - all collinear, or coincident) is
/// reported as a [`TerrainError`] rather than crashing.
pub fn triangulate(points: &[Vec3]) -> Result<Tin> {
    let n = points.len();
    if n < 3 {
        return Err(TerrainError::NotEnoughPoints(n));
    }
    for (i, p) in points.iter().enumerate() {
        if !p.x.is_finite() || !p.y.is_finite() || !p.z.is_finite() {
            return Err(TerrainError::NonFinitePoint { index: i });
        }
    }

    let (mut min_x, mut min_y, mut max_x, mut max_y) = (points[0].x, points[0].y, points[0].x, points[0].y);
    for p in points {
        min_x = min_x.min(p.x);
        min_y = min_y.min(p.y);
        max_x = max_x.max(p.x);
        max_y = max_y.max(p.y);
    }
    let dx = (max_x - min_x).max(1.0);
    let dy = (max_y - min_y).max(1.0);
    let margin = dx.max(dy) * 20.0 + 10.0;
    let cx = (min_x + max_x) / 2.0;
    let cy = (min_y + max_y) / 2.0;

    // A super-triangle generously enclosing every input point, appended after them so its
    // vertex indices (n, n+1, n+2) are easy to recognise and strip out at the end.
    let mut pts2: Vec<Vec2> = points.iter().map(|p| Vec2::new(p.x, p.y)).collect();
    pts2.push(Vec2::new(cx - 2.0 * margin, cy - margin));
    pts2.push(Vec2::new(cx, cy + 2.0 * margin));
    pts2.push(Vec2::new(cx + 2.0 * margin, cy - margin));
    let (s0, s1, s2) = (n, n + 1, n + 2);

    let mut triangles: Vec<[usize; 3]> = vec![[s0, s1, s2]];

    for i in 0..n {
        let p = pts2[i];
        let mut bad = Vec::new();
        for (ti, tri) in triangles.iter().enumerate() {
            let (Some(a), Some(b), Some(c)) = (pts2.get(tri[0]).copied(), pts2.get(tri[1]).copied(), pts2.get(tri[2]).copied()) else { continue };
            if in_circumcircle(a, b, c, p) {
                bad.push(ti);
            }
        }
        if bad.is_empty() {
            // p coincides with an already-placed point, or falls exactly on an edge under this
            // algorithm's tolerance: skip it rather than corrupt the triangulation.
            continue;
        }

        // Directed edges of every bad triangle; one that doesn't also appear reversed among
        // them bounds the hole, in the orientation that keeps new triangles counter-clockwise.
        let mut edges: Vec<(usize, usize)> = Vec::with_capacity(bad.len() * 3);
        for &ti in &bad {
            let Some(tri) = triangles.get(ti) else { continue };
            edges.push((tri[0], tri[1]));
            edges.push((tri[1], tri[2]));
            edges.push((tri[2], tri[0]));
        }
        let boundary: Vec<(usize, usize)> = edges.iter().copied().filter(|&(u, v)| !edges.iter().any(|&(x, y)| x == v && y == u)).collect();

        let mut kept = Vec::with_capacity(triangles.len());
        for (ti, tri) in triangles.into_iter().enumerate() {
            if !bad.contains(&ti) {
                kept.push(tri);
            }
        }
        triangles = kept;
        for (u, v) in boundary {
            triangles.push([u, v, i]);
        }
    }

    triangles.retain(|t| t[0] < n && t[1] < n && t[2] < n);
    if triangles.is_empty() {
        return Err(TerrainError::Degenerate);
    }

    Ok(Tin { points: points.to_vec(), triangles })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_too_few_points() {
        assert_eq!(triangulate(&[Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0)]), Err(TerrainError::NotEnoughPoints(2)));
    }

    #[test]
    fn rejects_non_finite_coordinates() {
        let pts = [Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0), Vec3::new(f64::NAN, 1.0, 0.0)];
        assert_eq!(triangulate(&pts), Err(TerrainError::NonFinitePoint { index: 2 }));
    }

    #[test]
    fn rejects_collinear_points() {
        let pts = [Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0), Vec3::new(2.0, 0.0, 0.0), Vec3::new(3.0, 0.0, 0.0)];
        assert_eq!(triangulate(&pts), Err(TerrainError::Degenerate));
    }

    #[test]
    fn triangulates_a_single_square_into_two_triangles() {
        let pts = [Vec3::new(0.0, 0.0, 1.0), Vec3::new(10.0, 0.0, 2.0), Vec3::new(10.0, 10.0, 3.0), Vec3::new(0.0, 10.0, 4.0)];
        let tin = triangulate(&pts).expect("a square should triangulate");
        assert_eq!(tin.triangles.len(), 2, "a convex quad always splits into exactly two triangles");
        // Every point index must appear in the triangulation and stay in range.
        let mut seen = [false; 4];
        for tri in &tin.triangles {
            for &i in tri {
                assert!(i < 4, "triangle references an out-of-range index {i}");
                seen[i] = true;
            }
        }
        assert!(seen.iter().all(|&s| s), "every input point should be used");
    }

    #[test]
    fn a_center_point_creates_four_triangles_in_a_square() {
        let pts =
            [Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 0.0), Vec3::new(10.0, 10.0, 0.0), Vec3::new(0.0, 10.0, 0.0), Vec3::new(5.0, 5.0, 5.0)];
        let tin = triangulate(&pts).expect("should triangulate");
        assert_eq!(tin.triangles.len(), 4);
    }

    #[test]
    fn elevation_at_interpolates_inside_a_flat_tilted_plane() {
        // z = x (a tilted plane), so elevation_at should just return x inside the hull.
        let pts = [Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 10.0), Vec3::new(10.0, 10.0, 10.0), Vec3::new(0.0, 10.0, 0.0)];
        let tin = triangulate(&pts).expect("should triangulate");
        let z = tin.elevation_at(5.0, 5.0).expect("(5,5) should be inside the square");
        assert!((z - 5.0).abs() < 1e-6, "z = {z}");
        assert!(tin.elevation_at(-5.0, -5.0).is_none(), "outside the hull should be None");
    }

    #[test]
    fn elevation_range_reports_min_and_max() {
        let pts = [Vec3::new(0.0, 0.0, 1.0), Vec3::new(10.0, 0.0, -3.0), Vec3::new(5.0, 10.0, 7.0)];
        let tin = triangulate(&pts).expect("should triangulate");
        assert_eq!(tin.elevation_range(), Some((-3.0, 7.0)));
    }

    #[test]
    fn duplicate_points_never_panic() {
        let pts =
            [Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 0.0), Vec3::new(10.0, 10.0, 0.0), Vec3::new(10.0, 10.0, 0.0), Vec3::new(0.0, 10.0, 0.0)];
        // Just must not panic; the exact triangle count isn't the point of this test.
        let _ = triangulate(&pts);
    }

    #[test]
    fn a_larger_random_like_point_set_triangulates_without_panicking() {
        // A deterministic pseudo-random-looking grid with jitter, large enough to exercise many
        // Bowyer-Watson insertion steps.
        let mut pts = Vec::new();
        for i in 0..12 {
            for j in 0..12 {
                let jitter = ((i * 7 + j * 13) % 5) as f64 * 0.1;
                pts.push(Vec3::new(i as f64 + jitter, j as f64 - jitter, (i + j) as f64 * 0.5));
            }
        }
        let tin = triangulate(&pts).expect("a 12x12 jittered grid should triangulate");
        // Euler's formula for a triangulated convex point set: triangles = 2*n - 2 - boundary,
        // so just sanity-check it's in a plausible range rather than pin an exact count.
        assert!(tin.triangles.len() > pts.len(), "{} triangles for {} points looks too few", tin.triangles.len(), pts.len());
    }
}
