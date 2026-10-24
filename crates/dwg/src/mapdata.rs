//! Reads back Autodesk Map 3D / Civil 3D "Object Data" (named per-entity attribute tables, as
//! shown in AutoCAD's own Properties panel under "Attributs") from a DWG's in-memory model.
//!
//! This is a different, richer mechanism than plain XDATA (see `dxf_ext`/`palettes.rs`'s
//! "External Data"): the schema (table and field names) and the per-entity values are kept in
//! custom objects (`IRD_DSC_RECORD`/`IRD_OBJ_RECORD`) that `acadrust` doesn't model - it falls
//! back to [`acadrust::objects::ObjectType::Unknown`] for them, but keeps their raw DWG bytes
//! (`raw_dwg_data`), so nothing is lost at the `acadrust` read step.
//!
//! The byte layout of these objects isn't in any public DXF/DWG spec (it's an Autodesk Map
//! proprietary extension); this reads it purely by pattern-matching bytes observed in real
//! sample files (clean-room: no Autodesk documentation or software was consulted), and only
//! recovers what that pattern-matching finds: text (`ClassChar`) field values matched to a table
//! whose decoded name matches the entity's own layer, the table's first `ClassLong` field (in
//! every real table seen so far, that field is `FeatId`) from a fixed byte offset in the record,
//! and the table's first `ClassReal` field - both numeric kinds confirmed against a real file's
//! own Properties panel (`FeatId = 3`, `Hauteur_GN = 8.042` on the same object).
//!
//! The `ClassReal` decoding is weaker than the other two and is approximate on purpose - see
//! [`real_field`]'s doc comment for why - unlike the rest of this module, which never shows a
//! value it isn't confident is exactly right.
//!
//! Failsafe throughout: a DWG that doesn't use this feature, or one whose bytes don't match the
//! expected pattern, simply yields no attributes for the affected entities - never an error, let
//! alone a panic.

use std::collections::HashMap;

use acadrust::document::CadDocument;
use acadrust::objects::ObjectType;

/// Per-entity Object Data: table name plus the text fields recovered for that entity, keyed by
/// the entity's handle (`acadrust::Handle::value()`, which is also the handle CADCraft's own DXF
/// reader assigns the entity from the regenerated DXF's group 5).
pub type MapAttributes = HashMap<u64, (String, Vec<(String, String)>)>;

/// A text run is kept only from this length up, to cut down on short false positives from
/// unrelated binary bytes that happen to decode to a printable character.
const MIN_RUN_LEN: usize = 3;

/// Most bytes worth scanning in one record; real records are well under this, and it bounds the
/// (data-derived) size of the scan for a hostile/damaged file.
const MAX_SCAN_BYTES: usize = 1 << 16;

/// How many consecutive undecodable bytes a run tolerates before ending for good (see below).
const MAX_GAP: usize = 1;

/// Decodes `data[start..]` at a fixed `stride` (1 or 2 bytes per character), starting the first
/// character at `start`, for as long as each byte decodes to a printable ASCII character: a byte
/// `b` decodes to character `b / 2` when `b` is even and `b / 2` falls in `0x20..0x7f`
/// (reverse-engineered from sample files; see the module doc).
///
/// This encoding has no representation at all for a non-ASCII (e.g. accented Latin-1) character:
/// doubling its code point either doesn't fit the `0x20..0x7f` window or isn't even, so the byte
/// for that one character fails the check above while every ASCII byte around it keeps passing.
/// Without this special case, that single bad byte used to end the run there (e.g. "tranchée"
/// stopping after "tranch"), handing the rest of the word to a *different*, unrelated run that
/// happens to start past it - which could then look longer/better-placed to `text_runs`'s
/// dedup and get reported standalone (e.g. a stray "ie" from further on), silently welding onto
/// what looks like one word ("tranchie") even though the two runs share no real relationship.
/// Instead, up to [`MAX_GAP`] consecutive bad bytes are tolerated *mid-run* (never at the very
/// start - a run has to begin with a real decoded character, or every starting offset in the
/// scan would spawn a spurious placeholder-prefixed run that out-sorts and shadows the real,
/// later run it was supposed to merely extend): each is emitted as a placeholder `?` (there's no
/// way to recover what the real character was - see the module doc) and scanning continues, only
/// ending the run for good once a real gap is found. A gap only bridges when every byte in it is
/// odd - a byte that can never decode at any value (see `decode_byte_at`), standing in for a
/// character (such as an accented letter) this encoding has no slot for at all. An even byte
/// that's merely out of the printable range (e.g. 0x00) is a value the encoding is able to
/// represent and chose not to - most likely a genuine structural separator between fields, not a
/// missing letter - so that still ends the run normally. This never changes the decoding of a
/// byte that already passes the check, so plain ASCII text decodes exactly as before.
fn run_at(data: &[u8], start: usize, stride: usize) -> String {
    let mut out = String::new();
    let mut i = start;
    while i < data.len() {
        match decode_byte_at(data, i) {
            Some(c) => {
                out.push(c as char);
                i = i.saturating_add(stride);
            }
            None => {
                if out.is_empty() {
                    // Never bridge at the very start of a run - see the doc comment above.
                    break;
                }
                // Peek past up to MAX_GAP consecutive odd bytes: only commit to the
                // placeholder(s) if a real character resumes right after, so a run that's
                // genuinely just ending here (the common case - most bytes past a real run are
                // unrelated binary noise) isn't padded with bogus trailing '?'s.
                let mut gap = 0usize;
                let mut j = i;
                let mut bridged = false;
                while gap < MAX_GAP {
                    match data.get(j) {
                        Some(&b) if b % 2 != 0 => {}
                        _ => break,
                    }
                    j = j.saturating_add(stride);
                    gap += 1;
                    if decode_byte_at(data, j).is_some() {
                        bridged = true;
                        break;
                    }
                }
                if bridged {
                    for _ in 0..gap {
                        out.push('?');
                    }
                    i = j;
                } else {
                    break;
                }
            }
        }
        if out.len() >= MAX_SCAN_BYTES {
            break;
        }
    }
    out
}

/// A single byte's decoded character, when it's a valid one (see [`run_at`]'s doc comment).
fn decode_byte_at(data: &[u8], i: usize) -> Option<u8> {
    let &b = data.get(i)?;
    if b % 2 != 0 {
        return None;
    }
    let c = b / 2;
    if !(0x20..0x7f).contains(&c) {
        return None;
    }
    Some(c)
}

/// Every maximal text run in `data` of at least [`MIN_RUN_LEN`] characters, trying a 1-byte and
/// a 2-byte stride at each starting offset (short identifiers in these records are stored two
/// bytes per character; longer description text, one). Overlapping runs are deduplicated,
/// longest first, so a 2-byte-stride run doesn't also get reported as a shorter 1-byte-stride
/// fragment of itself.
fn text_runs(data: &[u8]) -> Vec<String> {
    let limit = data.len().min(MAX_SCAN_BYTES);
    let mut found: Vec<(usize, usize, String)> = Vec::new(); // (start, end_exclusive_bytes, text)
    for start in 0..limit {
        for stride in [1usize, 2usize] {
            let s = run_at(data, start, stride);
            if s.chars().count() >= MIN_RUN_LEN {
                let end = start + (s.chars().count().saturating_sub(1)) * stride + 1;
                found.push((start, end, s));
            }
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    let mut out = Vec::new();
    let mut covered_until = 0usize;
    for (start, end, s) in found {
        if start < covered_until {
            continue;
        }
        covered_until = end;
        out.push(s);
    }
    out
}

/// One schema field: its name and declared type word (`ClassChar`, `ClassLong`, `ClassReal`, …).
struct Field {
    name: String,
    kind: String,
}

/// One table's schema, parsed from an `IRD_DSC_RECORD`: its name and ordered fields.
struct Table {
    name: String,
    fields: Vec<Field>,
}

/// Parses an `IRD_DSC_RECORD`'s decoded text runs into a [`Table`], when they look like a real
/// feature table rather than one of the built-in type-descriptor records (`Class`, `ClassChar`,
/// …) that describe the type system itself. A real table's runs start with its name, then the
/// literal marker `ade`, then `name, type, "Ird"` triples for each field.
fn parse_table(runs: &[String]) -> Option<Table> {
    let name = runs.first()?.clone();
    if runs.get(1).map(String::as_str) != Some("ade") {
        return None;
    }
    // `ade` is followed by a `Class`/`Ird` pair naming the table's own base class, before the
    // field list starts.
    let mut i = if runs.get(2).map(String::as_str) == Some("Class") && runs.get(3).map(String::as_str) == Some("Ird") { 4 } else { 2 };
    let mut fields = Vec::new();
    // Bounded by the record's own run count; a real schema has a handful of fields.
    while i + 1 < runs.len() {
        let field_name = &runs[i];
        let field_type = &runs[i + 1];
        if !field_type.starts_with("Class") || field_name.starts_with("Class") || field_name == "Ird" {
            break;
        }
        fields.push(Field { name: field_name.clone(), kind: field_type.clone() });
        i += 2;
        if runs.get(i).map(String::as_str) == Some("Ird") {
            i += 1;
        } else {
            break;
        }
    }
    if fields.is_empty() { None } else { Some(Table { name, fields }) }
}

/// Table-name comparison ignoring case, underscores and spaces, and tolerant of a decoded name
/// that's a truncated/altered prefix of the real one: this encoding only represents the printable
/// ASCII range (`b / 2` must land in `0x20..0x7f`), so a genuinely accented table name (seen in a
/// real sample: a layer named "...Isolé" whose decoded `IRD_DSC_RECORD` name came back
/// "...Isoli", the accented letter decoding to something else entirely) can't always come back
/// byte-for-byte - matching is still safe here because it's always checked against the small set
/// of tables actually declared in this document, not against arbitrary text.
fn names_match(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.chars().filter(|c| *c != '_' && *c != ' ').collect::<String>().to_ascii_lowercase();
    let (a, b) = (norm(a), norm(b));
    if a == b {
        return true;
    }
    // A real table/layer name is never this short, so a common-prefix match below this length is
    // too likely to be a coincidence to trust.
    const MIN_PREFIX_LEN: usize = 6;
    let common = a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count();
    let shortest = a.chars().count().min(b.chars().count());
    // Tolerate the last couple of characters differing (where an accented letter the encoding
    // can't represent would land), not a mismatch anywhere earlier in the name.
    common >= MIN_PREFIX_LEN && common + 2 >= shortest
}

/// Whether `s` looks like a real field value rather than binary noise that happened to decode to
/// a few printable characters: only letters, digits, spaces, apostrophes and hyphens (a `ClassChar`
/// field's value can itself be a short code like "T1"), with at least one letter or digit, so
/// pure punctuation noise (seen in practice: things like `"d@n!"`, `"$p="`) is rejected.
fn looks_like_a_word(s: &str) -> bool {
    s.chars().all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '\'' || c == '-') && s.chars().any(|c| c.is_ascii_alphanumeric())
}

fn raw_bytes(doc: &CadDocument, h: acadrust::Handle) -> Option<&[u8]> {
    match doc.objects.get(&h)? {
        ObjectType::Unknown { raw_dwg_data: Some(d), .. } => Some(d.as_slice()),
        _ => None,
    }
}

fn type_name(doc: &CadDocument, h: acadrust::Handle) -> Option<&str> {
    match doc.objects.get(&h)? {
        ObjectType::Unknown { type_name, .. } => Some(type_name.as_str()),
        _ => None,
    }
}

/// Byte offset, within an `IRD_OBJ_RECORD`'s raw bytes, of the table's first `ClassLong` field
/// (`FeatId` in every real table seen so far), stored as a 4-byte little-endian integer doubled
/// the same way a character is (`value * 2`) - reverse-engineered from, and confirmed exactly
/// against, a real sample file's own AutoCAD Properties panel (`FeatId = 3`).
const LONG_FIELD_OFFSET: usize = 18;

/// Reads the table's first `ClassLong` field from `data`, when the fixed-position bytes are
/// present and even (the same validity check every other field in this format relies on - an odd
/// byte can never be a valid `value * 2`, so this simply yields nothing rather than a wrong number
/// on a record this offset doesn't apply to).
fn long_field(data: &[u8]) -> Option<u32> {
    let bytes: [u8; 4] = data.get(LONG_FIELD_OFFSET..LONG_FIELD_OFFSET + 4)?.try_into().ok()?;
    let raw = u32::from_le_bytes(bytes);
    if !raw.is_multiple_of(2) {
        return None;
    }
    Some(raw / 2)
}

/// Lowest magnitude a decoded `ClassReal` value is accepted at - tuned to real-world survey/
/// archaeology measurements (heights, elevations, areas), not general numbers.
const REAL_FIELD_MIN: f64 = 1e-4;
/// Highest magnitude a decoded `ClassReal` value is accepted at (same reasoning as
/// [`REAL_FIELD_MIN`]).
const REAL_FIELD_MAX: f64 = 1e6;

/// Reads the table's first `ClassReal` field from `data` - **approximately**: this value's own
/// bytes are doubled the same way every other value in this format is (`byte = (value * 2) %
/// 256`), but unlike a character's byte (always < 0x80, so doubling it never wraps) or
/// [`LONG_FIELD_OFFSET`]'s small integer (confirmed to never exceed one byte in every real sample
/// seen), an arbitrary byte of an IEEE-754 double routinely needs its top bit set, and doubling
/// that wraps - permanently losing exactly that bit. Recovering it isn't possible from this byte
/// alone; this always resolves the ambiguity by assuming the bit was unset (true byte < 0x80),
/// which was confirmed, by brute-forcing every 2^8 combination against a real file's own
/// Properties panel (`Hauteur_GN = 8.042`), to recover the real value's two most significant
/// bytes (the ones that set its order of magnitude) exactly - but for the remaining, lower-order
/// bytes (which only affect its later decimal digits) that same file needed the opposite
/// assumption for two of them, so the fractional digits here can be slightly off (confirmed
/// example: this decodes that same record to `8.041`, not `8.042`).
///
/// Since the field's exact byte offset isn't fixed the way [`LONG_FIELD_OFFSET`] is (it depends
/// on how long the record's preceding text fields are), this scans backward from the end of the
/// record (the field was found just before a short, fixed-shape trailer) for the first 8-byte
/// window whose doubled-bytes-undone decoding is a plausible measurement
/// ([`REAL_FIELD_MIN`]..[`REAL_FIELD_MAX`], always positive by the assumption above) - bounded by
/// the record's own (small) length, never by [`MAX_SCAN_BYTES`]. Every other field in this format
/// starts at an even offset, so only even offsets are tried: on the real sample this was
/// confirmed against, the very next (odd) offset also happens to decode to a finite,
/// plausible-looking number (pure coincidence - one wrong byte of a double can still look like a
/// valid measurement), so without this, the scan finds that one first and never reaches the real
/// field at all.
fn real_field(data: &[u8]) -> Option<f64> {
    if data.len() < 8 {
        return None;
    }
    for start in (0..=data.len() - 8).rev().filter(|s| s % 2 == 0) {
        let mut bytes = [0u8; 8];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = data[start + i] / 2;
        }
        let v = f64::from_le_bytes(bytes);
        if v.is_finite() && (REAL_FIELD_MIN..REAL_FIELD_MAX).contains(&v) {
            return Some(v);
        }
    }
    None
}

/// Every table found anywhere in the document (there is normally one `IRD_DSC_DICT` holding all
/// of them, but this doesn't assume that - any `IRD_DSC_RECORD` in the file is tried).
fn schema(doc: &CadDocument) -> Vec<Table> {
    doc.objects
        .values()
        .filter_map(|o| match o {
            ObjectType::Unknown { type_name, raw_dwg_data: Some(d), .. } if type_name == "IRD_DSC_RECORD" => Some(d.as_slice()),
            _ => None,
        })
        .filter_map(|d| parse_table(&text_runs(d)))
        .collect()
}

/// Reads [`MapAttributes`] out of an already-read `acadrust` document. Never panics or errors:
/// a document with no Map Object Data, or one this pattern-matching doesn't recognize, yields an
/// empty map.
pub fn map_attributes(doc: &CadDocument) -> MapAttributes {
    let tables = schema(doc);
    if tables.is_empty() {
        return MapAttributes::new();
    }
    let mut out = MapAttributes::new();
    for e in doc.entities() {
        let common = e.common();
        let Some(xdict) = common.xdictionary_handle.filter(|h| h.is_valid()) else { continue };
        let Some(ObjectType::Dictionary(d)) = doc.objects.get(&xdict) else { continue };
        let Some(table) = tables.iter().find(|t| names_match(&t.name, &common.layer)) else { continue };
        let text_fields: Vec<&Field> = table.fields.iter().filter(|f| f.kind.contains("Char")).collect();
        let long_field_name = table.fields.iter().find(|f| f.kind.contains("Long")).map(|f| f.name.clone());
        let real_field_name = table.fields.iter().find(|f| f.kind.contains("Real")).map(|f| f.name.clone());
        if text_fields.is_empty() && long_field_name.is_none() && real_field_name.is_none() {
            continue;
        }
        // Every IRD_OBJ_RECORD under this entity's extension dictionary, in dictionary order; a
        // real file has one or two, but there's no fixed bound from the format itself, so this is
        // simply bounded by the dictionary's own (small) entry count.
        let mut values: Vec<String> = Vec::new();
        let mut long_value: Option<u32> = None;
        let mut real_value: Option<f64> = None;
        for (_, child) in &d.entries {
            if type_name(doc, *child) != Some("IRD_OBJ_RECORD") {
                continue;
            }
            let Some(data) = raw_bytes(doc, *child) else { continue };
            if long_value.is_none() {
                long_value = long_field(data);
            }
            if real_value.is_none() && real_field_name.is_some() {
                real_value = real_field(data);
            }
            for run in text_runs(data) {
                // Field/type/marker words from the schema itself can also match as runs inside an
                // OBJ_RECORD (the record embeds the class name); skip anything that's a known
                // schema word rather than real field data. The rest of the buffer is binary
                // (lengths, offsets, handles) that can still coincidentally decode to a few
                // printable characters, so a run is only kept as a value when it also looks like
                // a word - letters, spaces, apostrophes, hyphens, nothing else - which a real
                // "Nature"/"Num"-style text field value is, and binary noise usually isn't.
                let is_schema_word = run == table.name || run == "ade" || run.starts_with("Class") || run == "Ird";
                if !is_schema_word && looks_like_a_word(&run) {
                    values.push(run);
                }
            }
        }
        if values.is_empty() && long_value.is_none() && real_value.is_none() {
            continue;
        }
        // Only trust the schema's field order (and so its field names) when every text field
        // produced a value: some text fields are simply blank for a given entity, and a run
        // missing from the middle would silently shift every name after it onto the wrong value.
        // Short of locating each value's exact field within the record's binary layout (not
        // decoded yet - see the module doc), a generic, still-truthful label beats a wrong name.
        let mut fields: Vec<(String, String)> = if values.len() == text_fields.len() {
            text_fields.iter().zip(values.iter()).map(|(f, v)| (f.name.clone(), v.clone())).collect()
        } else {
            values.iter().enumerate().map(|(i, v)| (format!("Attribut {}", i + 1), v.clone())).collect()
        };
        // "≈" flags this one as approximate (see `real_field`'s doc comment) - everything else
        // here is exact.
        if let (Some(name), Some(v)) = (real_field_name, real_value) {
            fields.push((name, format!("≈{v:.3}")));
        }
        if let (Some(name), Some(v)) = (long_field_name, long_value) {
            fields.insert(0, (name, v.to_string()));
        }
        if !fields.is_empty() {
            out.insert(common.handle.value(), (table.name.clone(), fields));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_two_byte_stride_identifier() {
        // "Ab" at stride 2, interleaved with odd filler bytes.
        let data = [b'A' * 2, 1, b'b' * 2, 3];
        assert_eq!(run_at(&data, 0, 2), "Ab");
    }

    #[test]
    fn decodes_a_one_byte_stride_sentence() {
        let encoded: Vec<u8> = "Hi there".bytes().map(|b| b * 2).collect();
        assert_eq!(run_at(&encoded, 0, 1), "Hi there");
    }

    #[test]
    fn text_runs_finds_a_run_amid_noise() {
        let mut data = vec![1, 2, 3, 255, 0, 5];
        data.extend("oka".bytes().map(|b| b * 2));
        data.extend([7, 9, 11]);
        assert!(text_runs(&data).contains(&"oka".to_string()));
    }

    #[test]
    fn run_at_bridges_a_single_undecodable_byte_with_a_placeholder() {
        // "tranch" + one byte that can never decode (odd, so it fails the `b % 2 == 0` check -
        // standing in for the real accented byte this encoding has no slot for) + "e".
        let mut data: Vec<u8> = "tranch".bytes().map(|b| b * 2).collect();
        data.push(199); // odd: undecodable at any stride.
        data.push(b'e' * 2);
        assert_eq!(run_at(&data, 0, 1), "tranch?e");
    }

    #[test]
    fn run_at_does_not_pad_a_run_that_genuinely_ends() {
        // "oka" followed by two bad bytes in a row (one more than MAX_GAP tolerates), then
        // nothing decodable - the run must end at "oka", not grow spurious trailing '?'s.
        let mut data: Vec<u8> = "oka".bytes().map(|b| b * 2).collect();
        data.extend([199, 201, 255, 255]);
        assert_eq!(run_at(&data, 0, 1), "oka");
    }

    #[test]
    fn text_runs_keeps_an_accented_word_whole_instead_of_a_shorter_unrelated_overlap() {
        // Regression for "tranchée" decoding as "tranchie": a real accented byte used to break
        // the run after "tranch", leaving a later, unrelated run (here a standalone "ie" a few
        // bytes further on) to be reported on its own and look, out of context, like it finished
        // the word. With the gap bridged, "tranch?e" is reported as one run and nothing merges
        // the two into "tranchie" - `text_runs` only ever reports what [`run_at`] itself produced.
        let mut data: Vec<u8> = "tranch".bytes().map(|b| b * 2).collect();
        data.push(199); // the undecodable byte standing in for 'é'.
        data.push(b'e' * 2);
        data.push(0); // a gap of real binary noise between the word and the decoy.
        data.extend("ie".bytes().map(|b| b * 2)); // too short to be kept (< MIN_RUN_LEN) anyway.
        let runs = text_runs(&data);
        assert!(runs.contains(&"tranch?e".to_string()));
        assert!(!runs.contains(&"tranchie".to_string()));
        assert!(!runs.iter().any(|r| r == "ie"));
    }

    #[test]
    fn text_runs_on_empty_or_garbage_is_empty_not_a_panic() {
        assert!(text_runs(&[]).is_empty());
        assert!(text_runs(&[255; 50]).is_empty());
    }

    #[test]
    fn parse_table_recognizes_a_real_table_and_rejects_a_type_descriptor() {
        let table_runs = vec![
            "Ouverturesarcheo".to_string(),
            "ade".to_string(),
            "FeatId".to_string(),
            "ClassLong".to_string(),
            "Ird".to_string(),
            "Num".to_string(),
            "ClassChar".to_string(),
            "Ird".to_string(),
        ];
        let t = parse_table(&table_runs).expect("should parse as a table");
        assert_eq!(t.name, "Ouverturesarcheo");
        assert_eq!(t.fields.len(), 2);
        assert_eq!(t.fields[1].name, "Num");

        let descriptor_runs = vec!["Class".to_string(), "Ird".to_string(), "Class".to_string(), "The base superclass".to_string()];
        assert!(parse_table(&descriptor_runs).is_none());
    }

    #[test]
    fn looks_like_a_word_rejects_symbol_noise_but_keeps_real_values() {
        assert!(looks_like_a_word("sondage"));
        assert!(looks_like_a_word("tranchee"));
        assert!(looks_like_a_word("T1"));
        assert!(!looks_like_a_word("d@n!"));
        assert!(!looks_like_a_word("$p="));
        assert!(!looks_like_a_word(""));
    }

    #[test]
    fn names_match_ignores_case_and_underscores() {
        assert!(names_match("Ouverturesarcheo", "Ouvertures_archeo"));
        assert!(!names_match("Topo", "Ouvertures_archeo"));
    }

    #[test]
    fn names_match_tolerates_an_accented_letter_the_encoding_cant_represent() {
        // Real sample: layer "Mobilier_Isolé", decoded schema name "Mobilier_Isoli" (the accented
        // "é" came back as something else).
        assert!(names_match("Mobilier_Isoli", "Mobilier_Isolé"));
        // But short, unrelated names don't spuriously prefix-match.
        assert!(!names_match("Num", "Nature"));
    }

    #[test]
    fn long_field_decodes_a_doubled_little_endian_u32() {
        let mut data = vec![0u8; LONG_FIELD_OFFSET];
        data.extend_from_slice(&6u32.to_le_bytes()); // FeatId 3, stored doubled.
        assert_eq!(long_field(&data), Some(3));
    }

    #[test]
    fn long_field_on_odd_or_short_data_is_none_not_a_panic() {
        let mut odd = vec![0u8; LONG_FIELD_OFFSET];
        odd.extend_from_slice(&7u32.to_le_bytes());
        assert_eq!(long_field(&odd), None);
        assert_eq!(long_field(&[0u8; 4]), None);
        assert_eq!(long_field(&[]), None);
    }

    #[test]
    fn real_field_on_short_or_implausible_data_is_none_not_a_panic() {
        assert_eq!(real_field(&[]), None);
        assert_eq!(real_field(&[0u8; 7]), None);
        // All-zero bytes decode to 0.0, outside the accepted (nonzero) magnitude range.
        assert_eq!(real_field(&[0u8; 16]), None);
    }
}

/// A regression test against a real `IRD_DSC_RECORD`'s bytes (one table definition from a real
/// sample DWG's "Ouvertures_archeo" layer), so a future change to the heuristics above can't
/// silently stop recognizing real files.
#[cfg(test)]
mod real_sample_regression {
    use super::*;

    const OUVERTURES_ARCHEO_SCHEMA_RECORD: &[u8] = &[
        3, 0, 141, 130, 64, 0, 0, 182, 254, 36, 6, 2, 1, 83, 200, 2, 96, 0, 0, 158, 0, 234, 0, 236, 0, 202, 0, 228, 0, 232, 0, 234, 0, 228, 0, 202,
        0, 230, 0, 190, 0, 194, 0, 228, 0, 198, 0, 208, 0, 202, 0, 222, 0, 0, 0, 194, 0, 200, 0, 202, 0, 0, 0, 134, 0, 216, 0, 194, 0, 230, 0, 230,
        0, 0, 0, 146, 0, 228, 0, 200, 0, 0, 0, 0, 0, 0, 0, 6, 0, 1, 0, 0, 0, 140, 0, 202, 0, 194, 0, 232, 0, 146, 0, 200, 0, 0, 0, 134, 0, 216, 0,
        194, 0, 230, 0, 230, 0, 152, 0, 222, 0, 220, 0, 206, 0, 0, 0, 146, 0, 228, 0, 200, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 156, 0, 234, 0,
        218, 0, 0, 0, 134, 0, 216, 0, 194, 0, 230, 0, 230, 0, 134, 0, 208, 0, 194, 0, 228, 0, 0, 0, 146, 0, 228, 0, 200, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 152, 0, 0, 72, 186, 120, 3, 168, 3, 176, 3, 40, 3, 144, 3, 160, 3, 168, 3, 144, 3, 40, 3, 152, 2, 248, 3, 8, 3, 144, 3, 24, 3, 64,
        3, 40, 3, 120, 1, 104, 2, 248, 1, 104, 3, 8, 3, 32, 3, 40, 3, 208, 15, 4, 53, 11, 111, 175,
    ];

    #[test]
    fn decodes_the_ouvertures_archeo_table_schema() {
        let runs = text_runs(OUVERTURES_ARCHEO_SCHEMA_RECORD);
        let t = parse_table(&runs).expect("should parse as a table");
        assert_eq!(t.name, "Ouvertures_archeo");
        let names: Vec<&str> = t.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["FeatId", "Num"]);
        assert_eq!(t.fields[1].kind, "ClassChar");
    }

    /// A real `IRD_OBJ_RECORD` for a "Mobilier_Isolé" point, whose AutoCAD Properties panel
    /// showed `FeatId = 3`, `Num = 2909` and `Hauteur_GN = 8.042` for this exact object.
    const MOBILIER_ISOLE_OBJ_RECORD: &[u8] = &[
        3, 0, 152, 0, 64, 0, 0, 128, 72, 164, 6, 2, 1, 32, 50, 60, 2, 0, 6, 0, 0, 0, 100, 0, 114, 0, 96, 0, 114, 0, 0, 0, 0, 0, 0, 0, 95, 186, 72,
        13, 2, 42, 64, 128, 128, 66, 1, 33, 65, 155,
    ];

    #[test]
    fn decodes_feat_id_and_num_from_a_real_obj_record() {
        assert_eq!(long_field(MOBILIER_ISOLE_OBJ_RECORD), Some(3));
        let runs = text_runs(MOBILIER_ISOLE_OBJ_RECORD);
        assert!(runs.contains(&"2909".to_string()));
    }

    #[test]
    fn real_field_approximates_the_real_value_within_its_known_margin() {
        // The real value is 8.042; this format's lost-high-bit ambiguity (see `real_field`'s doc
        // comment) means what's recoverable here is 8.041 - close, not exact.
        let v = real_field(MOBILIER_ISOLE_OBJ_RECORD).expect("should find a plausible window");
        assert!((v - 8.042).abs() < 0.01, "expected close to 8.042, got {v}");
    }
}
