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
//! recovers what that pattern-matching finds with confidence - text (`ClassChar`) field values
//! matched to a table whose decoded name matches the entity's own layer. Numeric field types
//! (`ClassLong`, `ClassReal`) aren't decoded yet and are left out rather than guessed.
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

/// Decodes `data[start..]` at a fixed `stride` (1 or 2 bytes per character), starting the first
/// character at `start`, for as long as each byte decodes to a printable ASCII character: a byte
/// `b` decodes to character `b / 2` when `b` is even and `b / 2` falls in `0x20..0x7f`
/// (reverse-engineered from sample files; see the module doc).
fn run_at(data: &[u8], start: usize, stride: usize) -> String {
    let mut out = String::new();
    let mut i = start;
    while let Some(&b) = data.get(i) {
        if b % 2 != 0 {
            break;
        }
        let c = b / 2;
        if !(0x20..0x7f).contains(&c) {
            break;
        }
        out.push(c as char);
        i = i.saturating_add(stride);
        if out.len() >= MAX_SCAN_BYTES {
            break;
        }
    }
    out
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

/// Table-name comparison ignoring case and underscores, since this encoding can't represent
/// underscores (an odd byte can never be a valid `code * 2`) and drops them from decoded names.
fn names_match(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.chars().filter(|c| *c != '_').collect::<String>().to_ascii_lowercase();
    norm(a) == norm(b)
}

/// Whether `s` looks like a real word rather than binary noise that happened to decode to a few
/// printable characters: only letters, spaces, apostrophes and hyphens, with at least one letter.
fn looks_like_a_word(s: &str) -> bool {
    s.chars().all(|c| c.is_ascii_alphabetic() || c == ' ' || c == '\'' || c == '-') && s.chars().any(|c| c.is_ascii_alphabetic())
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
        if text_fields.is_empty() {
            continue;
        }
        // Every IRD_OBJ_RECORD under this entity's extension dictionary, in dictionary order; a
        // real file has one or two, but there's no fixed bound from the format itself, so this is
        // simply bounded by the dictionary's own (small) entry count.
        let mut values: Vec<String> = Vec::new();
        for (_, child) in &d.entries {
            if type_name(doc, *child) != Some("IRD_OBJ_RECORD") {
                continue;
            }
            let Some(data) = raw_bytes(doc, *child) else { continue };
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
        if values.is_empty() {
            continue;
        }
        // Only trust the schema's field order (and so its field names) when every text field
        // produced a value: some text fields are simply blank for a given entity, and a run
        // missing from the middle would silently shift every name after it onto the wrong value.
        // Short of locating each value's exact field within the record's binary layout (not
        // decoded yet - see the module doc), a generic, still-truthful label beats a wrong name.
        let fields: Vec<(String, String)> = if values.len() == text_fields.len() {
            text_fields.iter().zip(values.iter()).map(|(f, v)| (f.name.clone(), v.clone())).collect()
        } else {
            values.iter().enumerate().map(|(i, v)| (format!("Attribut {}", i + 1), v.clone())).collect()
        };
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
        assert!(!looks_like_a_word("d@n!"));
        assert!(!looks_like_a_word("$p="));
        assert!(!looks_like_a_word(""));
    }

    #[test]
    fn names_match_ignores_case_and_underscores() {
        assert!(names_match("Ouverturesarcheo", "Ouvertures_archeo"));
        assert!(!names_match("Topo", "Ouvertures_archeo"));
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
}
