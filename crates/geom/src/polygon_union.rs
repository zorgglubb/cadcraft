//! Union of two simple (non-self-intersecting), straight-edged closed polygons into one
//! boundary, for the "merge 2 polygons" tool. Restricted scope, stated plainly: this only
//! produces a single closed-loop answer when the union of the two polygons' interiors is
//! itself simply connected (no holes) - i.e. the two polygons touch or overlap into one
//! blob. Two disjoint polygons, or a union with a hole in it (e.g. a ring left when one
//! polygon only partly swallows a notch of the other), are reported as errors rather than
//! guessed at: better to say "can't merge this" than to silently hand back a wrong or
//! self-intersecting outline.
//!
//! Algorithm: classic Weiler-Atherton polygon clipping, specialised to the union case.
//! Both polygons' edges are split at every intersection with the other polygon, intersection
//! points are inserted into both vertex lists in parameter order, each resulting vertex is
//! tagged as "inside" or "outside" the other polygon, and the outer boundary is walked by
//! always taking the piece of whichever polygon lies outside the other, hopping to the other
//! polygon's list at each crossing where it does so. Degenerate inputs (fewer than 3 vertices,
//! non-finite coordinates, zero-length edges) are rejected up front.

use crate::{Vec2, point_in_polygon, shoelace};

/// Why two polygons could not be merged into a single closed boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnionError {
    /// Fewer than 3 finite, distinct vertices.
    DegeneratePolygon,
    /// The polygons don't touch or overlap at all.
    Disjoint,
    /// They touch/overlap, but the result isn't a single simple closed loop (e.g. it would
    /// have a hole, or the traversal couldn't close cleanly) - too complex for this tool.
    TooComplex,
}

impl std::fmt::Display for UnionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UnionError::DegeneratePolygon => write!(f, "degenerate polygon (fewer than 3 valid vertices)"),
            UnionError::Disjoint => write!(f, "the polygons do not touch or overlap"),
            UnionError::TooComplex => write!(f, "could not merge: polygons may be disjoint or too complex"),
        }
    }
}

const TOL: f64 = 1e-7;

fn clean(poly: &[Vec2]) -> Option<Vec<Vec2>> {
    let mut out: Vec<Vec2> = Vec::new();
    for p in poly {
        if !p.is_finite() {
            return None;
        }
        if out.last().is_none_or(|last: &Vec2| !last.near(*p, TOL)) {
            out.push(*p);
        }
    }
    if out.len() > 1 {
        let same = match (out.first(), out.last()) {
            (Some(f), Some(l)) => f.near(*l, TOL),
            _ => false,
        };
        if same {
            out.pop();
        }
    }
    if out.len() < 3 {
        return None;
    }
    // Normalize to counter-clockwise (positive shoelace) so "outside the other polygon"
    // always means the same turn direction for both inputs.
    if shoelace(&out) < 0.0 {
        out.reverse();
    }
    Some(out)
}

/// Insert intersections between ring `a`'s edges and ring `b`'s edges into `a`'s own vertex
/// list (both already simple closed CCW polygons), returning the augmented ring. Returns
/// `None` if the segment math finds something non-finite.
fn split_at_intersections(a: &[Vec2], b: &[Vec2]) -> Option<Vec<Vec2>> {
    fn insert_sorted(hits: &mut Vec<(f64, Vec2)>, t: f64, p: Vec2) {
        if hits.iter().any(|(_, q)| q.near(p, TOL)) {
            return;
        }
        hits.push((t, p));
    }

    let n = a.len();
    let m = b.len();
    let mut out = Vec::new();
    for i in 0..n {
        let a0 = *a.get(i)?;
        let a1 = *a.get((i + 1) % n)?;
        let seg_len = (a1 - a0).len();
        out.push(a0);
        if seg_len < TOL {
            continue;
        }
        let mut hits: Vec<(f64, Vec2)> = Vec::new();
        for j in 0..m {
            let b0 = *b.get(j)?;
            let b1 = *b.get((j + 1) % m)?;
            if let Some((t, _u)) = segment_intersect_param(a0, a1, b0, b1) {
                let edge_tol = (TOL / seg_len).min(0.49);
                if t > edge_tol && t < 1.0 - edge_tol {
                    insert_sorted(&mut hits, t, a0.lerp(a1, t));
                }
            }
        }
        hits.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));
        for (_, p) in hits {
            out.push(p);
        }
    }
    Some(out)
}

/// True if segment (p0,p1) lies on the same infinite line as, and overlaps with, segment
/// (q0,q1) - i.e. they run collinear along a shared stretch rather than crossing at a point.
fn collinear_overlap(p0: Vec2, p1: Vec2, q0: Vec2, q1: Vec2) -> bool {
    let d = p1 - p0;
    if d.len() < TOL {
        return false;
    }
    let cross0 = (q0 - p0).cross(d);
    let cross1 = (q1 - p0).cross(d);
    if cross0.abs() > TOL * d.len() || cross1.abs() > TOL * d.len() {
        return false;
    }
    // Collinear; check 1D overlap of projections onto d.
    let len2 = d.len2();
    if len2 < TOL * TOL {
        return false;
    }
    let proj = |v: Vec2| v.dot(d) / len2;
    let (mut t0, mut t1) = (proj(q0 - p0), proj(q1 - p0));
    if t0 > t1 {
        std::mem::swap(&mut t0, &mut t1);
    }
    t1 > TOL && t0 < 1.0 - TOL
}

/// True if the midpoint of (a0,a1) lies on a stretch collinear-overlapping with some edge of
/// `ring`, meaning this edge is shared frontier between the two polygons rather than a true
/// boundary-of-the-union edge.
fn is_shared_frontier(a0: Vec2, a1: Vec2, ring: &[Vec2]) -> bool {
    let n = ring.len();
    for i in 0..n {
        let (Some(&b0), Some(&b1)) = (ring.get(i), ring.get((i + 1) % n)) else { continue };
        if collinear_overlap(a0, a1, b0, b1) {
            return true;
        }
    }
    false
}

/// Parametric intersection of segment (p0,p1) with (q0,q1): returns (t, u) with p0+t*(p1-p0)
/// == q0+u*(q1-q0), both in `[0,1]`, or `None` if parallel/non-crossing. Collinear-overlap
/// edges are treated as non-crossing here (handled instead as touching via shared vertices).
fn segment_intersect_param(p0: Vec2, p1: Vec2, q0: Vec2, q1: Vec2) -> Option<(f64, f64)> {
    let r = p1 - p0;
    let s = q1 - q0;
    let denom = r.cross(s);
    if denom.abs() < 1e-12 {
        return None;
    }
    let qp = q0 - p0;
    let t = qp.cross(s) / denom;
    let u = qp.cross(r) / denom;
    if !t.is_finite() || !u.is_finite() {
        return None;
    }
    if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) { Some((t, u)) } else { None }
}

/// Merge two simple closed polygons into their union boundary. Vertices need not be
/// deduplicated or pre-closed (first != last is fine); `a` wins no special role except as the
/// traversal's starting ring.
pub fn union(a: &[Vec2], b: &[Vec2]) -> Result<Vec<Vec2>, UnionError> {
    let pa = clean(a).ok_or(UnionError::DegeneratePolygon)?;
    let pb = clean(b).ok_or(UnionError::DegeneratePolygon)?;

    let ra = split_at_intersections(&pa, &pb).ok_or(UnionError::TooComplex)?;
    let rb = split_at_intersections(&pb, &pa).ok_or(UnionError::TooComplex)?;

    let has_crossing = ra.len() > pa.len() || rb.len() > pb.len();
    if !has_crossing {
        // No new intersection points: either disjoint, one fully contains the other, or they
        // only touch along shared edges/vertices with no interior overlap.
        let a_in_b = pa.iter().all(|p| point_in_polygon(&pb, *p) || is_shared_frontier_vertex(*p, &pb));
        let b_in_a = pb.iter().all(|p| point_in_polygon(&pa, *p) || is_shared_frontier_vertex(*p, &pa));
        if a_in_b && !b_in_a {
            return Ok(pb);
        }
        if b_in_a && !a_in_b {
            return Ok(pa);
        }
        if !touches(&pa, &pb) {
            return Err(UnionError::Disjoint);
        }
        // They touch (shared edge/vertex) without either containing the other: fall through
        // to the keep-and-chain stitch below, which handles this via shared-frontier edges.
    }

    // Keep every edge of each augmented ring whose midpoint lies strictly outside the other
    // polygon, dropping edges that run along a stretch shared with the other polygon's
    // boundary (those are interior to the union, not on its edge). The kept edges from both
    // rings should then chain into exactly one closed loop, since the union is assumed
    // simply connected.
    let mut edges: Vec<(Vec2, Vec2)> = Vec::new();
    collect_outer_edges(&ra, &pb, &mut edges);
    collect_outer_edges(&rb, &pa, &mut edges);

    if edges.is_empty() {
        return Err(UnionError::Disjoint);
    }

    let result = chain_edges(&edges).ok_or(UnionError::TooComplex)?;
    if result.len() < 3 {
        return Err(UnionError::TooComplex);
    }

    // Sanity-check the stitched loop's area against both inputs: a correct union never has
    // smaller area than the larger input nor more than the sum of both.
    let area_a = shoelace(&pa).abs();
    let area_b = shoelace(&pb).abs();
    let area_r = shoelace(&result).abs();
    if area_r + 1e-6 < area_a.max(area_b) || area_r > area_a + area_b + 1e-6 {
        return Err(UnionError::TooComplex);
    }

    Ok(result)
}

fn is_shared_frontier_vertex(p: Vec2, ring: &[Vec2]) -> bool {
    ring.iter().any(|q| q.near(p, TOL))
}

fn touches(a: &[Vec2], b: &[Vec2]) -> bool {
    let n = a.len();
    let m = b.len();
    for i in 0..n {
        let (Some(&a0), Some(&a1)) = (a.get(i), a.get((i + 1) % n)) else { continue };
        for j in 0..m {
            let (Some(&b0), Some(&b1)) = (b.get(j), b.get((j + 1) % m)) else { continue };
            if segment_intersect_param(a0, a1, b0, b1).is_some() || collinear_overlap(a0, a1, b0, b1) {
                return true;
            }
            if a0.near(b0, TOL) || a0.near(b1, TOL) || a1.near(b0, TOL) || a1.near(b1, TOL) {
                return true;
            }
        }
    }
    false
}

/// Push every edge of augmented ring `ring` whose midpoint is strictly outside polygon
/// `other` (and which isn't a shared-frontier stretch with `other`) onto `out`.
fn collect_outer_edges(ring: &[Vec2], other: &[Vec2], out: &mut Vec<(Vec2, Vec2)>) {
    let n = ring.len();
    for i in 0..n {
        let (Some(&p0), Some(&p1)) = (ring.get(i), ring.get((i + 1) % n)) else { continue };
        if p0.near(p1, TOL) {
            continue;
        }
        if is_shared_frontier(p0, p1, other) {
            continue;
        }
        let mid = p0.mid(p1);
        if !point_in_polygon(other, mid) {
            out.push((p0, p1));
        }
    }
}

/// Stitch a bag of directed edges into one closed loop by matching endpoints. Returns `None`
/// if they don't form exactly one simple closed loop (a vertex with other than exactly one
/// outgoing unused edge at each step, or leftover edges once the loop closes).
fn chain_edges(edges: &[(Vec2, Vec2)]) -> Option<Vec<Vec2>> {
    if edges.is_empty() {
        return None;
    }
    let mut used = vec![false; edges.len()];
    let (start, _) = *edges.first()?;
    let mut result = vec![start];
    let mut current = start;
    let max_steps = edges.len().saturating_add(1);

    for _ in 0..max_steps {
        let mut next: Option<(usize, Vec2)> = None;
        for (idx, (p0, p1)) in edges.iter().enumerate() {
            if used[idx] {
                continue;
            }
            if p0.near(current, TOL) {
                next = Some((idx, *p1));
                break;
            }
            if p1.near(current, TOL) {
                next = Some((idx, *p0));
                break;
            }
        }
        let Some((idx, pt)) = next else { break };
        used[idx] = true;
        if pt.near(start, TOL) {
            // Loop closed.
            if used.iter().all(|u| *u) {
                return Some(result);
            }
            return None;
        }
        result.push(pt);
        current = pt;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x0: f64, y0: f64, s: f64) -> Vec<Vec2> {
        vec![Vec2::new(x0, y0), Vec2::new(x0 + s, y0), Vec2::new(x0 + s, y0 + s), Vec2::new(x0, y0 + s)]
    }

    #[test]
    fn disjoint_squares_error() {
        let a = square(0.0, 0.0, 1.0);
        let b = square(10.0, 10.0, 1.0);
        assert_eq!(union(&a, &b), Err(UnionError::Disjoint));
    }

    #[test]
    fn touching_edge_squares_union_area_is_sum() {
        // Share the edge x=1: union is a 2x1 rectangle, area 2.
        let a = square(0.0, 0.0, 1.0);
        let b = square(1.0, 0.0, 1.0);
        let r = union(&a, &b).expect("should merge touching squares");
        assert!((shoelace(&r).abs() - 2.0).abs() < 1e-6, "area={}", shoelace(&r).abs());
    }

    #[test]
    fn overlapping_squares_union_area() {
        // [0,2]x[0,2] and [1,3]x[1,3]: areas 4 and 4, intersection [1,2]x[1,2] area 1.
        // union area = 4+4-1 = 7.
        let a = square(0.0, 0.0, 2.0);
        let b = square(1.0, 1.0, 2.0);
        let r = union(&a, &b).expect("should merge overlapping squares");
        assert!((shoelace(&r).abs() - 7.0).abs() < 1e-6, "area={}", shoelace(&r).abs());
    }

    #[test]
    fn one_polygon_fully_inside_other() {
        let outer = square(0.0, 0.0, 10.0);
        let inner = square(2.0, 2.0, 1.0);
        let r = union(&outer, &inner).expect("containment should yield the outer polygon");
        assert!((shoelace(&r).abs() - 100.0).abs() < 1e-6);
    }

    #[test]
    fn degenerate_polygon_errors() {
        let a = vec![Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0)];
        let b = square(0.0, 0.0, 1.0);
        assert_eq!(union(&a, &b), Err(UnionError::DegeneratePolygon));
    }

    #[test]
    fn l_shaped_union_offset_squares() {
        // Two unit squares offset diagonally by 0.5, overlapping in a 0.5x0.5 corner.
        let a = square(0.0, 0.0, 1.0);
        let b = square(0.5, 0.5, 1.0);
        let r = union(&a, &b).expect("should merge corner-overlapping squares forming an L");
        // areas 1 + 1 - 0.25 = 1.75
        assert!((shoelace(&r).abs() - 1.75).abs() < 1e-6, "area={}", shoelace(&r).abs());
    }
}
