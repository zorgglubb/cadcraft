//! A small convention for keeping an imported feature's external attribute table and fields on
//! its entity, as plain XDATA (DXF group 1000 strings under a 1001 application id) - so they
//! survive save/load through ordinary DXF, round-trip through undo like any other entity data,
//! and can be shown (and, via the importer's own command, edited) in Properties without
//! CADCraft inventing a second, DXF-incompatible storage mechanism.
//!
//! Used today by `plan.import_shp` to keep a Shapefile's `.dbf` fields; any future importer with
//! the same shape of data (one table name plus flat field/value pairs) can reuse it under its
//! own application id.

use crate::{Entity, RawTag};

/// XDATA application id for attributes imported from a Shapefile's `.dbf` (`plan.import_shp`).
pub const SHP_APPID: &str = "CADCRAFT_SHP";

/// XDATA application id for a user-defined attribute table attached by `data.assign_table`
/// (table *schema* lives separately, on `Drawing::attribute_tables`; this is only the per-entity
/// table name + field values, same shape as the Shapefile mechanism above).
pub const ATTR_TABLE_APPID: &str = "CADCRAFT_ATTRTABLE";

/// Sentinel key the table name rides on, as the first field-shaped `1000` tag after the APPID
/// marker - chosen unlikely to collide with a real field name, which is why it isn't simply
/// `"table"`. There is nowhere else to put it: XDATA is a flat list of `(code, value)` pairs, and
/// formats like Shapefile carry no table name of their own (one table per file).
const TABLE_KEY: &str = "__CADCRAFT_TABLE__";

/// Builds the XDATA tagging `table` and `fields` under `appid`, ready to assign to an entity's
/// `common.xdata`.
pub fn tag_fields(appid: &str, table: &str, fields: &[(String, String)]) -> Vec<RawTag> {
    let mut xdata = vec![RawTag { code: 1001, value: appid.to_string() }, RawTag { code: 1000, value: format!("{TABLE_KEY}={table}") }];
    xdata.extend(fields.iter().map(|(k, v)| RawTag { code: 1000, value: format!("{k}={v}") }));
    xdata
}

/// Reads back `entity`'s table name and fields (in file order), if it carries `appid`'s XDATA
/// (see [`tag_fields`]). `None` for an entity with no such XDATA.
pub fn fields(entity: &Entity, appid: &str) -> Option<(String, Vec<(String, String)>)> {
    let xdata = &entity.common.xdata;
    if !xdata.first().is_some_and(|t| t.code == 1001 && t.value == appid) {
        return None;
    }
    let mut table = String::new();
    let mut out = Vec::new();
    for tag in xdata.iter().skip(1).filter(|t| t.code == 1000) {
        let Some((k, v)) = tag.value.split_once('=') else { continue };
        if k == TABLE_KEY {
            table = v.to_string();
        } else {
            out.push((k.to_string(), v.to_string()));
        }
    }
    Some((table, out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EntityKind, Handle, Point};
    use cadcraft_geom::Vec3;

    fn point_entity() -> Entity {
        Entity::new(Handle(1), EntityKind::Point(Point { p: Vec3::ZERO, angle: 0.0 }))
    }

    #[test]
    fn tag_fields_round_trips_through_fields() {
        let mut e = point_entity();
        e.common.xdata = tag_fields(SHP_APPID, "PARCELS", &[("NOM".to_string(), "Parcelle A".to_string())]);
        let (table, fields) = fields(&e, SHP_APPID).expect("tagged entity should read back");
        assert_eq!(table, "PARCELS");
        assert_eq!(fields, vec![("NOM".to_string(), "Parcelle A".to_string())]);
    }

    #[test]
    fn fields_is_none_for_an_untagged_entity_or_a_different_appid() {
        let e = point_entity();
        assert!(fields(&e, SHP_APPID).is_none());

        let mut e2 = point_entity();
        e2.common.xdata = tag_fields("SOME_OTHER_APP", "T", &[]);
        assert!(fields(&e2, SHP_APPID).is_none());
    }
}
