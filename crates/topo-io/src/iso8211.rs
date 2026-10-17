//! Generic ISO/IEC 8211 ("Specification for a data descriptive file for information
//! interchange") record reader.
//!
//! ISO 8211 is the self-describing binary container that EDIGEO (and other geographic exchange
//! formats, such as S-57) is built on top of: a stream of records, each made of a fixed 24-byte
//! leader, a directory of `(tag, length, position)` triples, and a field area sliced by that
//! directory into named fields, each of which is itself a list of subfields separated by a unit
//! separator byte. This module reads that *generic* container only - it knows nothing about
//! what any particular tag means - which is exactly what [`crate::edigeo`] builds on.
//!
//! Leader layout (24 bytes, all-ASCII-digit fields unless noted), the one part of the format
//! this module hard-codes:
//!
//! | bytes | field                        |
//! |------:|------------------------------|
//! | 0..5  | record length                |
//! | 5     | interchange level            |
//! | 6     | leader identifier (`L`/`D`)  |
//! | 7     | inline code extension ind.   |
//! | 8     | version number               |
//! | 9     | application indicator        |
//! | 10..12| field control length         |
//! | 12..17| base address of field area   |
//! | 17..20| extended character set ind.  |
//! | 20    | size of field length         |
//! | 21    | size of field position       |
//! | 22    | reserved                     |
//! | 23    | size of field tag            |

const LEADER_LEN: usize = 24;
const FIELD_TERMINATOR: u8 = 0x1E;
const UNIT_SEPARATOR: u8 = 0x1F;

/// Hard caps against a hostile or corrupt file: a real EDIGEO export is a modest list of
/// surveyed points and lines, not millions of records or fields.
const MAX_RECORDS: usize = 500_000;
const MAX_FIELDS_PER_RECORD: usize = 100_000;

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum Iso8211Error {
    #[error("truncated leader: need {LEADER_LEN} bytes at offset {offset}, found {found}")]
    TruncatedLeader { offset: usize, found: usize },
    #[error("leader field `{field}` ({text:?}) at offset {offset} is not a number")]
    BadLeaderNumber { field: &'static str, text: String, offset: usize },
    #[error("record at offset {offset}: base address {base} of the field area is before the leader+directory")]
    BaseAddressTooSmall { offset: usize, base: usize },
    #[error("record at offset {offset}: declared record length {declared} does not reach past the leader")]
    RecordTooShort { offset: usize, declared: usize },
    #[error("record at offset {offset}: declared record length {declared} runs past the end of the file ({available} bytes available)")]
    RecordRunsPastEof { offset: usize, declared: usize, available: usize },
    #[error("record at offset {offset}: the directory is not a whole number of entries")]
    RaggedDirectory { offset: usize },
    #[error("record at offset {offset}: entry {index} names a field that runs past the field area")]
    FieldRunsPastFieldArea { offset: usize, index: usize },
    #[error("record at offset {offset}: too many fields ({count}), rejecting as hostile input")]
    TooManyFields { offset: usize, count: usize },
    #[error("too many records ({0}), rejecting as hostile input")]
    TooManyRecords(usize),
}

/// One field of one record: its directory tag, and its subfields split on the unit separator
/// (control/field terminators already stripped).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub tag: String,
    pub subfields: Vec<String>,
}

/// One ISO 8211 record. `leader_identifier` is `'L'` for the file's first record (the Data
/// Descriptive Record, which describes field *formats* and is otherwise skipped by
/// [`crate::edigeo`]) and `'D'` for every Data Record after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub leader_identifier: char,
    pub fields: Vec<Field>,
}

/// Bytes are decoded as Latin-1 (ISO 8859-1), the encoding EDIGEO text normally uses: every byte
/// 0..=255 maps 1:1 to the Unicode scalar value of the same number, so this can never fail or
/// panic, unlike a strict UTF-8 decode of a file that may not be UTF-8 at all.
fn decode_latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

fn ascii_digits(bytes: &[u8], field: &'static str, offset: usize) -> Result<usize, Iso8211Error> {
    let text = decode_latin1(bytes);
    text.trim().parse::<usize>().map_err(|_| Iso8211Error::BadLeaderNumber { field, text, offset })
}

/// Reads every record in `data`, in order. Stops cleanly at the end of the file; any trailing
/// bytes too short to hold another leader are ignored rather than treated as an error, since a
/// real export's last record is sometimes followed by a short run of padding.
pub fn read_records(data: &[u8]) -> Result<Vec<Record>, Iso8211Error> {
    if !data.is_empty() && data.len() < LEADER_LEN {
        return Err(Iso8211Error::TruncatedLeader { offset: 0, found: data.len() });
    }
    let mut records = Vec::new();
    let mut offset = 0usize;
    while offset + LEADER_LEN <= data.len() {
        // A record entirely made of padding/terminators marks logical end-of-file.
        if data[offset..offset + LEADER_LEN].iter().all(|&b| b == FIELD_TERMINATOR || b == 0) {
            break;
        }
        let (record, consumed) = read_one_record(data, offset)?;
        records.push(record);
        if records.len() > MAX_RECORDS {
            return Err(Iso8211Error::TooManyRecords(records.len()));
        }
        offset += consumed;
    }
    Ok(records)
}

fn read_one_record(data: &[u8], offset: usize) -> Result<(Record, usize), Iso8211Error> {
    let leader = data.get(offset..offset + LEADER_LEN).ok_or(Iso8211Error::TruncatedLeader { offset, found: data.len() - offset })?;

    let record_length = ascii_digits(&leader[0..5], "record length", offset)?;
    let leader_identifier = leader[6] as char;
    let base_address = ascii_digits(&leader[12..17], "base address of field area", offset)?;
    let size_len = ascii_digits(&leader[20..21], "size of field length", offset)?;
    let size_pos = ascii_digits(&leader[21..22], "size of field position", offset)?;
    let size_tag = ascii_digits(&leader[23..24], "size of field tag", offset)?;

    if base_address < LEADER_LEN {
        return Err(Iso8211Error::BaseAddressTooSmall { offset, base: base_address });
    }
    if record_length < base_address {
        return Err(Iso8211Error::RecordTooShort { offset, declared: record_length });
    }
    let record_end = offset.checked_add(record_length).ok_or(Iso8211Error::RecordRunsPastEof {
        offset,
        declared: record_length,
        available: data.len() - offset,
    })?;
    if record_end > data.len() {
        return Err(Iso8211Error::RecordRunsPastEof { offset, declared: record_length, available: data.len() - offset });
    }

    let dir_start = offset + LEADER_LEN;
    let dir_end = offset + base_address;
    // The directory is terminated by a field terminator byte, which is not itself an entry.
    let dir_bytes = &data[dir_start..dir_end.min(data.len())];
    let dir_bytes = dir_bytes.split(|&b| b == FIELD_TERMINATOR).next().unwrap_or(dir_bytes);
    let entry_width = size_tag + size_len + size_pos;
    if entry_width == 0 || dir_bytes.len() % entry_width != 0 {
        return Err(Iso8211Error::RaggedDirectory { offset });
    }

    let field_area_start = offset + base_address;
    let mut fields = Vec::new();
    for (index, entry) in dir_bytes.chunks_exact(entry_width).enumerate() {
        if fields.len() > MAX_FIELDS_PER_RECORD {
            return Err(Iso8211Error::TooManyFields { offset, count: fields.len() });
        }
        let Some(tag_bytes) = entry.get(0..size_tag) else { continue };
        let Some(len_bytes) = entry.get(size_tag..size_tag + size_len) else { continue };
        let Some(pos_bytes) = entry.get(size_tag + size_len..entry_width) else { continue };
        let tag = decode_latin1(tag_bytes).trim().to_string();
        let len = ascii_digits(len_bytes, "field length", offset)?;
        let pos = ascii_digits(pos_bytes, "field position", offset)?;

        let field_start = field_area_start.checked_add(pos).ok_or(Iso8211Error::FieldRunsPastFieldArea { offset, index })?;
        let field_end = field_start.checked_add(len).ok_or(Iso8211Error::FieldRunsPastFieldArea { offset, index })?;
        let raw = data.get(field_start..field_end).ok_or(Iso8211Error::FieldRunsPastFieldArea { offset, index })?;
        let trimmed = raw.split(|&b| b == FIELD_TERMINATOR).next().unwrap_or(raw);
        let subfields: Vec<String> = trimmed.split(|&b| b == UNIT_SEPARATOR).map(decode_latin1).collect();
        fields.push(Field { tag, subfields });
    }

    Ok((Record { leader_identifier, fields }, record_length.max(LEADER_LEN)))
}

/// Test/tooling helper: hand-builds a well-formed ISO 8211 record so other crates' tests (and
/// ad-hoc tooling) can construct synthetic EDIGEO-shaped fixtures without duplicating this
/// module's own byte layout. Not used by this crate's own read path.
pub mod test_support {
    use super::{FIELD_TERMINATOR, UNIT_SEPARATOR};

    /// Hand-builds one valid ISO 8211 record from a list of `(tag, subfields)` fields, for
    /// tests - the inverse of [`super::read_one_record`], written independently of it so a bug
    /// shared between encoder and decoder is less likely to hide behind a passing round trip.
    pub fn build_record(leader_identifier: u8, fields: &[(&str, &[&str])]) -> Vec<u8> {
        let size_tag = fields.iter().map(|(t, _)| t.len()).max().unwrap_or(1).max(1);
        let mut field_area = Vec::new();
        let mut entries = Vec::new();
        for (tag, subfields) in fields {
            let pos = field_area.len();
            for (i, sf) in subfields.iter().enumerate() {
                if i > 0 {
                    field_area.push(UNIT_SEPARATOR);
                }
                field_area.extend_from_slice(sf.as_bytes());
            }
            field_area.push(FIELD_TERMINATOR);
            let len = field_area.len() - pos;
            entries.push((tag.to_string(), len, pos));
        }

        let size_len = 3usize;
        let size_pos = 3usize;
        let mut directory = Vec::new();
        for (tag, len, pos) in &entries {
            directory.extend_from_slice(format!("{tag:0<width$}", width = size_tag).as_bytes());
            directory.extend_from_slice(format!("{len:0width$}", width = size_len).as_bytes());
            directory.extend_from_slice(format!("{pos:0width$}", width = size_pos).as_bytes());
        }
        directory.push(FIELD_TERMINATOR);

        let base_address = 24 + directory.len();
        let record_length = base_address + field_area.len();

        let mut leader = vec![b' '; 24];
        leader[0..5].copy_from_slice(format!("{record_length:05}").as_bytes());
        leader[5] = b'1';
        leader[6] = leader_identifier;
        leader[7] = b' ';
        leader[8] = b'1';
        leader[9] = b' ';
        leader[10..12].copy_from_slice(b"09");
        leader[12..17].copy_from_slice(format!("{base_address:05}").as_bytes());
        leader[17..20].copy_from_slice(b"   ");
        leader[20] = (b'0' + size_len as u8).min(b'9');
        leader[21] = (b'0' + size_pos as u8).min(b'9');
        leader[22] = b'0';
        leader[23] = (b'0' + size_tag as u8).min(b'9');

        let mut out = leader;
        out.extend_from_slice(&directory);
        out.extend_from_slice(&field_area);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::build_record;
    use super::*;

    #[test]
    fn reads_a_single_field_single_subfield_record() {
        let bytes = build_record(b'D', &[("PNO", &["42"])]);
        let records = read_records(&bytes).expect("a well-formed record should parse");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].leader_identifier, 'D');
        assert_eq!(records[0].fields.len(), 1);
        assert_eq!(records[0].fields[0].tag, "PNO");
        assert_eq!(records[0].fields[0].subfields, vec!["42"]);
    }

    #[test]
    fn reads_multiple_fields_and_multiple_subfields() {
        let bytes = build_record(b'D', &[("GEO", &["1524.300", "8215.120"]), ("LAB", &["P1"])]);
        let records = read_records(&bytes).expect("should parse");
        assert_eq!(records[0].fields.len(), 2);
        assert_eq!(records[0].fields[0].subfields, vec!["1524.300", "8215.120"]);
        assert_eq!(records[0].fields[1].subfields, vec!["P1"]);
    }

    #[test]
    fn reads_a_sequence_of_records_back_to_back() {
        let mut bytes = build_record(b'L', &[("0000", &["DDR"])]);
        bytes.extend(build_record(b'D', &[("PNO", &["1"])]));
        bytes.extend(build_record(b'D', &[("PNO", &["2"])]));
        let records = read_records(&bytes).expect("should parse");
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].leader_identifier, 'L');
        assert_eq!(records[1].fields[0].subfields, vec!["1"]);
        assert_eq!(records[2].fields[0].subfields, vec!["2"]);
    }

    #[test]
    fn empty_input_yields_no_records() {
        assert_eq!(read_records(&[]).expect("empty is not an error"), vec![]);
    }

    #[test]
    fn a_truncated_leader_is_an_error_not_a_panic() {
        let err = read_records(&[b'0'; 10]).expect_err("10 bytes is not a full leader");
        assert!(matches!(err, Iso8211Error::TruncatedLeader { .. }));
    }

    #[test]
    fn a_record_claiming_to_run_past_eof_is_rejected() {
        let mut bytes = build_record(b'D', &[("PNO", &["1"])]);
        // Inflate the declared record length so it claims more bytes than the buffer has.
        bytes[0..5].copy_from_slice(b"99999");
        let err = read_records(&bytes).expect_err("an inflated length should be rejected");
        assert!(matches!(err, Iso8211Error::RecordRunsPastEof { .. }));
    }

    #[test]
    fn garbage_leader_numbers_are_an_error_not_a_panic() {
        let mut bytes = build_record(b'D', &[("PNO", &["1"])]);
        bytes[12..17].copy_from_slice(b"XXXXX");
        let err = read_records(&bytes).expect_err("a non-numeric base address should be rejected");
        assert!(matches!(err, Iso8211Error::BadLeaderNumber { .. }));
    }
}
