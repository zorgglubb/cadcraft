//! Pure-Rust map projections for [`crate::crs::CrsRegistry`]'s own PROJ4 strings - parsed only
//! from the exact `+proj=lcc`/`+proj=utm`/`+proj=longlat` forms this crate itself generates
//! (`crs::default_definitions`, `crs::CrsRegistry::register_proj4`'s pasted-in strings are never
//! fed through this parser for a transform - see [`Projection::parse`]'s doc).
//!
//! All built on one ellipsoid: GRS80 (the French systems' datum) and WGS84 differ by nothing in
//! the semi-major axis and about 1.4e-9 in flattening, well under a millimetre of ground
//! distance anywhere on Earth, so one ellipsoid serves every system this registry knows. Every
//! built-in definition also carries `+towgs84=0,0,0,0,0,0,0` (or no datum shift at all for the
//! WGS84-based ones): there is no horizontal datum shift to apply between any pair of them, only
//! a change of projection. A registry entry with no `proj4` string (the legacy Lambert NTF zones,
//! the DOM-TOM systems - `crs.rs`'s module doc says why) can't be transformed through until one
//! is registered, and `register_proj4` lets a real Helmert-shifted definition replace an assumed
//! zero-shift one if NTF/DOM-TOM support is ever needed.
//!
//! Formulas are Snyder's standard ellipsoidal projection equations (USGS Professional Paper
//! 1395, "Map Projections: A Working Manual" - a public-domain US government publication),
//! implemented here from the published equations, not copied from any PROJ/GDAL source.

use cadcraft_geom::Vec2;

/// GRS80/WGS84 semi-major axis (metres) and inverse flattening - see this module's doc for why
/// one ellipsoid serves every system in the registry.
const SEMI_MAJOR_AXIS: f64 = 6_378_137.0;
const INVERSE_FLATTENING: f64 = 298.257_222_101;

fn eccentricity_squared() -> f64 {
    let f = 1.0 / INVERSE_FLATTENING;
    f * (2.0 - f)
}

/// A projection this module can run forward and inverse, parsed from one of the registry's own
/// PROJ4 strings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Projection {
    /// `+proj=longlat`: geographic coordinates themselves (degrees in, degrees out) - the pivot
    /// every other projection here converts through.
    Geographic,
    /// `+proj=lcc`: Lambert Conformal Conic with two standard parallels (every French Lambert
    /// zone this registry defines, CC42-CC50 and Lambert-93 among them).
    Lcc { lat1: f64, lat2: f64, lat0: f64, lon0: f64, x0: f64, y0: f64 },
    /// `+proj=utm`: Transverse Mercator at UTM's fixed scale factor and false easting, northern
    /// hemisphere (the only zones this registry defines).
    Utm { zone: u32 },
}

impl Projection {
    /// Parses one of this crate's own PROJ4 strings. Returns `None` for anything this module
    /// doesn't implement (a different `+proj=`, or a string missing a parameter this one needs) -
    /// never a guess. Only ever called on a string this crate generated itself
    /// (`crs::default_definitions`); a `register_proj4` string pasted in by a caller is a
    /// *stored* definition, not necessarily one this parser understands, and `CrsRegistry`
    /// reports that plainly as "no definition to reproject through" rather than silently failing
    /// to parse it.
    pub fn parse(proj4: &str) -> Option<Projection> {
        let get = |key: &str| -> Option<f64> { proj4.split_whitespace().find_map(|tok| tok.strip_prefix(&format!("+{key}=")))?.parse().ok() };
        if proj4.contains("+proj=longlat") {
            return Some(Projection::Geographic);
        }
        if proj4.contains("+proj=lcc") {
            return Some(Projection::Lcc {
                lat1: get("lat_1")?.to_radians(),
                lat2: get("lat_2")?.to_radians(),
                lat0: get("lat_0")?.to_radians(),
                lon0: get("lon_0")?.to_radians(),
                x0: get("x_0")?,
                y0: get("y_0")?,
            });
        }
        if proj4.contains("+proj=utm") {
            let zone = get("zone")?;
            if !(1.0..=60.0).contains(&zone) {
                return None;
            }
            return Some(Projection::Utm { zone: zone as u32 });
        }
        None
    }

    /// This projection's coordinates (its own units - metres for Lambert/UTM, degrees for
    /// geographic) to geographic radians (lon, lat).
    pub fn to_geographic(self, p: Vec2) -> Vec2 {
        match self {
            Projection::Geographic => Vec2::new(p.x.to_radians(), p.y.to_radians()),
            Projection::Lcc { lat1, lat2, lat0, lon0, x0, y0 } => lcc_inverse(p, lat1, lat2, lat0, lon0, x0, y0),
            Projection::Utm { zone } => utm_inverse(p, zone),
        }
    }

    /// Geographic radians (lon, lat) to this projection's own coordinates.
    pub fn from_geographic(self, ll: Vec2) -> Vec2 {
        match self {
            Projection::Geographic => Vec2::new(ll.x.to_degrees(), ll.y.to_degrees()),
            Projection::Lcc { lat1, lat2, lat0, lon0, x0, y0 } => lcc_forward(ll, lat1, lat2, lat0, lon0, x0, y0),
            Projection::Utm { zone } => utm_forward(ll, zone),
        }
    }
}

/// Snyder's `m(phi)`: the ellipsoidal parallel radius factor used by the Lambert conic.
fn m(lat: f64, e2: f64) -> f64 {
    lat.cos() / (1.0 - e2 * lat.sin().powi(2)).sqrt()
}

/// Snyder's `t(phi)`: the isometric colatitude factor used by the Lambert conic (both ways).
fn isometric_t(lat: f64, e: f64) -> f64 {
    let s = lat.sin();
    (std::f64::consts::FRAC_PI_4 - lat / 2.0).tan() / ((1.0 - e * s) / (1.0 + e * s)).powf(e / 2.0)
}

/// `n` and `F`, the Lambert conic's cone constant and scale factor, shared by the forward and
/// inverse formulas.
fn lcc_constants(lat1: f64, lat2: f64, e2: f64, e: f64) -> (f64, f64) {
    let (m1, m2) = (m(lat1, e2), m(lat2, e2));
    let (t1, t2) = (isometric_t(lat1, e), isometric_t(lat2, e));
    // Two coincident standard parallels (a tangent, one-parallel conic) is the sin(lat1) limit.
    let n = if (lat1 - lat2).abs() < 1e-12 { lat1.sin() } else { (m1.ln() - m2.ln()) / (t1.ln() - t2.ln()) };
    let f = m1 / (n * t1.powf(n));
    (n, f)
}

fn lcc_forward(ll: Vec2, lat1: f64, lat2: f64, lat0: f64, lon0: f64, x0: f64, y0: f64) -> Vec2 {
    let e2 = eccentricity_squared();
    let e = e2.sqrt();
    let (n, f) = lcc_constants(lat1, lat2, e2, e);
    let t0 = isometric_t(lat0, e);
    let tt = isometric_t(ll.y, e);
    let rho = SEMI_MAJOR_AXIS * f * tt.powf(n);
    let rho0 = SEMI_MAJOR_AXIS * f * t0.powf(n);
    let theta = n * (ll.x - lon0);
    Vec2::new(x0 + rho * theta.sin(), y0 + rho0 - rho * theta.cos())
}

fn lcc_inverse(p: Vec2, lat1: f64, lat2: f64, lat0: f64, lon0: f64, x0: f64, y0: f64) -> Vec2 {
    let e2 = eccentricity_squared();
    let e = e2.sqrt();
    let (n, f) = lcc_constants(lat1, lat2, e2, e);
    let t0 = isometric_t(lat0, e);
    let rho0 = SEMI_MAJOR_AXIS * f * t0.powf(n);
    let (dx, dy) = (p.x - x0, rho0 - (p.y - y0));
    let rho = dx.hypot(dy).copysign(n);
    let theta = dx.atan2(dy);
    let tt = (rho / (SEMI_MAJOR_AXIS * f)).powf(1.0 / n);
    let lon = theta / n + lon0;
    // Snyder's Newton iteration for latitude from the isometric colatitude `t` - converges to
    // machine precision in well under ten steps for any latitude on Earth.
    let mut lat = std::f64::consts::FRAC_PI_2 - 2.0 * tt.atan();
    for _ in 0..10 {
        let s = lat.sin();
        let next = std::f64::consts::FRAC_PI_2 - 2.0 * (tt * ((1.0 - e * s) / (1.0 + e * s)).powf(e / 2.0)).atan();
        let converged = (next - lat).abs() < 1e-14;
        lat = next;
        if converged {
            break;
        }
    }
    Vec2::new(lon, lat)
}

/// UTM's fixed scale factor (every zone).
const UTM_K0: f64 = 0.9996;
/// UTM's false easting (every zone); false northing is 0 in the northern hemisphere, the only
/// zones this registry defines.
const UTM_FALSE_EASTING: f64 = 500_000.0;

fn utm_central_meridian(zone: u32) -> f64 {
    (f64::from(zone) * 6.0 - 183.0).to_radians()
}

/// Snyder's ellipsoidal meridian-arc length from the equator to `lat`.
fn meridian_arc(lat: f64, e2: f64) -> f64 {
    SEMI_MAJOR_AXIS
        * ((1.0 - e2 / 4.0 - 3.0 * e2 * e2 / 64.0 - 5.0 * e2.powi(3) / 256.0) * lat
            - (3.0 * e2 / 8.0 + 3.0 * e2 * e2 / 32.0 + 45.0 * e2.powi(3) / 1024.0) * (2.0 * lat).sin()
            + (15.0 * e2 * e2 / 256.0 + 45.0 * e2.powi(3) / 1024.0) * (4.0 * lat).sin()
            - (35.0 * e2.powi(3) / 3072.0) * (6.0 * lat).sin())
}

fn utm_forward(ll: Vec2, zone: u32) -> Vec2 {
    let (lon, lat) = (ll.x, ll.y);
    let lon0 = utm_central_meridian(zone);
    let e2 = eccentricity_squared();
    let ep2 = e2 / (1.0 - e2);
    let nn = SEMI_MAJOR_AXIS / (1.0 - e2 * lat.sin().powi(2)).sqrt();
    let tn = lat.tan();
    let t = tn * tn;
    let c = ep2 * lat.cos().powi(2);
    let aa = (lon - lon0) * lat.cos();
    let mm = meridian_arc(lat, e2);
    let x = UTM_K0 * nn * (aa + (1.0 - t + c) * aa.powi(3) / 6.0 + (5.0 - 18.0 * t + t * t + 72.0 * c - 58.0 * ep2) * aa.powi(5) / 120.0)
        + UTM_FALSE_EASTING;
    let y = UTM_K0
        * (mm
            + nn * tn
                * (aa * aa / 2.0
                    + (5.0 - t + 9.0 * c + 4.0 * c * c) * aa.powi(4) / 24.0
                    + (61.0 - 58.0 * t + t * t + 600.0 * c - 330.0 * ep2) * aa.powi(6) / 720.0));
    Vec2::new(x, y)
}

fn utm_inverse(p: Vec2, zone: u32) -> Vec2 {
    let lon0 = utm_central_meridian(zone);
    let e2 = eccentricity_squared();
    let ep2 = e2 / (1.0 - e2);
    let e1 = (1.0 - (1.0 - e2).sqrt()) / (1.0 + (1.0 - e2).sqrt());
    let m = p.y / UTM_K0;
    let mu = m / (SEMI_MAJOR_AXIS * (1.0 - e2 / 4.0 - 3.0 * e2 * e2 / 64.0 - 5.0 * e2.powi(3) / 256.0));
    let phi1 = mu
        + (3.0 * e1 / 2.0 - 27.0 * e1.powi(3) / 32.0) * (2.0 * mu).sin()
        + (21.0 * e1 * e1 / 16.0 - 55.0 * e1.powi(4) / 32.0) * (4.0 * mu).sin()
        + (151.0 * e1.powi(3) / 96.0) * (6.0 * mu).sin()
        + (1097.0 * e1.powi(4) / 512.0) * (8.0 * mu).sin();
    let n1 = SEMI_MAJOR_AXIS / (1.0 - e2 * phi1.sin().powi(2)).sqrt();
    let t1 = phi1.tan().powi(2);
    let c1 = ep2 * phi1.cos().powi(2);
    let r1 = SEMI_MAJOR_AXIS * (1.0 - e2) / (1.0 - e2 * phi1.sin().powi(2)).powf(1.5);
    let d = (p.x - UTM_FALSE_EASTING) / (n1 * UTM_K0);
    let lat = phi1
        - (n1 * phi1.tan() / r1)
            * (d * d / 2.0 - (5.0 + 3.0 * t1 + 10.0 * c1 - 4.0 * c1 * c1 - 9.0 * ep2) * d.powi(4) / 24.0
                + (61.0 + 90.0 * t1 + 298.0 * c1 + 45.0 * t1 * t1 - 252.0 * ep2 - 3.0 * c1 * c1) * d.powi(6) / 720.0);
    let lon = lon0
        + (d - (1.0 + 2.0 * t1 + c1) * d.powi(3) / 6.0
            + (5.0 - 2.0 * c1 + 28.0 * t1 - 3.0 * c1 * c1 + 8.0 * ep2 + 24.0 * t1 * t1) * d.powi(5) / 120.0)
            / phi1.cos();
    Vec2::new(lon, lat)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `tol` in metres: the Lambert conic's closed-form inverse round-trips to machine precision,
    /// but UTM's forward/inverse are each Snyder's *truncated* power series, so a fraction of a
    /// millimetre of residual error between them is expected, not a bug.
    fn roundtrip_within(proj4: &str, native: Vec2, tol: f64) {
        let proj = Projection::parse(proj4).unwrap_or_else(|| panic!("should parse {proj4}"));
        let ll = proj.to_geographic(native);
        let back = proj.from_geographic(ll);
        assert!((back.x - native.x).abs() < tol, "{proj4}: x {} vs {}", back.x, native.x);
        assert!((back.y - native.y).abs() < tol, "{proj4}: y {} vs {}", back.y, native.y);
    }

    #[test]
    fn lambert_93_round_trips_through_geographic() {
        roundtrip_within(
            "+proj=lcc +lat_1=49 +lat_2=44 +lat_0=46.5 +lon_0=3 +x_0=700000 +y_0=6600000 +ellps=GRS80 +towgs84=0,0,0,0,0,0,0 +units=m +no_defs",
            Vec2::new(652_000.0, 6_862_000.0),
            1e-6,
        );
    }

    #[test]
    fn cc43_round_trips_through_geographic() {
        roundtrip_within(
            "+proj=lcc +lat_1=42.25 +lat_2=43.75 +lat_0=43 +lon_0=3 +x_0=1700000 +y_0=3200000 +ellps=GRS80 +towgs84=0,0,0,0,0,0,0 +units=m +no_defs",
            Vec2::new(1_750_000.0, 3_210_000.0),
            1e-6,
        );
    }

    #[test]
    fn utm_31n_round_trips_through_geographic() {
        roundtrip_within("+proj=utm +zone=31 +datum=WGS84 +units=m +no_defs", Vec2::new(500_000.0, 5_000_000.0), 1e-3);
    }

    #[test]
    fn geographic_is_a_plain_degrees_radians_conversion() {
        let proj = Projection::parse("+proj=longlat +datum=WGS84 +no_defs").unwrap();
        let ll = proj.to_geographic(Vec2::new(2.3522, 48.8566)); // Paris
        assert!((ll.x - 2.3522_f64.to_radians()).abs() < 1e-12);
        assert!((ll.y - 48.8566_f64.to_radians()).abs() < 1e-12);
    }

    /// Paris sits a little west and a little north of the Lambert-93 false origin
    /// (lon_0=3°, lat_0=46.5°), so this is a coarse sanity check, not a reference value: the
    /// published IGN figure for central Paris is close to (652000, 6862000).
    #[test]
    fn paris_lands_in_the_expected_lambert_93_neighbourhood() {
        let proj = Projection::parse(
            "+proj=lcc +lat_1=49 +lat_2=44 +lat_0=46.5 +lon_0=3 +x_0=700000 +y_0=6600000 +ellps=GRS80 +towgs84=0,0,0,0,0,0,0 +units=m +no_defs",
        )
        .unwrap();
        let p = proj.from_geographic(Vec2::new(2.3522_f64.to_radians(), 48.8566_f64.to_radians()));
        assert!((550_000.0..750_000.0).contains(&p.x), "x = {}", p.x);
        assert!((6_800_000.0..6_950_000.0).contains(&p.y), "y = {}", p.y);
    }

    #[test]
    fn unsupported_or_malformed_proj4_strings_do_not_parse() {
        assert_eq!(Projection::parse("+proj=merc +lon_0=0"), None);
        assert_eq!(Projection::parse("+proj=lcc +lat_1=49"), None); // missing lat_2, lat_0, ...
        assert_eq!(Projection::parse(""), None);
        assert_eq!(Projection::parse("+proj=utm +zone=99 +datum=WGS84"), None); // out of range
    }
}
