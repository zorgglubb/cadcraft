//! Generic CSV/TXT point-list reader: the common "ID, X, Y, Z[, code]" exports every total
//! station, GPS receiver and survey app can produce, in whichever of the usual delimiters
//! (`;`, `,`, tab, or runs of spaces) the file happens to use, with or without a header row.

use std::collections::HashMap;

/// One point read from a delimited text file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CsvPoint {
    pub id: String,
    pub x: f64,
    pub y: f64,
    pub z: Option<f64>,
    pub code: Option<String>,
}

/// The result of reading a whole points file: the points decoded, plus how many non-empty lines
/// didn't fit the shape (wrong field count, X or Y not a number) and were skipped rather than
/// failing the whole import.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CsvImport {
    pub points: Vec<CsvPoint>,
    pub skipped_lines: usize,
}

#[derive(Clone, Copy)]
enum Delimiter {
    Char(char),
    Whitespace,
}

/// The delimiter an import dialog lets a person pick explicitly, instead of leaving it to
/// [`detect_delimiter`]. `Auto` reproduces [`parse`]'s own detection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CsvDelimiter {
    Auto,
    Comma,
    Semicolon,
    Colon,
    Space,
    Tab,
}

impl Delimiter {
    fn from_choice(choice: CsvDelimiter, sample: &str) -> Self {
        match choice {
            CsvDelimiter::Auto => detect_delimiter(sample),
            CsvDelimiter::Comma => Delimiter::Char(','),
            CsvDelimiter::Semicolon => Delimiter::Char(';'),
            CsvDelimiter::Colon => Delimiter::Char(':'),
            CsvDelimiter::Space => Delimiter::Whitespace,
            CsvDelimiter::Tab => Delimiter::Char('\t'),
        }
    }
}

fn detect_delimiter(sample: &str) -> Delimiter {
    for c in [';', ',', '\t'] {
        if sample.matches(c).count() >= 1 {
            return Delimiter::Char(c);
        }
    }
    Delimiter::Whitespace
}

fn split_fields(line: &str, delim: Delimiter) -> Vec<String> {
    match delim {
        Delimiter::Char(c) => line.split(c).map(|f| f.trim().to_string()).collect(),
        Delimiter::Whitespace => line.split_whitespace().map(str::to_string).collect(),
    }
}

const ID_NAMES: &[&str] = &["id", "pt", "point", "matricule", "nom", "name"];
const X_NAMES: &[&str] = &["x", "east", "easting", "est", "e"];
const Y_NAMES: &[&str] = &["y", "north", "northing", "nord", "n"];
const Z_NAMES: &[&str] = &["z", "alt", "altitude", "elevation", "elev", "h"];
const CODE_NAMES: &[&str] = &["code", "desc", "description", "type"];

/// A header row names at least one recognised column; a plain data row of coordinates won't.
fn looks_like_header(fields: &[String]) -> bool {
    fields.iter().any(|f| {
        let lower = f.to_ascii_lowercase();
        ID_NAMES.contains(&lower.as_str())
            || X_NAMES.contains(&lower.as_str())
            || Y_NAMES.contains(&lower.as_str())
            || Z_NAMES.contains(&lower.as_str())
            || CODE_NAMES.contains(&lower.as_str())
    })
}

fn find_column(fields: &[String], names: &[&str]) -> Option<usize> {
    fields.iter().position(|f| names.contains(&f.to_ascii_lowercase().as_str()))
}

enum Layout {
    Named {
        id: Option<usize>,
        x: usize,
        y: usize,
        z: Option<usize>,
        code: Option<usize>,
    },
    /// No header: columns are, in order, ID, X, Y, Z[, code] - or X, Y[, Z] with no id when
    /// there are too few fields for one.
    Positional,
}

fn named_layout(header: &[String]) -> Option<Layout> {
    let x = find_column(header, X_NAMES)?;
    let y = find_column(header, Y_NAMES)?;
    Some(Layout::Named { id: find_column(header, ID_NAMES), x, y, z: find_column(header, Z_NAMES), code: find_column(header, CODE_NAMES) })
}

fn parse_row(fields: &[String], layout: &Layout, row_number: usize) -> Option<CsvPoint> {
    match layout {
        Layout::Named { id, x, y, z, code } => {
            let xv: f64 = fields.get(*x)?.parse().ok()?;
            let yv: f64 = fields.get(*y)?.parse().ok()?;
            if !xv.is_finite() || !yv.is_finite() {
                return None;
            }
            let zv = z.and_then(|i| fields.get(i)).and_then(|s| s.parse::<f64>().ok()).filter(|v| v.is_finite());
            let idv = id.and_then(|i| fields.get(i)).filter(|s| !s.is_empty()).cloned().unwrap_or_else(|| row_number.to_string());
            let codev = code.and_then(|i| fields.get(i)).filter(|s| !s.is_empty()).cloned();
            Some(CsvPoint { id: idv, x: xv, y: yv, z: zv, code: codev })
        }
        Layout::Positional => match fields.len() {
            0..=1 => None,
            2 => {
                let x: f64 = fields[0].parse().ok()?;
                let y: f64 = fields[1].parse().ok()?;
                (x.is_finite() && y.is_finite()).then_some(CsvPoint { id: row_number.to_string(), x, y, z: None, code: None })
            }
            3 => {
                // Ambiguous: could be X,Y,Z or ID,X,Y. An id column is never purely numeric in
                // practice for this shape, so try ID,X,Y first and fall back to X,Y,Z.
                if let (Ok(x), Ok(y)) = (fields[1].parse::<f64>(), fields[2].parse::<f64>())
                    && x.is_finite()
                    && y.is_finite()
                    && fields[0].parse::<f64>().is_err()
                {
                    return Some(CsvPoint { id: fields[0].clone(), x, y, z: None, code: None });
                }
                let x: f64 = fields[0].parse().ok()?;
                let y: f64 = fields[1].parse().ok()?;
                let z = fields[2].parse::<f64>().ok().filter(|v| v.is_finite());
                (x.is_finite() && y.is_finite()).then_some(CsvPoint { id: row_number.to_string(), x, y, z, code: None })
            }
            _ => {
                let x: f64 = fields.get(1)?.parse().ok()?;
                let y: f64 = fields.get(2)?.parse().ok()?;
                if !x.is_finite() || !y.is_finite() {
                    return None;
                }
                let z = fields.get(3).and_then(|s| s.parse::<f64>().ok()).filter(|v| v.is_finite());
                let code = fields.get(4).filter(|s| !s.is_empty()).cloned();
                Some(CsvPoint { id: fields[0].clone(), x, y, z, code })
            }
        },
    }
}

/// Reads a whole delimited points file: detects the delimiter from the first non-empty line,
/// detects a header by column names (falling back to the positional ID/X/Y/Z[/code] convention
/// when none is recognised), and decodes every remaining line independently - a malformed row
/// (wrong field count, non-numeric X or Y) is counted in [`CsvImport::skipped_lines`] rather than
/// failing the whole file.
pub fn parse(text: &str) -> CsvImport {
    parse_with(text, CsvDelimiter::Auto)
}

/// Same as [`parse`], but with the delimiter given explicitly rather than detected - what an
/// import dialog uses once the person has picked one (or left it on `Auto`).
pub fn parse_with(text: &str, delimiter: CsvDelimiter) -> CsvImport {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let Some(first) = lines.next() else {
        return CsvImport::default();
    };
    let delim = Delimiter::from_choice(delimiter, first);
    let first_fields = split_fields(first, delim);

    let (layout, start_with_first_as_data) = if looks_like_header(&first_fields) {
        (named_layout(&first_fields).unwrap_or(Layout::Positional), false)
    } else {
        (Layout::Positional, true)
    };

    let mut points = Vec::new();
    let mut skipped = 0usize;
    let mut row_number = 1usize;

    let mut handle = |fields: Vec<String>, row_number: &mut usize| {
        *row_number += 1;
        match parse_row(&fields, &layout, *row_number) {
            Some(p) => points.push(p),
            None => skipped += 1,
        }
    };

    if start_with_first_as_data {
        handle(first_fields, &mut row_number);
    }
    for line in lines {
        handle(split_fields(line, delim), &mut row_number);
    }

    CsvImport { points, skipped_lines: skipped }
}

/// The first `max_lines` non-empty lines, each split into fields by `delimiter` (or detected,
/// for `Auto`, from the first one) - a raw preview an import dialog can show and recompute as
/// the person changes the delimiter, without running the full [`parse_with`] (column roles,
/// header detection) on every keystroke.
pub fn preview(text: &str, delimiter: CsvDelimiter, max_lines: usize) -> Vec<Vec<String>> {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let Some(first) = lines.next() else {
        return Vec::new();
    };
    let delim = Delimiter::from_choice(delimiter, first);
    std::iter::once(first).chain(lines).take(max_lines.max(1)).map(|l| split_fields(l, delim)).collect()
}

/// Groups points by code, for callers that want to reconstruct geometry from coded field codes
/// (walls, fences, breaklines) rather than inserting bare points.
pub fn by_code(import: &CsvImport) -> HashMap<String, Vec<&CsvPoint>> {
    let mut map: HashMap<String, Vec<&CsvPoint>> = HashMap::new();
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
    fn semicolon_without_code() {
        let import = parse("STE18;1454924.489;8226052.972;25.380");
        assert_eq!(import.points.len(), 1, "skipped {}", import.skipped_lines);
        let p = &import.points[0];
        assert_eq!(p.id, "STE18");
        assert!((p.x - 1454924.489).abs() < 1e-6);
        assert!((p.y - 8226052.972).abs() < 1e-6);
        assert!((p.z.unwrap_or(f64::NAN) - 25.380).abs() < 1e-6);
        assert_eq!(p.code, None);
    }

    #[test]
    fn parse_with_an_explicit_delimiter_ignores_what_auto_would_have_picked() {
        // A semicolon-delimited line, parsed as comma-delimited: the whole line is one field,
        // which isn't a valid ID/X/Y row, so it's skipped rather than silently using `;`.
        let import = parse_with("STE18;1454924.489;8226052.972;25.380", CsvDelimiter::Comma);
        assert_eq!(import.points.len(), 0);
        assert_eq!(import.skipped_lines, 1);
    }

    #[test]
    fn preview_splits_the_first_lines_by_the_chosen_delimiter() {
        let text = "ID;X;Y;Z\nP1;100.0;200.0;10.0\nP2;110.0;210.0;11.0\n";
        let rows = preview(text, CsvDelimiter::Semicolon, 10);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0], vec!["ID", "X", "Y", "Z"]);
        assert_eq!(rows[1], vec!["P1", "100.0", "200.0", "10.0"]);
    }

    #[test]
    fn preview_caps_at_max_lines_and_respects_auto() {
        let text = (1..=15).map(|i| format!("P{i},{i}.0,{i}.0")).collect::<Vec<_>>().join("\n");
        let rows = preview(&text, CsvDelimiter::Auto, 10);
        assert_eq!(rows.len(), 10);
        assert_eq!(rows[0].len(), 3, "comma should have been auto-detected");
    }

    #[test]
    fn comma_separated() {
        let import = parse("ST5,1454886.654,8226097.003,25.980");
        assert_eq!(import.points.len(), 1);
        assert_eq!(import.points[0].id, "ST5");
    }

    #[test]
    fn tab_or_space_separated() {
        let import = parse("A1\t471100.000\t6800200.000\t55.000");
        assert_eq!(import.points.len(), 1);
        assert_eq!(import.points[0].id, "A1");
        let import2 = parse("REP1 471150.000 6800250.000 54.500");
        assert_eq!(import2.points.len(), 1);
        assert_eq!(import2.points[0].id, "REP1");
    }

    #[test]
    fn header_with_named_columns_any_order() {
        let text = "Code;Y;X;Z;ID\nMUR;8226052.972;1454924.489;25.380;P1";
        let import = parse(text);
        assert_eq!(import.points.len(), 1, "skipped {}", import.skipped_lines);
        let p = &import.points[0];
        assert_eq!(p.id, "P1");
        assert!((p.x - 1454924.489).abs() < 1e-6);
        assert!((p.y - 8226052.972).abs() < 1e-6);
        assert_eq!(p.code.as_deref(), Some("MUR"));
    }

    #[test]
    fn header_in_english_with_easting_northing() {
        let text = "id,easting,northing,elevation\nP2,500.0,600.0,12.5";
        let import = parse(text);
        assert_eq!(import.points.len(), 1);
        let p = &import.points[0];
        assert_eq!(p.id, "P2");
        assert!((p.x - 500.0).abs() < 1e-9);
        assert!((p.y - 600.0).abs() < 1e-9);
        assert!((p.z.unwrap_or(f64::NAN) - 12.5).abs() < 1e-9);
    }

    #[test]
    fn x_y_only_no_id_no_header() {
        let import = parse("100.0,200.0\n150.0,250.0");
        assert_eq!(import.points.len(), 2);
        assert!((import.points[1].x - 150.0).abs() < 1e-9);
        assert_ne!(import.points[0].id, import.points[1].id);
    }

    #[test]
    fn malformed_rows_are_skipped_not_fatal() {
        let text = "ID;X;Y;Z\nP1;100.0;200.0;10.0\nP2;not-a-number;200.0;10.0\nP3;110.0;210.0;11.0";
        let import = parse(text);
        assert_eq!(import.points.len(), 2, "skipped {}", import.skipped_lines);
        assert_eq!(import.skipped_lines, 1);
    }

    #[test]
    fn empty_input_yields_no_points() {
        let import = parse("");
        assert!(import.points.is_empty());
        assert_eq!(import.skipped_lines, 0);
    }

    #[test]
    fn blank_lines_between_rows_are_ignored() {
        let text = "P1,100.0,200.0\n\n\nP2,110.0,210.0";
        let import = parse(text);
        assert_eq!(import.points.len(), 2);
    }
}
