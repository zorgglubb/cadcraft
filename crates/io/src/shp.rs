//! Esri Shapefile read/write (`.shp` geometry + `.shx` index + `.dbf` attributes), from the
//! public technical description ("ESRI Shapefile Technical Description", July 1998) and the
//! dBASE III field-descriptor layout it reuses for `.dbf` - no proprietary source consulted.
//!
//! **Scope, stated plainly**: this reads/writes the three plain (non-Z, non-M) shape types that
//! cover ordinary cadastral/GIS interchange - Point (1), PolyLine (3) and Polygon (5) - plus
//! their `Z` variants (11/13/15) for elevation, read-only: a Z value is kept when present, but
//! `write` always emits plain 2D shapes, since CADCraft entities don't carry a per-vertex Z
//! concept beyond POINT/line endpoints. MultiPoint, PatchM/PatchZ and the `M` (measure) channel
//! are not handled; a file using them is reported through `ShpError` rather than silently
//! misread. A `.shp` with no matching `.dbf` reads with empty attributes rather than failing.

use std::io::{Read, Write};

use cadcraft_geom::Vec3;

#[derive(Debug, Clone, thiserror::Error, PartialEq)]
pub enum ShpError {
    #[error("not a shapefile: file code {0:#x}, expected 0x270a (9994)")]
    BadFileCode(u32),
    #[error("unsupported shape type {0} (only Point/PolyLine/Polygon, plain or Z, are read)")]
    UnsupportedShapeType(i32),
    #[error("truncated or corrupt shapefile: {0}")]
    Truncated(&'static str),
    #[error("{0} has more than {1} {2}, rejecting as hostile input")]
    TooLarge(&'static str, usize, &'static str),
    #[error("I/O error: {0}")]
    Io(String),
}

impl From<std::io::Error> for ShpError {
    fn from(e: std::io::Error) -> Self {
        ShpError::Io(e.to_string())
    }
}

/// Hard caps against a corrupt or hostile file.
const MAX_RECORDS: usize = 5_000_000;
const MAX_PARTS: usize = 1_000_000;
const MAX_POINTS: usize = 20_000_000;
const MAX_FIELDS: usize = 2_048;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShapeKind {
    Point,
    PolyLine,
    Polygon,
}

/// One shapefile record's geometry: a single point, or one or more parts (rings for a polygon,
/// separate strands for a polyline), each a list of vertices in file order.
#[derive(Debug, Clone, PartialEq)]
pub enum Geometry {
    Point(Vec3),
    Parts { kind: ShapeKind, parts: Vec<Vec<Vec3>> },
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Feature {
    pub geometry: Option<Geometry>,
    /// Attribute column name → text value, in `.dbf` column order.
    pub attributes: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShpImport {
    pub features: Vec<Feature>,
    /// Records whose shape type or content didn't fit this reader's scope, kept out of
    /// `features` rather than aborting the whole file.
    pub skipped_records: usize,
}

fn read_exact_or<const N: usize>(r: &mut impl Read) -> Result<[u8; N], ShpError> {
    let mut buf = [0u8; N];
    r.read_exact(&mut buf).map_err(|_| ShpError::Truncated("unexpected end of file"))?;
    Ok(buf)
}
fn be_i32(r: &mut impl Read) -> Result<i32, ShpError> {
    Ok(i32::from_be_bytes(read_exact_or(r)?))
}
fn le_i32(r: &mut impl Read) -> Result<i32, ShpError> {
    Ok(i32::from_le_bytes(read_exact_or(r)?))
}
fn le_f64(r: &mut impl Read) -> Result<f64, ShpError> {
    Ok(f64::from_le_bytes(read_exact_or(r)?))
}

const SHP_POINT: i32 = 1;
const SHP_POLYLINE: i32 = 3;
const SHP_POLYGON: i32 = 5;
const SHP_POINT_Z: i32 = 11;
const SHP_POLYLINE_Z: i32 = 13;
const SHP_POLYGON_Z: i32 = 15;

/// Reads one `.shp` file's geometry (no attributes - pair with [`read_dbf`]).
pub fn read_shp(bytes: &[u8]) -> Result<Vec<Option<Geometry>>, ShpError> {
    let mut r = bytes;
    let file_code = be_i32(&mut r)? as u32;
    if file_code != 9994 {
        return Err(ShpError::BadFileCode(file_code));
    }
    // Skip the rest of the fixed 100-byte header (we don't need the bbox or overall shape type).
    if bytes.len() < 100 {
        return Err(ShpError::Truncated("header shorter than 100 bytes"));
    }
    r = &bytes[100..];

    let mut out = Vec::new();
    while !r.is_empty() {
        if out.len() >= MAX_RECORDS {
            return Err(ShpError::TooLarge(".shp", MAX_RECORDS, "records"));
        }
        let _record_number = be_i32(&mut r)?;
        let content_words = be_i32(&mut r)?;
        let content_len = usize::try_from(content_words).map_err(|_| ShpError::Truncated("negative content length"))? * 2;
        if content_len > r.len() {
            return Err(ShpError::Truncated("record content runs past end of file"));
        }
        let (content, rest) = r.split_at(content_len);
        r = rest;
        out.push(read_record(content)?);
    }
    Ok(out)
}

fn read_record(content: &[u8]) -> Result<Option<Geometry>, ShpError> {
    let mut r = content;
    let shape_type = le_i32(&mut r)?;
    if shape_type == 0 {
        return Ok(None); // Null shape.
    }
    match shape_type {
        SHP_POINT | SHP_POINT_Z => {
            let x = le_f64(&mut r)?;
            let y = le_f64(&mut r)?;
            let z = if shape_type == SHP_POINT_Z { le_f64(&mut r).unwrap_or(0.0) } else { 0.0 };
            Ok(Some(Geometry::Point(Vec3::new(x, y, z))))
        }
        SHP_POLYLINE | SHP_POLYGON | SHP_POLYLINE_Z | SHP_POLYGON_Z => {
            let kind = if matches!(shape_type, SHP_POLYLINE | SHP_POLYLINE_Z) { ShapeKind::PolyLine } else { ShapeKind::Polygon };
            let has_z = matches!(shape_type, SHP_POLYLINE_Z | SHP_POLYGON_Z);
            // Skip the record's own bounding box.
            for _ in 0..4 {
                le_f64(&mut r)?;
            }
            let num_parts = usize::try_from(le_i32(&mut r)?).map_err(|_| ShpError::Truncated("negative part count"))?;
            let num_points = usize::try_from(le_i32(&mut r)?).map_err(|_| ShpError::Truncated("negative point count"))?;
            if num_parts > MAX_PARTS {
                return Err(ShpError::TooLarge("a record", MAX_PARTS, "parts"));
            }
            if num_points > MAX_POINTS {
                return Err(ShpError::TooLarge("a record", MAX_POINTS, "points"));
            }
            let mut part_starts = Vec::with_capacity(num_parts);
            for _ in 0..num_parts {
                part_starts.push(usize::try_from(le_i32(&mut r)?).map_err(|_| ShpError::Truncated("negative part index"))?);
            }
            let mut xy = Vec::with_capacity(num_points);
            for _ in 0..num_points {
                xy.push((le_f64(&mut r)?, le_f64(&mut r)?));
            }
            let mut z = vec![0.0; num_points];
            if has_z {
                // Z range (2 doubles) then one Z per point, in the same order as the XY array.
                let _ = le_f64(&mut r);
                let _ = le_f64(&mut r);
                for zi in z.iter_mut().take(num_points) {
                    *zi = le_f64(&mut r).unwrap_or(0.0);
                }
            }
            let mut parts = Vec::with_capacity(num_parts.max(1));
            for (i, &start) in part_starts.iter().enumerate() {
                let end = part_starts.get(i + 1).copied().unwrap_or(num_points);
                let end = end.min(num_points);
                let start = start.min(end);
                parts.push((start..end).map(|j| Vec3::new(xy[j].0, xy[j].1, z[j])).collect());
            }
            if parts.is_empty() && !xy.is_empty() {
                parts.push(xy.iter().zip(&z).map(|(&(x, y), &zz)| Vec3::new(x, y, zz)).collect());
            }
            Ok(Some(Geometry::Parts { kind, parts }))
        }
        other => Err(ShpError::UnsupportedShapeType(other)),
    }
}

/// Decodes a `.dbf` text field. dBASE stores text in a single-byte codepage, almost always
/// Windows-1252 (or its DOS/Latin-1 cousins, which agree with it on the accented letters French
/// surveyors actually type) rather than UTF-8 - so a lossy UTF-8 decode turns `é`/`è`/`à`/`ç`
/// into the replacement character `�` on any file that isn't already UTF-8. Valid UTF-8 bytes
/// are kept as UTF-8 (a `.dbf` written by a UTF-8-aware tool still reads correctly); anything
/// else is decoded as Windows-1252, which is also a correct reading of plain Latin-1.
fn decode_dbf_text(raw: &[u8]) -> String {
    if let Ok(s) = std::str::from_utf8(raw) {
        return s.to_string();
    }
    raw.iter().map(|&b| cp1252_char(b)).collect()
}

/// Windows-1252 code point for one byte. Bytes 0x00-0x7F and 0xA0-0xFF match Unicode directly
/// (Latin-1); 0x80-0x9F hold Windows-1252's own punctuation/currency marks instead of the C1
/// control codes Latin-1 would give them, so those are mapped explicitly (`U+FFFD` for the five
/// bytes Windows-1252 leaves undefined).
fn cp1252_char(b: u8) -> char {
    match b {
        0x80 => '\u{20AC}',
        0x82 => '\u{201A}',
        0x83 => '\u{0192}',
        0x84 => '\u{201E}',
        0x85 => '\u{2026}',
        0x86 => '\u{2020}',
        0x87 => '\u{2021}',
        0x88 => '\u{02C6}',
        0x89 => '\u{2030}',
        0x8A => '\u{0160}',
        0x8B => '\u{2039}',
        0x8C => '\u{0152}',
        0x8E => '\u{017D}',
        0x91 => '\u{2018}',
        0x92 => '\u{2019}',
        0x93 => '\u{201C}',
        0x94 => '\u{201D}',
        0x95 => '\u{2022}',
        0x96 => '\u{2013}',
        0x97 => '\u{2014}',
        0x98 => '\u{02DC}',
        0x99 => '\u{2122}',
        0x9A => '\u{0161}',
        0x9B => '\u{203A}',
        0x9C => '\u{0153}',
        0x9E => '\u{017E}',
        0x9F => '\u{0178}',
        0x81 | 0x8D | 0x8F | 0x90 | 0x9D => '\u{FFFD}',
        _ => b as char,
    }
}

/// Reads a `.dbf` file's records as column-name/text-value pairs, one `Vec` per record, in
/// field-definition order. A `.dbf` this reader can't make sense of (bad header, unknown field
/// layout) reads as an empty attribute set per record rather than failing the whole import.
pub fn read_dbf(bytes: &[u8]) -> Vec<Vec<(String, String)>> {
    read_dbf_checked(bytes).unwrap_or_default()
}

fn read_dbf_checked(bytes: &[u8]) -> Option<Vec<Vec<(String, String)>>> {
    if bytes.len() < 32 {
        return None;
    }
    let num_records = u32::from_le_bytes(bytes.get(4..8)?.try_into().ok()?) as usize;
    let header_size = u16::from_le_bytes(bytes.get(8..10)?.try_into().ok()?) as usize;
    let record_size = u16::from_le_bytes(bytes.get(10..12)?.try_into().ok()?) as usize;
    if num_records > MAX_RECORDS || header_size < 32 || header_size > bytes.len() || record_size == 0 {
        return None;
    }

    let mut fields: Vec<(String, usize)> = Vec::new();
    let mut off = 32;
    while off + 32 <= header_size && bytes.get(off) != Some(&0x0D) {
        if fields.len() >= MAX_FIELDS {
            return None;
        }
        let name_bytes = bytes.get(off..off + 11)?;
        let name_len = name_bytes.iter().position(|&b| b == 0).unwrap_or(11);
        let name = decode_dbf_text(&name_bytes[..name_len]).trim().to_string();
        let len = *bytes.get(off + 16)? as usize;
        fields.push((name, len));
        off += 32;
    }

    let mut out = Vec::with_capacity(num_records.min(MAX_RECORDS));
    let mut rec_off = header_size;
    for _ in 0..num_records {
        let Some(record) = bytes.get(rec_off..rec_off + record_size) else { break };
        rec_off += record_size;
        if record.first() == Some(&b'*') {
            continue; // deleted record
        }
        let mut row = Vec::with_capacity(fields.len());
        let mut foff = 1; // skip the deletion-flag byte
        for (name, len) in &fields {
            let Some(raw) = record.get(foff..foff + len) else { break };
            foff += len;
            row.push((name.clone(), decode_dbf_text(raw).trim().to_string()));
        }
        out.push(row);
    }
    Some(out)
}

/// Joins `.shp` geometry with `.dbf` attributes by record order (the shapefile convention); a
/// missing or shorter `.dbf` leaves the remaining features with no attributes.
pub fn read(shp_bytes: &[u8], dbf_bytes: Option<&[u8]>) -> Result<ShpImport, ShpError> {
    let geoms = read_shp(shp_bytes)?;
    let attrs = dbf_bytes.map(read_dbf).unwrap_or_default();
    let mut skipped_records = 0;
    let features = geoms
        .into_iter()
        .enumerate()
        .filter_map(|(i, g)| {
            if g.is_none() {
                skipped_records += 1;
            }
            g.map(|geometry| Feature { geometry: Some(geometry), attributes: attrs.get(i).cloned().unwrap_or_default() })
        })
        .collect();
    Ok(ShpImport { features, skipped_records })
}

// ---------------------------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------------------------

/// One output feature: its 2D geometry (Z is not written - see the module's scope note) and the
/// flat attribute row that becomes its `.dbf` record.
pub struct OutFeature<'a> {
    pub geometry: &'a Geometry,
    pub attributes: &'a [(String, String)],
}

/// Writes a `.shp` + `.shx` + `.dbf` triple. All features must share one shape kind, chosen from
/// the first feature's geometry (an Esri shapefile cannot mix shape types); a feature of a
/// different kind is skipped and counted in the returned `skipped` total rather than corrupting
/// the file. Every feature's attribute row is expected to share the same column names as the
/// first (shorter/longer rows are padded/truncated); columns are written as 80-character text
/// fields, generous enough for this reader's own xdata-derived values.
pub fn write(features: &[OutFeature]) -> (Vec<u8> /* shp */, Vec<u8> /* shx */, Vec<u8> /* dbf */, usize /* skipped */) {
    let kind_of = |g: &Geometry| match g {
        Geometry::Point(_) => None,
        Geometry::Parts { kind, .. } => Some(*kind),
    };
    let wants_points = features.first().is_some_and(|f| matches!(f.geometry, Geometry::Point(_)));
    let wants_kind = if wants_points { None } else { features.iter().find_map(|f| kind_of(f.geometry)) };

    let mut shp = Vec::new();
    let mut shx = Vec::new();
    let mut records: Vec<Vec<u8>> = Vec::new();
    let mut skipped = 0;
    for f in features {
        let ok = match (&f.geometry, wants_points) {
            (Geometry::Point(_), true) => true,
            (Geometry::Parts { kind, .. }, false) => Some(*kind) == wants_kind,
            _ => false,
        };
        if !ok {
            skipped += 1;
            continue;
        }
        records.push(encode_record(f.geometry));
    }

    let shape_type = if wants_points {
        SHP_POINT
    } else if wants_kind == Some(ShapeKind::Polygon) {
        SHP_POLYGON
    } else {
        SHP_POLYLINE
    };
    let (bbox_min, bbox_max) = bounds(features.iter().map(|f| f.geometry));

    let mut offset_words = 50i32; // 100-byte header = 50 words.
    for rec in &records {
        let content_words = (rec.len() / 2) as i32;
        shx.extend((0i32).to_be_bytes()); // overwritten below with the real index, sequentially
        let _ = content_words;
    }
    shx.clear();

    let mut body = Vec::new();
    for (i, rec) in records.iter().enumerate() {
        let content_words = (rec.len() / 2) as i32;
        body.extend(((i + 1) as i32).to_be_bytes());
        body.extend(content_words.to_be_bytes());
        body.extend(rec.iter());
        shx.extend(offset_words.to_be_bytes());
        shx.extend(content_words.to_be_bytes());
        offset_words += 4 + content_words;
    }
    let file_words = offset_words;

    shp.extend(shp_header(file_words, shape_type, bbox_min, bbox_max));
    shp.extend(body);

    let mut shx_full = Vec::new();
    shx_full.extend(shp_header(50 + (records.len() as i32) * 4, shape_type, bbox_min, bbox_max));
    shx_full.extend(shx);

    let dbf = write_dbf(features, &records, wants_points, wants_kind);
    (shp, shx_full, dbf, skipped)
}

fn shp_header(file_words: i32, shape_type: i32, bbox_min: (f64, f64), bbox_max: (f64, f64)) -> Vec<u8> {
    let mut h = Vec::with_capacity(100);
    h.extend(9994i32.to_be_bytes());
    h.extend([0u8; 20]); // 5 unused int32s
    h.extend(file_words.to_be_bytes());
    h.extend(1000i32.to_le_bytes());
    h.extend(shape_type.to_le_bytes());
    h.extend(bbox_min.0.to_le_bytes());
    h.extend(bbox_min.1.to_le_bytes());
    h.extend(bbox_max.0.to_le_bytes());
    h.extend(bbox_max.1.to_le_bytes());
    h.extend([0u8; 32]); // Zmin, Zmax, Mmin, Mmax - unused, plain shapes only.
    h
}

fn bounds<'a>(geoms: impl Iterator<Item = &'a Geometry>) -> ((f64, f64), (f64, f64)) {
    let (mut min, mut max) = ((f64::INFINITY, f64::INFINITY), (f64::NEG_INFINITY, f64::NEG_INFINITY));
    let mut see = |x: f64, y: f64| {
        if x.is_finite() && y.is_finite() {
            min.0 = min.0.min(x);
            min.1 = min.1.min(y);
            max.0 = max.0.max(x);
            max.1 = max.1.max(y);
        }
    };
    for g in geoms {
        match g {
            Geometry::Point(p) => see(p.x, p.y),
            Geometry::Parts { parts, .. } => {
                for part in parts {
                    for p in part {
                        see(p.x, p.y);
                    }
                }
            }
        }
    }
    if !min.0.is_finite() {
        min = (0.0, 0.0);
        max = (0.0, 0.0);
    }
    (min, max)
}

fn encode_record(g: &Geometry) -> Vec<u8> {
    let mut b = Vec::new();
    match g {
        Geometry::Point(p) => {
            b.extend(SHP_POINT.to_le_bytes());
            b.extend(p.x.to_le_bytes());
            b.extend(p.y.to_le_bytes());
        }
        Geometry::Parts { kind, parts } => {
            let shape_type = if *kind == ShapeKind::Polygon { SHP_POLYGON } else { SHP_POLYLINE };
            b.extend(shape_type.to_le_bytes());
            let (bmin, bmax) = bounds(std::iter::once(g));
            b.extend(bmin.0.to_le_bytes());
            b.extend(bmin.1.to_le_bytes());
            b.extend(bmax.0.to_le_bytes());
            b.extend(bmax.1.to_le_bytes());
            b.extend((parts.len() as i32).to_le_bytes());
            let total_points: usize = parts.iter().map(Vec::len).sum();
            b.extend((total_points as i32).to_le_bytes());
            let mut running = 0i32;
            for part in parts {
                b.extend(running.to_le_bytes());
                running += part.len() as i32;
            }
            for part in parts {
                for p in part {
                    b.extend(p.x.to_le_bytes());
                    b.extend(p.y.to_le_bytes());
                }
            }
        }
    }
    b
}

const DBF_FIELD_WIDTH: usize = 80;

fn write_dbf(features: &[OutFeature], kept_records: &[Vec<u8>], wants_points: bool, wants_kind: Option<ShapeKind>) -> Vec<u8> {
    let columns: Vec<String> = features.first().map(|f| f.attributes.iter().map(|(k, _)| k.clone()).collect()).unwrap_or_default();
    let kept: Vec<&[(String, String)]> = features
        .iter()
        .filter(|f| match (&f.geometry, wants_points) {
            (Geometry::Point(_), true) => true,
            (Geometry::Parts { kind, .. }, false) => Some(*kind) == wants_kind,
            _ => false,
        })
        .map(|f| f.attributes)
        .collect();
    debug_assert_eq!(kept.len(), kept_records.len());

    let header_size = 32 + columns.len() * 32 + 1;
    let record_size = 1 + columns.len() * DBF_FIELD_WIDTH;
    let mut h = Vec::new();
    h.push(0x03); // dBASE III, no memo
    h.extend([0u8; 3]); // date (unused)
    h.extend((kept.len() as u32).to_le_bytes());
    h.extend((header_size as u16).to_le_bytes());
    h.extend((record_size as u16).to_le_bytes());
    h.extend([0u8; 20]);
    for name in &columns {
        let mut field = [0u8; 32];
        let bytes = name.as_bytes();
        let n = bytes.len().min(10);
        field[..n].copy_from_slice(&bytes[..n]);
        field[11] = b'C'; // Character
        field[16] = DBF_FIELD_WIDTH as u8;
        h.extend(field);
    }
    h.push(0x0D); // field descriptor terminator

    let mut body = Vec::new();
    for row in &kept {
        body.push(b' '); // not deleted
        for name in &columns {
            let value = row.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str()).unwrap_or("");
            let mut field = vec![b' '; DBF_FIELD_WIDTH];
            let bytes = value.as_bytes();
            let n = bytes.len().min(DBF_FIELD_WIDTH);
            field[..n].copy_from_slice(&bytes[..n]);
            body.extend(field);
        }
    }
    h.extend(body);
    h.push(0x1A); // EOF marker
    h
}

/// Convenience: writes the three files to disk at `base` + `.shp`/`.shx`/`.dbf`.
pub fn write_to_files(base: &std::path::Path, features: &[OutFeature]) -> Result<usize, ShpError> {
    let (shp, shx, dbf, skipped) = write(features);
    let mut f = std::fs::File::create(base.with_extension("shp"))?;
    f.write_all(&shp)?;
    let mut f = std::fs::File::create(base.with_extension("shx"))?;
    f.write_all(&shx)?;
    let mut f = std::fs::File::create(base.with_extension("dbf"))?;
    f.write_all(&dbf)?;
    Ok(skipped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square_polygon() -> Geometry {
        Geometry::Parts {
            kind: ShapeKind::Polygon,
            parts: vec![vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(10.0, 0.0, 0.0),
                Vec3::new(10.0, 10.0, 0.0),
                Vec3::new(0.0, 10.0, 0.0),
                Vec3::new(0.0, 0.0, 0.0),
            ]],
        }
    }

    #[test]
    fn writes_then_reads_back_a_polygon_with_an_attribute() {
        let geom = square_polygon();
        let attrs = vec![("ID".to_string(), "P1".to_string())];
        let (shp, _shx, dbf, skipped) = write(&[OutFeature { geometry: &geom, attributes: &attrs }]);
        assert_eq!(skipped, 0);

        let import = read(&shp, Some(&dbf)).expect("round trip should read back");
        assert_eq!(import.features.len(), 1);
        let f = &import.features[0];
        let Some(Geometry::Parts { kind, parts }) = &f.geometry else { panic!("expected a polygon: {f:?}") };
        assert_eq!(*kind, ShapeKind::Polygon);
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].len(), 5);
        assert!((parts[0][2].x - 10.0).abs() < 1e-9 && (parts[0][2].y - 10.0).abs() < 1e-9);
        assert_eq!(f.attributes, vec![("ID".to_string(), "P1".to_string())]);
    }

    #[test]
    fn writes_then_reads_back_points() {
        let a = Geometry::Point(Vec3::new(1.0, 2.0, 0.0));
        let b = Geometry::Point(Vec3::new(3.0, 4.0, 0.0));
        let (shp, _shx, dbf, skipped) = write(&[OutFeature { geometry: &a, attributes: &[] }, OutFeature { geometry: &b, attributes: &[] }]);
        assert_eq!(skipped, 0);
        let import = read(&shp, Some(&dbf)).expect("should read");
        assert_eq!(import.features.len(), 2);
        assert_eq!(import.features[0].geometry, Some(Geometry::Point(Vec3::new(1.0, 2.0, 0.0))));
        assert_eq!(import.features[1].geometry, Some(Geometry::Point(Vec3::new(3.0, 4.0, 0.0))));
    }

    #[test]
    fn a_feature_of_a_different_shape_kind_is_skipped_not_mixed_in() {
        let poly = square_polygon();
        let line = Geometry::Parts { kind: ShapeKind::PolyLine, parts: vec![vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 1.0, 0.0)]] };
        let (_shp, _shx, _dbf, skipped) = write(&[OutFeature { geometry: &poly, attributes: &[] }, OutFeature { geometry: &line, attributes: &[] }]);
        assert_eq!(skipped, 1);
    }

    #[test]
    fn bad_file_code_is_an_error_not_a_panic() {
        let bytes = [0u8; 100];
        assert!(matches!(read_shp(&bytes), Err(ShpError::BadFileCode(0))));
    }

    #[test]
    fn truncated_file_is_an_error_not_a_panic() {
        let bytes = [0u8; 10];
        assert!(read_shp(&bytes).is_err());
    }

    #[test]
    fn missing_dbf_imports_geometry_with_empty_attributes() {
        let geom = square_polygon();
        let (shp, _shx, _dbf, _) = write(&[OutFeature { geometry: &geom, attributes: &[] }]);
        let import = read(&shp, None).expect("should read without a dbf");
        assert_eq!(import.features.len(), 1);
        assert!(import.features[0].attributes.is_empty());
    }

    #[test]
    fn a_shape_type_this_reader_does_not_handle_is_reported_not_misread() {
        // MultiPoint (8): shape type only, no further bytes needed to trigger the error.
        let mut shp = Vec::new();
        shp.extend(9994i32.to_be_bytes());
        shp.extend([0u8; 96]);
        shp.extend(1i32.to_be_bytes()); // record number
        shp.extend(2i32.to_be_bytes()); // content words (4 bytes)
        shp.extend(8i32.to_le_bytes()); // shape type 8 = MultiPoint
        assert!(matches!(read_shp(&shp), Err(ShpError::UnsupportedShapeType(8))));
    }
}
