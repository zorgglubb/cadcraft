//! Leica GSI-8/16 total-station carnet reader.
//!
//! GSI ("Geo Serial Interface") is Leica's line-oriented export format: each line holds one
//! point as a sequence of whitespace-separated *words*. A word is `WWIIUUS` followed by a
//! value: a 2-digit Word Index (what the value means), 2 digits of info, 2 of unit, a sign
//! character always at position 6, then the digits of the value. The Word Index — not the unit
//! field — decides how to scale the value back to a real number (gon, metres); this reader
//! follows the same mapping GeoMax and Leica exports both use in practice, picking the
//! horizontal-distance divisor (`÷1 000` vs `÷10 000`) per file from whether it also carries
//! GeoMax-style station words (WI 84/85/86).
//!
//! Malformed tokens and unrecognised Word Indices are skipped, never rejected: a carnet from
//! the field is never trusted input, and one bad line must not lose the rest of the file.

use std::collections::HashMap;

/// One point decoded from a GSI carnet line: whichever fields that line's words carried.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GsiPoint {
    pub id: String,
    pub code: Option<String>,
    /// Horizontal circle reading, gon.
    pub hz_gon: Option<f64>,
    /// Zenith angle, gon (100 gon = horizontal).
    pub vz_gon: Option<f64>,
    /// Slope (inclined) distance, metres.
    pub slope_distance: Option<f64>,
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub z: Option<f64>,
    /// Target height, metres.
    pub target_height: Option<f64>,
    /// Instrument height, metres.
    pub instrument_height: Option<f64>,
}

/// The result of reading a whole GSI file: the points it could make sense of, plus how many
/// non-empty lines were skipped outright (station headers, or lines with no usable Word Index).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GsiImport {
    pub points: Vec<GsiPoint>,
    pub skipped_lines: usize,
}

#[derive(Clone, Copy, PartialEq)]
enum Field {
    Id,
    Code,
    Hz,
    Vz,
    Dist,
    X,
    Y,
    Z,
    TargetHeight,
    InstrumentHeight,
    /// A known, deliberately ignored Word Index (station metadata, PPM, signal status).
    Ignored,
}

/// Word Index to field + divisor. The divisor for `31`/`32` (slope/horizontal distance) is
/// decided per file (see [`parse`]), not fixed here.
fn word_map(wi: &str) -> Option<(Field, f64)> {
    Some(match wi {
        "11" => (Field::Id, 1.0),
        "21" => (Field::Hz, 100_000.0),
        "22" => (Field::Vz, 100_000.0),
        "31" | "32" => (Field::Dist, 10_000.0), // overridden per-file below
        "33" => (Field::Ignored, 1.0),
        "41" => (Field::Code, 1.0),
        "51" | "58" => (Field::Ignored, 1.0),
        "71" => (Field::Code, 1.0),
        "81" | "84" => (Field::X, 1_000.0),
        "82" | "85" => (Field::Y, 1_000.0),
        "83" | "86" => (Field::Z, 1_000.0),
        "87" => (Field::TargetHeight, 1_000.0),
        "88" => (Field::InstrumentHeight, 1_000.0),
        "42" | "43" | "44" | "45" | "46" | "47" | "48" | "49" => (Field::Ignored, 1.0),
        _ => return None,
    })
}

fn is_text_field(f: Field) -> bool {
    matches!(f, Field::Id | Field::Code)
}

struct DecodedToken {
    wi: String,
    field: Field,
    value: Option<f64>,
    raw: String,
}

/// Decodes one GSI word. Never panics: any shape that doesn't fit (too short, no digits, no
/// sign) is simply `None`, leaving the token to be skipped by the caller.
fn decode_token(token: &[char], dist_divisor: f64) -> Option<DecodedToken> {
    if token.len() < 8 {
        return None;
    }
    let wi: String = token.get(0..2)?.iter().collect();
    if !wi.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let (field, mut divisor) = word_map(&wi)?;
    if wi == "31" || wi == "32" {
        divisor = dist_divisor;
    }

    // The sign is documented at position 6; a handful of real exports shift by a char or two,
    // so scan a small window rather than failing outright.
    let sign_pos = (5..token.len().min(9)).find(|&i| token.get(i) == Some(&'+') || token.get(i) == Some(&'-'))?;
    let sign = *token.get(sign_pos)?;
    let raw: String = token.get(sign_pos + 1..)?.iter().collect();
    if raw.is_empty() {
        return None;
    }

    if is_text_field(field) {
        let cleaned = raw.trim_start_matches('0');
        let cleaned = if cleaned.is_empty() { "0" } else { cleaned };
        return Some(DecodedToken { wi, field, value: None, raw: cleaned.to_string() });
    }
    if field == Field::Ignored {
        return Some(DecodedToken { wi, field, value: None, raw });
    }

    let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() || digits.len() > 18 {
        return None;
    }
    let magnitude: f64 = digits.parse().ok()?;
    let signed = if sign == '-' { -magnitude } else { magnitude };
    Some(DecodedToken { wi, field, value: Some(signed / divisor), raw })
}

/// A valid GSI token looks enough like one (2-digit Word Index, a sign in the expected window)
/// to be treated as a word rather than a stray continuation of the previous one.
fn looks_like_gsi_token(token: &[char]) -> bool {
    if token.len() < 8 {
        return false;
    }
    if !token.get(0..2).is_some_and(|w| w.iter().all(|c| c.is_ascii_digit())) {
        return false;
    }
    (5..token.len().min(9)).any(|i| token.get(i) == Some(&'+') || token.get(i) == Some(&'-'))
}

fn parse_line(line: &str, dist_divisor: f64) -> Option<GsiPoint> {
    let tokens: Vec<Vec<char>> = line.split_whitespace().map(|t| t.chars().collect()).collect();
    let first_wi: Option<String> = tokens.first().and_then(|t| t.get(0..2)).map(|c| c.iter().collect());
    // A line led by a code word (WI 41) is a station/header record in practice, not a point.
    if first_wi.as_deref() == Some("41") {
        return None;
    }

    let mut point = GsiPoint::default();
    let mut skip_next = false;
    for (i, token) in tokens.iter().enumerate() {
        if skip_next {
            skip_next = false;
            continue;
        }
        let Some(decoded) = decode_token(token, dist_divisor) else { continue };
        match decoded.field {
            Field::Id => {
                let mut value = decoded.raw;
                if let Some(next) = tokens.get(i + 1)
                    && !looks_like_gsi_token(next)
                {
                    value.push(' ');
                    value.extend(next.iter());
                    skip_next = true;
                }
                point.id = value;
            }
            Field::Code => {
                let mut value = decoded.raw;
                if let Some(next) = tokens.get(i + 1)
                    && !value.is_empty()
                    && !looks_like_gsi_token(next)
                {
                    value.push(' ');
                    value.extend(next.iter());
                    skip_next = true;
                }
                if !value.is_empty() {
                    point.code = Some(value);
                }
            }
            Field::Hz => point.hz_gon = decoded.value,
            Field::Vz => point.vz_gon = decoded.value,
            Field::Dist => {
                // WI 31 (slope) takes priority over WI 32 if both appear on one line.
                if decoded.wi == "32" && point.slope_distance.is_some() {
                    // already set from a prior WI 31 word
                } else if decoded.value.is_some() {
                    point.slope_distance = decoded.value;
                }
            }
            Field::X => point.x = decoded.value,
            Field::Y => point.y = decoded.value,
            Field::Z => point.z = decoded.value,
            Field::TargetHeight => point.target_height = decoded.value,
            Field::InstrumentHeight => point.instrument_height = decoded.value,
            Field::Ignored => {}
        }
    }

    (!point.id.is_empty()).then_some(point)
}

/// Reads a whole GSI carnet (GSI-8 or GSI-16, auto-detected per line by word length; the
/// horizontal-distance divisor is picked once for the file from whether it carries GeoMax-style
/// station words, WI 84/85/86). Lines are read independently: a malformed or unrecognised one is
/// counted in [`GsiImport::skipped_lines`] rather than failing the whole import.
pub fn parse(text: &str) -> GsiImport {
    let is_geomax = text.lines().any(|line| {
        line.split_whitespace().any(|tok| {
            let chars: Vec<char> = tok.chars().collect();
            matches!(chars.get(0..2).map(|c| c.iter().collect::<String>()).as_deref(), Some("84") | Some("85") | Some("86"))
        })
    });
    let dist_divisor = if is_geomax { 1_000.0 } else { 10_000.0 };

    let mut points = Vec::new();
    let mut skipped = 0usize;
    for raw_line in text.lines() {
        let line = raw_line.trim_start_matches('*').trim();
        if line.is_empty() {
            continue;
        }
        match parse_line(line, dist_divisor) {
            Some(p) => points.push(p),
            None => skipped += 1,
        }
    }
    GsiImport { points, skipped_lines: skipped }
}

/// Groups a flat GSI import into carnets per occupied station: every point up to (but not
/// including) the next point whose own words describe a station setup (none here — GSI alone
/// doesn't mark that) is of no use without an explicit station id, so callers that need
/// per-station grouping should split by blank lines or station comments themselves; this keeps
/// `parse` a pure, line-for-line decode.
pub fn by_code(import: &GsiImport) -> HashMap<String, Vec<&GsiPoint>> {
    let mut map: HashMap<String, Vec<&GsiPoint>> = HashMap::new();
    for p in &import.points {
        if let Some(code) = &p.code {
            map.entry(code.clone()).or_default().push(p);
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_simple_leica_point() {
        // Hand-built GSI-16 style line: ID P001, Hz 12.34 gon, Vz 100.00 gon, slope dist 12.3456 m, code S1.
        // Hz/Vz words carry value * 100000 (5 decimals): 12.34 -> 1234000, 100.0 -> 10000000.
        let line = "110001+0000P001 21.324+01234000 22.324+10000000 31..00+00123456 41..00+0000S1";
        let import = parse(line);
        assert_eq!(import.points.len(), 1, "skipped {} lines", import.skipped_lines);
        let p = &import.points[0];
        assert_eq!(p.id, "P001");
        assert!((p.hz_gon.unwrap_or(f64::NAN) - 12.34).abs() < 1e-9);
        assert!((p.vz_gon.unwrap_or(f64::NAN) - 100.0).abs() < 1e-9);
        assert!((p.slope_distance.unwrap_or(f64::NAN) - 12.3456).abs() < 1e-9);
        assert_eq!(p.code.as_deref(), Some("S1"));
    }

    #[test]
    fn decodes_xyz_coordinates() {
        let line = "110001+0000P002 81..00+00471100000 82..00+00680020000 83..00+00000055000";
        let import = parse(line);
        assert_eq!(import.points.len(), 1, "skipped {} lines", import.skipped_lines);
        let p = &import.points[0];
        assert!((p.x.unwrap_or(f64::NAN) - 471100.0).abs() < 1e-6);
        assert!((p.y.unwrap_or(f64::NAN) - 680020.0).abs() < 1e-6);
        assert!((p.z.unwrap_or(f64::NAN) - 55.0).abs() < 1e-6);
    }

    #[test]
    fn negative_sign_is_applied() {
        let line = "110001+0000P003 83..00-00000012000";
        let import = parse(line);
        assert_eq!(import.points.len(), 1);
        assert!((import.points[0].z.unwrap_or(f64::NAN) - -12.0).abs() < 1e-6);
    }

    #[test]
    fn station_header_line_is_skipped_not_kept_as_a_point() {
        let line = "41..00+0000STN1 84..00+00471100000 85..00+00680020000";
        let import = parse(line);
        assert!(import.points.is_empty());
        assert_eq!(import.skipped_lines, 1);
    }

    #[test]
    fn empty_and_star_prefixed_lines_are_handled() {
        let text = "\n   \n*110001+0000P004 21.324+00001000 22.324+00010000\n";
        let import = parse(text);
        assert_eq!(import.points.len(), 1);
        assert_eq!(import.points[0].id, "P004");
    }

    #[test]
    fn garbage_tokens_are_skipped_without_panicking() {
        let text = "not-a-gsi-line at all\n110001+0000P005 ??\n";
        let import = parse(text);
        // The second line still yields a point from its valid ID word. A trailing token that
        // doesn't itself look like a GSI word is folded into the id as a continuation (real
        // carnets use this for free-text ids), rather than causing a crash or a dropped point.
        // The first line has no valid word at all, so it is only counted as skipped.
        assert_eq!(import.points.len(), 1);
        assert_eq!(import.points[0].id, "P005 ??");
        assert_eq!(import.skipped_lines, 1);
    }

    #[test]
    fn geomax_station_words_switch_the_distance_divisor() {
        // A file carrying WI 84/85/86 anywhere switches WI 31/32 to ÷1000 instead of ÷10000.
        let text = "110001+0000STA1 84..00+00471100000 85..00+00680020000 86..00+00000050000\n\
                    110001+0000P006 31..00+00012345\n";
        let import = parse(text);
        let p6 = import.points.iter().find(|p| p.id == "P006").expect("P006 should be present");
        assert!((p6.slope_distance.unwrap_or(f64::NAN) - 12.345).abs() < 1e-6, "dist = {:?}", p6.slope_distance);
    }
}
