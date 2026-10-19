//! Reading carnet/CSV text files whose encoding isn't known up front.
//!
//! Field software (GSI exports, CSV/TXT carnets typed or exported from older tools) very often
//! writes accented letters in Windows-1252 rather than UTF-8 - plain `std::fs::read_to_string`
//! then fails outright with an "invalid utf-8" error on the first `é`/`è`/`à`/`ç`. This reads the
//! file as bytes and decodes it as UTF-8 when it is valid UTF-8 (the common case today), falling
//! back to Windows-1252 otherwise - which also reads plain Latin-1 correctly, since the two agree
//! on every byte outside 0x80-0x9F.

/// Reads a text file, decoding it as UTF-8 when valid and as Windows-1252 otherwise. Only the
/// file-not-found/permission class of error is reported; a non-UTF-8 file is never rejected.
pub fn read_to_string(path: &str) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(decode(&bytes))
}

/// Decodes bytes as UTF-8 when valid, otherwise as Windows-1252/Latin-1.
pub fn decode(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| cp1252_char(b)).collect(),
    }
}

/// Windows-1252 code point for one byte - see `cadcraft_io::shp`'s identical table, duplicated
/// here rather than shared across the L2/L3 layer boundary for a handful of lines.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_valid_utf8_unchanged() {
        assert_eq!(decode("Point élévation ça".as_bytes()), "Point élévation ça");
    }

    #[test]
    fn falls_back_to_windows_1252_for_non_utf8_bytes() {
        // "Matricule" with a Windows-1252 'é' (0xE9), which is not valid standalone UTF-8.
        let bytes = [b'M', b'a', b't', 0xE9, b'.'];
        assert_eq!(decode(&bytes), "Mat\u{e9}.");
    }
}
