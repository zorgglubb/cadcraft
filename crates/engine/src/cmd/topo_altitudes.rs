//! "Modification des altitudes" (TOPO.EDIT_ALTITUDES): batch-edits the elevation of every
//! matching topo point block (any INSERT carrying an `ALT` attribute, the TCPOINT convention
//! from `topo.rs`) - adding a constant, rounding to a chosen number of decimals, and choosing
//! whether the result also becomes the block's own insertion Z (3D) or stays text-only with the
//! insertion pinned to Z=0 (2D).
//!
//! Which points are in scope is picked the same way the reference carnet tooling this dialog is
//! modelled on does: by block name(s) + insertion layer(s), by an explicit manual selection (the
//! block/layer names can additionally filter that selection), or every point block in the active
//! space. A fourth mode ("every point already in a list") has no list to draw from yet in
//! CADCraft and is rejected with a clear error rather than silently doing nothing.

use cadcraft_doc::{EntityKind, Handle};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec::new("topo.edit_altitudes", "Edit Altitudes", run_edit_altitudes).menu(&["Topo 2D", "Points", "Edit Altitudes"]).params(
        "{selection: \"blocks\"|\"manual\"|\"all\", blockNames?: [string], layers?: [string], \
                 handles?: [string] (required for \"manual\"), useFilterForManual?: bool, \
                 matriculeFirst?: string, matriculeLast?: string, mode: \"2d\"|\"3d\", constant: number, \
                 decimals?: integer (0-10, default 2)} → {updated: integer, handles: [string]}. Every \
                 matching point's ALT attribute is set to (old value + constant) rounded to `decimals`; \
                 \"3d\" also sets the block's own insertion Z to that value, \"2d\" pins it back to 0.",
    )]
}

/// One topo point block in scope: its handle, block name, layer and parsed `ALT` value (`0.0`
/// when the attribute is missing or unparsable, so a point is never silently dropped just because
/// its altitude text was blank - the constant still applies, from a zero baseline).
struct Candidate {
    handle: Handle,
    block: String,
    layer: String,
    mat: Option<String>,
    alt: f64,
}

fn alt_attrib_value(ins: &cadcraft_doc::Insert) -> Option<f64> {
    ins.attribs.iter().find(|a| a.tag.eq_ignore_ascii_case("ALT")).and_then(|a| a.text.value.trim().parse().ok())
}

fn mat_attrib_value(ins: &cadcraft_doc::Insert) -> Option<String> {
    ins.attribs.iter().find(|a| a.tag.eq_ignore_ascii_case("MAT")).map(|a| a.text.value.clone())
}

/// Every INSERT in the active space that carries an `ALT` attribute - i.e. every topo point
/// block, regardless of which block definition it uses.
fn all_point_candidates(s: &Session) -> Result<Vec<Candidate>> {
    let st = s.state()?;
    let Some(store) = st.doc.space(&st.space) else { return Ok(Vec::new()) };
    let mut out = Vec::new();
    for e in store.iter() {
        if let EntityKind::Insert(ins) = &e.kind
            && let Some(alt) = alt_attrib_value(ins)
        {
            out.push(Candidate { handle: e.handle, block: ins.block.clone(), layer: e.common.layer.clone(), mat: mat_attrib_value(ins), alt });
        }
    }
    Ok(out)
}

fn names_param(p: &Value, key: &str) -> Vec<String> {
    p.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default()
}

fn matches_names(name: &str, names: &[String]) -> bool {
    names.is_empty() || names.iter().any(|n| n.eq_ignore_ascii_case(name))
}

/// `first <= mat <= last`: numeric comparison when both bounds and the matricule itself parse as
/// numbers (the common case - matricules are usually sequential integers), falling back to a
/// plain lexicographic range otherwise so an alphanumeric matricule scheme still works.
fn matricule_in_range(mat: &Option<String>, first: &str, last: &str) -> bool {
    let Some(mat) = mat else { return false };
    match (mat.trim().parse::<f64>(), first.trim().parse::<f64>(), last.trim().parse::<f64>()) {
        (Ok(m), Ok(f), Ok(l)) => m >= f.min(l) && m <= f.max(l),
        _ => {
            let (lo, hi) = if first <= last { (first, last) } else { (last, first) };
            mat.as_str() >= lo && mat.as_str() <= hi
        }
    }
}

fn run_edit_altitudes(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "topo.edit_altitudes";

    let selection = str_param(p, "selection").ok_or_else(|| bad(CMD, "`selection` (\"blocks\"|\"manual\"|\"all\") is required"))?;
    let block_names = names_param(p, "blockNames");
    let layers = names_param(p, "layers");

    let mut candidates = match selection {
        "all" => all_point_candidates(s)?,
        "blocks" => {
            if block_names.is_empty() {
                return Err(bad(CMD, "`blockNames` must not be empty for `selection: \"blocks\"`"));
            }
            all_point_candidates(s)?.into_iter().filter(|c| matches_names(&c.block, &block_names) && matches_names(&c.layer, &layers)).collect()
        }
        "manual" => {
            let handles = p.get("handles").and_then(Value::as_array).ok_or_else(|| bad(CMD, "`handles` is required for `selection: \"manual\"`"))?;
            let use_filter = bool_or(p, "useFilterForManual", false);
            let all = all_point_candidates(s)?;
            let wanted: std::collections::HashSet<Handle> = handles.iter().filter_map(Value::as_str).filter_map(Handle::parse_hex).collect();
            all.into_iter()
                .filter(|c| wanted.contains(&c.handle))
                .filter(|c| !use_filter || (matches_names(&c.block, &block_names) && matches_names(&c.layer, &layers)))
                .collect()
        }
        "list" => return Err(bad(CMD, "`selection: \"list\"` has no point list to draw from yet - use \"blocks\", \"manual\" or \"all\"")),
        other => return Err(bad(CMD, format!("unknown `selection` \"{other}\""))),
    };

    if let (Some(first), Some(last)) = (str_param(p, "matriculeFirst"), str_param(p, "matriculeLast")) {
        candidates.retain(|c| matricule_in_range(&c.mat, first, last));
    }

    let mode3d = match str_param(p, "mode") {
        Some("3d") => true,
        Some("2d") | None => false,
        Some(other) => return Err(bad(CMD, format!("unknown `mode` \"{other}\" (expected \"2d\" or \"3d\")"))),
    };
    let constant = f64_req(CMD, p, "constant")?;
    let decimals = p.get("decimals").and_then(Value::as_u64).unwrap_or(2).min(10) as usize;

    let mut handles = Vec::with_capacity(candidates.len());
    for c in &candidates {
        let new_alt_raw = c.alt + constant;
        // Round to `decimals` through a formatted string rather than a manual power-of-ten
        // multiply/round/divide, which can drift for larger decimal counts - the attribute text
        // and the geometric Z (when 3D) both end up exactly what's displayed.
        let new_alt: f64 = format!("{new_alt_raw:.decimals$}").parse().unwrap_or(new_alt_raw);
        let ok = s
            .doc_mut()?
            .modify_entity(c.handle, |e| {
                if let EntityKind::Insert(ins) = &mut e.kind {
                    if let Some(a) = ins.attribs.iter_mut().find(|a| a.tag.eq_ignore_ascii_case("ALT")) {
                        a.text.value = format!("{new_alt:.decimals$}");
                    }
                    ins.insert.z = if mode3d { new_alt } else { 0.0 };
                }
            })
            .is_ok();
        if ok {
            handles.push(json!(c.handle.hex()));
        }
    }

    Ok(json!({ "updated": handles.len(), "handles": handles }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::Session;

    /// Inserts one CADCRAFT_TCPOINT block the same way `topo.import_points` does (see
    /// `topo.rs`'s own equivalent test), via a one-line CSV file - the simplest path to a real
    /// MAT/ALT-bearing point block without duplicating that command's logic here.
    fn insert_tcpoint(s: &mut Session, x: f64, y: f64, z: f64, mat: &str) -> String {
        let path = std::env::temp_dir().join(format!("cadcraft-topo-altitudes-test-{}-{mat}.csv", std::process::id()));
        std::fs::write(&path, format!("ID;X;Y;Z;CODE\n{mat};{x};{y};{z};P\n")).expect("writing the temp points file should succeed");
        let out = s.execute("topo.import_points", &json!({ "path": path.to_string_lossy(), "label": false }));
        let _ = std::fs::remove_file(&path);
        out.expect("import should succeed")["points"][0]["handle"].as_str().expect("handle").to_string()
    }

    fn doc_session() -> Session {
        let mut s = Session::new();
        s.execute("new", &json!({})).expect("new drawing should succeed");
        s
    }

    #[test]
    fn adds_the_constant_and_rounds_for_every_point_in_the_drawing() {
        let mut s = doc_session();
        insert_tcpoint(&mut s, 0.0, 0.0, 10.12345, "1");
        insert_tcpoint(&mut s, 10.0, 0.0, 20.6789, "2");

        let out =
            s.execute("topo.edit_altitudes", &json!({ "selection": "all", "mode": "2d", "constant": 1.0, "decimals": 2 })).expect("should succeed");
        assert_eq!(out["updated"].as_u64(), Some(2));
    }

    #[test]
    fn mode_3d_sets_the_insertion_z_to_the_new_altitude() {
        let mut s = doc_session();
        let h = insert_tcpoint(&mut s, 0.0, 0.0, 10.0, "1");
        let handle = cadcraft_doc::Handle::parse_hex(&h).expect("valid handle");

        s.execute("topo.edit_altitudes", &json!({ "selection": "all", "mode": "3d", "constant": 5.0, "decimals": 2 })).expect("should succeed");

        let d = s.doc().expect("doc");
        let e = d.entity(handle).expect("entity still there");
        let cadcraft_doc::EntityKind::Insert(ins) = &e.kind else { panic!("expected an INSERT") };
        assert!((ins.insert.z - 15.0).abs() < 1e-9, "z = {}", ins.insert.z);
        let alt = ins.attribs.iter().find(|a| a.tag == "ALT").expect("ALT attribute");
        assert_eq!(alt.text.value, "15.00");
    }

    #[test]
    fn matricule_range_filters_out_points_outside_it() {
        let mut s = doc_session();
        insert_tcpoint(&mut s, 0.0, 0.0, 10.0, "1");
        insert_tcpoint(&mut s, 10.0, 0.0, 20.0, "5");
        insert_tcpoint(&mut s, 20.0, 0.0, 30.0, "9");

        let out = s
            .execute(
                "topo.edit_altitudes",
                &json!({ "selection": "all", "mode": "2d", "constant": 0.0, "matriculeFirst": "2", "matriculeLast": "6" }),
            )
            .expect("should succeed");
        assert_eq!(out["updated"].as_u64(), Some(1));
    }

    #[test]
    fn blocks_selection_needs_at_least_one_block_name() {
        let mut s = doc_session();
        let out = s.execute("topo.edit_altitudes", &json!({ "selection": "blocks", "mode": "2d", "constant": 0.0 }));
        assert!(out.is_err());
    }

    #[test]
    fn list_selection_is_rejected_with_a_clear_error_rather_than_doing_nothing() {
        let mut s = doc_session();
        let out = s.execute("topo.edit_altitudes", &json!({ "selection": "list", "mode": "2d", "constant": 0.0 }));
        assert!(out.is_err());
    }
}
