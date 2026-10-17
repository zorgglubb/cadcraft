//! EDIGEO's plain-text ("ASCII profile") serialization - the one real French cadastral
//! PCI-Vecteur exports actually use (confirmed against a real export from the Calvados land
//! registry while building this reader), as opposed to the binary ISO 8211 profile
//! [`crate::edigeo`] inspects.
//!
//! Every physical line is self-describing and fixed in shape: a 4-character tag, a 1-character
//! type indicator (`A` text, `N` number, `R` real, `T` text/date, `C` composite/list, or a
//! literal space for a couple of header fields), a 2-digit decimal length, a colon, then exactly
//! that many characters of value, then CRLF:
//!
//! ```text
//! RIDSA07:Noeud_1
//! CORCC23:+503172.97;+6925499.98;
//! ```
//!
//! A blank line ends a "field group". A record begins at the field group whose first field is
//! tagged `RTYS` (the record's type, e.g. `PNO`, `PAR`, `PFE`, `FEA`, `LNK`) immediately followed
//! by `RIDS` (the record's id); every following field group, up to the next `RTYS` group,
//! belongs to that record. The whole file is bounded by a `BOMT ...` / `EOMT 00:` pair.

use cadcraft_geom::Vec2;

/// Hard caps against a hostile or corrupt file.
const MAX_LINES: usize = 5_000_000;
const MAX_RECORDS: usize = 2_000_000;

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum EdigeoTextError {
    #[error("the file has {0} lines, rejecting as hostile input")]
    TooManyLines(usize),
    #[error("the file has {0} records, rejecting as hostile input")]
    TooManyRecords(usize),
}

/// One `TAG` `:` `value` line, already stripped of its length prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextField {
    pub tag: String,
    pub type_char: char,
    pub value: String,
}

/// One record: the field group naming its type and id, plus every field of every field group
/// that follows it (field-group boundaries are not kept - callers read fields by tag).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextRecord {
    pub rtype: String,
    pub rid: String,
    pub fields: Vec<TextField>,
}

/// A whole parsed file: every record found, plus how many lines didn't fit the `TAGCLL:value`
/// shape and were skipped (a byte-corrupt line is not fatal to the rest of the file).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextFile {
    pub records: Vec<TextRecord>,
    pub skipped_lines: usize,
}

fn decode_latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

/// Parses one `TAGCLL:value` line. `None` if the line is too short or its length byte(s) aren't
/// digits - the caller counts that as a skipped line rather than failing the whole file.
fn parse_line(line: &str) -> Option<TextField> {
    let bytes = line.as_bytes();
    if bytes.len() < 8 || bytes[7] != b':' {
        return None;
    }
    let tag = line.get(0..4)?.to_string();
    let type_char = line[4..5].chars().next()?;
    // The 2-digit length prefix is read for shape validation elsewhere but not enforced here:
    // trusting "everything after the colon" is more forgiving of a length that's off by one in
    // a hand-edited or slightly non-conformant file, without losing any real value text.
    let _len: usize = line.get(5..7)?.parse().ok()?;
    let value = line.get(8..)?.to_string();
    Some(TextField { tag, type_char, value })
}

/// Reads a whole EDIGEO ASCII-profile byte buffer into its records.
pub fn parse(bytes: &[u8]) -> Result<TextFile, EdigeoTextError> {
    let text = decode_latin1(bytes);
    let lines: Vec<&str> = text.split("\r\n").flat_map(|l| l.split('\n')).collect();
    if lines.len() > MAX_LINES {
        return Err(EdigeoTextError::TooManyLines(lines.len()));
    }

    let mut file = TextFile::default();
    let mut current: Option<TextRecord> = None;
    let mut pending_group: Vec<TextField> = Vec::new();

    let flush_group = |current: &mut Option<TextRecord>, group: &mut Vec<TextField>, file: &mut TextFile| {
        if group.is_empty() {
            return;
        }
        if group.first().is_some_and(|f| f.tag == "RTYS") {
            if let Some(rec) = current.take() {
                file.records.push(rec);
            }
            let rtype = group.first().map(|f| f.value.clone()).unwrap_or_default();
            let rid = group.get(1).filter(|f| f.tag == "RIDS").map(|f| f.value.clone()).unwrap_or_default();
            *current = Some(TextRecord { rtype, rid, fields: Vec::new() });
        } else if let Some(rec) = current.as_mut() {
            rec.fields.append(group);
        }
        group.clear();
    };

    for line in lines {
        if line.is_empty() {
            flush_group(&mut current, &mut pending_group, &mut file);
            continue;
        }
        match parse_line(line) {
            Some(field) => pending_group.push(field),
            None => file.skipped_lines += 1,
        }
    }
    flush_group(&mut current, &mut pending_group, &mut file);
    if let Some(rec) = current {
        file.records.push(rec);
    }

    if file.records.len() > MAX_RECORDS {
        return Err(EdigeoTextError::TooManyRecords(file.records.len()));
    }
    Ok(file)
}

/// Parses a `CORC` value such as `"+503172.97;+6925499.98;"` into a point, ignoring any third
/// (Z) coordinate - this reader only reconstructs 2D parcel boundaries.
pub fn parse_coord(value: &str) -> Option<Vec2> {
    let mut parts = value.split(';').filter(|s| !s.is_empty());
    let x: f64 = parts.next()?.parse().ok()?;
    let y: f64 = parts.next()?.parse().ok()?;
    (x.is_finite() && y.is_finite()).then_some(Vec2::new(x, y))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_point_and_an_arc_record() {
        let text = "BOMT 12:ED0A01T1.VEC\r\n\
                     CSET 03:IRV\r\n\
                     \r\n\
                     RTYSA03:PNO\r\n\
                     RIDSA07:Noeud_1\r\n\
                     \r\n\
                     TYPSN01:1\r\n\
                     CORCC23:+503172.97;+6925499.98;\r\n\
                     ATCSN01:0\r\n\
                     \r\n\
                     RTYSA03:PAR\r\n\
                     RIDSA05:Arc_2\r\n\
                     \r\n\
                     PTCSN01:2\r\n\
                     CORCC23:+503172.97;+6925499.98;\r\n\
                     CORCC23:+503263.90;+6925492.83;\r\n\
                     \r\n\
                     EOMT 00:\r\n";
        let file = parse(text.as_bytes()).expect("a well-formed file should parse");
        assert_eq!(file.skipped_lines, 0);
        assert_eq!(file.records.len(), 2);

        let pno = &file.records[0];
        assert_eq!(pno.rtype, "PNO");
        assert_eq!(pno.rid, "Noeud_1");
        let cor = pno.fields.iter().find(|f| f.tag == "CORC").expect("CORC field");
        assert_eq!(parse_coord(&cor.value), Some(Vec2::new(503172.97, 6925499.98)));

        let par = &file.records[1];
        assert_eq!(par.rtype, "PAR");
        assert_eq!(par.rid, "Arc_2");
        let coords: Vec<Vec2> = par.fields.iter().filter(|f| f.tag == "CORC").filter_map(|f| parse_coord(&f.value)).collect();
        assert_eq!(coords.len(), 2);
    }

    #[test]
    fn a_malformed_line_is_skipped_not_fatal() {
        let text = "RTYSA03:PNO\r\nRIDSA07:Noeud_1\r\n\r\nnot a valid line at all\r\nATCSN01:0\r\n";
        let file = parse(text.as_bytes()).expect("should parse despite the bad line");
        assert_eq!(file.skipped_lines, 1);
        assert_eq!(file.records.len(), 1);
        assert_eq!(file.records[0].fields.len(), 1);
    }

    #[test]
    fn empty_input_yields_no_records() {
        let file = parse(b"").expect("empty is not an error");
        assert_eq!(file.records.len(), 0);
    }

    #[test]
    fn a_record_with_no_trailing_blank_line_before_eof_is_still_captured() {
        let text = "RTYSA03:PNO\r\nRIDSA07:Noeud_1\r\n\r\nTYPSN01:1\r\n";
        let file = parse(text.as_bytes()).expect("should parse");
        assert_eq!(file.records.len(), 1);
        assert_eq!(file.records[0].fields.len(), 1);
    }

    #[test]
    fn parse_coord_rejects_garbage_without_panicking() {
        assert_eq!(parse_coord("not;a;number"), None);
        assert_eq!(parse_coord(""), None);
        assert_eq!(parse_coord("+1.0"), None);
    }
}
