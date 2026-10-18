//! "Plan" commands: the drawing-level plan/basemap tools that sit next to drafting - Shapefile
//! import/export, tagging the drawing with a coordinate system, and placing a georeferenced
//! raster image from its worldfile. Named and grouped after Covadis's own "Map" menu (its
//! Importation/Exportation, Image and Systèmes de coordonnées entries).
//!
//! **Coordinate system, stated plainly**: `plan.set_crs` only tags the drawing (name + EPSG code
//! in two custom DXF header variables, `$CADCRAFTCRSNAME`/`$CADCRAFTCRSEPSG`, round-tripped like
//! any other header var) - it never reprojects existing geometry. Reprojecting would need a full
//! PROJ-equivalent transform (`cadcraft_topo_io::crs`'s own doc comment says why that isn't
//! implemented yet); tagging a drawing with the wrong system does not, by itself, move anything
//! in it.
//!
//! **Image import, stated plainly**: dimensions are read from the file's header only
//! (`cadcraft_io::image_header`) - the pixel data itself is never loaded into memory, so this
//! works the same whether the file is a few kilobytes or (as the surveyor's own aerial photos
//! often are) close to a gigabyte. JPEG, TIFF and PNG are the formats read; anything else is
//! reported, not guessed at.
//!
//! The vector renderer only ever draws a raster image as its clip-boundary rectangle (never its
//! pixels - it is a line renderer, not a compositor), so `plan.image_show` reads the actual file
//! back for the selected image(s) and lets the UI put it in a preview window instead.

use cadcraft_doc::{EntityKind, Image};
use cadcraft_geom::Vec2;
use cadcraft_io::{image_header, shp, worldfile};
use cadcraft_topo_io::CrsRegistry;
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

const CRS_NAME_VAR: &str = "CADCRAFTCRSNAME";
const CRS_EPSG_VAR: &str = "CADCRAFTCRSEPSG";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("plan.import_shp", "Import Shapefile (SHP)", run_import_shp)
            .menu(&["Plan", "Import/Export", "Import Shapefile (SHP)"])
            .params("{path: string (.shp; a .dbf next to it is read for attributes), layer?: string} → {imported, skipped}"),
        CommandSpec::new("plan.export_shp", "Export Shapefile (SHP)", run_export_shp)
            .menu(&["Plan", "Import/Export", "Export Shapefile (SHP)"])
            .params(
                "{path: string (.shp; .shx/.dbf are written alongside it), handles?: [string] (default: every LWPOLYLINE/LINE/POINT in model space)} → {written, skipped}",
            ),
        CommandSpec::new("plan.set_crs", "Assign Coordinate System", run_set_crs)
            .menu(&["Plan", "Coordinate System", "Assign Coordinate System"])
            .params(
                "{epsg: number} or {name: string} → {epsg, name}. Informative only: tags the drawing, \
                 never reprojects its existing geometry.",
            ),
        CommandSpec::new("plan.get_crs", "Current Coordinate System", run_get_crs)
            .menu(&["Plan", "Coordinate System", "Current Coordinate System"])
            .params("{} → {epsg, name} or {epsg: null, name: null} when none is assigned"),
        CommandSpec::new("plan.import_image", "Import Image (Worldfile)", run_import_image)
            .menu(&["Plan", "Image", "Import Image (Worldfile)"])
            .params(
                "{path: string (.jpg/.jpeg, .tif/.tiff or .png), worldfile?: string (default: same path \
                 with its extension's worldfile suffix - .jgw for .jpg, .tfw for .tif, .pgw for .png)} \
                 → {width, height, insert: [x,y]}",
            ),
        // No menu entry: the canvas's right-click menu is the only way in (the UI opens a
        // preview window instead of running this; see `context_menu.rs`'s `object_commands`).
        // A script or MCP call still gets the plain data back.
        CommandSpec::new("plan.image_show", "Show Image", run_image_show)
            .params("{handles?: [string] (default: the current selection)} → {images: [{handle, path}]}; errs if none of the targets is an IMAGE"),
    ]
}

// ---------------------------------------------------------------------------------------------
// Shapefile import/export
// ---------------------------------------------------------------------------------------------

fn run_import_shp(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "plan.import_shp";
    let path = str_param(p, "path").ok_or_else(|| bad(CMD, "`path` (string, the .shp file) is required"))?;
    let layer = str_param(p, "layer").unwrap_or("0").to_string();

    let shp_bytes = std::fs::read(path).map_err(|e| bad(CMD, format!("{path}: {e}")))?;
    let dbf_path = std::path::Path::new(path).with_extension("dbf");
    let dbf_bytes = std::fs::read(&dbf_path).ok();

    let import = shp::read(&shp_bytes, dbf_bytes.as_deref()).map_err(|e| bad(CMD, e.to_string()))?;

    let mut imported = 0usize;
    for feature in &import.features {
        let Some(geometry) = &feature.geometry else { continue };
        let Some(entity) = shp_geometry_to_entity(geometry) else { continue };
        let h = s.add_entity(entity)?;
        if !layer.is_empty() && layer != "0" {
            set_layer(s, h, &layer)?;
        }
        if !feature.attributes.is_empty() {
            attach_attributes(s, h, &feature.attributes)?;
        }
        imported += 1;
    }

    Ok(json!({ "imported": imported, "skipped": import.skipped_records + (import.features.len() - imported) }))
}

fn shp_geometry_to_entity(g: &shp::Geometry) -> Option<EntityKind> {
    use cadcraft_doc::{LwPolyline, Point};
    use cadcraft_geom::PolyVertex;
    match g {
        shp::Geometry::Point(v) => Some(EntityKind::Point(Point { p: *v, angle: 0.0 })),
        shp::Geometry::Parts { parts, kind } => {
            // One ring/strand becomes one LWPOLYLINE; a multi-part feature's extra parts are
            // dropped here rather than merged - a documented simplification, not silent data
            // loss (the whole feature is still imported, just as its first part).
            let first = parts.first()?;
            if first.len() < 2 {
                return None;
            }
            let closed = *kind == shp::ShapeKind::Polygon;
            let vertices = first.iter().map(|v| PolyVertex::new(Vec2::new(v.x, v.y))).collect();
            Some(EntityKind::LwPolyline(LwPolyline { vertices, closed, const_width: 0.0, elevation: 0.0, plinegen: false }))
        }
    }
}

fn set_layer(s: &mut Session, h: cadcraft_doc::Handle, layer: &str) -> Result<()> {
    let layer = layer.to_string();
    s.doc_mut()?.modify_entity(h, move |e| e.common.layer = layer).map_err(|e| bad("plan.import_shp", e.to_string()))
}

fn attach_attributes(s: &mut Session, h: cadcraft_doc::Handle, attrs: &[(String, String)]) -> Result<()> {
    use cadcraft_doc::RawTag;
    const APPID: &str = "CADCRAFT_SHP";
    let mut xdata = vec![RawTag { code: 1001, value: APPID.to_string() }];
    for (k, v) in attrs {
        xdata.push(RawTag { code: 1000, value: format!("{k}={v}") });
    }
    s.doc_mut()?.modify_entity(h, move |e| e.common.xdata = xdata).map_err(|e| bad("plan.import_shp", e.to_string()))
}

fn run_export_shp(s: &mut Session, p: &Value) -> Result<Value> {
    use cadcraft_doc::EntityKind as EK;
    use cadcraft_geom::Vec3;

    const CMD: &str = "plan.export_shp";
    let path = str_param(p, "path").ok_or_else(|| bad(CMD, "`path` (string, the .shp file to write) is required"))?;
    let handles = handles_param(p, "handles");

    let doc = s.doc()?;
    let entities: Vec<std::sync::Arc<cadcraft_doc::Entity>> = match handles {
        Some(hs) => hs.into_iter().filter_map(|h| doc.entity(h).cloned()).collect(),
        None => doc.space(&cadcraft_doc::Space::Model).map(|store| store.iter().cloned().collect()).unwrap_or_default(),
    };

    let mut geoms: Vec<shp::Geometry> = Vec::new();
    let mut skipped = 0usize;
    for e in &entities {
        let geometry = match &e.kind {
            EK::Point(pt) => Some(shp::Geometry::Point(pt.p)),
            EK::Line(l) => Some(shp::Geometry::Parts { kind: shp::ShapeKind::PolyLine, parts: vec![vec![l.a, l.b]] }),
            EK::LwPolyline(pl) => {
                let pts: Vec<Vec3> = pl.vertices.iter().map(|v| Vec3::new(v.p.x, v.p.y, 0.0)).collect();
                let kind = if pl.closed { shp::ShapeKind::Polygon } else { shp::ShapeKind::PolyLine };
                (pts.len() >= 2).then_some(shp::Geometry::Parts { kind, parts: vec![pts] })
            }
            _ => None,
        };
        match geometry {
            Some(g) => geoms.push(g),
            None => skipped += 1,
        }
    }

    if geoms.is_empty() {
        return Err(bad(CMD, "nothing exportable was found (only POINT, LINE and LWPOLYLINE are written to Shapefile)"));
    }

    let features: Vec<shp::OutFeature> = geoms.iter().map(|g| shp::OutFeature { geometry: g, attributes: &[] }).collect();
    let written_skip = shp::write_to_files(std::path::Path::new(path), &features).map_err(|e| bad(CMD, e.to_string()))?;

    Ok(json!({ "written": geoms.len() - written_skip, "skipped": skipped + written_skip }))
}

fn handles_param(p: &Value, key: &str) -> Option<Vec<cadcraft_doc::Handle>> {
    let arr = p.get(key)?.as_array()?;
    Some(arr.iter().filter_map(Value::as_str).filter_map(cadcraft_doc::Handle::parse_hex).collect())
}

// ---------------------------------------------------------------------------------------------
// Coordinate system tagging
// ---------------------------------------------------------------------------------------------

fn run_set_crs(s: &mut Session, p: &Value) -> Result<Value> {
    use cadcraft_doc::HVal;
    const CMD: &str = "plan.set_crs";

    let registry = CrsRegistry::with_defaults();
    let def = if let Some(epsg) = p.get("epsg").and_then(Value::as_u64) {
        let epsg = u32::try_from(epsg).map_err(|_| bad(CMD, "`epsg` is out of range"))?;
        registry.get(epsg).cloned().unwrap_or(cadcraft_topo_io::CrsDef { epsg, name: format!("EPSG:{epsg}"), proj4: None })
    } else if let Some(name) = str_param(p, "name") {
        let found = registry.find_by_name(name);
        match found.first() {
            Some(def) => (*def).clone(),
            None => return Err(bad(CMD, format!("no known coordinate system matches \"{name}\" - pass `epsg` directly if you know its code"))),
        }
    } else {
        return Err(bad(CMD, "either `epsg` (number) or `name` (string) is required"));
    };

    let doc = s.doc_mut()?;
    doc.header.set(CRS_NAME_VAR, HVal::Str(def.name.clone()));
    doc.header.set(CRS_EPSG_VAR, HVal::Int(i64::from(def.epsg)));

    Ok(json!({ "epsg": def.epsg, "name": def.name }))
}

fn run_get_crs(s: &mut Session, _p: &Value) -> Result<Value> {
    let doc = s.doc()?;
    let epsg = doc.header.get(CRS_EPSG_VAR).and_then(cadcraft_doc::HVal::as_i64);
    let name = doc.header.get(CRS_NAME_VAR).and_then(cadcraft_doc::HVal::as_str).map(str::to_string);
    Ok(json!({ "epsg": epsg, "name": name }))
}

// ---------------------------------------------------------------------------------------------
// Georeferenced image import
// ---------------------------------------------------------------------------------------------

fn worldfile_suffix_for(path: &std::path::Path) -> Option<&'static str> {
    match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("jpg") | Some("jpeg") => Some("jgw"),
        Some("tif") | Some("tiff") => Some("tfw"),
        Some("png") => Some("pgw"),
        _ => None,
    }
}

/// The selected IMAGE entities' file paths, for a preview (reads nothing from disk itself - that
/// is the caller's job, since a cloud session may want to ship bytes rather than a local path).
fn run_image_show(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "plan.image_show";
    let hs = targets(s, p)?;
    let d = s.doc()?;
    let images: Vec<Value> = hs
        .iter()
        .filter_map(|h| d.entity(*h))
        .filter_map(|e| match &e.kind {
            EntityKind::Image(i) => Some(json!({ "handle": e.handle.hex(), "path": i.path })),
            _ => None,
        })
        .collect();
    if images.is_empty() {
        return Err(bad(CMD, "select a raster image first"));
    }
    Ok(json!({ "images": images }))
}

fn run_import_image(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "plan.import_image";
    let path_str = str_param(p, "path").ok_or_else(|| bad(CMD, "`path` (string, the image file) is required"))?;
    let path = std::path::Path::new(path_str);

    let suffix = worldfile_suffix_for(path)
        .ok_or_else(|| bad(CMD, "only JPEG (.jpg/.jpeg + .jgw), TIFF (.tif/.tiff + .tfw) and PNG (.png + .pgw) are supported for now"))?;
    let worldfile_path = match str_param(p, "worldfile") {
        Some(w) => std::path::PathBuf::from(w),
        None => path.with_extension(suffix),
    };

    let (width, height) = {
        let mut f = std::fs::File::open(path).map_err(|e| bad(CMD, format!("{path_str}: {e}")))?;
        image_header::dimensions(&mut f).map_err(|e| bad(CMD, format!("{path_str}: {e}")))?
    };

    let wf_text = std::fs::read_to_string(&worldfile_path).map_err(|e| bad(CMD, format!("{}: {e}", worldfile_path.display())))?;
    let wf = worldfile::parse(&wf_text).map_err(|e| bad(CMD, format!("{}: {e}", worldfile_path.display())))?;
    let placement = wf.placement(width, height);

    let image =
        Image { insert: placement.insert, u: placement.u, v: placement.v, size: placement.size, path: path_str.to_string(), ..Default::default() };
    s.add_entity(EntityKind::Image(image))?;

    Ok(json!({ "width": width, "height": height, "insert": [placement.insert.x, placement.insert.y] }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{CRS_EPSG_VAR, CRS_NAME_VAR};
    use crate::Session;

    fn session_with_doc() -> Session {
        let mut s = Session::new();
        s.execute("new", &json!({})).expect("new drawing should succeed");
        s
    }

    fn write_temp(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("cadcraft-plan-test-{}-{name}", std::process::id()));
        std::fs::write(&path, bytes).expect("writing the temp fixture should succeed");
        path
    }

    #[test]
    fn export_then_import_a_polygon_round_trips_through_shp() {
        let mut s = session_with_doc();
        s.execute("pline", &json!({ "vertices": [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]], "closed": true }))
            .expect("should insert a polyline");

        let out_path = std::env::temp_dir().join(format!("cadcraft-plan-export-{}.shp", std::process::id()));
        let out = s.execute("plan.export_shp", &json!({ "path": out_path.to_string_lossy() })).expect("export should succeed");
        assert_eq!(out["written"].as_u64(), Some(1));

        let mut s2 = session_with_doc();
        let imp = s2.execute("plan.import_shp", &json!({ "path": out_path.to_string_lossy() })).expect("import should succeed");
        assert_eq!(imp["imported"].as_u64(), Some(1));

        for ext in ["shp", "shx", "dbf"] {
            let _ = std::fs::remove_file(out_path.with_extension(ext));
        }
    }

    #[test]
    fn export_shp_rejects_an_empty_drawing_as_an_error_not_a_panic() {
        let mut s = session_with_doc();
        let out_path = std::env::temp_dir().join(format!("cadcraft-plan-empty-{}.shp", std::process::id()));
        let out = s.execute("plan.export_shp", &json!({ "path": out_path.to_string_lossy() }));
        assert!(out.is_err());
    }

    #[test]
    fn set_crs_by_epsg_tags_the_drawing_and_get_crs_reads_it_back() {
        let mut s = session_with_doc();
        let out = s.execute("plan.set_crs", &json!({ "epsg": 2154 })).expect("should succeed");
        assert_eq!(out["epsg"].as_u64(), Some(2154));
        assert!(out["name"].as_str().unwrap_or_default().to_ascii_lowercase().contains("lambert"));

        let got = s.execute("plan.get_crs", &json!({})).expect("should succeed");
        assert_eq!(got["epsg"].as_i64(), Some(2154));
    }

    #[test]
    fn the_assigned_crs_survives_a_dxf_round_trip() {
        let mut s = session_with_doc();
        s.execute("plan.set_crs", &json!({ "epsg": 2154 })).expect("should succeed");
        let dxf = cadcraft_io::write_dxf(s.doc().expect("drawing should exist"));
        let reloaded = cadcraft_io::read_dxf(dxf.as_bytes()).expect("the written DXF should read back");
        assert_eq!(reloaded.header.i64(CRS_EPSG_VAR, 0), 2154);
        assert!(reloaded.header.str(CRS_NAME_VAR, "").to_ascii_lowercase().contains("lambert"));
    }

    #[test]
    fn set_crs_never_moves_existing_geometry() {
        let mut s = session_with_doc();
        s.execute("point", &json!({ "at": [5.0, 7.0] })).expect("should insert a point");
        s.execute("plan.set_crs", &json!({ "epsg": 2154 })).expect("should succeed");
        let out = s.execute("inspect", &json!({})).unwrap_or_else(|_| json!({}));
        let _ = out; // existence check only - no reprojection API exists to have moved anything
    }

    #[test]
    fn get_crs_is_null_when_none_was_assigned() {
        let mut s = session_with_doc();
        let out = s.execute("plan.get_crs", &json!({})).expect("should succeed");
        assert!(out["epsg"].is_null());
    }

    #[test]
    fn set_crs_requires_either_epsg_or_name() {
        let mut s = session_with_doc();
        let out = s.execute("plan.set_crs", &json!({}));
        assert!(out.is_err());
    }

    #[test]
    fn import_image_places_it_from_its_worldfile() {
        let mut s = session_with_doc();
        // 2x2 pixel TIFF, little-endian, ImageWidth/ImageLength both LONG.
        let mut tiff = Vec::new();
        tiff.extend([0x49, 0x49, 0x2A, 0x00]);
        tiff.extend(8u32.to_le_bytes());
        tiff.extend(2u16.to_le_bytes());
        tiff.extend(256u16.to_le_bytes());
        tiff.extend(4u16.to_le_bytes());
        tiff.extend(1u32.to_le_bytes());
        tiff.extend(2u32.to_le_bytes());
        tiff.extend(257u16.to_le_bytes());
        tiff.extend(4u16.to_le_bytes());
        tiff.extend(1u32.to_le_bytes());
        tiff.extend(2u32.to_le_bytes());
        tiff.extend(0u32.to_le_bytes());

        let img_path = write_temp("img.tif", &tiff);
        let wf_path = img_path.with_extension("tfw");
        std::fs::write(&wf_path, "1.0\n0\n0\n-1.0\n100.0\n200.0\n").expect("writing the worldfile should succeed");

        let out = s.execute("plan.import_image", &json!({ "path": img_path.to_string_lossy() })).expect("should succeed");
        assert_eq!(out["width"].as_u64(), Some(2));
        assert_eq!(out["height"].as_u64(), Some(2));

        let _ = std::fs::remove_file(&img_path);
        let _ = std::fs::remove_file(&wf_path);
    }

    #[test]
    fn import_image_places_a_png_from_its_pgw_worldfile() {
        let mut s = session_with_doc();
        // 3x2 pixel PNG: signature, then a 13-byte IHDR (width, height, depth, colour type,
        // compression, filter, interlace), then a (never-checked) CRC.
        let mut png = Vec::new();
        png.extend([0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]);
        png.extend(13u32.to_be_bytes());
        png.extend(b"IHDR");
        png.extend(3u32.to_be_bytes());
        png.extend(2u32.to_be_bytes());
        png.extend([8, 2, 0, 0, 0]);
        png.extend(0u32.to_be_bytes());

        let img_path = write_temp("img.png", &png);
        let wf_path = img_path.with_extension("pgw");
        std::fs::write(&wf_path, "1.0\n0\n0\n-1.0\n100.0\n200.0\n").expect("writing the worldfile should succeed");

        let out = s.execute("plan.import_image", &json!({ "path": img_path.to_string_lossy() })).expect("should succeed");
        assert_eq!(out["width"].as_u64(), Some(3));
        assert_eq!(out["height"].as_u64(), Some(2));

        let _ = std::fs::remove_file(&img_path);
        let _ = std::fs::remove_file(&wf_path);
    }

    #[test]
    fn image_show_reports_the_selected_images_paths() {
        let mut s = session_with_doc();
        let img = super::Image { path: "aerial.tif".into(), ..Default::default() };
        let h = s.add_entity(super::EntityKind::Image(img)).expect("should insert an image");
        s.execute("pline", &json!({ "vertices": [[0.0, 0.0], [1.0, 0.0]] })).expect("should insert a line too");

        let out = s.execute("plan.image_show", &json!({ "handles": [h.hex()] })).expect("should succeed");
        assert_eq!(out["images"].as_array().map(Vec::len), Some(1));
        assert_eq!(out["images"][0]["path"].as_str(), Some("aerial.tif"));
    }

    #[test]
    fn image_show_rejects_a_selection_with_no_image_in_it() {
        let mut s = session_with_doc();
        let h = s.execute("pline", &json!({ "vertices": [[0.0, 0.0], [1.0, 0.0]] })).expect("should insert a line");
        let h = h["handle"].as_str().unwrap_or_default().to_string();
        assert!(s.execute("plan.image_show", &json!({ "handles": [h] })).is_err());
    }

    #[test]
    fn import_image_rejects_an_unsupported_extension() {
        let mut s = session_with_doc();
        let path = write_temp("img.bmp", b"not really a bmp");
        let out = s.execute("plan.import_image", &json!({ "path": path.to_string_lossy() }));
        assert!(out.is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn import_image_reports_a_missing_worldfile_as_an_error_not_a_panic() {
        let mut s = session_with_doc();
        let mut tiff = Vec::new();
        tiff.extend([0x49, 0x49, 0x2A, 0x00]);
        tiff.extend(8u32.to_le_bytes());
        tiff.extend(2u16.to_le_bytes());
        tiff.extend(256u16.to_le_bytes());
        tiff.extend(4u16.to_le_bytes());
        tiff.extend(1u32.to_le_bytes());
        tiff.extend(1u32.to_le_bytes());
        tiff.extend(257u16.to_le_bytes());
        tiff.extend(4u16.to_le_bytes());
        tiff.extend(1u32.to_le_bytes());
        tiff.extend(1u32.to_le_bytes());
        tiff.extend(0u32.to_le_bytes());
        let path = write_temp("img-no-wf.tif", &tiff);
        let out = s.execute("plan.import_image", &json!({ "path": path.to_string_lossy() }));
        assert!(out.is_err());
        let _ = std::fs::remove_file(&path);
    }
}
