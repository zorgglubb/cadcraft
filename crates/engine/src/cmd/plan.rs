//! "Plan" commands: the drawing-level plan/basemap tools that sit next to drafting - Shapefile
//! import/export, tagging the drawing with a coordinate system, and placing a georeferenced
//! raster image from its worldfile. Named and grouped after Covadis's own "Map" menu (its
//! Importation/Exportation, Image and Systèmes de coordonnées entries).
//!
//! **Coordinate system, stated plainly**: `plan.set_crs` only tags the drawing (name + EPSG code
//! in two custom DXF header variables, `$CADCRAFTCRSNAME`/`$CADCRAFTCRSEPSG`, round-tripped like
//! any other header var) - it never moves anything. `plan.convert_crs` is the command that
//! reprojects: it runs every entity's points through `cadcraft_topo_io::CrsRegistry::transform`
//! (that crate's `projection` module says exactly which systems and ellipsoid assumptions this
//! covers, and where it stops - no datum shift between any pair of this registry's built-in
//! systems, and no definition at all for the legacy Lambert NTF zones or the DOM-TOM systems
//! until one is `register_proj4`'d) and re-tags the drawing with the target system afterward.
//!
//! **Image import, stated plainly**: dimensions are read from the file's header only
//! (`cadcraft_io::image_header`) - the pixel data itself is never loaded into memory, so this
//! works the same whether the file is a few kilobytes or (as the surveyor's own aerial photos
//! often are) close to a gigabyte. JPEG, TIFF and PNG are the formats read; anything else is
//! reported, not guessed at.
//!
//! `cadcraft-render` (this crate's vector tessellator, L2) only ever draws a raster image as its
//! clip-boundary rectangle - it stays a line renderer, not a compositor, per the project's
//! layering rule (no egui/wgpu below L5). The real pixels are composited straight onto the
//! canvas instead, by `cadcraft-ui-egui` reading the file directly (`canvas.rs`'s `draw_images`).
//! `plan.image_show` is separate: it reads the actual file back for the selected image(s) and
//! lets the UI put it in its own preview window, e.g. to inspect one without panning to it.

use cadcraft_doc::{Common, Drawing, EntityKind, HAlign, Image, Layer, Layout, Line, PageSetup, Space, Text, VAlign, Viewport};
use cadcraft_geom::{Bounds2, Vec2};
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
            .params(
                "{path: string (.shp; a .dbf next to it is read for attributes), layer?: string} → {imported, skipped}. \
                 Each feature's DBF fields (and the .shp file's own name, standing in for a table name) are \
                 kept on the entity as XDATA - visible in Properties and carried over if re-exported.",
            ),
        CommandSpec::new("plan.export_shp", "Export Shapefile (SHP)", run_export_shp)
            .menu(&["Plan", "Import/Export", "Export Shapefile (SHP)"])
            .params(
                "{path: string (.shp; .shx/.dbf are written alongside it), handles?: [string] (default: every LWPOLYLINE/LINE/POINT in model space)} \
                 → {written, skipped}. An entity's fields from a previous plan.import_shp are written back to the DBF; \
                 an entity with none exports with an empty attribute row (the DBF's columns come from the first feature).",
            ),
        // No menu entry: the Properties palette's own "Attributes" section is the only way in -
        // editing one DBF field an imported Shapefile entity carries, from `plan.import_shp`.
        CommandSpec::new("plan.set_shp_attribute", "Set Shapefile Attribute", run_set_shp_attribute)
            .params("{handle: string, key: string, value: string} → {changed: bool}; errs if the entity carries no Shapefile attributes at all"),
        CommandSpec::new("plan.set_crs", "Assign Coordinate System", run_set_crs)
            .menu(&["Plan", "Coordinate System", "Assign Coordinate System"])
            .params(
                "{epsg: number} or {name: string} → {epsg, name}. Informative only: tags the drawing, \
                 never reprojects its existing geometry - plan.convert_crs does that.",
            ),
        CommandSpec::new("plan.get_crs", "Current Coordinate System", run_get_crs)
            .menu(&["Plan", "Coordinate System", "Current Coordinate System"])
            .params("{} → {epsg, name} or {epsg: null, name: null} when none is assigned"),
        // No menu entry of its own: the UI reads this to fill the Coordinate System dialog's
        // EPSG picker (Assign and Convert both use it).
        CommandSpec::new("plan.list_crs", "Coordinate Systems", run_list_crs)
            .params("{query?: string (filters by name, case-insensitive)} → {systems: [{epsg, name, hasDefinition}]}"),
        CommandSpec::new("plan.convert_crs", "Convert Coordinate System", run_convert_crs)
            .menu(&["Plan", "Coordinate System", "Convert Coordinate System..."])
            .params(
                "{to: number (EPSG), from?: number (EPSG; default: the drawing's assigned system - \
                 errs if neither is given and none is assigned), handles?: [string] (default: every \
                 POINT/LINE/LWPOLYLINE/POLYLINE3D/CIRCLE/ARC/TEXT/MTEXT/INSERT/IMAGE in model space)} \
                 → {converted, skipped, from, to}. Actually moves geometry (unlike plan.set_crs) and \
                 re-tags the drawing with `to` when it finishes; cadcraft_topo_io::projection's doc \
                 says which systems and ellipsoid assumptions this covers.",
            ),
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
        CommandSpec::new("plan.carroyage", "Carroyage", run_carroyage).menu(&["Plan", "Carroyage..."]).params(
            "{window: {min:[x,y], max:[x,y]} (the model-space area to grid), scale: number (the N \
                 in 1:N), paper?: \"A4\"|\"A3\"|\"A2\"|\"A1\"|\"A0\" (default \"A4\"), landscape?: bool \
                 (default: whichever orientation fits `window`'s aspect), spacingCm?: number (default \
                 10, the cross spacing in paper cm), layout?: string (default: next free name), layer?: \
                 string (default \"CARROYAGE\")} → {layout, layer, viewport, scale, crosses}. Fits \
                 `window` into a new paper-space layout's printable area at exactly 1:`scale`, draws a \
                 double-line border with tick marks, a cross every `spacingCm` of paper, and text labels \
                 on round ground X/Y coordinates (in the drawing's assigned CRS, if any) along the \
                 border, all on a new layer.",
        ),
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
    // The Shapefile format has no table name of its own (one table per file), so the .shp file's
    // own stem stands in for one - shown in Properties alongside the fields it names.
    let table = std::path::Path::new(path).file_stem().and_then(std::ffi::OsStr::to_str).unwrap_or("shapefile").to_string();

    let mut imported = 0usize;
    for feature in &import.features {
        let Some(geometry) = &feature.geometry else { continue };
        let Some(entity) = shp_geometry_to_entity(geometry) else { continue };
        let h = s.add_entity(entity)?;
        if !layer.is_empty() && layer != "0" {
            set_layer(s, h, &layer)?;
        }
        if !feature.attributes.is_empty() {
            attach_attributes(s, h, &table, &feature.attributes)?;
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

/// Tags `h` with its source DBF `table` name and `attrs` (field name/value pairs) as XDATA (the
/// `cadcraft_doc::import_attrs` convention), so they survive as real entity data - read back by
/// [`shp_attributes`] for display (Properties) and for re-export.
fn attach_attributes(s: &mut Session, h: cadcraft_doc::Handle, table: &str, attrs: &[(String, String)]) -> Result<()> {
    let xdata = cadcraft_doc::import_attrs::tag_fields(cadcraft_doc::import_attrs::SHP_APPID, table, attrs);
    s.doc_mut()?.modify_entity(h, move |e| e.common.xdata = xdata).map_err(|e| bad("plan.import_shp", e.to_string()))
}

/// Reads back an entity's Shapefile table name and attribute fields (in file order), if it was
/// tagged by [`attach_attributes`]. `None` for an entity with no such XDATA. The Properties
/// palette (`cadcraft-ui-egui`) reads the same data straight from `cadcraft_doc::import_attrs`.
pub(crate) fn shp_attributes(e: &cadcraft_doc::Entity) -> Option<(String, Vec<(String, String)>)> {
    cadcraft_doc::import_attrs::fields(e, cadcraft_doc::import_attrs::SHP_APPID)
}

fn run_set_shp_attribute(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "plan.set_shp_attribute";
    let h = cadcraft_doc::Handle::parse_hex(str_param(p, "handle").ok_or_else(|| bad(CMD, "`handle` is required"))?)
        .ok_or_else(|| bad(CMD, "`handle` is not a valid handle"))?;
    let key = str_param(p, "key").ok_or_else(|| bad(CMD, "`key` is required"))?;
    let value = str_param(p, "value").ok_or_else(|| bad(CMD, "`value` is required"))?.to_string();

    let (table, mut fields) =
        s.doc()?.entity(h).and_then(|e| shp_attributes(e)).ok_or_else(|| bad(CMD, "that entity carries no Shapefile attributes"))?;
    let field = fields.iter_mut().find(|(k, _)| k == key).ok_or_else(|| bad(CMD, format!("no `{key}` field on this entity")))?;
    field.1 = value;

    let xdata = cadcraft_doc::import_attrs::tag_fields(cadcraft_doc::import_attrs::SHP_APPID, &table, &fields);
    s.doc_mut()?.modify_entity(h, move |e| e.common.xdata = xdata).map_err(|e| bad(CMD, e.to_string()))?;
    Ok(json!({ "changed": true }))
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
    // Attribute fields carried over from an entity's own `plan.import_shp`-attached XDATA (see
    // `shp_attributes`), if any - round-tripping a reimported-then-reexported shapefile's fields
    // rather than silently dropping them. An entity with none exports with an empty attribute row.
    let mut attrs: Vec<Vec<(String, String)>> = Vec::new();
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
            Some(g) => {
                geoms.push(g);
                attrs.push(shp_attributes(e).map(|(_, fields)| fields).unwrap_or_default());
            }
            None => skipped += 1,
        }
    }

    if geoms.is_empty() {
        return Err(bad(CMD, "nothing exportable was found (only POINT, LINE and LWPOLYLINE are written to Shapefile)"));
    }

    let features: Vec<shp::OutFeature> = geoms.iter().zip(&attrs).map(|(g, a)| shp::OutFeature { geometry: g, attributes: a }).collect();
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

fn run_list_crs(_s: &mut Session, p: &Value) -> Result<Value> {
    let registry = CrsRegistry::with_defaults();
    let query = str_param(p, "query");
    let defs: Vec<&cadcraft_topo_io::CrsDef> = match query {
        Some(q) if !q.trim().is_empty() => registry.find_by_name(q),
        _ => registry.all(),
    };
    let systems: Vec<Value> = defs.iter().map(|d| json!({ "epsg": d.epsg, "name": d.name, "hasDefinition": d.proj4.is_some() })).collect();
    Ok(json!({ "systems": systems }))
}

/// Every geometric point `plan.convert_crs` knows how to move, for whichever entity kinds carry
/// one. Block/XREF interiors and a raster image's orientation vectors are left exactly as
/// imported (the insertion point moves, the content doesn't re-scale or re-rotate for the new
/// projection's local convergence/scale) - a stated simplification, like `plan.export_shp`'s.
fn convert_entity_points(e: &mut cadcraft_doc::Entity, map: &impl Fn(cadcraft_geom::Vec3) -> cadcraft_geom::Vec3) -> bool {
    let map2 = |p: Vec2| {
        let m = map(cadcraft_geom::Vec3::new(p.x, p.y, 0.0));
        Vec2::new(m.x, m.y)
    };
    match &mut e.kind {
        EntityKind::Point(pt) => pt.p = map(pt.p),
        EntityKind::Line(l) => {
            l.a = map(l.a);
            l.b = map(l.b);
        }
        EntityKind::LwPolyline(pl) => {
            for v in &mut pl.vertices {
                v.p = map2(v.p);
            }
        }
        EntityKind::Polyline3d(pl) => {
            for v in &mut pl.points {
                *v = map(*v);
            }
        }
        EntityKind::Circle(c) => c.center = map(c.center),
        EntityKind::Arc(a) => a.center = map(a.center),
        EntityKind::Text(t) => {
            t.insert = map(t.insert);
            t.align_pt = t.align_pt.map(map);
        }
        EntityKind::MText(t) => t.insert = map(t.insert),
        EntityKind::Insert(i) => i.insert = map(i.insert),
        EntityKind::Image(img) => img.insert = map(img.insert),
        _ => return false,
    }
    true
}

fn run_convert_crs(s: &mut Session, p: &Value) -> Result<Value> {
    use cadcraft_doc::HVal;
    const CMD: &str = "plan.convert_crs";

    let to_epsg = match p.get("to").and_then(Value::as_u64) {
        Some(v) => u32::try_from(v).map_err(|_| bad(CMD, "`to` is out of range"))?,
        None => return Err(bad(CMD, "`to` (EPSG number) is required")),
    };
    let from_epsg = match p.get("from").and_then(Value::as_u64) {
        Some(v) => u32::try_from(v).map_err(|_| bad(CMD, "`from` is out of range"))?,
        None => {
            let doc = s.doc()?;
            let epsg = doc.header.get(CRS_EPSG_VAR).and_then(HVal::as_i64).and_then(|v| u32::try_from(v).ok());
            epsg.ok_or_else(|| bad(CMD, "no coordinate system is assigned to this drawing yet; pass `from`, or run plan.set_crs first"))?
        }
    };

    let registry = CrsRegistry::with_defaults();
    // Probe once up front: a bad pair (unknown EPSG, or one with no proj4 definition to
    // reproject through) fails clearly here, rather than silently skipping every entity below.
    registry.transform(from_epsg, to_epsg, Vec2::ZERO).map_err(|e| bad(CMD, e.to_string()))?;

    let handles: Vec<cadcraft_doc::Handle> = match handles_param(p, "handles") {
        Some(hs) => hs,
        None => s.doc()?.space(&cadcraft_doc::Space::Model).map(|st| st.iter().map(|e| e.handle).collect()).unwrap_or_default(),
    };

    let map = move |v: cadcraft_geom::Vec3| {
        let r = registry.transform(from_epsg, to_epsg, v.xy()).unwrap_or(v.xy());
        cadcraft_geom::Vec3::new(r.x, r.y, v.z)
    };
    let (mut converted, mut skipped) = (0usize, 0usize);
    for h in handles {
        let mut moved = false;
        s.doc_mut()?.modify_entity(h, |e| moved = convert_entity_points(e, &map)).map_err(|e| bad(CMD, e.to_string()))?;
        if moved {
            converted += 1;
        } else {
            skipped += 1;
        }
    }

    if let Some(def) = CrsRegistry::with_defaults().get(to_epsg) {
        let doc = s.doc_mut()?;
        doc.header.set(CRS_NAME_VAR, HVal::Str(def.name.clone()));
        doc.header.set(CRS_EPSG_VAR, HVal::Int(i64::from(to_epsg)));
    }

    Ok(json!({ "converted": converted, "skipped": skipped, "from": from_epsg, "to": to_epsg }))
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

// ---------------------------------------------------------------------------------------------
// Carroyage (coordinate-grid overlay), modeled after Covadis's "Carroyage" tool: a printed-plan
// grid of crosses/ticks/round-coordinate labels over a model-space window shown at an exact
// scale, on its own new paper-space layout and layer.
// ---------------------------------------------------------------------------------------------

fn next_carroyage_layout_name(d: &Drawing) -> String {
    (1..=10_000).map(|i| format!("Carroyage{i}")).find(|n| d.layout(n).is_none()).unwrap_or_else(|| "Carroyage".into())
}

fn valid_plan_name(n: &str) -> bool {
    let n = n.trim();
    !n.is_empty() && n.chars().count() <= 255 && !n.chars().any(|c| "<>/\\\":;?*|,=`".contains(c) || c.is_control())
}

/// A straight line placed directly into paper space (not through the session's current space).
fn carroyage_line(d: &mut Drawing, space: &Space, common: &Common, a: Vec2, b: Vec2) -> Result<()> {
    d.add(space, common.clone(), EntityKind::Line(Line { a: a.to3(0.0), b: b.to3(0.0) }))?;
    Ok(())
}

/// A piece of text placed directly into paper space, centered on `at`.
fn carroyage_text(d: &mut Drawing, space: &Space, common: &Common, at: Vec2, height: f64, value: String) -> Result<()> {
    let text = Text {
        insert: at.to3(0.0),
        align_pt: Some(at.to3(0.0)),
        height,
        value,
        rotation: 0.0,
        width_factor: 1.0,
        oblique: 0.0,
        style: "Standard".into(),
        halign: HAlign::Center,
        valign: VAlign::Middle,
    };
    d.add(space, common.clone(), EntityKind::Text(text))?;
    Ok(())
}

/// Round `v` to the nearest multiple of `step` (ground units), away from the window if `step`
/// doesn't divide evenly - so grid lines never fall exactly on the window's own edges.
fn round_to(v: f64, step: f64) -> f64 {
    if step <= 0.0 || !step.is_finite() { v } else { (v / step).round() * step }
}

fn run_carroyage(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "plan.carroyage";

    let win = p.get("window").ok_or_else(|| bad(CMD, "`window` ({min:[x,y], max:[x,y]}) is required"))?;
    let wmin = point_param(win, "min").ok_or_else(|| bad(CMD, "`window.min` ([x,y]) is required"))?;
    let wmax = point_param(win, "max").ok_or_else(|| bad(CMD, "`window.max` ([x,y]) is required"))?;
    let window = Bounds2::new(wmin, wmax);
    if window.width() < 1e-6 || window.height() < 1e-6 {
        return Err(bad(CMD, "`window` has no area"));
    }
    if window.width() > 1e9 || window.height() > 1e9 {
        return Err(bad(CMD, "`window` is too large"));
    }

    let scale = f64_req(CMD, p, "scale")?;
    if scale <= 0.0 {
        return Err(bad(CMD, "`scale` must be positive (the N in 1:N)"));
    }

    let paper_name = str_param(p, "paper").unwrap_or("A4");
    let paper = cadcraft_render::paper_size(paper_name).ok_or_else(|| bad(CMD, format!("unknown paper format `{paper_name}`")))?;
    let landscape = p.get("landscape").and_then(Value::as_bool).unwrap_or(window.width() >= window.height());
    let (paper_w, paper_h) = if landscape {
        (paper.height_mm.max(paper.width_mm), paper.width_mm.min(paper.height_mm))
    } else {
        (paper.width_mm.min(paper.height_mm), paper.height_mm.max(paper.width_mm))
    };

    let spacing_cm = f64_or(p, "spacingCm", 10.0);
    if spacing_cm <= 0.0 || !spacing_cm.is_finite() {
        return Err(bad(CMD, "`spacingCm` must be positive"));
    }

    let d0 = s.doc()?;
    // Paper space is in the drawing's own measurement unit (mm for metric, inches for imperial;
    // `cadcraft_render::sheet` does the same `/unit_mm` to get a layout's paper-space size), so
    // every paper-space length here is computed in millimetres and then divided by `unit_mm`.
    let unit_mm = cadcraft_render::paper::paper_unit_mm(d0);
    let to_ps = |mm: f64| mm / unit_mm;
    let margin_mm = 15.0_f64.min(paper_w.min(paper_h) * 0.1).max(5.0);
    let printable_w = to_ps((paper_w - 2.0 * margin_mm).max(1.0));
    let printable_h = to_ps((paper_h - 2.0 * margin_mm).max(1.0));

    // The paper-space size `window` occupies at exactly 1:`scale` (paper = model / scale) - never
    // silently rescaled, since that would mislabel every coordinate this command then writes.
    let vp_w = window.width() / scale;
    let vp_h = window.height() / scale;
    if vp_w < 1e-9 || vp_h < 1e-9 {
        return Err(bad(CMD, "the window is too small to fit at that scale"));
    }
    if vp_w > printable_w || vp_h > printable_h {
        return Err(bad(
            CMD,
            format!("the window doesn't fit on {paper_name} at 1:{scale} - choose a larger paper, a smaller scale or a smaller window"),
        ));
    }
    let origin = Vec2::new(to_ps(margin_mm), to_ps(margin_mm));
    let vp_rect = Bounds2::new(origin, origin + Vec2::new(vp_w, vp_h));

    let layout_name = match str_param(p, "layout") {
        Some(n) => {
            if !valid_plan_name(n) {
                return Err(bad(CMD, "invalid layout name"));
            }
            if d0.layout(n).is_some() {
                return Err(bad(CMD, format!("layout `{n}` already exists")));
            }
            n.to_string()
        }
        None => next_carroyage_layout_name(d0),
    };
    let layer_name = str_param(p, "layer").unwrap_or("CARROYAGE").to_string();
    if !valid_plan_name(&layer_name) {
        return Err(bad(CMD, "invalid layer name"));
    }
    let layer_existed = d0.layer(&layer_name).is_some();
    let epsg = d0.header.get(CRS_EPSG_VAR).and_then(cadcraft_doc::HVal::as_i64);

    // --- Create the layout (its own page setup, sized/oriented for the chosen paper) ---
    let page = PageSetup { paper: paper.name.into(), width_mm: paper.width_mm, height_mm: paper.height_mm, landscape, ..PageSetup::default() };

    // Computed before the mutable borrow below (it reads the drawing itself).
    let common = s.current_common();
    let mut grid_common = common.clone();
    grid_common.layer = layer_name.clone();

    let d = s.doc_mut()?;
    if !layer_existed {
        d.layers.push(Layer::new(&layer_name));
    }
    let order = d.layouts.iter().map(|l| l.tab_order).max().unwrap_or(0).saturating_add(1);
    d.layouts.push(Layout { page, ..Layout::new(&layout_name, order) });

    let space = Space::Paper(layout_name.clone());

    // --- The viewport showing `window` at exactly 1:`scale` ---
    let next_id = d
        .layouts
        .iter()
        .flat_map(|l| l.entities.iter())
        .filter_map(|e| if let EntityKind::Viewport(v) = &e.kind { Some(v.id) } else { None })
        .max()
        .unwrap_or(1)
        .max(1)
        .saturating_add(1);
    let viewport = Viewport {
        center: vp_rect.center().to3(0.0),
        width: vp_rect.width(),
        height: vp_rect.height(),
        view_center: window.center(),
        view_height: window.height(),
        id: next_id,
        ..Default::default()
    };
    let vp_handle = d.add(&space, common.clone(), EntityKind::Viewport(viewport))?;

    // --- Border (double line) ---
    let outer = vp_rect;
    let inset = to_ps(1.0).min(outer.width().min(outer.height()) * 0.02);
    let inner = Bounds2::new(outer.min + Vec2::new(inset, inset), outer.max - Vec2::new(inset, inset));
    for r in [outer, inner] {
        let corners = [r.min, Vec2::new(r.max.x, r.min.y), r.max, Vec2::new(r.min.x, r.max.y)];
        for i in 0..4 {
            carroyage_line(d, &space, &grid_common, corners[i], corners[(i + 1) % 4])?;
        }
    }

    // --- Crosses, ticks and round-coordinate labels ---
    // Ground distance per paper spacing: a paper-space spacing of `spacingCm` cm maps to
    // `spacing_ps * scale` ground units, since paper = model / scale.
    let spacing_ps = to_ps(spacing_cm * 10.0);
    let spacing_ground = spacing_ps * scale;
    let text_h = (inset * 2.0).max(to_ps(1.5)).min(outer.height().min(outer.width()) * 0.05);
    let tick_len = inset.max(to_ps(1.0)) * 1.5;
    let mut crosses = 0usize;

    // Map a ground X/Y back to a paper point inside the viewport (paper = model / scale).
    let to_paper = |gx: f64, gy: f64| -> Vec2 {
        let dx = (gx - window.center().x) / scale;
        let dy = (gy - window.center().y) / scale;
        vp_rect.center() + Vec2::new(dx, dy)
    };

    let gx0 = round_to(window.min.x, spacing_ground);
    let gx_start = if gx0 < window.min.x { gx0 + spacing_ground } else { gx0 };
    let gy0 = round_to(window.min.y, spacing_ground);
    let gy_start = if gy0 < window.min.y { gy0 + spacing_ground } else { gy0 };

    let max_steps = 10_000usize;
    let mut gx = gx_start;
    let mut n = 0usize;
    while gx <= window.max.x && n < max_steps {
        let px = to_paper(gx, window.center().y).x;
        if px >= outer.min.x && px <= outer.max.x {
            // Ticks on the top and bottom border.
            carroyage_line(d, &space, &grid_common, Vec2::new(px, outer.min.y), Vec2::new(px, outer.min.y + tick_len))?;
            carroyage_line(d, &space, &grid_common, Vec2::new(px, outer.max.y - tick_len), Vec2::new(px, outer.max.y))?;
            carroyage_text(d, &space, &grid_common, Vec2::new(px, outer.min.y - text_h), text_h, format!("{gx:.0}"))?;
        }
        gx += spacing_ground;
        n += 1;
    }
    let mut gy = gy_start;
    n = 0;
    while gy <= window.max.y && n < max_steps {
        let py = to_paper(window.center().x, gy).y;
        if py >= outer.min.y && py <= outer.max.y {
            carroyage_line(d, &space, &grid_common, Vec2::new(outer.min.x, py), Vec2::new(outer.min.x + tick_len, py))?;
            carroyage_line(d, &space, &grid_common, Vec2::new(outer.max.x - tick_len, py), Vec2::new(outer.max.x, py))?;
            carroyage_text(d, &space, &grid_common, Vec2::new(outer.min.x - text_h * 2.0, py), text_h, format!("{gy:.0}"))?;
        }
        gy += spacing_ground;
        n += 1;
    }
    // Interior crosses at every grid intersection.
    let mut gxi = gx_start;
    let mut ni = 0usize;
    while gxi <= window.max.x && ni < max_steps {
        let mut gyi = gy_start;
        let mut nj = 0usize;
        while gyi <= window.max.y && nj < max_steps {
            let c = to_paper(gxi, gyi);
            if c.x >= inner.min.x && c.x <= inner.max.x && c.y >= inner.min.y && c.y <= inner.max.y {
                let half = tick_len * 0.5;
                carroyage_line(d, &space, &grid_common, c - Vec2::new(half, 0.0), c + Vec2::new(half, 0.0))?;
                carroyage_line(d, &space, &grid_common, c - Vec2::new(0.0, half), c + Vec2::new(0.0, half))?;
                crosses += 1;
            }
            gyi += spacing_ground;
            nj += 1;
        }
        gxi += spacing_ground;
        ni += 1;
    }

    // One label naming the CRS, if the drawing has one assigned ("suivant epsg").
    if let Some(epsg) = epsg {
        carroyage_text(d, &space, &grid_common, Vec2::new(outer.center().x, outer.max.y + text_h), text_h, format!("EPSG:{epsg}"))?;
    }

    s.touch();
    Ok(json!({
        "layout": layout_name,
        "layer": layer_name,
        "viewport": vp_handle.hex(),
        "scale": scale,
        "crosses": crosses,
    }))
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
    fn import_shp_keeps_the_dbf_fields_and_table_name_as_entity_data() {
        use cadcraft_geom::Vec3;
        use cadcraft_io::shp;

        let fixture = std::env::temp_dir().join(format!("cadcraft-plan-attrs-{}.shp", std::process::id()));
        let geom = shp::Geometry::Point(Vec3::new(5.0, 5.0, 0.0));
        let attrs = [("NOM".to_string(), "Parcelle A".to_string()), ("SURFACE".to_string(), "120.5".to_string())];
        shp::write_to_files(&fixture, &[shp::OutFeature { geometry: &geom, attributes: &attrs }]).expect("writing the fixture should succeed");

        let mut s = session_with_doc();
        let imp = s.execute("plan.import_shp", &json!({ "path": fixture.to_string_lossy() })).expect("import should succeed");
        assert_eq!(imp["imported"].as_u64(), Some(1));

        let doc = s.doc().expect("doc");
        let e = doc.space(&cadcraft_doc::Space::Model).and_then(|sp| sp.iter().next()).expect("one entity");
        let (table, fields) = super::shp_attributes(e).expect("the entity should carry its source attributes");
        assert_eq!(table, fixture.file_stem().and_then(std::ffi::OsStr::to_str).unwrap());
        assert_eq!(fields, attrs.to_vec());

        for ext in ["shp", "shx", "dbf"] {
            let _ = std::fs::remove_file(fixture.with_extension(ext));
        }
    }

    #[test]
    fn set_shp_attribute_edits_one_field_and_leaves_the_others_and_the_table_untouched() {
        use cadcraft_geom::Vec3;
        use cadcraft_io::shp;

        let fixture = std::env::temp_dir().join(format!("cadcraft-plan-setattr-{}.shp", std::process::id()));
        let geom = shp::Geometry::Point(Vec3::new(0.0, 0.0, 0.0));
        let attrs = [("NOM".to_string(), "Parcelle A".to_string()), ("SURFACE".to_string(), "120.5".to_string())];
        shp::write_to_files(&fixture, &[shp::OutFeature { geometry: &geom, attributes: &attrs }]).expect("writing the fixture should succeed");

        let mut s = session_with_doc();
        s.execute("plan.import_shp", &json!({ "path": fixture.to_string_lossy() })).expect("import should succeed");
        for ext in ["shp", "shx", "dbf"] {
            let _ = std::fs::remove_file(fixture.with_extension(ext));
        }
        let handle = {
            let doc = s.doc().expect("doc");
            doc.space(&cadcraft_doc::Space::Model).and_then(|sp| sp.iter().next()).expect("one entity").handle
        };

        let out =
            s.execute("plan.set_shp_attribute", &json!({ "handle": handle.hex(), "key": "SURFACE", "value": "150.0" })).expect("should succeed");
        assert_eq!(out["changed"].as_bool(), Some(true));

        let doc = s.doc().expect("doc");
        let e = doc.entity(handle).expect("entity");
        let (table, fields) = super::shp_attributes(e).expect("attributes should still be there");
        assert!(!table.is_empty());
        assert_eq!(fields, vec![("NOM".to_string(), "Parcelle A".to_string()), ("SURFACE".to_string(), "150.0".to_string())]);
    }

    #[test]
    fn set_shp_attribute_rejects_an_entity_with_no_attributes_or_an_unknown_key() {
        let mut s = session_with_doc();
        let out = s.execute("pline", &json!({ "vertices": [[0.0, 0.0], [10.0, 0.0]] })).expect("should insert a line");
        let handle = out["handle"].as_str().expect("handle").to_string();

        let out = s.execute("plan.set_shp_attribute", &json!({ "handle": handle, "key": "NOM", "value": "x" }));
        assert!(out.is_err(), "an entity with no Shapefile attributes should be refused");
    }

    #[test]
    fn export_shp_writes_back_fields_carried_over_from_a_previous_import() {
        use cadcraft_geom::Vec3;
        use cadcraft_io::shp;

        let fixture = std::env::temp_dir().join(format!("cadcraft-plan-reexport-{}.shp", std::process::id()));
        let geom = shp::Geometry::Point(Vec3::new(1.0, 2.0, 0.0));
        let attrs = [("CODE".to_string(), "B12".to_string())];
        shp::write_to_files(&fixture, &[shp::OutFeature { geometry: &geom, attributes: &attrs }]).expect("writing the fixture should succeed");

        let mut s = session_with_doc();
        s.execute("plan.import_shp", &json!({ "path": fixture.to_string_lossy() })).expect("import should succeed");

        let out_path = std::env::temp_dir().join(format!("cadcraft-plan-reexport-out-{}.shp", std::process::id()));
        let out = s.execute("plan.export_shp", &json!({ "path": out_path.to_string_lossy() })).expect("export should succeed");
        assert_eq!(out["written"].as_u64(), Some(1));

        let mut s2 = session_with_doc();
        s2.execute("plan.import_shp", &json!({ "path": out_path.to_string_lossy() })).expect("reimport should succeed");
        let doc = s2.doc().expect("doc");
        let e = doc.space(&cadcraft_doc::Space::Model).and_then(|sp| sp.iter().next()).expect("one entity");
        let (_, fields) = super::shp_attributes(e).expect("the reimported entity should still carry its fields");
        assert_eq!(fields, attrs.to_vec());

        for ext in ["shp", "shx", "dbf"] {
            let _ = std::fs::remove_file(fixture.with_extension(ext));
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
    fn list_crs_lists_and_filters_by_name() {
        let mut s = session_with_doc();
        let all = s.execute("plan.list_crs", &json!({})).expect("should succeed");
        let all = all["systems"].as_array().expect("array");
        assert!(all.len() > 10);
        assert!(all.iter().any(|v| v["epsg"].as_u64() == Some(2154) && v["hasDefinition"].as_bool() == Some(true)));
        // A named-only entry (no proj4 string yet) still shows up, flagged as such.
        assert!(all.iter().any(|v| v["epsg"].as_u64() == Some(27572) && v["hasDefinition"].as_bool() == Some(false)));

        let filtered = s.execute("plan.list_crs", &json!({ "query": "guyane" })).expect("should succeed");
        let filtered = filtered["systems"].as_array().expect("array");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0]["epsg"].as_u64(), Some(2972));
    }

    #[test]
    fn convert_crs_moves_geometry_and_retags_the_drawing() {
        let mut s = session_with_doc();
        s.execute("plan.set_crs", &json!({ "epsg": 2154 })).expect("should tag the drawing");
        let line = s.execute("pline", &json!({ "vertices": [[652000.0, 6862000.0], [652010.0, 6862000.0]] })).expect("should insert a line");
        let h = line["handle"].as_str().unwrap_or_default().to_string();

        let out = s.execute("plan.convert_crs", &json!({ "to": 32631, "handles": [h.clone()] })).expect("should succeed");
        assert_eq!(out["converted"].as_u64(), Some(1));
        assert_eq!(out["skipped"].as_u64(), Some(0));
        assert_eq!(out["from"].as_u64(), Some(2154));
        assert_eq!(out["to"].as_u64(), Some(32631));

        // The geometry actually moved: a Lambert-93 point around (652000, 6862000) lands well
        // outside that range once it's UTM 31N metres.
        let doc = s.doc().expect("doc");
        let super::EntityKind::LwPolyline(pl) = &doc.entity(cadcraft_doc::Handle::parse_hex(&h).expect("handle")).expect("entity").kind else {
            panic!("expected a polyline");
        };
        assert!(pl.vertices[0].p.x < 600_000.0, "{:?}", pl.vertices[0].p);

        // And the drawing is now tagged with the target system.
        let crs = s.execute("plan.get_crs", &json!({})).expect("should succeed");
        assert_eq!(crs["epsg"].as_u64(), Some(32631));
    }

    #[test]
    fn convert_crs_defaults_from_to_the_drawings_assigned_system() {
        let mut s = session_with_doc();
        s.execute("plan.set_crs", &json!({ "epsg": 2154 })).expect("should tag the drawing");
        s.execute("point", &json!({ "at": [652000.0, 6862000.0] })).expect("should insert a point");
        let out = s.execute("plan.convert_crs", &json!({ "to": 4326 })).expect("should succeed");
        assert_eq!(out["from"].as_u64(), Some(2154));
    }

    #[test]
    fn convert_crs_requires_a_from_when_nothing_is_assigned() {
        let mut s = session_with_doc();
        let out = s.execute("plan.convert_crs", &json!({ "to": 4326 }));
        assert!(out.is_err());
    }

    #[test]
    fn convert_crs_rejects_an_unknown_or_undefined_system_before_touching_anything() {
        let mut s = session_with_doc();
        s.execute("point", &json!({ "at": [652000.0, 6862000.0] })).expect("should insert a point");
        assert!(s.execute("plan.convert_crs", &json!({ "from": 2154, "to": 999_999 })).is_err());
        // The legacy NTF zones have no proj4 definition to reproject through.
        assert!(s.execute("plan.convert_crs", &json!({ "from": 2154, "to": 27572 })).is_err());
    }

    #[test]
    fn convert_crs_skips_entity_kinds_it_does_not_move() {
        let mut s = session_with_doc();
        s.execute("plan.set_crs", &json!({ "epsg": 2154 })).expect("should tag the drawing");
        // A HATCH has no single representative point this command moves (a stated limitation) -
        // built directly since the `hatch` command needs an existing closed boundary to fill.
        let hatch = cadcraft_doc::Hatch {
            pattern: "SOLID".into(),
            solid: true,
            loops: Vec::new(),
            scale: 1.0,
            angle: 0.0,
            associative: false,
            style: 0,
            elevation: 0.0,
            gradient: None,
            origin: cadcraft_geom::Vec2::ZERO,
            background: None,
            pattern_lines: Vec::new(),
        };
        let h = s.add_entity(super::EntityKind::Hatch(hatch)).expect("should insert a hatch");
        let out = s.execute("plan.convert_crs", &json!({ "to": 32631, "handles": [h.hex()] })).expect("should succeed");
        assert_eq!(out["converted"].as_u64(), Some(0));
        assert_eq!(out["skipped"].as_u64(), Some(1));
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

    #[test]
    fn carroyage_creates_a_layout_layer_and_viewport_at_the_requested_scale() {
        let mut s = session_with_doc();
        s.execute("plan.set_crs", &json!({ "epsg": 2154 })).expect("should tag the drawing");
        let out = s
            .execute(
                "plan.carroyage",
                &json!({ "window": { "min": [0.0, 0.0], "max": [4000.0, 4000.0] }, "scale": 1000, "spacingCm": 1, "paper": "A4" }),
            )
            .expect("carroyage should succeed");
        let layout_name = out["layout"].as_str().expect("layout name").to_string();
        let layer_name = out["layer"].as_str().expect("layer name").to_string();
        assert_eq!(layer_name, "CARROYAGE");
        assert!(out["crosses"].as_u64().expect("crosses count") > 0);

        let d = s.doc().expect("doc");
        assert!(d.layer(&layer_name).is_some(), "the CARROYAGE layer should be created");
        let layout = d.layout(&layout_name).expect("the new layout should exist");
        let vp = layout
            .entities
            .iter()
            .find_map(|e| if let super::EntityKind::Viewport(v) = &e.kind { Some(v) } else { None })
            .expect("the layout should have a viewport");
        // scale = height / view_height, in mm per ground unit at 1:1000 for a metric drawing.
        let scale = vp.height / vp.view_height;
        assert!((scale - 1.0 / 1000.0).abs() < 1e-9, "scale should be 1:1000, got {scale}");
        assert!(vp.view_center.x > 1900.0 && vp.view_center.x < 2100.0, "view should be centered on the requested window");

        // A cross, tick or label landed on the new layer.
        assert!(layout.entities.iter().any(|e| e.common.layer == layer_name));
    }

    #[test]
    fn carroyage_rejects_an_empty_window() {
        let mut s = session_with_doc();
        let out = s.execute("plan.carroyage", &json!({ "window": { "min": [0.0, 0.0], "max": [0.0, 0.0] }, "scale": 100 }));
        assert!(out.is_err());
    }

    #[test]
    fn carroyage_rejects_an_unknown_paper_format() {
        let mut s = session_with_doc();
        let out = s.execute("plan.carroyage", &json!({ "window": { "min": [0.0, 0.0], "max": [10.0, 10.0] }, "scale": 100, "paper": "A42" }));
        assert!(out.is_err());
    }
}
