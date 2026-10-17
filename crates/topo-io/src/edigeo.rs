//! EDIGEO ("Echange de Données Informatisé GEOgraphique") file inspection.
//!
//! EDIGEO is the container format French cadastral vector exports (PCI Vecteur) and other IGN
//! products are distributed in. Its generic transport layer is plain ISO 8211 (see
//! [`crate::iso8211`]), but *which* field and subfield tags carry which meaning - point
//! coordinates, the ordered list of points making an arc, the ordered list of arcs making a
//! parcel's boundary, the cadastral section/numéro attributes - is defined by the EDIGEO
//! specification's object/relation nomenclature, not by the container format itself.
//!
//! **Scope, stated plainly**: this increment reads the container correctly and lets a caller
//! (or the `topo.inspect_edigeo` command, for a person) see exactly what tags and subfields a
//! real file actually contains. It deliberately does **not** hard-code a geometry-extraction
//! mapping (which field holds a point's X/Y, which relation chains arcs into a parcel ring) -
//! doing that from memory, without a real sample file to check it against, risks silently
//! mis-reading coordinates or boundaries while looking like it worked. The next increment
//! (`topo.import_edigeo`, reconstructing parcel polylines) should be written against the tags
//! a real export actually shows in this inspection, not guessed ones.

use std::collections::BTreeMap;

use crate::iso8211::{self, Iso8211Error, Record};

/// Hard cap on how many sample records [`inspect`] echoes back, independent of how many records
/// the file actually has - a safeguard against a hostile or very large file blowing up the
/// result size, not a limit on how many records are read and counted.
const MAX_SAMPLE_RECORDS: usize = 20;

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum EdigeoError {
    #[error(transparent)]
    Container(#[from] Iso8211Error),
}

/// One field tag's shape, aggregated across every data record that carries it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FieldTagStats {
    pub record_count: usize,
    pub subfield_counts_seen: Vec<usize>,
}

/// A summary of an EDIGEO (ISO 8211) file's structure: how many records it has, which field
/// tags appear and how often, and a small sample of the first few data records in full, so a
/// person (or a later increment) can see the real shape of the file without guessing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EdigeoSummary {
    pub total_records: usize,
    pub has_ddr: bool,
    /// Field tag → stats, in first-seen order (a `BTreeMap` for a stable, readable listing).
    pub field_tags: BTreeMap<String, FieldTagStats>,
    pub sample: Vec<Record>,
}

/// Reads `bytes` as an ISO 8211 / EDIGEO file and summarizes its structure. Never fails on
/// content it merely doesn't recognise semantically - only on bytes that aren't a well-formed
/// ISO 8211 container at all (see [`Iso8211Error`]).
pub fn inspect(bytes: &[u8]) -> Result<EdigeoSummary, EdigeoError> {
    let records = iso8211::read_records(bytes)?;

    let mut summary = EdigeoSummary { total_records: records.len(), ..Default::default() };
    for (i, record) in records.iter().enumerate() {
        if i == 0 && record.leader_identifier == 'L' {
            summary.has_ddr = true;
        }
        for field in &record.fields {
            let stats = summary.field_tags.entry(field.tag.clone()).or_default();
            stats.record_count += 1;
            if !stats.subfield_counts_seen.contains(&field.subfields.len()) {
                stats.subfield_counts_seen.push(field.subfields.len());
            }
        }
        if summary.sample.len() < MAX_SAMPLE_RECORDS {
            summary.sample.push(record.clone());
        }
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::iso8211::test_support::build_record;

    #[test]
    fn summarizes_tag_frequency_and_subfield_shapes_across_records() {
        let mut bytes = build_record(b'L', &[("0000", &["DDR"])]);
        bytes.extend(build_record(b'D', &[("GEO", &["100.0", "200.0"]), ("LAB", &["P1"])]));
        bytes.extend(build_record(b'D', &[("GEO", &["110.0", "210.0", "5.0"]), ("LAB", &["P2"])]));

        let summary = inspect(&bytes).expect("a well-formed file should inspect cleanly");
        assert_eq!(summary.total_records, 3);
        assert!(summary.has_ddr);

        let geo = summary.field_tags.get("GEO").expect("GEO tag should be counted");
        assert_eq!(geo.record_count, 2);
        assert_eq!(geo.subfield_counts_seen.len(), 2, "one 2D and one 3D shape should both be recorded");

        let lab = summary.field_tags.get("LAB").expect("LAB tag should be counted");
        assert_eq!(lab.record_count, 2);
        assert_eq!(lab.subfield_counts_seen, vec![1]);
    }

    #[test]
    fn empty_file_summarizes_to_nothing_rather_than_erroring() {
        let summary = inspect(&[]).expect("an empty file is not malformed");
        assert_eq!(summary.total_records, 0);
        assert!(summary.field_tags.is_empty());
    }

    #[test]
    fn sample_is_capped_even_with_many_records() {
        let mut bytes = Vec::new();
        for i in 0..(MAX_SAMPLE_RECORDS + 10) {
            bytes.extend(build_record(b'D', &[("PNO", &[&i.to_string()])]));
        }
        let summary = inspect(&bytes).expect("should inspect");
        assert_eq!(summary.total_records, MAX_SAMPLE_RECORDS + 10);
        assert_eq!(summary.sample.len(), MAX_SAMPLE_RECORDS);
    }

    #[test]
    fn a_malformed_container_is_an_error_not_a_panic() {
        let err = inspect(&[1, 2, 3]);
        assert!(err.is_err());
    }
}
