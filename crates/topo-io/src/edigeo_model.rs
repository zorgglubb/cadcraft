//! Reconstructs cadastral parcels from parsed EDIGEO ASCII-profile records
//! ([`crate::edigeo_text`]): the arc/node/face planar topology, the feature (`FEA`) objects and
//! their attributes, and the relations (`LNK`) tying them together.
//!
//! A real PCI-Vecteur export's "lot" is normally split across several `.VEC` files (e.g.
//! `T1`/`T2`/`T3`/`S1`), each carrying a subset of the same record set, sometimes with the same
//! record repeated verbatim in more than one file; [`build_model`] takes every file belonging
//! to one lot together and merges records by `(type, id)`.
//!
//! **Scope, stated plainly**: this reads the planar topology this export actually uses - each
//! arc (`PAR`) carries its own full, already-ordered vertex list, and knows which face (`PFE`)
//! lies to its left (`ID_S_RCO_FAC_GCHE`) and right (`ID_S_RCO_FAC_DRTE`) - and chains a face's
//! bordering arcs into one closed ring. It does not resolve attribute class codes (`IDU_id`,
//! `COPL_id`, ...) to their French labels via the accompanying `.DIC`/`.SCD` dictionary: those
//! are carried through verbatim as the attribute's class name, which is already enough to tell
//! parcels apart and to look the code up by hand. Curved arcs are not a case this export uses
//! (every `PAR`/`PNO` here declares `TYPS` straight, i.e. `1`) and aren't specially handled.

use std::collections::BTreeMap;

use cadcraft_geom::Vec2;

use crate::edigeo_text::{TextFile, parse_coord};

/// How close two arc endpoints must be (in the drawing's units) to count as the same node when
/// chaining a face's ring together.
pub const DEFAULT_TOLERANCE: f64 = 1e-3;

/// Hard cap on how many arcs a single ring may chain through, independent of how many arcs the
/// file actually has - guards a corrupt or cyclic relation set from looping forever.
const MAX_RING_ARCS: usize = 100_000;

#[derive(Debug, Clone, PartialEq)]
pub struct Arc {
    pub points: Vec<Vec2>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attribute {
    pub class: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Feature {
    pub class: String,
    pub attributes: Vec<Attribute>,
}

/// Every primitive, feature and relation read from one lot's files, merged and ready for ring
/// reconstruction.
#[derive(Debug, Clone, Default)]
pub struct Model {
    pub arcs: BTreeMap<String, Arc>,
    /// Face id → the arcs bordering it, each already oriented so walking its points in order
    /// keeps the face on the left (the standard left/right-of-arc topology convention).
    pub face_sides: BTreeMap<String, Vec<(String, Side)>>,
    pub features: BTreeMap<String, Feature>,
    /// Feature id → the face primitive it is linked to (via an `ID_S_RCO_<class>_id` relation).
    pub feature_face: BTreeMap<String, String>,
}

fn last_token(path: &str) -> &str {
    path.rsplit(';').next().unwrap_or(path)
}

/// `"lot;sous-lot;TYPE;id"` → `(TYPE, id)`, the shape every `FTPC` relation endpoint has.
fn primitive_ref(path: &str) -> Option<(&str, &str)> {
    let mut it = path.rsplit(';');
    let id = it.next()?;
    let ty = it.next()?;
    Some((ty, id))
}

/// Merges every record from every file of one lot into a [`Model`]. Records are read once per
/// `(type, id)`; a duplicate (the same primitive repeated across split files, as this format
/// allows) is simply skipped rather than overwriting what's already there.
pub fn build_model(files: &[TextFile]) -> Model {
    let mut model = Model::default();
    let mut seen: Vec<(String, String)> = Vec::new();

    for file in files {
        for rec in &file.records {
            let key = (rec.rtype.clone(), rec.rid.clone());
            if seen.contains(&key) {
                continue;
            }
            seen.push(key);

            match rec.rtype.as_str() {
                "PAR" => {
                    let points: Vec<Vec2> = rec.fields.iter().filter(|f| f.tag == "CORC").filter_map(|f| parse_coord(&f.value)).collect();
                    if points.len() >= 2 {
                        model.arcs.insert(rec.rid.clone(), Arc { points });
                    }
                }
                "FEA" => {
                    let mut attributes = Vec::new();
                    let mut pending_class: Option<String> = None;
                    for f in &rec.fields {
                        match f.tag.as_str() {
                            "ATPC" => pending_class = Some(last_token(&f.value).to_string()),
                            "ATVS" => {
                                if let Some(class) = pending_class.take() {
                                    attributes.push(Attribute { class, value: f.value.clone() });
                                }
                            }
                            _ => {}
                        }
                    }
                    model.features.insert(rec.rid.clone(), Feature { class: last_token_from_scp(rec), attributes });
                }
                "LNK" => {
                    let scp = rec.fields.iter().find(|f| f.tag == "SCPC").map(|f| last_token(&f.value).to_string()).unwrap_or_default();
                    let refs: Vec<(String, String)> = rec
                        .fields
                        .iter()
                        .filter(|f| f.tag == "FTPC")
                        .filter_map(|f| primitive_ref(&f.value))
                        .map(|(t, i)| (t.to_string(), i.to_string()))
                        .collect();
                    apply_relation(&mut model, &scp, &refs);
                }
                // PNO (nodes) and PFE (face placeholders) need no extra extraction: arcs
                // already carry their own full vertex list, and a face is otherwise just an id
                // that `face_sides`/`feature_face` key against.
                _ => {}
            }
        }
    }
    model
}

fn last_token_from_scp(rec: &crate::edigeo_text::TextRecord) -> String {
    rec.fields.iter().find(|f| f.tag == "SCPC").map(|f| last_token(&f.value).to_string()).unwrap_or_else(|| rec.rtype.clone())
}

fn apply_relation(model: &mut Model, relation_class: &str, refs: &[(String, String)]) {
    let find = |ty: &str| refs.iter().find(|(t, _)| t == ty).map(|(_, id)| id.clone());

    match relation_class {
        "ID_S_RCO_FAC_GCHE" | "ID_S_RCO_FAC_DRTE" => {
            let (Some(arc_id), Some(face_id)) = (find("PAR"), find("PFE")) else { return };
            let side = if relation_class.ends_with("GCHE") { Side::Left } else { Side::Right };
            model.face_sides.entry(face_id).or_default().push((arc_id, side));
        }
        "ID_S_RCO_NOD_INI" | "ID_S_RCO_NOD_FIN" => {
            // Node/arc endpoint relations: not needed here since each arc's own CORC list
            // already starts and ends at its nodes' coordinates.
        }
        other if other.starts_with("ID_S_RCO_") && other.ends_with("_id") => {
            // A feature-to-primitive composition, e.g. "ID_S_RCO_SUBDSECT_id" linking a FEA to
            // the PFE face it covers.
            if let (Some(feature_id), Some(face_id)) = (find("FEA"), find("PFE")) {
                model.feature_face.insert(feature_id, face_id);
            }
        }
        _ => {
            // Associations between two features (e.g. "SUBDSECT_SECTION") and anything else
            // this increment doesn't reconstruct geometry from.
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RingError {
    #[error("face {0}: no bordering arcs are known for it")]
    NoArcs(String),
    #[error("face {0}: a bordering arc ({1}) could not be found")]
    MissingArc(String, String),
    #[error("face {0}: the bordering arcs don't chain into a single closed ring")]
    CouldNotClose(String),
}

/// Chains a face's bordering arcs (each already given in the direction that keeps the face on
/// its left) end to end into one closed ring.
pub fn face_ring(model: &Model, face_id: &str, tolerance: f64) -> Result<Vec<Vec2>, RingError> {
    let sides = model.face_sides.get(face_id).ok_or_else(|| RingError::NoArcs(face_id.to_string()))?;
    if sides.is_empty() {
        return Err(RingError::NoArcs(face_id.to_string()));
    }

    let mut oriented: Vec<Vec<Vec2>> = Vec::with_capacity(sides.len());
    for (arc_id, side) in sides {
        let arc = model.arcs.get(arc_id).ok_or_else(|| RingError::MissingArc(face_id.to_string(), arc_id.clone()))?;
        let mut pts = arc.points.clone();
        if *side == Side::Right {
            pts.reverse();
        }
        oriented.push(pts);
    }

    let mut remaining = oriented;
    let mut chain = remaining.remove(0);
    let first_point = *chain.first().ok_or_else(|| RingError::CouldNotClose(face_id.to_string()))?;

    let mut guard = 0usize;
    while guard < MAX_RING_ARCS {
        guard += 1;
        let Some(&end) = chain.last() else { return Err(RingError::CouldNotClose(face_id.to_string())) };
        if end.near(first_point, tolerance) && chain.len() > 1 {
            return Ok(chain);
        }
        let Some(idx) = remaining.iter().position(|pts| pts.first().is_some_and(|&p| p.near(end, tolerance))) else {
            return Err(RingError::CouldNotClose(face_id.to_string()));
        };
        let mut next = remaining.remove(idx);
        next.remove(0); // the shared point is already the chain's last point.
        chain.extend(next);
        if remaining.is_empty() {
            let Some(&end) = chain.last() else { return Err(RingError::CouldNotClose(face_id.to_string())) };
            if end.near(first_point, tolerance) {
                chain.pop();
                return Ok(chain);
            }
            return Err(RingError::CouldNotClose(face_id.to_string()));
        }
    }
    Err(RingError::CouldNotClose(face_id.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edigeo_text::parse;

    /// A minimal lot: one square face (`Face_0`) bounded by 4 straight arcs, one `FEA` of class
    /// `SUBDSECT_id` linked to it with one attribute - the same shapes the real export uses, at
    /// a size small enough to read by eye.
    fn square_lot() -> TextFile {
        let text = "\
RTYSA03:PNO\r\nRIDSA02:N1\r\n\r\nCORCC14:+0.00;+0.00;\r\n\r\n\
RTYSA03:PNO\r\nRIDSA02:N2\r\n\r\nCORCC14:+10.0;+0.00;\r\n\r\n\
RTYSA03:PNO\r\nRIDSA02:N3\r\n\r\nCORCC16:+10.0;+10.0;\r\n\r\n\
RTYSA03:PNO\r\nRIDSA02:N4\r\n\r\nCORCC14:+0.00;+10.0;\r\n\r\n\
RTYSA03:PFE\r\nRIDSA06:Face_0\r\n\r\n\r\n\
RTYSA03:PAR\r\nRIDSA02:A1\r\n\r\nCORCC14:+0.00;+0.00;\r\nCORCC14:+10.0;+0.00;\r\n\r\n\
RTYSA03:PAR\r\nRIDSA02:A2\r\n\r\nCORCC14:+10.0;+0.00;\r\nCORCC16:+10.0;+10.0;\r\n\r\n\
RTYSA03:PAR\r\nRIDSA02:A3\r\n\r\nCORCC16:+10.0;+10.0;\r\nCORCC14:+0.00;+10.0;\r\n\r\n\
RTYSA03:PAR\r\nRIDSA02:A4\r\n\r\nCORCC14:+0.00;+10.0;\r\nCORCC14:+0.00;+0.00;\r\n\r\n\
RTYSA03:LNK\r\nRIDSA02:L1\r\n\r\nSCPCP18:LOT;SCP;REL;ID_S_RCO_FAC_GCHE\r\nFTCSN01:2\r\nFTPCP08:LOT;L;PAR;A1\r\nFTPCP08:LOT;L;PFE;Face_0\r\n\r\n\
RTYSA03:LNK\r\nRIDSA02:L2\r\n\r\nSCPCP18:LOT;SCP;REL;ID_S_RCO_FAC_GCHE\r\nFTCSN01:2\r\nFTPCP08:LOT;L;PAR;A2\r\nFTPCP08:LOT;L;PFE;Face_0\r\n\r\n\
RTYSA03:LNK\r\nRIDSA02:L3\r\n\r\nSCPCP18:LOT;SCP;REL;ID_S_RCO_FAC_GCHE\r\nFTCSN01:2\r\nFTPCP08:LOT;L;PAR;A3\r\nFTPCP08:LOT;L;PFE;Face_0\r\n\r\n\
RTYSA03:LNK\r\nRIDSA02:L4\r\n\r\nSCPCP18:LOT;SCP;REL;ID_S_RCO_FAC_GCHE\r\nFTCSN01:2\r\nFTPCP08:LOT;L;PAR;A4\r\nFTPCP08:LOT;L;PFE;Face_0\r\n\r\n\
RTYSA03:FEA\r\nRIDSA02:F1\r\n\r\nSCPCP22:LOT;SCP;OBJ;SUBDSECT_id\r\nATPCP18:LOT;SCP;ATT;IDU_id\r\nATVSA04:0001\r\n\r\n\
RTYSA03:LNK\r\nRIDSA02:L5\r\n\r\nSCPCP22:LOT;SCP;REL;ID_S_RCO_SUBDSECT_id\r\nFTCSN01:2\r\nFTPCP08:LOT;L;FEA;F1\r\nFTPCP08:LOT;L;PFE;Face_0\r\n\r\n\
";
        parse(text.as_bytes()).expect("the hand-built fixture should parse")
    }

    #[test]
    fn builds_arcs_features_and_face_links_from_a_square_lot() {
        let file = square_lot();
        let model = build_model(&[file]);
        assert_eq!(model.arcs.len(), 4);
        assert_eq!(model.features.len(), 1);
        assert_eq!(model.feature_face.get("F1"), Some(&"Face_0".to_string()));
        let feature = model.features.get("F1").expect("feature F1");
        assert_eq!(feature.class, "SUBDSECT_id");
        assert_eq!(feature.attributes, vec![Attribute { class: "IDU_id".to_string(), value: "0001".to_string() }]);
    }

    #[test]
    fn chains_four_arcs_into_a_closed_square_ring() {
        let model = build_model(&[square_lot()]);
        let ring = face_ring(&model, "Face_0", 1e-6).expect("the square's 4 arcs should chain into one ring");
        assert_eq!(ring.len(), 4, "a closing duplicate of the start point should not be kept");
        let area = cadcraft_geom::shoelace(&ring).abs();
        assert!((area - 100.0).abs() < 1e-6, "ring area should be 10x10=100, got {area} ({ring:?})");
    }

    #[test]
    fn a_face_with_no_known_arcs_is_an_error_not_a_panic() {
        let model = Model::default();
        let err = face_ring(&model, "Nonexistent", 1e-6);
        assert!(matches!(err, Err(RingError::NoArcs(_))));
    }

    #[test]
    fn merging_the_same_lot_twice_does_not_duplicate_records() {
        let file = square_lot();
        let model = build_model(&[file.clone(), file]);
        assert_eq!(model.arcs.len(), 4);
        assert_eq!(model.features.len(), 1);
    }
}
