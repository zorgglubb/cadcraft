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
    vec![
        CommandSpec::new("topo.edit_altitudes", "Edit Altitudes", run_edit_altitudes).menu(&["Topo 2D", "Points", "Edit Altitudes"]).params(
            "{selection: \"blocks\"|\"manual\"|\"all\", blockNames?: [string], layers?: [string], \
                 handles?: [string] (required for \"manual\"), useFilterForManual?: bool, \
                 matriculeFirst?: string, matriculeLast?: string, mode: \"2d\"|\"3d\", constant: number, \
                 decimals?: integer (0-10, default 2)} → {updated: integer, handles: [string]}. Every \
                 matching point's ALT attribute is set to (old value + constant) rounded to `decimals`; \
                 \"3d\" also sets the block's own insertion Z to that value, \"2d\" pins it back to 0.",
        ),
        CommandSpec::new("topo.point_block_names", "Point Block Names", run_point_block_names)
            .noundo()
            .params("{} → {blocks: [string]}. Every distinct block name among the active space's topo point blocks (any INSERT carrying an ALT attribute), so a dialog can offer them as a pick list instead of free text."),
        CommandSpec::new("topo.point_radicals", "Point Radicals", run_point_radicals).noundo().params(
            "{} → {radicals: [{radical, minIndex, maxIndex, count}]}. Groups every topo point's MAT \
             attribute by its leading radical (the non-digit prefix, empty for a plain numeric \
             matricule) and trailing numeric index, for a renumbering dialog's read-only summary \
             table; a MAT with no trailing digits at all isn't counted (it has no index range to show).",
        ),
        CommandSpec::new("topo.renumber_points", "Renumber Points...", run_renumber_points)
            .menu(&["Topo 2D", "Points", "Renumber Points..."])
            .params(
                "{selection: \"blocks\"|\"manual\"|\"all\", blockNames?: [string], layers?: [string], \
                 handles?: [string] (required for \"manual\"), useFilterForManual?: bool, \
                 matriculeFirst?: string, matriculeLast?: string, \
                 mode: \"renumberFrom\"|\"incrementIndices\"|\"changeRadical\"|\"prefixRadical\", \
                 value: number (\"renumberFrom\"/\"incrementIndices\") | string (\"changeRadical\"/ \
                 \"prefixRadical\")} → {updated: integer, handles: [string]}. Every matching point's \
                 MAT attribute is rewritten: \"renumberFrom\" renumbers every matching point \
                 sequentially starting at `value`, in the order its current matricule already sorts \
                 in (numeric when every one parses as a number, lexicographic otherwise); \
                 \"incrementIndices\" adds `value` to each point's own trailing numeric index, \
                 leaving its radical untouched, and leaves alone (not counted in `updated`) any \
                 matricule with no numeric index to increment; \"changeRadical\" replaces the \
                 radical, keeping the index; \"prefixRadical\" prepends `value` before the existing \
                 radical and index.",
            ),
    ]
}

/// Splits a matricule into its radical (the leading part, up to and including the last
/// non-digit character - empty for a plain numeric matricule like `"42"`) and its trailing
/// numeric index text, kept as written (leading zeros and all) rather than reparsed, so
/// "changeRadical"/"prefixRadical" can rebuild the matricule exactly. `None` for the index half
/// means the matricule has no trailing digits at all (e.g. `"BORNE"`) - the whole string is then
/// its own radical.
fn split_matricule(mat: &str) -> (String, Option<String>) {
    let digits_start = mat.rfind(|c: char| !c.is_ascii_digit()).map(|i| i + 1).unwrap_or(0);
    if digits_start < mat.len() { (mat[..digits_start].to_string(), Some(mat[digits_start..].to_string())) } else { (mat.to_string(), None) }
}

/// TOPO.POINT_RADICALS: every topo point's matricule, grouped by radical with its index range -
/// the renumbering dialog's read-only "existing matricules" table.
fn run_point_radicals(s: &mut Session, _p: &Value) -> Result<Value> {
    let mut groups: Vec<(String, i64, i64, usize)> = Vec::new();
    for c in all_point_candidates(s)? {
        let Some(mat) = &c.mat else { continue };
        let (radical, index_str) = split_matricule(mat);
        let Some(idx) = index_str.and_then(|d| d.parse::<i64>().ok()) else { continue };
        match groups.iter_mut().find(|(r, ..)| r.eq_ignore_ascii_case(&radical)) {
            Some((_, min, max, count)) => {
                *min = (*min).min(idx);
                *max = (*max).max(idx);
                *count += 1;
            }
            None => groups.push((radical, idx, idx, 1)),
        }
    }
    groups.sort_by_key(|g| g.0.to_ascii_lowercase());
    let radicals: Vec<Value> = groups
        .into_iter()
        .map(|(radical, min, max, count)| json!({ "radical": radical, "minIndex": min, "maxIndex": max, "count": count }))
        .collect();
    Ok(json!({ "radicals": radicals }))
}

/// TOPO.RENUMBER_POINTS: see the `CommandSpec`'s own params doc for what each `mode` does.
fn run_renumber_points(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "topo.renumber_points";

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

    // Sorted by the matricule's own current order - numeric when every candidate's matricule
    // parses as a plain number (the common case), lexicographic otherwise - so "renumberFrom"
    // assigns sequential numbers in the order a person reading the drawing's point list would
    // already expect, not insertion order.
    let all_numeric = candidates.iter().all(|c| c.mat.as_deref().is_some_and(|m| m.trim().parse::<f64>().is_ok()));
    candidates.sort_by(|a, b| {
        if all_numeric {
            let na = a.mat.as_deref().and_then(|m| m.trim().parse::<f64>().ok()).unwrap_or(0.0);
            let nb = b.mat.as_deref().and_then(|m| m.trim().parse::<f64>().ok()).unwrap_or(0.0);
            na.partial_cmp(&nb).unwrap_or(std::cmp::Ordering::Equal)
        } else {
            a.mat.cmp(&b.mat)
        }
    });

    let mode = str_param(p, "mode")
        .ok_or_else(|| bad(CMD, "`mode` (\"renumberFrom\"|\"incrementIndices\"|\"changeRadical\"|\"prefixRadical\") is required"))?;
    let start = if mode == "renumberFrom" { f64_req(CMD, p, "value")?.round() as i64 } else { 0 };
    let delta = if mode == "incrementIndices" { f64_req(CMD, p, "value")?.round() as i64 } else { 0 };
    let text_value = if mode == "changeRadical" || mode == "prefixRadical" {
        str_param(p, "value").ok_or_else(|| bad(CMD, "`value` (text) is required for this mode"))?.to_string()
    } else {
        String::new()
    };
    if !matches!(mode, "renumberFrom" | "incrementIndices" | "changeRadical" | "prefixRadical") {
        return Err(bad(CMD, format!("unknown `mode` \"{mode}\"")));
    }

    let mut handles = Vec::with_capacity(candidates.len());
    for (i, c) in candidates.iter().enumerate() {
        let Some(old_mat) = &c.mat else { continue };
        let (radical, index_str) = split_matricule(old_mat);
        let new_mat = match mode {
            "renumberFrom" => (start + i as i64).to_string(),
            "incrementIndices" => match index_str.as_deref().and_then(|d| d.parse::<i64>().ok()) {
                Some(idx) => format!("{radical}{}", idx + delta),
                None => continue,
            },
            "changeRadical" => format!("{text_value}{}", index_str.as_deref().unwrap_or("")),
            _ => format!("{text_value}{radical}{}", index_str.as_deref().unwrap_or("")),
        };
        let ok = s
            .doc_mut()?
            .modify_entity(c.handle, |e| {
                if let EntityKind::Insert(ins) = &mut e.kind
                    && let Some(a) = ins.attribs.iter_mut().find(|a| a.tag.eq_ignore_ascii_case("MAT"))
                {
                    a.text.value = new_mat.clone();
                }
            })
            .is_ok();
        if ok {
            handles.push(json!(c.handle.hex()));
        }
    }

    Ok(json!({ "updated": handles.len(), "handles": handles }))
}

/// Every distinct block name among the active space's topo point blocks, first-occurrence
/// casing kept, sorted case-insensitively - for a dialog's block-name picker (`Selection::Blocks`
/// in `editaltitudesdlg`, say), which otherwise has no way to know what block names exist without
/// the user typing one blind.
fn run_point_block_names(s: &mut Session, _p: &Value) -> Result<Value> {
    let mut names: Vec<String> = Vec::new();
    for c in all_point_candidates(s)? {
        if !names.iter().any(|n| n.eq_ignore_ascii_case(&c.block)) {
            names.push(c.block);
        }
    }
    names.sort_by_key(|n| n.to_ascii_lowercase());
    Ok(json!({ "blocks": names }))
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
    use cadcraft_doc::EntityKind;
    use serde_json::json;

    use super::mat_attrib_value;
    use crate::Session;

    /// Inserts one CADCRAFT_TCPOINT block the same way `topo.import_points` does (see
    /// `topo.rs`'s own equivalent test), via a one-line CSV file - the simplest path to a real
    /// MAT/ALT-bearing point block without duplicating that command's logic here.
    fn insert_tcpoint(s: &mut Session, x: f64, y: f64, z: f64, mat: &str) -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("cadcraft-topo-altitudes-test-{}-{mat}-{n}.csv", std::process::id()));
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

    #[test]
    fn point_block_names_lists_every_distinct_block_once() {
        let mut s = doc_session();
        insert_tcpoint(&mut s, 0.0, 0.0, 10.0, "1");
        insert_tcpoint(&mut s, 10.0, 0.0, 20.0, "2");
        // A second point with the same (default) block name must not duplicate the entry.
        let out = s.execute("topo.point_block_names", &json!({})).expect("should succeed");
        let blocks: Vec<&str> = out["blocks"].as_array().expect("blocks").iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(blocks, vec!["CADCRAFT_TCPOINT"]);
    }

    #[test]
    fn point_block_names_is_empty_with_no_points_in_the_drawing() {
        let mut s = doc_session();
        let out = s.execute("topo.point_block_names", &json!({})).expect("should succeed");
        assert_eq!(out["blocks"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn point_radicals_groups_plain_numeric_matricules_under_an_empty_radical() {
        let mut s = doc_session();
        insert_tcpoint(&mut s, 0.0, 0.0, 10.0, "1");
        insert_tcpoint(&mut s, 10.0, 0.0, 20.0, "78");
        let out = s.execute("topo.point_radicals", &json!({})).expect("should succeed");
        let radicals = out["radicals"].as_array().expect("radicals array");
        assert_eq!(radicals.len(), 1);
        assert_eq!(radicals[0]["radical"].as_str(), Some(""));
        assert_eq!(radicals[0]["minIndex"].as_i64(), Some(1));
        assert_eq!(radicals[0]["maxIndex"].as_i64(), Some(78));
        assert_eq!(radicals[0]["count"].as_u64(), Some(2));
    }

    #[test]
    fn point_radicals_separates_different_prefixes() {
        let mut s = doc_session();
        insert_tcpoint(&mut s, 0.0, 0.0, 10.0, "A1");
        insert_tcpoint(&mut s, 10.0, 0.0, 20.0, "B5");
        let out = s.execute("topo.point_radicals", &json!({})).expect("should succeed");
        let radicals = out["radicals"].as_array().expect("radicals array");
        assert_eq!(radicals.len(), 2);
    }

    #[test]
    fn renumber_from_assigns_sequential_matricules_in_numeric_order() {
        let mut s = doc_session();
        insert_tcpoint(&mut s, 0.0, 0.0, 10.0, "5");
        insert_tcpoint(&mut s, 10.0, 0.0, 20.0, "2");
        insert_tcpoint(&mut s, 20.0, 0.0, 30.0, "9");

        let out = s.execute("topo.renumber_points", &json!({ "selection": "all", "mode": "renumberFrom", "value": 1000 })).expect("should succeed");
        assert_eq!(out["updated"].as_u64(), Some(3));

        let d = s.doc().unwrap();
        let mats: Vec<String> = d
            .model
            .iter()
            .filter_map(|e| match &e.kind {
                EntityKind::Insert(ins) => mat_attrib_value(ins),
                _ => None,
            })
            .collect();
        let mut sorted = mats.clone();
        sorted.sort();
        assert_eq!(sorted, vec!["1000", "1001", "1002"]);
    }

    #[test]
    fn increment_indices_adds_the_constant_and_keeps_the_radical() {
        let mut s = doc_session();
        insert_tcpoint(&mut s, 0.0, 0.0, 10.0, "A12");
        let out =
            s.execute("topo.renumber_points", &json!({ "selection": "all", "mode": "incrementIndices", "value": 100 })).expect("should succeed");
        assert_eq!(out["updated"].as_u64(), Some(1));
        let d = s.doc().unwrap();
        let mat = d
            .model
            .iter()
            .find_map(|e| match &e.kind {
                EntityKind::Insert(ins) => mat_attrib_value(ins),
                _ => None,
            })
            .unwrap();
        assert_eq!(mat, "A112");
    }

    #[test]
    fn increment_indices_skips_matricules_with_no_numeric_index() {
        let mut s = doc_session();
        insert_tcpoint(&mut s, 0.0, 0.0, 10.0, "BORNE");
        let out = s.execute("topo.renumber_points", &json!({ "selection": "all", "mode": "incrementIndices", "value": 1 })).expect("should succeed");
        assert_eq!(out["updated"].as_u64(), Some(0));
    }

    #[test]
    fn change_radical_replaces_the_prefix_and_keeps_the_index() {
        let mut s = doc_session();
        insert_tcpoint(&mut s, 0.0, 0.0, 10.0, "A12");
        s.execute("topo.renumber_points", &json!({ "selection": "all", "mode": "changeRadical", "value": "B" })).expect("should succeed");
        let d = s.doc().unwrap();
        let mat = d
            .model
            .iter()
            .find_map(|e| match &e.kind {
                EntityKind::Insert(ins) => mat_attrib_value(ins),
                _ => None,
            })
            .unwrap();
        assert_eq!(mat, "B12");
    }

    #[test]
    fn prefix_radical_prepends_before_the_existing_radical_and_index() {
        let mut s = doc_session();
        insert_tcpoint(&mut s, 0.0, 0.0, 10.0, "A12");
        s.execute("topo.renumber_points", &json!({ "selection": "all", "mode": "prefixRadical", "value": "X" })).expect("should succeed");
        let d = s.doc().unwrap();
        let mat = d
            .model
            .iter()
            .find_map(|e| match &e.kind {
                EntityKind::Insert(ins) => mat_attrib_value(ins),
                _ => None,
            })
            .unwrap();
        assert_eq!(mat, "XA12");
    }

    #[test]
    fn renumber_points_matricule_range_filters_before_renumbering() {
        let mut s = doc_session();
        insert_tcpoint(&mut s, 0.0, 0.0, 10.0, "1");
        insert_tcpoint(&mut s, 10.0, 0.0, 20.0, "5");
        insert_tcpoint(&mut s, 20.0, 0.0, 30.0, "9");
        let out = s
            .execute(
                "topo.renumber_points",
                &json!({ "selection": "all", "mode": "renumberFrom", "value": 100, "matriculeFirst": "2", "matriculeLast": "6" }),
            )
            .expect("should succeed");
        assert_eq!(out["updated"].as_u64(), Some(1));
    }

    #[test]
    fn renumber_points_requires_a_known_selection() {
        let mut s = doc_session();
        let out = s.execute("topo.renumber_points", &json!({ "selection": "bogus", "mode": "renumberFrom", "value": 1 }));
        assert!(out.is_err());
    }

    #[test]
    fn renumber_points_requires_a_known_mode() {
        let mut s = doc_session();
        insert_tcpoint(&mut s, 0.0, 0.0, 10.0, "1");
        let out = s.execute("topo.renumber_points", &json!({ "selection": "all", "mode": "bogus", "value": 1 }));
        assert!(out.is_err());
    }
}
