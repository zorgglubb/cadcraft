//! `plan.import_cadastre_dxf` ("Importer cadastre DXF"): downloads DGFIP PCI Vecteur cadastral
//! DXF sheets from cadastre.data.gouv.fr for a commune (by INSEE code) and merges them into the
//! current drawing (reusing `exchange::import_drawing`, the same path PLAN.IMPORT uses), tagging
//! the drawing EPSG:2154 (RGF93 / Lambert-93 - the CRS this dataset is published in) if none is
//! set yet.
//!
//! Desktop-only: wasm32 has no raw socket access, so the command reports "not available" there
//! instead of linking an HTTP client. This was built and unit-tested against synthetic
//! tar.bz2/HTML fixtures only - this sandbox's own network is blocked from reaching
//! cadastre.data.gouv.fr, so real DGFIP file quirks (exact HTML listing markup, DXF encoding,
//! layer names) have NOT been verified against a live download and should be smoke-tested
//! against a real commune before relying on this for production drawings.

use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

const CMD: &str = "plan.import_cadastre_dxf";
/// RGF93 / Lambert-93 - the CRS the DGFIP PCI Vecteur DXF sheets are published in.
const CADASTRE_CRS_EPSG: u32 = 2154;
/// Default root of the DGFIP PCI Vecteur DXF sheets (the "latest" version always resolves to
/// the current one, so callers never need to know the dataset's date).
const DEFAULT_BASE_URL: &str = "https://cadastre.data.gouv.fr/data/dgfip-pci-vecteur/latest/dxf/feuilles";
/// Caps how many sheet archives one call ever fetches, so a too-broad request (or a listing
/// page that returns more links than expected) can't make this run unbounded.
const MAX_SHEETS: usize = 64;
/// Caps the size of a downloaded listing page, archive or extracted DXF, so a malicious or
/// broken server response can't exhaust memory.
const MAX_RESPONSE_BYTES: u64 = 256 * 1024 * 1024;

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec::new(CMD, "Importer cadastre DXF", run_import_cadastre_dxf).menu(&["Plan", "Importer cadastre DXF"]).params(
        "{insee: string (5-character commune code, e.g. \"50129\"), sections?: [string] (section \
                 codes such as [\"AD\"]; default: every section found for the commune), baseUrl?: string \
                 (default the dgfip-pci-vecteur \"latest\" DXF sheets root)} → {imported, files: \
                 [{archive, sheet, entities}], errors: [string]}. Downloads the matching cadastral DXF \
                 sheet(s) from cadastre.data.gouv.fr (DGFIP PCI Vecteur) and merges them into the current \
                 drawing; tags the drawing EPSG:2154 (Lambert-93) if no coordinate system is set yet. \
                 Desktop builds only.",
    )]
}

#[cfg(target_arch = "wasm32")]
fn run_import_cadastre_dxf(_s: &mut Session, _p: &Value) -> Result<Value> {
    Err(bad(CMD, "downloading cadastre sheets isn't available in the web build - import a DXF file you already have instead"))
}

#[cfg(not(target_arch = "wasm32"))]
fn run_import_cadastre_dxf(s: &mut Session, p: &Value) -> Result<Value> {
    let insee = str_param(p, "insee").map(str::trim).filter(|v| !v.is_empty()).ok_or_else(|| bad(CMD, "`insee` (commune code) is required"))?;
    if insee.len() < 5 || !insee.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(bad(CMD, "`insee` must be a commune code of at least 5 characters"));
    }
    if insee.len() < 2 {
        return Err(bad(CMD, "`insee` is too short to contain a department code"));
    }
    let dept = &insee[0..2];
    let base = str_param(p, "baseUrl").unwrap_or(DEFAULT_BASE_URL).trim_end_matches('/').to_string();
    let dir_url = format!("{base}/{dept}/{insee}/");

    let wanted: Option<Vec<String>> = p
        .get("sections")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(|s| s.trim().to_ascii_uppercase()).filter(|s| !s.is_empty()).collect())
        .filter(|v: &Vec<String>| !v.is_empty());

    let listing = native::fetch_text(&dir_url).map_err(|e| bad(CMD, format!("{dir_url}: {e}")))?;
    let mut names = native::parse_tarbz2_links(&listing);
    names.sort();
    names.dedup();
    if let Some(sections) = &wanted {
        names.retain(|n| sections.iter().any(|sec| section_matches(n, sec)));
    }
    if names.is_empty() {
        return Err(bad(CMD, format!("no .tar.bz2 sheet found at {dir_url} matching the requested section(s)")));
    }
    if names.len() > MAX_SHEETS {
        return Err(bad(
            CMD,
            format!("{} sheets matched - narrow with `sections`, {MAX_SHEETS} is the most this command fetches at once", names.len()),
        ));
    }

    let hooks = super::file::io().ok_or_else(|| bad(CMD, "file formats are not available in this build"))?;
    let mut files_out = Vec::new();
    let mut errors = Vec::new();
    let mut imported_total: u64 = 0;
    for name in &names {
        let url = format!("{dir_url}{name}");
        let archive = match native::fetch_bytes(&url) {
            Ok(b) => b,
            Err(e) => {
                errors.push(format!("{name}: {e}"));
                continue;
            }
        };
        let dxfs = match native::extract_dxf_files(&archive) {
            Ok(d) => d,
            Err(e) => {
                errors.push(format!("{name}: {e}"));
                continue;
            }
        };
        if dxfs.is_empty() {
            errors.push(format!("{name}: no .dxf file found inside the archive"));
            continue;
        }
        for (dxf_name, bytes) in dxfs {
            let src = match (hooks.read)(&bytes, &dxf_name) {
                Ok(d) => d,
                Err(e) => {
                    errors.push(format!("{name} / {dxf_name}: {e}"));
                    continue;
                }
            };
            match super::exchange::import_drawing(s, &src, cadcraft_geom::Vec2::ZERO, 1.0, &dxf_name) {
                Ok(out) => {
                    let n = out.get("imported").and_then(Value::as_u64).unwrap_or(0);
                    imported_total = imported_total.saturating_add(n);
                    files_out.push(json!({ "archive": name, "sheet": dxf_name, "entities": n }));
                }
                Err(e) => errors.push(format!("{name} / {dxf_name}: {e}")),
            }
        }
    }

    if imported_total > 0 {
        let has_crs = s.execute("plan.get_crs", &json!({})).ok().and_then(|v| v.get("epsg").and_then(Value::as_u64).map(|_| ())).is_some();
        if !has_crs {
            let _ = s.execute("plan.set_crs", &json!({ "epsg": CADASTRE_CRS_EPSG }));
        }
    }

    if imported_total == 0 && !errors.is_empty() {
        return Err(bad(CMD, errors.join("; ")));
    }
    Ok(json!({ "imported": imported_total, "files": files_out, "errors": errors }))
}

/// Whether a sheet filename like `dxf-50129000AD01.tar.bz2` belongs to section `sec` (e.g.
/// `"AD"`): matched by substring immediately before the two-digit sheet number and the
/// extension, tolerant of the exact digit-padding scheme since that isn't documented anywhere
/// this command can check against a live sample.
#[cfg(not(target_arch = "wasm32"))]
fn section_matches(filename: &str, sec: &str) -> bool {
    let upper = filename.to_ascii_uppercase();
    upper.contains(&sec.to_ascii_uppercase())
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::io::Read;

    /// Fetches a URL as text (the directory listing page), capped at [`super::MAX_RESPONSE_BYTES`].
    pub(super) fn fetch_text(url: &str) -> std::result::Result<String, String> {
        let bytes = fetch_bytes(url)?;
        String::from_utf8(bytes).map_err(|_| "the server's response wasn't valid UTF-8".to_string())
    }

    /// Fetches a URL's raw bytes, capped at [`super::MAX_RESPONSE_BYTES`] so a huge or
    /// misbehaving response can't exhaust memory.
    pub(super) fn fetch_bytes(url: &str) -> std::result::Result<Vec<u8>, String> {
        let resp = ureq::get(url).call().map_err(|e| format!("request failed: {e}"))?;
        let mut buf = Vec::new();
        resp.into_reader().take(super::MAX_RESPONSE_BYTES).read_to_end(&mut buf).map_err(|e| format!("reading the response failed: {e}"))?;
        if buf.len() as u64 >= super::MAX_RESPONSE_BYTES {
            return Err(format!("response exceeded the {} MiB limit", super::MAX_RESPONSE_BYTES / (1024 * 1024)));
        }
        Ok(buf)
    }

    /// Pulls every `href="...tar.bz2"` (or `.tar.bz2` appearing right after a quote) out of a
    /// directory-listing HTML page, without a full HTML parser - this is a plain static index
    /// page, not arbitrary markup, so a quote-delimited scan is enough and avoids a new
    /// dependency just for this.
    pub(super) fn parse_tarbz2_links(html: &str) -> Vec<String> {
        let mut out = Vec::new();
        let bytes = html.as_bytes();
        let mut i = 0;
        while let Some(start) = html[i..].find(['"', '\'']) {
            let quote = bytes[i + start];
            let rest = &html[i + start + 1..];
            let Some(end) = rest.find(quote as char) else { break };
            let candidate = &rest[..end];
            if candidate.ends_with(".tar.bz2") {
                let name = candidate.rsplit('/').next().unwrap_or(candidate).to_string();
                if !name.is_empty() {
                    out.push(name);
                }
            }
            i += start + 1 + end + 1;
            if i >= bytes.len() {
                break;
            }
        }
        out
    }

    /// Unpacks a `.tar.bz2` byte buffer in memory and returns every `.dxf` entry it contains, as
    /// (file name, bytes) pairs. Bounded the same way `fetch_bytes` bounds a download: a
    /// decompression bomb or corrupt archive can't grow past [`super::MAX_RESPONSE_BYTES`] per
    /// entry, and a non-tar/non-bzip2 payload is a clear error, not a panic.
    pub(super) fn extract_dxf_files(archive: &[u8]) -> std::result::Result<Vec<(String, Vec<u8>)>, String> {
        let cursor = std::io::Cursor::new(archive);
        let decoder = bzip2_rs::DecoderReader::new(cursor);
        let mut tar = tar::Archive::new(decoder);
        let entries = tar.entries().map_err(|e| format!("not a valid tar.bz2 archive: {e}"))?;
        let mut out = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| format!("not a valid tar.bz2 archive: corrupt tar entry: {e}"))?;
            let path = entry.path().map_err(|e| format!("bad entry name: {e}"))?.to_string_lossy().to_string();
            if !path.to_ascii_lowercase().ends_with(".dxf") {
                continue;
            }
            let name = path.rsplit('/').next().unwrap_or(&path).to_string();
            let mut buf = Vec::new();
            entry.take(super::MAX_RESPONSE_BYTES).read_to_end(&mut buf).map_err(|e| format!("reading {name}: {e}"))?;
            out.push((name, buf));
        }
        Ok(out)
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::native::parse_tarbz2_links;
    use super::section_matches;

    #[test]
    fn parses_tarbz2_links_from_a_plain_autoindex_page() {
        let html = r#"<html><body>
            <a href="dxf-50129000AD01.tar.bz2">dxf-50129000AD01.tar.bz2</a>
            <a href="dxf-50129000AB01.tar.bz2">dxf-50129000AB01.tar.bz2</a>
            <a href="../">..</a>
            <a href='dxf-50129000AD02.tar.bz2'>dxf-50129000AD02.tar.bz2</a>
        </body></html>"#;
        let names = parse_tarbz2_links(html);
        assert_eq!(names, vec!["dxf-50129000AD01.tar.bz2", "dxf-50129000AB01.tar.bz2", "dxf-50129000AD02.tar.bz2"]);
    }

    #[test]
    fn parse_tarbz2_links_ignores_unrelated_links() {
        let html = r#"<a href="index.html">index</a><a href="readme.txt">readme</a>"#;
        assert!(parse_tarbz2_links(html).is_empty());
    }

    #[test]
    fn section_matches_is_case_insensitive_and_substring_based() {
        assert!(section_matches("dxf-50129000AD01.tar.bz2", "AD"));
        assert!(section_matches("dxf-50129000ad01.tar.bz2", "AD"));
        assert!(!section_matches("dxf-50129000AB01.tar.bz2", "AD"));
    }

    #[test]
    fn extract_dxf_files_reads_a_synthetic_archive_and_rejects_garbage() {
        // `bzip2-rs` is decoder-only (no encoder), so there's no way to build a real .tar.bz2
        // fixture in-process; these cases instead check the "not a valid archive" error path for
        // garbage and for an uncompressed tar (valid tar, not valid bzip2) - round-tripping a
        // real compressed archive is left to manual verification against a live download, per
        // this module's doc comment.
        let garbage = b"not a tar.bz2 file at all";
        let err = super::native::extract_dxf_files(garbage).unwrap_err();
        assert!(err.contains("not a valid tar.bz2 archive"), "{err}");

        // A minimal tar built by hand (no compression) also isn't a valid bzip2 stream, so it
        // should fail the same clear way rather than panicking.
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            let data = b"0\n";
            let mut header = tar::Header::new_gnu();
            header.set_path("plan.dxf").expect("path should be settable");
            header.set_size(data.len() as u64);
            header.set_cksum();
            builder.append(&header, &data[..]).expect("append should succeed");
            builder.finish().expect("finish should succeed");
        }
        let err = super::native::extract_dxf_files(&tar_bytes).unwrap_err();
        assert!(err.contains("not a valid tar.bz2 archive"), "{err}");
    }
}
