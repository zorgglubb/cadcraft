//! Coordinate reference system registry: the systems used daily in French surveying
//! (RGF93/Lambert-93, the nine Lambert CC zones, WGS84/UTM), plus the ability to add any other
//! EPSG code at runtime by pasting its PROJ4 or WKT definition.
//!
//! This registry identifies and stores CRS definitions; it does not itself reproject
//! coordinates (that needs a full PROJ-equivalent, future work). A definition with no `proj4`
//! string is a *known, named* system whose exact datum parameters this registry doesn't embed
//! (the legacy Lambert NTF zones, the DOM-TOM systems): shipping a guessed string for those would
//! be worse than admitting it isn't here yet, so callers that need to reproject through one
//! should supply its definition explicitly via [`CrsRegistry::register_proj4`].

use std::collections::HashMap;

use thiserror::Error;

/// One coordinate reference system: its EPSG code, a human-readable name, and (when known) its
/// PROJ4 definition string.
#[derive(Clone, Debug, PartialEq)]
pub struct CrsDef {
    pub epsg: u32,
    pub name: String,
    pub proj4: Option<String>,
}

#[derive(Debug, Error, PartialEq)]
pub enum CrsError {
    #[error("EPSG:0 is not a valid code")]
    InvalidEpsg,
    #[error("a CRS name must not be empty")]
    EmptyName,
    #[error("EPSG:{0} is not in the registry")]
    UnknownEpsg(u32),
    #[error("EPSG:{epsg} ({name}) has no definition this registry can reproject through - register_proj4 one first")]
    NoDefinition { epsg: u32, name: String },
}

/// A lookup table of coordinate reference systems, pre-loaded with the ones most French survey
/// work needs ([`CrsRegistry::with_defaults`]), open to adding any other EPSG code at runtime.
#[derive(Clone, Debug, Default)]
pub struct CrsRegistry {
    entries: HashMap<u32, CrsDef>,
}

impl CrsRegistry {
    /// An empty registry with none of the built-in definitions.
    pub fn empty() -> Self {
        CrsRegistry { entries: HashMap::new() }
    }

    /// A registry pre-loaded with the common French systems: RGF93/Lambert-93, the nine Lambert
    /// CC zones (CC42-CC50), WGS84, UTM 30N-32N, plus named (but parameter-less) entries for the
    /// legacy Lambert NTF zones and the DOM-TOM systems, so they at least resolve by EPSG code or
    /// name even before a full definition is registered for them.
    pub fn with_defaults() -> Self {
        let mut reg = Self::empty();
        for def in default_definitions() {
            reg.entries.insert(def.epsg, def);
        }
        reg
    }

    /// Looks up a system by its EPSG code.
    pub fn get(&self, epsg: u32) -> Option<&CrsDef> {
        self.entries.get(&epsg)
    }

    /// Finds every system whose name contains `query`, case-insensitively - for a picker that
    /// lets someone type "lambert" or "guyane" rather than an EPSG number.
    pub fn find_by_name(&self, query: &str) -> Vec<&CrsDef> {
        let q = query.to_ascii_lowercase();
        let mut found: Vec<&CrsDef> = self.entries.values().filter(|d| d.name.to_ascii_lowercase().contains(&q)).collect();
        found.sort_by_key(|d| d.epsg);
        found
    }

    /// Registers (or replaces) a system from its EPSG code, a name and a PROJ4 definition
    /// string - the "paste the PROJ4 string for a missing code" escape hatch. The string itself
    /// is stored as given, not parsed or validated beyond being non-empty.
    pub fn register_proj4(&mut self, epsg: u32, name: &str, proj4: &str) -> Result<(), CrsError> {
        if epsg == 0 {
            return Err(CrsError::InvalidEpsg);
        }
        let name = name.trim();
        if name.is_empty() {
            return Err(CrsError::EmptyName);
        }
        let proj4 = proj4.trim();
        self.entries.insert(epsg, CrsDef { epsg, name: name.to_string(), proj4: (!proj4.is_empty()).then(|| proj4.to_string()) });
        Ok(())
    }

    /// Registers a system known only by code and name (no PROJ4 definition yet).
    pub fn register_named(&mut self, epsg: u32, name: &str) -> Result<(), CrsError> {
        if epsg == 0 {
            return Err(CrsError::InvalidEpsg);
        }
        let name = name.trim();
        if name.is_empty() {
            return Err(CrsError::EmptyName);
        }
        self.entries.insert(epsg, CrsDef { epsg, name: name.to_string(), proj4: None });
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every entry, sorted by EPSG code - for a picker listing every system this registry knows.
    pub fn all(&self) -> Vec<&CrsDef> {
        let mut v: Vec<&CrsDef> = self.entries.values().collect();
        v.sort_by_key(|d| d.epsg);
        v
    }

    /// Reprojects a point from one system to another, by way of geographic coordinates (both
    /// systems' `to_geographic`/`from_geographic`) - see `projection.rs`'s module doc for the
    /// ellipsoid and zero-datum-shift assumptions this rests on, and for which systems have no
    /// definition to transform through yet. The identity (`from == to`) always succeeds, even for
    /// an EPSG code this registry doesn't carry a usable definition for.
    pub fn transform(&self, from_epsg: u32, to_epsg: u32, p: cadcraft_geom::Vec2) -> Result<cadcraft_geom::Vec2, CrsError> {
        if from_epsg == to_epsg {
            return Ok(p);
        }
        let projection_of = |epsg: u32| -> Result<crate::projection::Projection, CrsError> {
            let def = self.get(epsg).ok_or(CrsError::UnknownEpsg(epsg))?;
            let proj4 = def.proj4.as_deref().ok_or_else(|| CrsError::NoDefinition { epsg, name: def.name.clone() })?;
            crate::projection::Projection::parse(proj4).ok_or_else(|| CrsError::NoDefinition { epsg, name: def.name.clone() })
        };
        let from = projection_of(from_epsg)?;
        let to = projection_of(to_epsg)?;
        Ok(to.from_geographic(from.to_geographic(p)))
    }
}

fn lambert_cc(zone_lat: u32) -> CrsDef {
    // IGN's Lambert CC (Conique Conforme) zones: lat_0 = 42..50 (one per degree of latitude),
    // standard parallels one degree apart centred on it, lon_0 = 3 (Greenwich), x_0 = 1 700 000,
    // y_0 stepping by 1 000 000 per zone starting at 2 200 000 for CC42. EPSG:3942 is CC42,
    // EPSG:3943 CC43, ... EPSG:3950 CC50 - a fixed +8 offset from (zone_lat - 42).
    let epsg = 3942 + (zone_lat - 42);
    let y0 = (zone_lat - 40) * 1_000_000 + 200_000;
    CrsDef {
        epsg,
        name: format!("RGF93 / Lambert-93 CC{zone_lat}"),
        proj4: Some(format!(
            "+proj=lcc +lat_1={a:.2} +lat_2={b:.2} +lat_0={zone_lat} +lon_0=3 +x_0=1700000 +y_0={y0} +ellps=GRS80 +towgs84=0,0,0,0,0,0,0 +units=m +no_defs",
            a = zone_lat as f64 - 0.75,
            b = zone_lat as f64 + 0.75,
        )),
    }
}

fn utm_wgs84(zone: u32, epsg: u32) -> CrsDef {
    CrsDef { epsg, name: format!("WGS 84 / UTM zone {zone}N"), proj4: Some(format!("+proj=utm +zone={zone} +datum=WGS84 +units=m +no_defs")) }
}

fn default_definitions() -> Vec<CrsDef> {
    let mut defs = vec![
        CrsDef {
            epsg: 2154,
            name: "RGF93 / Lambert-93".to_string(),
            proj4: Some(
                "+proj=lcc +lat_1=49 +lat_2=44 +lat_0=46.5 +lon_0=3 +x_0=700000 +y_0=6600000 +ellps=GRS80 +towgs84=0,0,0,0,0,0,0 +units=m +no_defs"
                    .to_string(),
            ),
        },
        CrsDef { epsg: 4326, name: "WGS 84".to_string(), proj4: Some("+proj=longlat +datum=WGS84 +no_defs".to_string()) },
    ];
    defs.extend((42..=50).map(lambert_cc));
    defs.push(utm_wgs84(30, 32630));
    defs.push(utm_wgs84(31, 32631));
    defs.push(utm_wgs84(32, 32632));

    // Legacy Lambert NTF zones: named and numbered correctly, but their datum (NTF, Clarke 1880
    // IGN ellipsoid, Paris prime meridian) is involved enough that we don't ship a guessed PROJ4
    // string here - register_proj4 the real one before relying on these for a transform.
    defs.push(CrsDef { epsg: 27571, name: "NTF (Paris) / Lambert zone I".to_string(), proj4: None });
    defs.push(CrsDef { epsg: 27572, name: "NTF (Paris) / Lambert zone II".to_string(), proj4: None });
    defs.push(CrsDef { epsg: 27573, name: "NTF (Paris) / Lambert zone III".to_string(), proj4: None });
    defs.push(CrsDef { epsg: 27574, name: "NTF (Paris) / Lambert zone IV".to_string(), proj4: None });

    // DOM-TOM: named and correctly numbered, same reasoning as the NTF zones above.
    defs.push(CrsDef { epsg: 5490, name: "RRAF 1991 / UTM zone 20N (Martinique, Guadeloupe)".to_string(), proj4: None });
    defs.push(CrsDef { epsg: 2972, name: "RGFG95 / UTM zone 22N (Guyane)".to_string(), proj4: None });
    defs.push(CrsDef { epsg: 2975, name: "RGR92 / UTM zone 40S (Réunion)".to_string(), proj4: None });

    defs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lambert_93_is_present_with_a_proj4_string() {
        let reg = CrsRegistry::with_defaults();
        let d = reg.get(2154).expect("Lambert-93 should be in the default registry");
        assert_eq!(d.name, "RGF93 / Lambert-93");
        assert!(d.proj4.as_deref().is_some_and(|p| p.contains("lcc")));
    }

    #[test]
    fn all_nine_cc_zones_are_present_and_correctly_numbered() {
        let reg = CrsRegistry::with_defaults();
        for (zone, epsg) in (42..=50).zip(3942..=3950) {
            let d = reg.get(epsg).unwrap_or_else(|| panic!("CC{zone} (EPSG:{epsg}) should be present"));
            assert_eq!(d.name, format!("RGF93 / Lambert-93 CC{zone}"));
            assert!(d.proj4.as_deref().is_some_and(|p| p.contains(&format!("lat_0={zone}"))));
        }
    }

    #[test]
    fn cc43_matches_the_published_ign_parameters() {
        // EPSG:3943 (CC43): lat_1=42.25, lat_2=43.75, lat_0=43, x_0=1700000, y_0=3200000.
        let reg = CrsRegistry::with_defaults();
        let d = reg.get(3943).expect("CC43 should be present");
        let p = d.proj4.as_deref().expect("CC43 should carry a proj4 string");
        assert!(p.contains("lat_1=42.25"), "{p}");
        assert!(p.contains("lat_2=43.75"), "{p}");
        assert!(p.contains("lat_0=43"), "{p}");
        assert!(p.contains("y_0=3200000"), "{p}");
    }

    #[test]
    fn utm_zones_30_to_32_north_are_present() {
        let reg = CrsRegistry::with_defaults();
        assert_eq!(reg.get(32630).map(|d| d.name.as_str()), Some("WGS 84 / UTM zone 30N"));
        assert_eq!(reg.get(32631).map(|d| d.name.as_str()), Some("WGS 84 / UTM zone 31N"));
        assert_eq!(reg.get(32632).map(|d| d.name.as_str()), Some("WGS 84 / UTM zone 32N"));
    }

    #[test]
    fn legacy_and_dom_tom_systems_are_named_but_carry_no_guessed_proj4() {
        let reg = CrsRegistry::with_defaults();
        let lambert_ii = reg.get(27572).expect("Lambert zone II should be present");
        assert_eq!(lambert_ii.proj4, None);
        let guyane = reg.get(2972).expect("Guyane should be present");
        assert_eq!(guyane.name, "RGFG95 / UTM zone 22N (Guyane)");
        assert_eq!(guyane.proj4, None);
    }

    #[test]
    fn unknown_code_is_absent_until_registered() {
        let mut reg = CrsRegistry::with_defaults();
        assert!(reg.get(99999).is_none());
        reg.register_proj4(99999, "Test CRS", "+proj=longlat +datum=WGS84 +no_defs").expect("registering should succeed");
        let d = reg.get(99999).expect("the just-registered CRS should now be present");
        assert_eq!(d.name, "Test CRS");
        assert!(d.proj4.is_some());
    }

    #[test]
    fn register_rejects_epsg_zero_and_empty_name() {
        let mut reg = CrsRegistry::empty();
        assert_eq!(reg.register_proj4(0, "x", "+proj=longlat"), Err(CrsError::InvalidEpsg));
        assert_eq!(reg.register_proj4(1234, "  ", "+proj=longlat"), Err(CrsError::EmptyName));
        assert_eq!(reg.register_named(0, "x"), Err(CrsError::InvalidEpsg));
    }

    #[test]
    fn register_can_replace_an_existing_entry() {
        let mut reg = CrsRegistry::with_defaults();
        reg.register_named(27572, "Lambert II étendu (à vérifier)").expect("replacing should succeed");
        assert_eq!(reg.get(27572).map(|d| d.name.as_str()), Some("Lambert II étendu (à vérifier)"));
    }

    #[test]
    fn find_by_name_is_case_insensitive_and_sorted_by_epsg() {
        let reg = CrsRegistry::with_defaults();
        let hits = reg.find_by_name("lambert");
        assert!(hits.len() >= 10, "expected Lambert-93 plus the nine CC zones, got {}", hits.len());
        assert!(hits.windows(2).all(|w| w[0].epsg <= w[1].epsg));

        let guyane = reg.find_by_name("GUYANE");
        assert_eq!(guyane.len(), 1);
        assert_eq!(guyane[0].epsg, 2972);
    }

    #[test]
    fn all_lists_every_entry_sorted_by_epsg() {
        let reg = CrsRegistry::with_defaults();
        let all = reg.all();
        assert_eq!(all.len(), reg.len());
        assert!(all.windows(2).all(|w| w[0].epsg <= w[1].epsg));
    }

    #[test]
    fn transform_is_the_identity_when_from_and_to_are_the_same() {
        let reg = CrsRegistry::with_defaults();
        let p = cadcraft_geom::Vec2::new(123.0, 456.0);
        // Even for an EPSG code the registry doesn't define at all.
        assert_eq!(reg.transform(999_999, 999_999, p), Ok(p));
    }

    #[test]
    fn transform_round_trips_lambert_93_through_utm_31n() {
        let reg = CrsRegistry::with_defaults();
        let p = cadcraft_geom::Vec2::new(652_000.0, 6_862_000.0); // central Paris, Lambert-93
        let utm = reg.transform(2154, 32631, p).expect("should transform");
        let back = reg.transform(32631, 2154, utm).expect("should transform back");
        // Three round trips through UTM's truncated series (Lambert -> geographic -> UTM ->
        // geographic -> Lambert), so a sub-millimetre residual is expected.
        assert!((back.x - p.x).abs() < 1e-3, "{back:?}");
        assert!((back.y - p.y).abs() < 1e-3, "{back:?}");
    }

    #[test]
    fn transform_reports_an_unknown_epsg_or_a_system_with_no_definition() {
        let reg = CrsRegistry::with_defaults();
        let p = cadcraft_geom::Vec2::ZERO;
        assert_eq!(reg.transform(999_999, 2154, p), Err(CrsError::UnknownEpsg(999_999)));
        // EPSG:27572 (Lambert II étendu) is a named-only entry with no proj4 definition.
        assert_eq!(reg.transform(27572, 2154, p), Err(CrsError::NoDefinition { epsg: 27572, name: "NTF (Paris) / Lambert zone II".to_string() }));
    }
}
