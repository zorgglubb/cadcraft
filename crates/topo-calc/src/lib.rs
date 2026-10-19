//! CADCraft topography calculations: angle conversions, bearings (*gisements*), the
//! instrument zero value (*VO*), radiation (*rayonnement*) from polar observations, and
//! (incrementally) traverse, intersection, free station and Helmert transform.
//!
//! Angles follow French surveying convention: **gon** (grades), `400 gon = 2π rad`, with a
//! bearing (*gisement*) measured clockwise from the Y (north) axis — matching the convention
//! used by total stations and by the reference carnet-de-levé calculations this crate reimplements
//! natively in Rust.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

use cadcraft_geom::Vec3;
use thiserror::Error;

/// One full turn, in gon.
pub const GON_TURN: f64 = 400.0;

#[derive(Debug, Error, PartialEq)]
pub enum TopoCalcError {
    #[error("need at least one observation, got none")]
    NoObservations,
    #[error("need at least {need} points, got {got}")]
    NotEnoughPoints { need: usize, got: usize },
    #[error("station and reference point coincide (zero distance), gisement is undefined")]
    CoincidentPoints,
    #[error("slope distance must be finite and non-negative, got {0}")]
    InvalidDistance(f64),
    #[error("zenith angle {0} gon is out of the (0, {GON_TURN}) range")]
    InvalidZenith(f64),
    #[error("non-finite input value")]
    NonFinite,
    #[error("free station resection did not converge from the given references/approximate position")]
    DidNotConverge,
}

type Result<T> = std::result::Result<T, TopoCalcError>;

/// Converts an angle in gon (grades) to radians.
pub fn gon_to_rad(g: f64) -> f64 {
    g * std::f64::consts::PI / (GON_TURN / 2.0)
}

/// Converts an angle in radians to gon (grades).
pub fn rad_to_gon(r: f64) -> f64 {
    r * (GON_TURN / 2.0) / std::f64::consts::PI
}

/// Normalizes an angle in gon to `[0, 400)`.
pub fn norm_gon(g: f64) -> f64 {
    if !g.is_finite() {
        return 0.0;
    }
    let r = g.rem_euclid(GON_TURN);
    if r >= GON_TURN { 0.0 } else { r }
}

/// The 2D *gisement* (bearing) from `from` to `to`, in gon, measured clockwise from the Y axis
/// (north), as used by French total-station workflows. `0 gon` points along +Y, `100 gon` along +X.
pub fn gisement(from: Vec3, to: Vec3) -> Result<f64> {
    let dx = to.x - from.x;
    let dy = to.y - from.y;
    if !dx.is_finite() || !dy.is_finite() {
        return Err(TopoCalcError::NonFinite);
    }
    if dx == 0.0 && dy == 0.0 {
        return Err(TopoCalcError::CoincidentPoints);
    }
    Ok(norm_gon(rad_to_gon(dx.atan2(dy))))
}

/// Horizontal 2D distance between two points.
pub fn dist2d(from: Vec3, to: Vec3) -> f64 {
    (from.x - to.x).hypot(from.y - to.y)
}

/// The point reached from `from`, travelling `distance` (horizontal) along `gisement_gon` (the
/// same clockwise-from-north bearing convention as [`gisement`]), keeping `from`'s elevation.
/// This is the "point à distance et angle" construction tool: a direct polar point, independent
/// of any station orientation (unlike [`radiation`], which resolves a raw instrument reading
/// through a zero value first).
pub fn point_at_bearing(from: Vec3, gisement_gon: f64, distance: f64) -> Result<Vec3> {
    if !gisement_gon.is_finite() {
        return Err(TopoCalcError::NonFinite);
    }
    if !distance.is_finite() || distance < 0.0 {
        return Err(TopoCalcError::InvalidDistance(distance));
    }
    let rad = gon_to_rad(norm_gon(gisement_gon));
    Ok(Vec3::new(from.x + distance * rad.sin(), from.y + distance * rad.cos(), from.z))
}

/// One observation of a known reference point from the station: the reference's known
/// coordinates and the horizontal circle reading (`Hz`, in gon) taken towards it.
#[derive(Clone, Copy, Debug)]
pub struct ReferenceObservation {
    pub reference: Vec3,
    pub hz_reading_gon: f64,
}

/// Result of a zero-value (*VO*) computation: the mean VO (gon) to add to a raw `Hz` reading
/// to get a true gisement, plus its standard deviation as a quality indicator.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZeroValueResult {
    pub vo_gon: f64,
    pub sigma_gon: f64,
    pub count: usize,
}

/// Computes the instrument zero value (*VO*) from one or more known references observed from
/// `station`: for each reference, `vo = gisement(station, reference) - hz_reading`, averaged
/// circularly (handling the 0/400 gon wraparound) with its standard deviation.
pub fn zero_value(station: Vec3, observations: &[ReferenceObservation]) -> Result<ZeroValueResult> {
    if observations.is_empty() {
        return Err(TopoCalcError::NoObservations);
    }
    let mut sin_sum = 0.0;
    let mut cos_sum = 0.0;
    let mut vos = Vec::with_capacity(observations.len());
    for obs in observations {
        let g = gisement(station, obs.reference)?;
        let vo = norm_gon(g - obs.hz_reading_gon);
        let rad = gon_to_rad(vo);
        sin_sum += rad.sin();
        cos_sum += rad.cos();
        vos.push(vo);
    }
    let n = observations.len() as f64;
    let mean_rad = (sin_sum / n).atan2(cos_sum / n);
    let mean_vo = norm_gon(rad_to_gon(mean_rad));

    // Circular standard deviation (angular dispersion), expressed back in gon.
    let r = ((sin_sum / n).powi(2) + (cos_sum / n).powi(2)).sqrt().clamp(0.0, 1.0);
    let sigma_rad = (-2.0 * r.ln().min(0.0)).sqrt();
    let sigma_gon = if sigma_rad.is_finite() { rad_to_gon(sigma_rad) } else { 0.0 };

    let _ = vos; // kept for a future per-reference residuals report
    Ok(ZeroValueResult { vo_gon: mean_vo, sigma_gon, count: observations.len() })
}

/// One polar observation to a new point, taken from an oriented station (known position and
/// zero value): horizontal reading (`Hz`), zenith angle (`Vz`, `100 gon` = horizontal) and the
/// slope (inclined) distance, plus instrument and target heights.
#[derive(Clone, Copy, Debug)]
pub struct RadiationObservation {
    pub hz_reading_gon: f64,
    pub zenith_gon: f64,
    pub slope_distance: f64,
    pub instrument_height: f64,
    pub target_height: f64,
}

/// Computes a point's 3D coordinates from a station (coordinates + zero value) and one polar
/// observation to it (*rayonnement*): horizontal and zenithal readings plus a slope distance.
pub fn radiation(station: Vec3, vo_gon: f64, obs: RadiationObservation) -> Result<Vec3> {
    if !obs.slope_distance.is_finite() || obs.slope_distance < 0.0 {
        return Err(TopoCalcError::InvalidDistance(obs.slope_distance));
    }
    if !(0.0..GON_TURN).contains(&obs.zenith_gon) {
        return Err(TopoCalcError::InvalidZenith(obs.zenith_gon));
    }
    if ![obs.hz_reading_gon, vo_gon, obs.instrument_height, obs.target_height].into_iter().all(f64::is_finite) {
        return Err(TopoCalcError::NonFinite);
    }

    let gisement_gon = norm_gon(obs.hz_reading_gon + vo_gon);
    let gisement_rad = gon_to_rad(gisement_gon);
    let zenith_rad = gon_to_rad(obs.zenith_gon);

    let horizontal_dist = obs.slope_distance * zenith_rad.sin();
    let dz = obs.slope_distance * zenith_rad.cos();

    let x = station.x + horizontal_dist * gisement_rad.sin();
    let y = station.y + horizontal_dist * gisement_rad.cos();
    let z = station.z + obs.instrument_height + dz - obs.target_height;

    Ok(Vec3::new(x, y, z))
}

/// One observation, from the unknown station, of a known reference point: its horizontal
/// (slope-corrected) distance, used for the trilateration, plus the horizontal circle reading
/// (`Hz`) towards it, used afterwards to resolve the station's zero value once its position is
/// known.
#[derive(Clone, Copy, Debug)]
pub struct FreeStationObservation {
    pub reference: Vec3,
    pub horizontal_distance: f64,
    pub hz_reading_gon: f64,
}

/// Result of a free-station resection: the station's planar position (elevation, if wanted, is
/// left to a separate levelling/radiation step - this is a 2D resection), its zero value and the
/// number of Gauss-Newton iterations it took to converge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FreeStationResult {
    pub station: Vec3,
    pub vo: ZeroValueResult,
    pub iterations: usize,
}

/// Resects an unknown station's position from the horizontal distances to **two or more** known
/// reference points ("station libre"/free station), then derives its zero value from their `Hz`
/// readings - independent of the instrument's own orientation, since distance alone fixes a
/// circle around each reference and their intersection fixes the station.
///
/// With exactly two references this is a direct two-circle intersection (closed form); with more,
/// it is a small Gauss-Newton least-squares fit minimising the squared residual between each
/// reference's observed and predicted distance, seeded at `approx` (the centroid of the
/// references when `None`). The elevation of the returned station is the mean of
/// `reference.z - horizontal_distance * 0` (i.e. just the references' own mean height) - a
/// placeholder useful for 2D work; a real height still needs a levelled/zenith observation.
pub fn free_station(obs: &[FreeStationObservation], approx: Option<Vec3>) -> Result<FreeStationResult> {
    if obs.len() < 2 {
        return Err(TopoCalcError::NotEnoughPoints { need: 2, got: obs.len() });
    }
    for o in obs {
        if !o.horizontal_distance.is_finite() || o.horizontal_distance < 0.0 {
            return Err(TopoCalcError::InvalidDistance(o.horizontal_distance));
        }
        if !o.hz_reading_gon.is_finite() || !o.reference.x.is_finite() || !o.reference.y.is_finite() {
            return Err(TopoCalcError::NonFinite);
        }
    }

    let mean_z = obs.iter().map(|o| o.reference.z).sum::<f64>() / obs.len() as f64;
    let mut x = approx.map(|p| p.x).unwrap_or_else(|| obs.iter().map(|o| o.reference.x).sum::<f64>() / obs.len() as f64);
    let mut y = approx.map(|p| p.y).unwrap_or_else(|| obs.iter().map(|o| o.reference.y).sum::<f64>() / obs.len() as f64);

    // Gauss-Newton on f_i(x,y) = dist((x,y), ref_i) - d_i = 0, normal equations solved directly
    // since there are only two unknowns.
    const MAX_ITER: usize = 50;
    let mut iterations = 0;
    for _ in 0..MAX_ITER {
        iterations += 1;
        let (mut jtj00, mut jtj01, mut jtj11, mut jtr0, mut jtr1) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for o in obs {
            let dx = x - o.reference.x;
            let dy = y - o.reference.y;
            let d = dx.hypot(dy);
            if d < 1e-9 {
                return Err(TopoCalcError::CoincidentPoints);
            }
            let r = d - o.horizontal_distance;
            let (gx, gy) = (dx / d, dy / d);
            jtj00 += gx * gx;
            jtj01 += gx * gy;
            jtj11 += gy * gy;
            jtr0 += gx * r;
            jtr1 += gy * r;
        }
        let det = jtj00 * jtj11 - jtj01 * jtj01;
        if det.abs() < 1e-12 {
            break;
        }
        let dx = (jtj11 * jtr0 - jtj01 * jtr1) / det;
        let dy = (jtj00 * jtr1 - jtj01 * jtr0) / det;
        x -= dx;
        y -= dy;
        if dx.hypot(dy) < 1e-9 {
            break;
        }
    }
    if !x.is_finite() || !y.is_finite() {
        return Err(TopoCalcError::DidNotConverge);
    }
    let station = Vec3::new(x, y, mean_z);

    let refs: Vec<ReferenceObservation> =
        obs.iter().map(|o| ReferenceObservation { reference: o.reference, hz_reading_gon: o.hz_reading_gon }).collect();
    let vo = zero_value(station, &refs)?;
    Ok(FreeStationResult { station, vo, iterations })
}

/// One traverse leg ("cheminement"): the gisement and horizontal distance from the previous
/// traverse station to the next one. Each station is assumed already oriented (its own VO known,
/// e.g. by backsighting the previous point), so a leg's gisement is a true bearing, not a raw
/// `Hz` reading - the caller adds the station's VO before building this.
#[derive(Clone, Copy, Debug)]
pub struct TraverseLeg {
    pub gisement_gon: f64,
    pub distance: f64,
}

/// Result of running a traverse: every station's coordinates (including the fixed starting
/// point, so `points.len() == legs.len() + 1`), and, when a known closing point was given, the
/// raw (pre-adjustment) closing error as `(error_x, error_y)` before the compass-rule correction
/// folded into `points` was applied.
#[derive(Clone, Debug, PartialEq)]
pub struct TraverseResult {
    pub points: Vec<Vec3>,
    pub closing_error: Option<(f64, f64)>,
}

/// Runs a traverse ("cheminement") of `legs` from `start`, optionally adjusting it to a known
/// `closing_point` by the compass (Bowditch) rule: the raw closing error is distributed across
/// every intermediate station in proportion to its cumulative distance from `start`, so the
/// corrected traverse ends exactly on `closing_point` instead of drifting off by the raw error.
/// Without a `closing_point` the raw (unadjusted) polyline is returned as-is.
pub fn traverse(start: Vec3, legs: &[TraverseLeg], closing_point: Option<Vec3>) -> Result<TraverseResult> {
    if legs.is_empty() {
        return Err(TopoCalcError::NoObservations);
    }
    let mut raw = Vec::with_capacity(legs.len() + 1);
    raw.push(start);
    let mut cur = start;
    let mut cumulative = Vec::with_capacity(legs.len() + 1);
    cumulative.push(0.0);
    let mut total = 0.0;
    for leg in legs {
        cur = point_at_bearing(cur, leg.gisement_gon, leg.distance)?;
        total += leg.distance;
        cumulative.push(total);
        raw.push(cur);
    }

    let Some(target) = closing_point else {
        return Ok(TraverseResult { points: raw, closing_error: None });
    };
    let last = raw[raw.len() - 1];
    let err = (target.x - last.x, target.y - last.y);
    if total <= 0.0 {
        return Ok(TraverseResult { points: raw, closing_error: Some(err) });
    }
    let mut adjusted = raw.clone();
    for (i, p) in adjusted.iter_mut().enumerate() {
        let frac = cumulative[i] / total;
        p.x += err.0 * frac;
        p.y += err.1 * frac;
    }
    Ok(TraverseResult { points: adjusted, closing_error: Some(err) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64, eps: f64) -> bool {
        (a - b).abs() <= eps
    }

    #[test]
    fn gon_rad_roundtrip() {
        assert!(approx(gon_to_rad(100.0), std::f64::consts::FRAC_PI_2, 1e-12));
        assert!(approx(rad_to_gon(std::f64::consts::PI), 200.0, 1e-9));
        assert!(approx(norm_gon(-50.0), 350.0, 1e-9));
        assert!(approx(norm_gon(450.0), 50.0, 1e-9));
    }

    #[test]
    fn gisement_cardinal_directions() {
        let s = Vec3::new(0.0, 0.0, 0.0);
        // +Y (north) = 0 gon
        assert!(approx(gisement(s, Vec3::new(0.0, 10.0, 0.0)).unwrap_or(f64::NAN), 0.0, 1e-9));
        // +X (east) = 100 gon
        assert!(approx(gisement(s, Vec3::new(10.0, 0.0, 0.0)).unwrap_or(f64::NAN), 100.0, 1e-9));
        // -Y (south) = 200 gon
        assert!(approx(gisement(s, Vec3::new(0.0, -10.0, 0.0)).unwrap_or(f64::NAN), 200.0, 1e-9));
        // -X (west) = 300 gon
        assert!(approx(gisement(s, Vec3::new(-10.0, 0.0, 0.0)).unwrap_or(f64::NAN), 300.0, 1e-9));
    }

    #[test]
    fn gisement_coincident_points_errors() {
        let p = Vec3::new(5.0, 5.0, 0.0);
        assert_eq!(gisement(p, p), Err(TopoCalcError::CoincidentPoints));
    }

    #[test]
    fn zero_value_single_reference() {
        let station = Vec3::new(1000.0, 1000.0, 100.0);
        // Reference due east of the station: true gisement = 100 gon. If Hz read 15 gon,
        // VO = 100 - 15 = 85 gon.
        let reference = Vec3::new(1050.0, 1000.0, 100.0);
        let result = zero_value(station, &[ReferenceObservation { reference, hz_reading_gon: 15.0 }]).expect("single reference should succeed");
        assert!(approx(result.vo_gon, 85.0, 1e-9));
        assert_eq!(result.count, 1);
        assert!(approx(result.sigma_gon, 0.0, 1e-9));
    }

    #[test]
    fn zero_value_averages_two_consistent_references() {
        let station = Vec3::new(0.0, 0.0, 0.0);
        let obs = [
            // due east, true gisement 100 gon
            ReferenceObservation { reference: Vec3::new(10.0, 0.0, 0.0), hz_reading_gon: 20.0 }, // VO = 80
            // due north, true gisement 0 gon
            ReferenceObservation { reference: Vec3::new(0.0, 10.0, 0.0), hz_reading_gon: 320.0 }, // VO = 80 (0 - 320 = -320 = 80 mod 400)
        ];
        let result = zero_value(station, &obs).expect("two consistent references should succeed");
        assert!(approx(result.vo_gon, 80.0, 1e-6), "VO = {}", result.vo_gon);
        assert!(result.sigma_gon < 1e-6, "sigma = {}", result.sigma_gon);
    }

    #[test]
    fn zero_value_no_observations_errors() {
        assert_eq!(zero_value(Vec3::ZERO, &[]), Err(TopoCalcError::NoObservations));
    }

    #[test]
    fn radiation_horizontal_shot_east() {
        // Station at (1000, 1000, 100), VO = 0 so Hz readings are true gisements directly.
        let station = Vec3::new(1000.0, 1000.0, 100.0);
        let obs = RadiationObservation {
            hz_reading_gon: 100.0, // due east
            zenith_gon: 100.0,     // horizontal sight
            slope_distance: 50.0,
            instrument_height: 1.5,
            target_height: 1.5,
        };
        let p = radiation(station, 0.0, obs).expect("horizontal shot should succeed");
        assert!(approx(p.x, 1050.0, 1e-9));
        assert!(approx(p.y, 1000.0, 1e-9));
        // Equal instrument/target heights on a horizontal shot: no elevation change.
        assert!(approx(p.z, 100.0, 1e-9));
    }

    #[test]
    fn radiation_rejects_invalid_zenith() {
        let station = Vec3::ZERO;
        let obs = RadiationObservation { hz_reading_gon: 0.0, zenith_gon: 400.0, slope_distance: 10.0, instrument_height: 1.5, target_height: 1.5 };
        assert_eq!(radiation(station, 0.0, obs), Err(TopoCalcError::InvalidZenith(400.0)));
    }

    #[test]
    fn radiation_rejects_negative_distance() {
        let station = Vec3::ZERO;
        let obs = RadiationObservation { hz_reading_gon: 0.0, zenith_gon: 100.0, slope_distance: -5.0, instrument_height: 1.5, target_height: 1.5 };
        assert_eq!(radiation(station, 0.0, obs), Err(TopoCalcError::InvalidDistance(-5.0)));
    }

    #[test]
    fn point_at_bearing_cardinal_directions() {
        let from = Vec3::new(1000.0, 1000.0, 50.0);
        let east = point_at_bearing(from, 100.0, 10.0).expect("should succeed");
        assert!(approx(east.x, 1010.0, 1e-9));
        assert!(approx(east.y, 1000.0, 1e-9));
        assert!(approx(east.z, 50.0, 1e-9), "elevation should carry over from `from`");

        let north = point_at_bearing(from, 0.0, 10.0).expect("should succeed");
        assert!(approx(north.x, 1000.0, 1e-9));
        assert!(approx(north.y, 1010.0, 1e-9));
    }

    #[test]
    fn point_at_bearing_rejects_negative_distance() {
        assert_eq!(point_at_bearing(Vec3::ZERO, 0.0, -1.0), Err(TopoCalcError::InvalidDistance(-1.0)));
    }

    #[test]
    fn point_at_bearing_normalizes_an_out_of_range_bearing() {
        let from = Vec3::ZERO;
        let a = point_at_bearing(from, 100.0, 5.0).expect("should succeed");
        let b = point_at_bearing(from, 500.0, 5.0).expect("500 gon should normalize to 100 gon");
        assert!(approx(a.x, b.x, 1e-9));
        assert!(approx(a.y, b.y, 1e-9));
    }

    #[test]
    fn carnet_round_trip_vo_then_radiation() {
        // Full workflow: orient the station on one known reference, then compute a new point.
        let station = Vec3::new(500.0, 500.0, 50.0);
        let reference = Vec3::new(500.0, 600.0, 50.0); // due north, true gisement 0 gon
        let vo = zero_value(station, &[ReferenceObservation { reference, hz_reading_gon: 10.0 }]).expect("orientation should succeed"); // VO = 0 - 10 = -10 = 390 gon

        // Shoot a new point at Hz = 110 gon (true gisement 110 + 390 - 400 = 100 gon, due east).
        let obs = RadiationObservation { hz_reading_gon: 110.0, zenith_gon: 100.0, slope_distance: 20.0, instrument_height: 1.6, target_height: 1.6 };
        let p = radiation(station, vo.vo_gon, obs).expect("radiation should succeed");
        assert!(approx(p.x, 520.0, 1e-6), "x = {}", p.x);
        assert!(approx(p.y, 500.0, 1e-6), "y = {}", p.y);
        assert!(approx(p.z, 50.0, 1e-6), "z = {}", p.z);
    }

    #[test]
    fn free_station_resects_from_three_known_points() {
        // Unknown station at (1000, 1000). Three known points around it; distances computed
        // exactly, so resection should converge back onto the true position.
        let truth = Vec3::new(1000.0, 1000.0, 0.0);
        let refs = [Vec3::new(1100.0, 1000.0, 10.0), Vec3::new(1000.0, 1120.0, 10.0), Vec3::new(900.0, 950.0, 10.0)];
        let obs: Vec<FreeStationObservation> = refs
            .iter()
            .map(|r| FreeStationObservation {
                reference: *r,
                horizontal_distance: dist2d(truth, *r),
                hz_reading_gon: gisement(truth, *r).expect("gisement"), // Hz == true gisement, so VO should come out ~0
            })
            .collect();
        let result = free_station(&obs, None).expect("resection should converge");
        assert!(approx(result.station.x, 1000.0, 1e-5), "x = {}", result.station.x);
        assert!(approx(result.station.y, 1000.0, 1e-5), "y = {}", result.station.y);
        assert!(approx(result.vo.vo_gon, 0.0, 1e-4), "vo = {}", result.vo.vo_gon);
    }

    #[test]
    fn free_station_needs_at_least_two_references() {
        let obs = [FreeStationObservation { reference: Vec3::new(10.0, 0.0, 0.0), horizontal_distance: 10.0, hz_reading_gon: 100.0 }];
        assert_eq!(free_station(&obs, None), Err(TopoCalcError::NotEnoughPoints { need: 2, got: 1 }));
    }

    #[test]
    fn traverse_chains_legs_from_a_start_point() {
        let start = Vec3::new(0.0, 0.0, 0.0);
        let legs = [TraverseLeg { gisement_gon: 100.0, distance: 10.0 }, TraverseLeg { gisement_gon: 0.0, distance: 10.0 }];
        let result = traverse(start, &legs, None).expect("traverse should succeed");
        assert_eq!(result.points.len(), 3);
        assert!(approx(result.points[1].x, 10.0, 1e-9));
        assert!(approx(result.points[2].x, 10.0, 1e-9));
        assert!(approx(result.points[2].y, 10.0, 1e-9));
        assert_eq!(result.closing_error, None);
    }

    #[test]
    fn traverse_with_a_closing_point_distributes_the_error_by_the_compass_rule() {
        let start = Vec3::new(0.0, 0.0, 0.0);
        // Two equal 10-unit legs due east; raw end is (20, 0), but the true closing point is
        // (20, 2) - a 2-unit northward closing error split evenly (equal cumulative distance at
        // each station) between the two legs.
        let legs = [TraverseLeg { gisement_gon: 100.0, distance: 10.0 }, TraverseLeg { gisement_gon: 100.0, distance: 10.0 }];
        let closing = Vec3::new(20.0, 2.0, 0.0);
        let result = traverse(start, &legs, Some(closing)).expect("traverse should succeed");
        let (ex, ey) = result.closing_error.expect("closing point was given");
        assert!(approx(ex, 0.0, 1e-9));
        assert!(approx(ey, 2.0, 1e-9));
        assert!(approx(result.points[1].y, 1.0, 1e-9), "midpoint should get half the correction, y = {}", result.points[1].y);
        assert!(approx(result.points[2].x, 20.0, 1e-9));
        assert!(approx(result.points[2].y, 2.0, 1e-9), "last point should land exactly on the closing point");
    }

    #[test]
    fn traverse_rejects_no_legs() {
        assert_eq!(traverse(Vec3::ZERO, &[], None), Err(TopoCalcError::NoObservations));
    }
}
