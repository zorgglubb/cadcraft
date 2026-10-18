//! Pixel dimensions of a JPEG, TIFF or PNG image read from its **header only** - never the pixel
//! data - so a multi-gigabyte aerial photo can be placed with a worldfile ([`crate::worldfile`])
//! without loading the whole file into memory. Formats and marker/tag/chunk layouts are from the
//! public JPEG (ITU-T T.81 / JFIF), TIFF 6.0 and PNG (ISO/IEC 15948) specifications.

use std::io::{Read, Seek, SeekFrom};

#[derive(Debug, Clone, thiserror::Error, PartialEq)]
pub enum ImageHeaderError {
    #[error("not a recognised JPEG, TIFF or PNG file (no matching magic bytes)")]
    UnknownFormat,
    #[error("truncated or corrupt {0} header: {1}")]
    Truncated(&'static str, &'static str),
    #[error("I/O error: {0}")]
    Io(String),
}

impl From<std::io::Error> for ImageHeaderError {
    fn from(e: std::io::Error) -> Self {
        ImageHeaderError::Io(e.to_string())
    }
}

/// Hard cap on how many bytes of a JPEG's marker stream we'll walk before giving up - guards
/// against a corrupt file with a marker chain that never reaches SOF or EOI.
const MAX_JPEG_SCAN_BYTES: u64 = 64 * 1024 * 1024;
/// Hard cap on how many IFD entries a TIFF directory may declare.
const MAX_TIFF_ENTRIES: u32 = 65536;

/// Reads just the pixel width/height of a JPEG or TIFF file via a seekable reader - a
/// `std::fs::File` for a file on disk, so a 1GB image costs only a handful of small reads.
pub fn dimensions<R: Read + Seek>(r: &mut R) -> Result<(u32, u32), ImageHeaderError> {
    let mut magic = [0u8; 8];
    r.seek(SeekFrom::Start(0))?;
    let n = r.read(&mut magic)?;
    if n >= 2 && magic[0] == 0xFF && magic[1] == 0xD8 {
        r.seek(SeekFrom::Start(0))?;
        jpeg_dimensions(r)
    } else if n >= 4 && (magic[..4] == [0x49, 0x49, 0x2A, 0x00] || magic[..4] == [0x4D, 0x4D, 0x00, 0x2A]) {
        r.seek(SeekFrom::Start(0))?;
        tiff_dimensions(r)
    } else if n >= 8 && magic == [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A] {
        png_dimensions(r)
    } else {
        Err(ImageHeaderError::UnknownFormat)
    }
}

/// PNG: an 8-byte signature, then its first chunk is always a 13-byte `IHDR` - a 4-byte length,
/// the `IHDR` tag, then big-endian width and height (ISO/IEC 15948 §11.2.2).
fn png_dimensions<R: Read + Seek>(r: &mut R) -> Result<(u32, u32), ImageHeaderError> {
    r.seek(SeekFrom::Start(8))?;
    let mut chunk_header = [0u8; 8];
    r.read_exact(&mut chunk_header).map_err(|_| ImageHeaderError::Truncated("PNG", "missing IHDR chunk"))?;
    if &chunk_header[4..8] != b"IHDR" {
        return Err(ImageHeaderError::Truncated("PNG", "first chunk is not IHDR"));
    }
    let mut dims = [0u8; 8];
    r.read_exact(&mut dims).map_err(|_| ImageHeaderError::Truncated("PNG", "truncated IHDR chunk"))?;
    let width = u32::from_be_bytes([dims[0], dims[1], dims[2], dims[3]]);
    let height = u32::from_be_bytes([dims[4], dims[5], dims[6], dims[7]]);
    if width == 0 || height == 0 {
        return Err(ImageHeaderError::Truncated("PNG", "IHDR reports a zero dimension"));
    }
    Ok((width, height))
}

/// Convenience: dimensions of a file on disk, by path.
pub fn dimensions_of_file(path: &std::path::Path) -> Result<(u32, u32), ImageHeaderError> {
    let mut f = std::fs::File::open(path)?;
    dimensions(&mut f)
}

fn read_u16_be<R: Read>(r: &mut R) -> Result<u16, ImageHeaderError> {
    let mut b = [0u8; 2];
    r.read_exact(&mut b).map_err(|_| ImageHeaderError::Truncated("JPEG", "unexpected end of file"))?;
    Ok(u16::from_be_bytes(b))
}

fn jpeg_dimensions<R: Read + Seek>(r: &mut R) -> Result<(u32, u32), ImageHeaderError> {
    // Skip the SOI marker (FFD8), then walk the marker chain looking for a Start-Of-Frame
    // marker (SOF0..SOF15, excluding DHP/JPG extension markers not used in practice), each of
    // which carries height/width right after its length+precision bytes.
    let mut hdr = [0u8; 2];
    r.read_exact(&mut hdr).map_err(|_| ImageHeaderError::Truncated("JPEG", "missing SOI marker"))?;
    if hdr != [0xFF, 0xD8] {
        return Err(ImageHeaderError::Truncated("JPEG", "missing SOI marker"));
    }

    let mut scanned: u64 = 2;
    loop {
        if scanned > MAX_JPEG_SCAN_BYTES {
            return Err(ImageHeaderError::Truncated("JPEG", "no SOF marker found within the scan limit"));
        }
        // Markers are prefixed by 0xFF; skip any fill bytes (0xFF 0xFF ...).
        let mut marker;
        loop {
            let mut b = [0u8; 1];
            r.read_exact(&mut b).map_err(|_| ImageHeaderError::Truncated("JPEG", "unexpected end of file scanning for a marker"))?;
            scanned += 1;
            if b[0] != 0xFF {
                return Err(ImageHeaderError::Truncated("JPEG", "expected a marker (0xFF prefix)"));
            }
            let mut m = [0u8; 1];
            r.read_exact(&mut m).map_err(|_| ImageHeaderError::Truncated("JPEG", "unexpected end of file scanning for a marker"))?;
            scanned += 1;
            marker = m[0];
            if marker != 0xFF {
                break; // a real marker, not a fill byte
            }
        }
        if marker == 0xD8 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            continue; // SOI / TEM / RSTn carry no length field
        }
        if marker == 0xD9 {
            return Err(ImageHeaderError::Truncated("JPEG", "reached end-of-image before any SOF marker"));
        }
        let len = read_u16_be(r)?;
        scanned += 2;
        if len < 2 {
            return Err(ImageHeaderError::Truncated("JPEG", "marker segment length is too short"));
        }
        let is_sof = matches!(marker, 0xC0..=0xCF) && marker != 0xC4 && marker != 0xC8 && marker != 0xCC;
        if is_sof {
            // Segment: [precision:1][height:2][width:2][...]
            let mut b = [0u8; 5];
            r.read_exact(&mut b).map_err(|_| ImageHeaderError::Truncated("JPEG", "SOF segment is shorter than expected"))?;
            let height = u16::from_be_bytes([b[1], b[2]]) as u32;
            let width = u16::from_be_bytes([b[3], b[4]]) as u32;
            if height == 0 || width == 0 {
                return Err(ImageHeaderError::Truncated("JPEG", "SOF segment reports a zero dimension"));
            }
            return Ok((width, height));
        }
        // Not a SOF segment: skip its remaining `len - 2` bytes and keep scanning.
        r.seek(SeekFrom::Current(i64::from(len) - 2))?;
        scanned += u64::from(len) - 2;
        if marker == 0xDA {
            // Start of scan - actual pixel data follows; no SOF was found before it.
            return Err(ImageHeaderError::Truncated("JPEG", "reached scan data before any SOF marker"));
        }
    }
}

fn tiff_dimensions<R: Read + Seek>(r: &mut R) -> Result<(u32, u32), ImageHeaderError> {
    let mut hdr = [0u8; 8];
    r.read_exact(&mut hdr).map_err(|_| ImageHeaderError::Truncated("TIFF", "header is shorter than 8 bytes"))?;
    let little_endian = hdr[0] == 0x49;
    let u16_at = |b: &[u8]| -> u16 { if little_endian { u16::from_le_bytes([b[0], b[1]]) } else { u16::from_be_bytes([b[0], b[1]]) } };
    let u32_at =
        |b: &[u8]| -> u32 { if little_endian { u32::from_le_bytes([b[0], b[1], b[2], b[3]]) } else { u32::from_be_bytes([b[0], b[1], b[2], b[3]]) } };

    let first_ifd = u32_at(&hdr[4..8]);
    r.seek(SeekFrom::Start(u64::from(first_ifd)))?;
    let mut count_buf = [0u8; 2];
    r.read_exact(&mut count_buf).map_err(|_| ImageHeaderError::Truncated("TIFF", "truncated at the first IFD"))?;
    let count = u16_at(&count_buf) as u32;
    if count > MAX_TIFF_ENTRIES {
        return Err(ImageHeaderError::Truncated("TIFF", "IFD declares an implausible number of entries"));
    }

    let (mut width, mut height) = (None, None);
    for _ in 0..count {
        let mut entry = [0u8; 12];
        r.read_exact(&mut entry).map_err(|_| ImageHeaderError::Truncated("TIFF", "truncated IFD entry"))?;
        let tag = u16_at(&entry[0..2]);
        let field_type = u16_at(&entry[2..4]);
        // Both ImageWidth (256) and ImageLength (257) are stored as SHORT (3) or LONG (4); the
        // value sits inline in the last 4 bytes of the entry either way (never an offset, since
        // both types fit within 4 bytes).
        let value = match field_type {
            3 => u16_at(&entry[8..10]) as u32,
            4 => u32_at(&entry[8..12]),
            _ => continue,
        };
        match tag {
            256 => width = Some(value),
            257 => height = Some(value),
            _ => {}
        }
        if width.is_some() && height.is_some() {
            break;
        }
    }

    match (width, height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => Ok((w, h)),
        _ => Err(ImageHeaderError::Truncated("TIFF", "ImageWidth/ImageLength tag not found in the first IFD")),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    /// A minimal baseline JPEG byte stream: SOI, APP0 (JFIF), SOF0 (8x6), then we stop (no scan
    /// data needed - dimensions are already read by then).
    fn tiny_jpeg(width: u16, height: u16) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend([0xFF, 0xD8]); // SOI
        b.extend([0xFF, 0xE0, 0x00, 0x10]); // APP0, length 16
        b.extend(b"JFIF\0"); // 5
        b.extend([1, 1]); // version 2
        b.extend([0]); // units 1
        b.extend([0, 1, 0, 1]); // density x/y 4
        b.extend([0, 0]); // thumbnail 2 -> total payload after length = 5+2+1+4+2=14, +2 for len itself = 16. good.
        b.extend([0xFF, 0xC0]); // SOF0
        let seg_len: u16 = 2 + 1 + 2 + 2 + 1 + 3; // len,precision,h,w,numcomp,1 component(3 bytes)
        b.extend(seg_len.to_be_bytes());
        b.push(8); // precision
        b.extend(height.to_be_bytes());
        b.extend(width.to_be_bytes());
        b.push(1); // 1 component
        b.extend([1, 0x11, 0]); // component id, sampling, qtable
        b.extend([0xFF, 0xD9]); // EOI
        b
    }

    #[test]
    fn reads_jpeg_dimensions_from_the_sof_marker() {
        let bytes = tiny_jpeg(800, 600);
        let mut c = Cursor::new(bytes);
        assert_eq!(dimensions(&mut c), Ok((800, 600)));
    }

    #[test]
    fn rejects_a_truncated_jpeg_without_panicking() {
        let mut c = Cursor::new(vec![0xFF, 0xD8, 0xFF]);
        assert!(dimensions(&mut c).is_err());
    }

    fn tiny_tiff_little_endian(width: u32, height: u32) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend([0x49, 0x49, 0x2A, 0x00]); // "II*\0"
        b.extend(8u32.to_le_bytes()); // first IFD offset
        // IFD at offset 8: 2 entries (ImageWidth, ImageLength), both LONG.
        b.extend(2u16.to_le_bytes());
        // Entry: tag, type, count, value(as 4 bytes since LONG fits inline)
        b.extend(256u16.to_le_bytes());
        b.extend(4u16.to_le_bytes()); // LONG
        b.extend(1u32.to_le_bytes()); // count
        b.extend(width.to_le_bytes());
        b.extend(257u16.to_le_bytes());
        b.extend(4u16.to_le_bytes());
        b.extend(1u32.to_le_bytes());
        b.extend(height.to_le_bytes());
        b.extend(0u32.to_le_bytes()); // next IFD offset = 0
        b
    }

    #[test]
    fn reads_tiff_dimensions_from_the_first_ifd() {
        let bytes = tiny_tiff_little_endian(4000, 3000);
        let mut c = Cursor::new(bytes);
        assert_eq!(dimensions(&mut c), Ok((4000, 3000)));
    }

    #[test]
    fn unknown_magic_is_an_error_not_a_panic() {
        let mut c = Cursor::new(vec![0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(dimensions(&mut c), Err(ImageHeaderError::UnknownFormat));
    }

    #[test]
    fn truncated_tiff_is_an_error_not_a_panic() {
        let mut c = Cursor::new(vec![0x49, 0x49, 0x2A, 0x00, 8, 0, 0, 0]);
        assert!(dimensions(&mut c).is_err());
    }

    fn tiny_png(width: u32, height: u32) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend([0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]); // signature
        b.extend(13u32.to_be_bytes()); // IHDR length
        b.extend(b"IHDR");
        b.extend(width.to_be_bytes());
        b.extend(height.to_be_bytes());
        b.extend([8, 2, 0, 0, 0]); // bit depth, colour type, compression, filter, interlace
        b.extend(0u32.to_be_bytes()); // CRC (never checked)
        b
    }

    #[test]
    fn reads_png_dimensions_from_the_ihdr_chunk() {
        let bytes = tiny_png(1920, 1080);
        let mut c = Cursor::new(bytes);
        assert_eq!(dimensions(&mut c), Ok((1920, 1080)));
    }

    #[test]
    fn truncated_png_is_an_error_not_a_panic() {
        let mut c = Cursor::new(vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13]);
        assert!(dimensions(&mut c).is_err());
    }

    #[test]
    fn png_reporting_a_zero_dimension_is_an_error_not_a_panic() {
        let bytes = tiny_png(0, 1080);
        let mut c = Cursor::new(bytes);
        assert!(dimensions(&mut c).is_err());
    }
}
