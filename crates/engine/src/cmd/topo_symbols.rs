//! "Symboles et habillage": a small library of standard, code-drawn topographic symbols (no
//! copied assets - every shape here is original vector geometry, per the project's asset rules)
//! inserted at the drawing's scale, with CODE and ALTITUDE attributes automatically filled in
//! from the source topo point.
//!
//! Each symbol is a normal CADCraft block, built once (lazily, from plain LINE/CIRCLE primitives
//! plus two ATTDEF attributes) the first time it is needed, then reused by ordinary INSERT -
//! there is nothing symbol-specific in the document format, so these blocks show up like any
//! other in BLOCKS/INSERT/the block editor.

use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

/// Symbol block names, prefixed to avoid colliding with a user's own block of the same short name.
const SYMBOLS: &[(&str, &str)] =
    &[("borne", "CADCRAFT_TOPO_BORNE"), ("regard", "CADCRAFT_TOPO_REGARD"), ("arbre", "CADCRAFT_TOPO_ARBRE"), ("mobilier", "CADCRAFT_TOPO_MOBILIER")];

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec::new("topo.insert_symbol", "Insert Topo Symbol", run_insert_symbol).menu(&["Topo", "Symbols", "Insert"]).params(
        "{symbol: \"borne\"|\"regard\"|\"arbre\"|\"mobilier\", at: [x,y,z], code?: string, altitude?: number, scale?: number, rotation?: number}",
    )]
}

fn block_name(symbol: &str) -> Result<&'static str> {
    SYMBOLS
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(symbol))
        .map(|(_, name)| *name)
        .ok_or_else(|| bad("topo.insert_symbol", format!("unknown symbol \"{symbol}\" (expected one of: borne, regard, arbre, mobilier)")))
}

/// Pulls a result's `"handle"` (single) or `"handles"` (array) field into `out`, so block-building
/// steps can be chained without caring which shape each drawing command returned.
fn collect_handles(out: &mut Vec<Value>, result: &Value) {
    match result.get("handles").and_then(Value::as_array) {
        Some(hs) => out.extend(hs.iter().cloned()),
        None => {
            if let Some(h) = result.get("handle") {
                out.push(h.clone());
            }
        }
    }
}

/// Builds `name`'s block definition from scratch at the origin, if it does not already exist:
/// plain LINE/CIRCLE geometry plus a CODE and an ALTITUDE attribute, then BLOCK with `keep:
/// "delete"` to fold the construction geometry into the definition and clean up the drawing.
fn ensure_symbol_block(s: &mut Session, name: &str) -> Result<()> {
    if s.doc()?.blocks.keys().any(|k| k.eq_ignore_ascii_case(name)) {
        return Ok(());
    }

    let mut handles: Vec<Value> = Vec::new();
    let mut draw = |s: &mut Session, cmd: &str, params: Value| -> Result<()> {
        let r = s.execute(cmd, &params)?;
        collect_handles(&mut handles, &r);
        Ok(())
    };

    match name {
        "CADCRAFT_TOPO_BORNE" => {
            // A small square with a diagonal cross: the classic boundary-marker glyph.
            draw(s, "line", json!({ "points": [[-0.25, -0.25], [0.25, -0.25], [0.25, 0.25], [-0.25, 0.25]], "closed": true }))?;
            draw(s, "line", json!({ "points": [[-0.25, -0.25], [0.25, 0.25]] }))?;
            draw(s, "line", json!({ "points": [[-0.25, 0.25], [0.25, -0.25]] }))?;
        }
        "CADCRAFT_TOPO_REGARD" => {
            // A circle with a cross: the classic manhole/inspection-chamber glyph.
            draw(s, "circle", json!({ "center": [0.0, 0.0], "radius": 0.3 }))?;
            draw(s, "line", json!({ "points": [[-0.21, -0.21], [0.21, 0.21]] }))?;
            draw(s, "line", json!({ "points": [[-0.21, 0.21], [0.21, -0.21]] }))?;
        }
        "CADCRAFT_TOPO_ARBRE" => {
            // A canopy circle with a small centre dot: a tree.
            draw(s, "circle", json!({ "center": [0.0, 0.0], "radius": 0.4 }))?;
            draw(s, "circle", json!({ "center": [0.0, 0.0], "radius": 0.08 }))?;
        }
        "CADCRAFT_TOPO_MOBILIER" => {
            // A diamond: a generic, undecorated marker for street furniture (bins, benches,
            // lampposts...) distinct from the other three shapes.
            draw(s, "line", json!({ "points": [[0.0, 0.3], [0.3, 0.0], [0.0, -0.3], [-0.3, 0.0]], "closed": true }))?;
        }
        _ => return Err(bad("topo.insert_symbol", format!("no built-in geometry for \"{name}\""))),
    }

    draw(s, "attdef", json!({ "tag": "CODE", "prompt": "Code", "default": "", "at": [-0.4, -0.55], "height": 0.15 }))?;
    draw(s, "attdef", json!({ "tag": "ALTITUDE", "prompt": "Altitude", "default": "", "at": [-0.4, -0.75], "height": 0.15 }))?;

    s.execute("block", &json!({ "name": name, "base": [0.0, 0.0], "handles": handles, "keep": "delete" }))?;
    Ok(())
}

fn run_insert_symbol(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "topo.insert_symbol";
    let symbol = str_param(p, "symbol").ok_or_else(|| bad(CMD, "`symbol` is required"))?;
    let name = block_name(symbol)?;
    let at = super::topo::vec3_req(CMD, p, "at")?;
    let scale = f64_or(p, "scale", 1.0);
    let rotation = f64_or(p, "rotation", 0.0);

    ensure_symbol_block(s, name)?;

    let mut attribs = serde_json::Map::new();
    if let Some(code) = str_param(p, "code") {
        attribs.insert("CODE".into(), Value::String(code.to_string()));
    }
    if let Some(altitude) = p.get("altitude").and_then(Value::as_f64) {
        attribs.insert("ALTITUDE".into(), Value::String(format!("{altitude:.2}")));
    }

    let out = s.execute(
        "insert",
        &json!({
            "name": name,
            "at": [at.x, at.y, at.z],
            "scale": scale,
            "rotation": rotation,
            "attribs": Value::Object(attribs),
        }),
    )?;
    Ok(json!({ "handle": out.get("handle").cloned().unwrap_or(Value::Null), "block": name }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::Session;

    fn session_with_doc() -> Session {
        let mut s = Session::new();
        s.execute("new", &json!({})).expect("new drawing should succeed");
        s
    }

    #[test]
    fn insert_symbol_rejects_an_unknown_symbol_name() {
        let mut s = session_with_doc();
        let out = s.execute("topo.insert_symbol", &json!({ "symbol": "licorne", "at": [0.0, 0.0, 0.0] }));
        assert!(out.is_err());
    }

    #[test]
    fn insert_symbol_creates_the_block_once_and_inserts_with_attributes() {
        let mut s = session_with_doc();
        let out = s
            .execute("topo.insert_symbol", &json!({ "symbol": "borne", "at": [10.0, 20.0, 5.0], "code": "B12", "altitude": 123.456, "scale": 2.0 }))
            .expect("should succeed");
        assert!(out["handle"].is_string());
        assert!(s.doc().expect("doc").blocks.keys().any(|k| k.eq_ignore_ascii_case("CADCRAFT_TOPO_BORNE")), "the block should now exist");

        // A second insertion must reuse the existing block definition, not fail or duplicate it.
        let out2 = s.execute("topo.insert_symbol", &json!({ "symbol": "borne", "at": [30.0, 40.0, 0.0], "code": "B13" })).expect("should succeed");
        assert!(out2["handle"].is_string());
        let defs = s.doc().expect("doc").blocks.keys().filter(|k| k.eq_ignore_ascii_case("CADCRAFT_TOPO_BORNE")).count();
        assert_eq!(defs, 1, "the symbol block must only be defined once");
    }

    #[test]
    fn each_symbol_builds_its_own_distinct_block() {
        let mut s = session_with_doc();
        for symbol in ["borne", "regard", "arbre", "mobilier"] {
            let out = s.execute("topo.insert_symbol", &json!({ "symbol": symbol, "at": [0.0, 0.0, 0.0] }));
            assert!(out.is_ok(), "{symbol} should insert: {out:?}");
        }
        assert_eq!(
            s.doc().expect("doc").blocks.keys().filter(|k| k.starts_with("CADCRAFT_TOPO_")).count(),
            4,
            "all four symbol blocks should have been created"
        );
    }
}
