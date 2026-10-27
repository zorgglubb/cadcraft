//! Terrain commands built on `cadcraft-terrain`: triangulating a point cloud into a TIN,
//! querying its elevation at a point, and extracting contour-line segments - each as a stateless
//! calculation over the points given in the call (no persistent terrain object yet), consistent
//! with "programmatic calls never open dialogs". `terrain.create_mnt`, `terrain.create_contours`
//! and `terrain.dimension_contours` are the exceptions that read the drawing itself: the first
//! two gather their points for `triangulate`/`contours` from the active space instead of making
//! the caller assemble `points` by hand, the way a MNT/TIN creation wizard needs to (object
//! selection by block/layer or a manual pick, altitude filtering); the third reads back whatever
//! a prior `terrain.create_contours` (or anything else) left on the chosen layers and labels it,
//! since nothing here keeps the TIN itself around between calls.
//!
//! Contour segments come out of `cadcraft_terrain::contours` per-triangle, and
//! `terrain.create_contours` chains every level's raw segments into walkable runs
//! (`chain_segments`) before drawing - each run becomes one `Polyline3d` (a real 3D POLYLINE,
//! every vertex's own Z the contour's elevation), not a `LINE` per raw segment, so a contour
//! that's one connected line in the terrain is one drawing object, closed when its segments
//! happen to form a loop.

use std::collections::HashSet;

use cadcraft_doc::{Common, EntityKind, HAlign, Handle, Line, Polyline3d, Text, VAlign};
use cadcraft_geom::{Line as GLine, Vec2, Vec3, line_line};
use cadcraft_terrain::{TerrainError, contours, triangulate};
use serde_json::{Value, json};

use super::*;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

/// The MNT's own triangulation edges land here by default - a layer CADCraft creates and colours
/// green the first time it's needed, not copied from any reference application's convention.
const DEFAULT_MNT_LAYER: &str = "MNT";
const DEFAULT_CONTOUR_LABEL_LAYER: &str = "COTES_COURBES";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        // No `.menu()`: these three are the low-level, stateless building blocks (the caller
        // assembles `points` by hand) that `terrain.create_mnt`/`terrain.create_contours` wrap
        // for menu use with a real point-gathering wizard; kept for scripts/MCP, just not
        // duplicated in the Topo 3D menu alongside the commands people actually want there.
        CommandSpec::new("terrain.triangulate", "Triangulate (TIN)", run_triangulate).params("{points: [[x,y,z], ...], draw?: bool}"),
        CommandSpec::new("terrain.elevation_at", "Elevation At Point", run_elevation_at).params("{points: [[x,y,z], ...], x: number, y: number}"),
        CommandSpec::new("terrain.contours", "Contour Lines", run_contours)
            .alias(&["courbes"])
            .params("{points: [[x,y,z], ...], interval: number, draw?: bool}"),
        CommandSpec::new("terrain.create_mnt", "Create MNT...", run_create_mnt).menu(&["Topo 3D", "Create MNT..."]).params(
            "{selection: \"auto\"|\"manual\" (default \"auto\"), blockNames?: [string], layers?: [string] \
             (both narrow \"auto\"; empty/omitted means every point block and layer), \
             handles?: [string] (required for \"manual\"), ignoreNullAltitude?: bool (default true), \
             layer?: string (default \"MNT\"; created and coloured green the first time it's used), \
             draw?: bool (default true)} → {pointCount, skippedNullAltitude, triangleCount, edgeCount, \
             elevationRange, layer}. Points come from every topo point block (any INSERT with an ALT \
             attribute, at its insertion XY/ALT) and bare POINT entities (at their own XYZ) in the \
             active space.",
        ),
        CommandSpec::new("terrain.create_contours", "Create Contours...", run_create_contours).menu(&["Topo 3D", "Create Contours..."]).params(
            "{selection, blockNames?, layers?, handles?, ignoreNullAltitude? (same as terrain.create_mnt), \
             orders: [{step: number>0, layer: string, color?: 1..255 (ACI), colorRgb?: [r,g,b] \
             (true colour, takes priority over `color`)}] (at least one; each order draws every \
             contour level that's a multiple of its own `step` into its own `layer`, independently of \
             the other orders), clearLayers?: bool (default true: erase what's already on an order's \
             layer before drawing), smooth?: bool (default true: one light corner-cutting pass over \
             each contour once its raw per-triangle segments are chained together), draw?: bool \
             (default true)} → {pointCount, elevationRange, orders: [{step, layer, levelCount, \
             polylineCount, vertexCount}]}. Each contour is drawn as one 3D POLYLINE \
             (`Polyline3d`) per connected run of triangle edges, every vertex's own Z the \
             contour's elevation - not one LINE per raw segment.",
        ),
        CommandSpec::new("terrain.dimension_contours", "Dimension Contours...", run_dimension_contours)
            .menu(&["Topo 3D", "Dimension Contours..."])
            .params(
                "{layers: [string] (contour layers to label; required), mode: \"regular\"|\"axis\" (default \
                 \"regular\"), interval?: number (world units between labels along a contour's length; \
                 default 10, \"regular\" mode), axis?: {a: [x,y], b: [x,y]} | {points: [[x,y], ...]} \
                 (required for \"axis\" mode: one label wherever any segment of this axis - a single \
                 a/b segment, or every segment of a longer polyline from `points` - crosses a contour), \
                 decimals?: integer (default 2), height?: number (default TEXTSIZE), suffix?: string \
                 (appended after the number, e.g. \" m\"; default \"\"), layer?: string (default \
                 \"COTES_COURBES\"; created if missing), draw?: bool (default true)} → {labelCount}. \
                 Every label's rotation is normalised to read left-to-right (never upside down), so \
                 consecutive labels along the same contour face the same way regardless of which \
                 direction its raw segments happen to run.",
            ),
        CommandSpec::new("terrain.draw_axis", "Draw Axis", run_draw_axis).noundo().interactive(|_| Ok(Box::new(DrawAxisM::default()))).params(
            "{} (typed/menu only: TERRAIN.DRAW_AXIS prompts for a sequence of axis points, finishing \
                 on Enter or a double-click) → sets Session::axis_pick to the clicked polyline, for the \
                 Dimension Contours dialog's \"Draw axis\" button to read back.",
        ),
    ]
}

/// One point usable for a MNT: its handle (for a manual selection to filter by), block name
/// (empty for a bare POINT, which `blockNames`/`layers` never narrow out), layer and XYZ.
struct MntCandidate {
    handle: Handle,
    block: String,
    layer: String,
    p: Vec3,
}

fn alt_attrib(ins: &cadcraft_doc::Insert) -> Option<f64> {
    ins.attribs.iter().find(|a| a.tag.eq_ignore_ascii_case("ALT")).and_then(|a| a.text.value.trim().parse().ok())
}

/// Every point in the active space a MNT could use: a topo point block (any INSERT carrying an
/// `ALT` attribute, regardless of which block definition it uses) or a bare POINT entity.
fn all_mnt_candidates(s: &Session) -> Result<Vec<MntCandidate>> {
    let st = s.state()?;
    let Some(store) = st.doc.space(&st.space) else { return Ok(Vec::new()) };
    let mut out = Vec::new();
    for e in store.iter() {
        match &e.kind {
            EntityKind::Insert(ins) => {
                if let Some(alt) = alt_attrib(ins) {
                    out.push(MntCandidate {
                        handle: e.handle,
                        block: ins.block.clone(),
                        layer: e.common.layer.clone(),
                        p: Vec3::new(ins.insert.x, ins.insert.y, alt),
                    });
                }
            }
            EntityKind::Point(pt) => {
                out.push(MntCandidate { handle: e.handle, block: String::new(), layer: e.common.layer.clone(), p: pt.p });
            }
            _ => {}
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

/// Ensures `layer` exists (creating it coloured `default_color` if it doesn't - an existing
/// layer's colour is left alone) and adds `kind` to the active space on it, everything else
/// ByLayer so it follows that layer's own look.
fn add_on_layer(s: &mut Session, layer: &str, default_color: cadcraft_color::Color, kind: EntityKind) -> Result<Handle> {
    let space = s.space();
    let d = s.doc_mut()?;
    if d.ensure_layer(layer)
        && let Some(l) = d.layer_mut(layer)
    {
        l.color = default_color;
    }
    let common = Common { layer: layer.to_string(), ..Common::default() };
    Ok(d.add(&space, common, kind)?)
}

/// Every entity on `layer` in the active space, erased - `terrain.create_contours`' `clearLayers`
/// re-running cleanly instead of piling up a new set of lines over the old one every time.
fn clear_layer(s: &mut Session, layer: &str) -> Result<()> {
    let space = s.space();
    let handles: Vec<Handle> = s
        .doc()?
        .space(&space)
        .map(|store| store.iter().filter(|e| e.common.layer.eq_ignore_ascii_case(layer)).map(|e| e.handle).collect())
        .unwrap_or_default();
    let d = s.doc_mut()?;
    for h in handles {
        d.remove_entity(h);
    }
    Ok(())
}

/// The points `terrain.create_mnt`/`terrain.create_contours` both triangulate from, gathered and
/// filtered the same way (`selection`/`blockNames`/`layers`/`handles`/`ignoreNullAltitude`).
/// Returns the usable points and how many were skipped for having no real altitude.
fn select_points(s: &Session, cmd: &str, p: &Value) -> Result<(Vec<Vec3>, usize)> {
    let selection = str_param(p, "selection").unwrap_or("auto");
    let block_names = names_param(p, "blockNames");
    let layers = names_param(p, "layers");
    let ignore_null = bool_or(p, "ignoreNullAltitude", true);

    let all = all_mnt_candidates(s)?;
    let mut chosen: Vec<Vec3> = match selection {
        "manual" => {
            let hs = p.get("handles").and_then(Value::as_array).ok_or_else(|| bad(cmd, "`handles` is required for `selection: \"manual\"`"))?;
            let wanted: HashSet<Handle> = hs.iter().filter_map(Value::as_str).filter_map(Handle::parse_hex).collect();
            all.into_iter().filter(|c| wanted.contains(&c.handle)).map(|c| c.p).collect()
        }
        "auto" => all.into_iter().filter(|c| matches_names(&c.block, &block_names) && matches_names(&c.layer, &layers)).map(|c| c.p).collect(),
        other => return Err(bad(cmd, format!("unknown `selection` \"{other}\" (expected \"auto\" or \"manual\")"))),
    };

    let before = chosen.len();
    let skipped = if ignore_null {
        chosen.retain(|p| p.z != 0.0);
        before - chosen.len()
    } else {
        0
    };
    Ok((chosen, skipped))
}

fn too_few_points_err(cmd: &str, usable: usize, skipped: usize) -> crate::EngineError {
    bad(
        cmd,
        format!(
            "not enough points with an altitude to triangulate: {usable} found (need at least 3); \
             {skipped} point(s) were skipped for having no altitude"
        ),
    )
}

fn run_create_mnt(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "terrain.create_mnt";
    let (chosen, skipped) = select_points(s, CMD, p)?;
    if chosen.len() < 3 {
        return Err(too_few_points_err(CMD, chosen.len(), skipped));
    }
    let draw = bool_or(p, "draw", true);
    let layer = str_param(p, "layer").filter(|l| !l.trim().is_empty()).unwrap_or(DEFAULT_MNT_LAYER).to_string();

    let tin = triangulate(&chosen).map_err(|e| terrain_err(CMD, e))?;
    let mut edge_count = 0usize;
    if draw {
        let mut drawn: std::collections::HashSet<(usize, usize)> = std::collections::HashSet::new();
        for tri in &tin.triangles {
            for (i, j) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                let key = if i < j { (i, j) } else { (j, i) };
                if !drawn.insert(key) {
                    continue;
                }
                let (Some(a), Some(b)) = (tin.points.get(i), tin.points.get(j)) else { continue };
                add_on_layer(s, &layer, cadcraft_color::Color::Index(3), EntityKind::Line(Line { a: *a, b: *b }))?;
                edge_count += 1;
            }
        }
    }

    Ok(json!({
        "pointCount": tin.points.len(),
        "skippedNullAltitude": skipped,
        "triangleCount": tin.triangles.len(),
        "edgeCount": edge_count,
        "elevationRange": tin.elevation_range(),
        "layer": layer,
    }))
}

fn run_create_contours(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "terrain.create_contours";
    let (chosen, skipped) = select_points(s, CMD, p)?;
    if chosen.len() < 3 {
        return Err(too_few_points_err(CMD, chosen.len(), skipped));
    }
    let orders = p.get("orders").and_then(Value::as_array).cloned().unwrap_or_default();
    if orders.is_empty() {
        return Err(bad(CMD, "`orders` (array of {step, layer}) must have at least one entry"));
    }
    let clear = bool_or(p, "clearLayers", true);
    let draw = bool_or(p, "draw", true);
    // A light, optional smoothing pass (one iteration of Chaikin corner-cutting) over each
    // contour once its raw per-triangle segments are chained into a walkable polyline - visually
    // softer without needing a true spline entity. Each chained (and now possibly smoothed)
    // contour becomes a single 3D POLYLINE (`Polyline3d`, every vertex's own Z the contour's
    // elevation - the same thing AutoCAD's own elevation/Z would show), not one `LINE` per
    // sub-segment: a contour that's one connected run of triangle edges is one drawing object,
    // the way a person tracing it by hand would expect, and only breaks into several polylines
    // where the raw segments genuinely don't connect (an island, a second hilltop at the same
    // level, …).
    let smooth = bool_or(p, "smooth", true);

    let tin = triangulate(&chosen).map_err(|e| terrain_err(CMD, e))?;

    let mut out_orders = Vec::with_capacity(orders.len());
    for (i, o) in orders.iter().enumerate() {
        let step = o
            .get("step")
            .and_then(Value::as_f64)
            .filter(|v| v.is_finite() && *v > 0.0)
            .ok_or_else(|| bad(CMD, format!("orders[{i}].step must be a positive finite number")))?;
        let layer = o
            .get("layer")
            .and_then(Value::as_str)
            .filter(|l| !l.trim().is_empty())
            .ok_or_else(|| bad(CMD, format!("orders[{i}].layer is required")))?;
        let color = order_color(o);

        if draw && clear {
            clear_layer(s, layer)?;
        }

        let segments = contours(&tin, step);
        let mut levels: Vec<f64> = segments.iter().map(|sg| sg.elevation).collect();
        levels.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        levels.dedup();

        let (mut polyline_count, mut vertex_count) = (0usize, 0usize);
        if draw {
            for (elevation, segs) in group_segments_by_level(&segments) {
                for chain in chain_segments(segs) {
                    let pts = if smooth { smooth_chain(&chain) } else { chain };
                    // `chain_segments` can hand back a chain that closes on itself (every raw
                    // segment matched into a loop, the common case for an interior contour with
                    // no hull-edge end) - the closing edge is implicit in a closed polyline, so
                    // the repeated last point that would otherwise duplicate the first is dropped.
                    let closed = pts.len() > 2 && (pts[0] - pts[pts.len() - 1]).len() < 1e-6;
                    let points: Vec<Vec3> = pts[..pts.len() - usize::from(closed)].iter().map(|v| v.to3(elevation)).collect();
                    if points.len() < 2 {
                        continue;
                    }
                    vertex_count += points.len();
                    add_on_layer(s, layer, color.unwrap_or(cadcraft_color::Color::ByLayer), EntityKind::Polyline3d(Polyline3d { points, closed }))?;
                    polyline_count += 1;
                }
            }
        }
        out_orders
            .push(json!({ "step": step, "layer": layer, "levelCount": levels.len(), "polylineCount": polyline_count, "vertexCount": vertex_count }));
    }

    Ok(json!({ "pointCount": tin.points.len(), "elevationRange": tin.elevation_range(), "orders": out_orders }))
}

/// An order's colour: `colorRgb: [r, g, b]` (a true colour, what the dialog's colour picker
/// sends) takes priority over `color` (a plain ACI index 1..=255, for script/MCP callers);
/// neither given leaves it ByLayer.
fn order_color(o: &Value) -> Option<cadcraft_color::Color> {
    if let Some(rgb) = o.get("colorRgb").and_then(Value::as_array)
        && let [Some(r), Some(g), Some(b)] =
            [rgb.first(), rgb.get(1), rgb.get(2)].map(|v| v.and_then(Value::as_u64).and_then(|n| u8::try_from(n).ok()))
    {
        return Some(cadcraft_color::Color::True(cadcraft_color::Rgb(r, g, b)));
    }
    o.get("color").and_then(Value::as_u64).and_then(|c| u8::try_from(c).ok()).filter(|c| *c >= 1).map(cadcraft_color::Color::Index)
}

/// Groups raw contour segments by elevation level, each level's segments ready for
/// [`chain_segments`] - shared by `terrain.create_contours` (chaining before it draws, to smooth)
/// and conceptually mirrored by `terrain.dimension_contours`' own grouping of already-drawn lines.
fn group_segments_by_level(segments: &[cadcraft_terrain::ContourSegment]) -> Vec<(f64, Vec<(Vec2, Vec2)>)> {
    let mut by_level: Vec<(f64, Vec<(Vec2, Vec2)>)> = Vec::new();
    for seg in segments {
        match by_level.iter_mut().find(|(lvl, _)| (*lvl - seg.elevation).abs() < 1e-6) {
            Some(entry) => entry.1.push((seg.start, seg.end)),
            None => by_level.push((seg.elevation, vec![(seg.start, seg.end)])),
        }
    }
    by_level
}

/// One pass of Chaikin corner-cutting: a light, non-interpolating smoothing that pulls each
/// interior vertex toward the midpoints of its neighbouring segments while leaving the chain's
/// two endpoints exactly where they were (so neighbouring orders/levels still meet where they
/// should). A chain too short to have an interior corner is returned unchanged.
fn smooth_chain(chain: &[Vec2]) -> Vec<Vec2> {
    if chain.len() < 3 {
        return chain.to_vec();
    }
    let mut out = Vec::with_capacity(chain.len() * 2);
    out.push(chain[0]);
    for w in chain.windows(2) {
        let (a, b) = (w[0], w[1]);
        out.push(a + (b - a) * 0.25);
        out.push(a + (b - a) * 0.75);
    }
    out.push(chain[chain.len() - 1]);
    out
}

/// Every contour-shaped entity on any of `layers` in the active space, broken back down into
/// flat 2D segments plus each one's elevation - `terrain.dimension_contours`' only way to find
/// contours, since nothing here keeps a TIN or a contour set around between calls. Reads both
/// what `terrain.create_contours` itself draws now (a `Polyline3d` per contour, every vertex's
/// own Z the elevation - its consecutive vertex pairs, plus the closing edge when `closed`) and
/// a plain `LINE` with a level Z at both ends (anything else that might have left contours that
/// way, or an older drawing from before contours became polylines).
fn contour_lines_on_layers(s: &Session, layers: &[String]) -> Vec<(Vec2, Vec2, f64)> {
    let Ok(st) = s.state() else { return Vec::new() };
    let Some(store) = st.doc.space(&st.space) else { return Vec::new() };
    let mut out = Vec::new();
    for e in store.iter() {
        if !layers.iter().any(|l| l.eq_ignore_ascii_case(&e.common.layer)) {
            continue;
        }
        match &e.kind {
            EntityKind::Line(l) if (l.a.z - l.b.z).abs() < 1e-6 => out.push((l.a.xy(), l.b.xy(), l.a.z)),
            EntityKind::Polyline3d(pl) => {
                for w in pl.points.windows(2) {
                    if (w[0].z - w[1].z).abs() < 1e-6 {
                        out.push((w[0].xy(), w[1].xy(), w[0].z));
                    }
                }
                if pl.closed
                    && let (Some(first), Some(last)) = (pl.points.first(), pl.points.last())
                    && (first.z - last.z).abs() < 1e-6
                {
                    out.push((last.xy(), first.xy(), last.z));
                }
            }
            _ => {}
        }
    }
    out
}

/// Chains raw, per-triangle contour segments sharing an elevation into walkable polylines by
/// matching endpoints within `EPS` - greedy and O(n^2), fine for the segment counts one MNT
/// produces. A segment nothing else connects to becomes its own 2-point chain.
fn chain_segments(mut remaining: Vec<(Vec2, Vec2)>) -> Vec<Vec<Vec2>> {
    const EPS: f64 = 1e-6;
    let mut chains: Vec<Vec<Vec2>> = Vec::new();
    while let Some((a, b)) = remaining.pop() {
        let mut chain = vec![a, b];
        while let Some(&tail) = chain.last() {
            if let Some(pos) = remaining.iter().position(|(p, q)| (*p - tail).len() < EPS || (*q - tail).len() < EPS) {
                let (p, q) = remaining.remove(pos);
                chain.push(if (p - tail).len() < EPS { q } else { p });
                continue;
            }
            let head = chain[0];
            if let Some(pos) = remaining.iter().position(|(p, q)| (*p - head).len() < EPS || (*q - head).len() < EPS) {
                let (p, q) = remaining.remove(pos);
                chain.insert(0, if (p - head).len() < EPS { q } else { p });
                continue;
            }
            break;
        }
        chains.push(chain);
    }
    chains
}

/// A label's rotation, kept within (-90°, 90°] of +X so text always reads left-to-right rather
/// than upside down - the same raw segment direction and its reverse (`d` and `-d`) land on the
/// same rotation, so consecutive labels along a contour face the same way regardless of which
/// way `chain_segments`/`contours()` happened to order that particular segment or raw crossing.
fn upright_rotation(d: Vec2) -> f64 {
    let a = d.y.atan2(d.x);
    if a > std::f64::consts::FRAC_PI_2 || a <= -std::f64::consts::FRAC_PI_2 {
        // Flip by PI, then wrap back into (-PI, PI] - `atan2` never returns past that range, but
        // `a + PI` can.
        let flipped = a + std::f64::consts::PI;
        if flipped > std::f64::consts::PI { flipped - 2.0 * std::f64::consts::PI } else { flipped }
    } else {
        a
    }
}

/// Positions (and local direction, for the label's rotation) spaced `interval` apart along a
/// chained contour, starting half an interval in so a short, isolated chain still gets one label
/// near its middle rather than none.
fn labels_along_chain(chain: &[Vec2], interval: f64) -> Vec<(Vec2, f64)> {
    let mut out = Vec::new();
    if chain.len() < 2 || !interval.is_finite() || interval <= 0.0 {
        return out;
    }
    let mut next_at = interval / 2.0;
    let mut acc = 0.0;
    for w in chain.windows(2) {
        let d = w[1] - w[0];
        let seg_len = d.len();
        if seg_len <= 0.0 {
            continue;
        }
        while next_at <= acc + seg_len {
            let t = (next_at - acc) / seg_len;
            out.push((w[0] + d * t, upright_rotation(d)));
            next_at += interval;
        }
        acc += seg_len;
    }
    out
}

/// The axis to cross contours against for `mode: "axis"`: either the original 2-point `axis.a`/
/// `axis.b` form, or a longer hand-drawn polyline from `axis.points` (what TERRAIN.DRAW_AXIS
/// leaves in `Session::axis_pick`) - every consecutive pair of points is one segment tested
/// against every contour segment, so a multi-point axis crosses as many contours as it actually
/// passes through, not just the two points a straight line would allow.
fn axis_points(cmd: &str, p: &Value) -> Result<Vec<Vec2>> {
    if let Some(arr) = p.get("axis").and_then(|o| o.get("points")).and_then(Value::as_array) {
        let pts: Vec<Vec2> = arr
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let a = v.as_array().ok_or_else(|| bad(cmd, format!("axis.points[{i}] must be an array [x, y]")))?;
                let get = |k: usize| a.get(k).and_then(Value::as_f64).filter(|n| n.is_finite());
                match (get(0), get(1)) {
                    (Some(x), Some(y)) => Ok(Vec2::new(x, y)),
                    _ => Err(bad(cmd, format!("axis.points[{i}] must be [x, y] of finite numbers"))),
                }
            })
            .collect::<Result<_>>()?;
        if pts.len() < 2 {
            return Err(bad(cmd, "`axis.points` must have at least 2 points"));
        }
        return Ok(pts);
    }
    let point = |which: &str| -> Result<Vec2> {
        let v = p
            .get("axis")
            .and_then(|o| o.get(which))
            .ok_or_else(|| bad(cmd, format!("`axis.{which}` ([x, y]) or `axis.points` ([[x, y], ...]) is required for `mode: \"axis\"`")))?;
        let a = v.as_array().ok_or_else(|| bad(cmd, format!("`axis.{which}` must be an array [x, y]")))?;
        let get = |k: usize| a.get(k).and_then(Value::as_f64).filter(|n| n.is_finite());
        match (get(0), get(1)) {
            (Some(x), Some(y)) => Ok(Vec2::new(x, y)),
            _ => Err(bad(cmd, format!("`axis.{which}` must be [x, y] of finite numbers"))),
        }
    };
    Ok(vec![point("a")?, point("b")?])
}

/// TERRAIN.DRAW_AXIS: lets the Dimension Contours dialog's "Draw axis" button collect the axis
/// on screen instead of typing its coordinates - a plain open polyline pick (at least 2 points),
/// finishing on Enter (or a double-click, which the UI also sends as Enter while this command
/// runs; see `canvas.rs`). The result never touches the drawing - it's left in
/// [`Session::axis_pick`] for the dialog to read back and hand to `terrain.dimension_contours` as
/// `axis.points`.
#[derive(Default)]
struct DrawAxisM {
    pts: Vec<Vec2>,
}

impl Interactive for DrawAxisM {
    fn name(&self) -> &'static str {
        "TERRAINDRAWAXIS"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        let base = self.pts.last().copied();
        let msg = if self.pts.is_empty() { "Specify first axis point" } else { "Specify next axis point (Enter to finish)" };
        let pr = Prompt::new(msg, Accept::POINT).base_opt(base);
        if self.pts.is_empty() { pr } else { pr.kw(&["Undo"]) }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match i {
            Input::Point(pt) => {
                self.pts.push(pt);
                Ok(Step::Continue)
            }
            Input::Keyword(k) if k.eq_ignore_ascii_case("undo") => {
                self.pts.pop();
                Ok(Step::Continue)
            }
            Input::Enter => {
                if self.pts.len() >= 2 {
                    s.axis_pick = Some(std::mem::take(&mut self.pts));
                    Ok(Step::Done)
                } else {
                    Ok(Step::Cancel)
                }
            }
            Input::Cancel => Ok(Step::Cancel),
            _ => Ok(Step::Continue),
        }
    }
    fn preview(&self, _s: &Session, cursor: Vec2) -> Vec<EntityKind> {
        let mut pts = self.pts.clone();
        pts.push(cursor);
        pts.windows(2).map(|w| EntityKind::Line(Line { a: w[0].to3(0.0), b: w[1].to3(0.0) })).collect()
    }
}

/// TERRAIN.DRAW_AXIS's non-interactive form: nothing to do headlessly, same convention as
/// DATA.PICK_ENTITIES.
fn run_draw_axis(_s: &mut Session, _p: &Value) -> Result<Value> {
    Err(bad("terrain.draw_axis", "draw the axis on screen (typed/menu only)"))
}

fn run_dimension_contours(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "terrain.dimension_contours";
    let layers = names_param(p, "layers");
    if layers.is_empty() {
        return Err(bad(CMD, "`layers` (array of contour layer names to label) must have at least one entry"));
    }
    let mode = str_param(p, "mode").unwrap_or("regular");
    let decimals = p.get("decimals").and_then(Value::as_u64).unwrap_or(2).min(10) as usize;
    let height = f64_or(p, "height", s.doc()?.header.f64("TEXTSIZE", 0.2));
    let suffix = str_param(p, "suffix").unwrap_or("").to_string();
    let target_layer = str_param(p, "layer").filter(|l| !l.trim().is_empty()).unwrap_or(DEFAULT_CONTOUR_LABEL_LAYER).to_string();
    let draw = bool_or(p, "draw", true);

    let raw = contour_lines_on_layers(s, &layers);
    let mut by_level: Vec<(f64, Vec<(Vec2, Vec2)>)> = Vec::new();
    for (a, b, z) in raw {
        match by_level.iter_mut().find(|(lvl, _)| (*lvl - z).abs() < 1e-6) {
            Some(entry) => entry.1.push((a, b)),
            None => by_level.push((z, vec![(a, b)])),
        }
    }

    let mut placements: Vec<(Vec2, f64, f64)> = Vec::new();
    match mode {
        "regular" => {
            let interval = f64_or(p, "interval", 10.0);
            if !interval.is_finite() || interval <= 0.0 {
                return Err(bad(CMD, "`interval` must be a positive finite number"));
            }
            for (z, segs) in &by_level {
                for chain in chain_segments(segs.clone()) {
                    for (pos, rot) in labels_along_chain(&chain, interval) {
                        placements.push((pos, rot, *z));
                    }
                }
            }
        }
        "axis" => {
            let axis_pts = axis_points(CMD, p)?;
            for axis_seg in axis_pts.windows(2) {
                let axis = GLine::new(axis_seg[0], axis_seg[1]);
                for (z, segs) in &by_level {
                    for (p0, p1) in segs {
                        let seg = GLine::new(*p0, *p1);
                        if let Some(pt) = line_line(&axis, &seg) {
                            let d = *p1 - *p0;
                            placements.push((pt, upright_rotation(d), *z));
                        }
                    }
                }
            }
        }
        other => return Err(bad(CMD, format!("unknown `mode` \"{other}\" (expected \"regular\" or \"axis\")"))),
    }

    let mut drawn = 0usize;
    if draw {
        for (pos, rot, z) in &placements {
            let value = format!("{:.*}{suffix}", decimals, z);
            add_on_layer(
                s,
                &target_layer,
                cadcraft_color::Color::ByLayer,
                EntityKind::Text(Text {
                    insert: Vec3::new(pos.x, pos.y, 0.0),
                    align_pt: None,
                    height,
                    value,
                    rotation: *rot,
                    width_factor: 1.0,
                    oblique: 0.0,
                    style: "Standard".into(),
                    halign: HAlign::Left,
                    valign: VAlign::Baseline,
                }),
            )?;
            drawn += 1;
        }
    }

    Ok(json!({ "labelCount": placements.len(), "drawn": drawn }))
}

fn points_param(cmd: &str, p: &Value) -> Result<Vec<Vec3>> {
    let arr = p.get("points").and_then(Value::as_array).ok_or_else(|| bad(cmd, "`points` (array of [x, y, z]) is required"))?;
    arr.iter()
        .enumerate()
        .map(|(i, v)| {
            let a = v.as_array().ok_or_else(|| bad(cmd, format!("points[{i}] must be an array [x, y, z]")))?;
            let get = |k: usize| a.get(k).and_then(Value::as_f64).filter(|n| n.is_finite());
            match (get(0), get(1)) {
                (Some(x), Some(y)) => Ok(Vec3::new(x, y, get(2).unwrap_or(0.0))),
                _ => Err(bad(cmd, format!("points[{i}] must be [x, y] or [x, y, z] of finite numbers"))),
            }
        })
        .collect()
}

fn terrain_err(cmd: &str, e: TerrainError) -> crate::EngineError {
    bad(cmd, e.to_string())
}

fn run_triangulate(s: &mut Session, p: &Value) -> Result<Value> {
    let pts = points_param("terrain.triangulate", p)?;
    let tin = triangulate(&pts).map_err(|e| terrain_err("terrain.triangulate", e))?;
    let draw = bool_or(p, "draw", true);

    let mut edge_count = 0usize;
    if draw {
        // Each undirected edge drawn once, even though it borders two triangles.
        let mut drawn: std::collections::HashSet<(usize, usize)> = std::collections::HashSet::new();
        for tri in &tin.triangles {
            for (i, j) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                let key = if i < j { (i, j) } else { (j, i) };
                if !drawn.insert(key) {
                    continue;
                }
                let (Some(a), Some(b)) = (tin.points.get(i), tin.points.get(j)) else { continue };
                s.add_entity(EntityKind::Line(Line { a: *a, b: *b }))?;
                edge_count += 1;
            }
        }
    }

    Ok(json!({
        "pointCount": tin.points.len(),
        "triangleCount": tin.triangles.len(),
        "edgeCount": edge_count,
        "elevationRange": tin.elevation_range(),
    }))
}

fn run_elevation_at(_s: &mut Session, p: &Value) -> Result<Value> {
    let pts = points_param("terrain.elevation_at", p)?;
    let x = f64_req("terrain.elevation_at", p, "x")?;
    let y = f64_req("terrain.elevation_at", p, "y")?;
    let tin = triangulate(&pts).map_err(|e| terrain_err("terrain.elevation_at", e))?;
    Ok(json!({ "elevation": tin.elevation_at(x, y) }))
}

/// Extracts and optionally draws contour-line segments. **Limitation, stated plainly**: each
/// segment is emitted per-triangle and is not yet chained into continuous polylines across
/// triangles - that stitching is a later increment.
fn run_contours(s: &mut Session, p: &Value) -> Result<Value> {
    let pts = points_param("terrain.contours", p)?;
    let interval = f64_req("terrain.contours", p, "interval")?;
    if !interval.is_finite() || interval <= 0.0 {
        return Err(bad("terrain.contours", "`interval` must be a positive finite number"));
    }
    let tin = triangulate(&pts).map_err(|e| terrain_err("terrain.contours", e))?;
    let segments = contours(&tin, interval);
    let draw = bool_or(p, "draw", true);

    if draw {
        for seg in &segments {
            s.add_entity(EntityKind::Line(Line { a: seg.start.to3(seg.elevation), b: seg.end.to3(seg.elevation) }))?;
        }
    }

    let mut levels: Vec<f64> = segments.iter().map(|sg| sg.elevation).collect();
    levels.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    levels.dedup();

    Ok(json!({ "segmentCount": segments.len(), "levels": levels }))
}

#[cfg(test)]
mod tests {
    use cadcraft_doc::{EntityKind, Line};
    use cadcraft_geom::{Vec2, Vec3};
    use serde_json::{Value, json};

    use super::{labels_along_chain, upright_rotation};
    use crate::{Input, Session};

    fn session_with_doc() -> Session {
        let mut s = Session::new();
        s.execute("new", &json!({})).expect("new drawing should succeed");
        s
    }

    fn square_pts() -> Value {
        json!([[0.0, 0.0, 0.0], [10.0, 0.0, 10.0], [10.0, 10.0, 10.0], [0.0, 10.0, 0.0]])
    }

    #[test]
    fn triangulate_draws_edges_and_reports_counts() {
        let mut s = session_with_doc();
        let out = s.execute("terrain.triangulate", &json!({ "points": square_pts() })).expect("triangulate should succeed");
        assert_eq!(out["triangleCount"].as_u64(), Some(2));
        assert_eq!(out["pointCount"].as_u64(), Some(4));
        // A convex quad split into 2 triangles has 5 undirected edges (4 sides + 1 diagonal).
        assert_eq!(out["edgeCount"].as_u64(), Some(5));
    }

    #[test]
    fn triangulate_without_draw_inserts_nothing() {
        let mut s = session_with_doc();
        let out = s.execute("terrain.triangulate", &json!({ "points": square_pts(), "draw": false })).expect("should succeed");
        assert_eq!(out["edgeCount"].as_u64(), Some(0));
    }

    #[test]
    fn triangulate_rejects_too_few_points_as_an_error_not_a_panic() {
        let mut s = session_with_doc();
        let out = s.execute("terrain.triangulate", &json!({ "points": [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]] }));
        assert!(out.is_err());
    }

    #[test]
    fn elevation_at_interpolates_on_the_tilted_plane() {
        let mut s = session_with_doc();
        let out = s.execute("terrain.elevation_at", &json!({ "points": square_pts(), "x": 5.0, "y": 5.0 })).expect("should succeed");
        let z = out["elevation"].as_f64().expect("elevation should be present inside the hull");
        assert!((z - 5.0).abs() < 1e-6);
    }

    #[test]
    fn elevation_at_outside_the_hull_is_null() {
        let mut s = session_with_doc();
        let out = s.execute("terrain.elevation_at", &json!({ "points": square_pts(), "x": -50.0, "y": -50.0 })).expect("should succeed");
        assert!(out["elevation"].is_null());
    }

    #[test]
    fn contours_draws_segments_and_reports_levels() {
        let mut s = session_with_doc();
        let out = s.execute("terrain.contours", &json!({ "points": square_pts(), "interval": 2.0 })).expect("should succeed");
        assert_eq!(out["levels"].as_array().map(|a| a.len()), Some(4));
        assert!(out["segmentCount"].as_u64().unwrap_or(0) > 0);
    }

    #[test]
    fn contours_rejects_a_non_positive_interval() {
        let mut s = session_with_doc();
        let out = s.execute("terrain.contours", &json!({ "points": square_pts(), "interval": 0.0 }));
        assert!(out.is_err());
    }

    /// Imports a one-line CSV as a topo point block, the same way other topo command tests do
    /// (`topo_altitudes.rs`'s own test helper), so `terrain.create_mnt` has a real ALT-bearing
    /// INSERT to find without duplicating `topo.import_points`' own import logic here.
    fn import_tcpoint(s: &mut Session, id: &str, x: f64, y: f64, z: f64) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("cadcraft-terrain-mnt-test-{}-{id}-{n}.csv", std::process::id()));
        std::fs::write(&path, format!("ID;X;Y;Z;CODE\n{id};{x};{y};{z};P\n")).expect("writing the temp points file should succeed");
        s.execute("topo.import_points", &json!({ "path": path.to_string_lossy(), "label": false })).expect("import should succeed");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn create_mnt_triangulates_every_topo_point_automatically() {
        let mut s = session_with_doc();
        import_tcpoint(&mut s, "1", 0.0, 0.0, 10.0);
        import_tcpoint(&mut s, "2", 10.0, 0.0, 12.0);
        import_tcpoint(&mut s, "3", 5.0, 10.0, 15.0);

        let out = s.execute("terrain.create_mnt", &json!({ "selection": "auto" })).expect("should succeed");
        assert_eq!(out["pointCount"].as_u64(), Some(3));
        assert_eq!(out["triangleCount"].as_u64(), Some(1));
        assert_eq!(out["skippedNullAltitude"].as_u64(), Some(0));
    }

    #[test]
    fn create_mnt_skips_points_with_a_null_altitude_by_default() {
        let mut s = session_with_doc();
        import_tcpoint(&mut s, "1", 0.0, 0.0, 10.0);
        import_tcpoint(&mut s, "2", 10.0, 0.0, 12.0);
        import_tcpoint(&mut s, "3", 5.0, 10.0, 15.0);
        // A 2D-only point (no real altitude): skipped by default, included with the flag off.
        import_tcpoint(&mut s, "4", 5.0, 5.0, 0.0);

        let out = s.execute("terrain.create_mnt", &json!({ "selection": "auto" })).expect("should succeed");
        assert_eq!(out["pointCount"].as_u64(), Some(3));
        assert_eq!(out["skippedNullAltitude"].as_u64(), Some(1));

        let out = s.execute("terrain.create_mnt", &json!({ "selection": "auto", "ignoreNullAltitude": false })).expect("should succeed");
        assert_eq!(out["pointCount"].as_u64(), Some(4));
        assert_eq!(out["skippedNullAltitude"].as_u64(), Some(0));
    }

    #[test]
    fn create_mnt_rejects_fewer_than_three_usable_points() {
        let mut s = session_with_doc();
        import_tcpoint(&mut s, "1", 0.0, 0.0, 10.0);
        import_tcpoint(&mut s, "2", 10.0, 0.0, 12.0);
        let out = s.execute("terrain.create_mnt", &json!({ "selection": "auto" }));
        assert!(out.is_err());
    }

    #[test]
    fn create_mnt_manual_selection_uses_only_the_given_handles() {
        let mut s = session_with_doc();
        import_tcpoint(&mut s, "1", 0.0, 0.0, 10.0);
        import_tcpoint(&mut s, "2", 10.0, 0.0, 12.0);
        import_tcpoint(&mut s, "3", 5.0, 10.0, 15.0);
        // A fourth point that manual selection will leave out.
        import_tcpoint(&mut s, "4", 20.0, 20.0, 20.0);

        let handles: Vec<String> = s
            .doc()
            .unwrap()
            .model
            .iter()
            .filter(|e| matches!(&e.kind, cadcraft_doc::EntityKind::Insert(_)))
            .take(3)
            .map(|e| e.handle.hex())
            .collect();
        let out = s.execute("terrain.create_mnt", &json!({ "selection": "manual", "handles": handles })).expect("should succeed");
        assert_eq!(out["pointCount"].as_u64(), Some(3));
    }

    #[test]
    fn create_mnt_without_draw_inserts_nothing() {
        let mut s = session_with_doc();
        import_tcpoint(&mut s, "1", 0.0, 0.0, 10.0);
        import_tcpoint(&mut s, "2", 10.0, 0.0, 12.0);
        import_tcpoint(&mut s, "3", 5.0, 10.0, 15.0);
        let before = s.doc().unwrap().model.iter().count();
        let out = s.execute("terrain.create_mnt", &json!({ "selection": "auto", "draw": false })).expect("should succeed");
        assert_eq!(out["edgeCount"].as_u64(), Some(0));
        assert_eq!(s.doc().unwrap().model.iter().count(), before);
    }

    #[test]
    fn create_mnt_draws_into_a_green_mnt_layer_by_default() {
        let mut s = session_with_doc();
        import_tcpoint(&mut s, "1", 0.0, 0.0, 10.0);
        import_tcpoint(&mut s, "2", 10.0, 0.0, 12.0);
        import_tcpoint(&mut s, "3", 5.0, 10.0, 15.0);
        let out = s.execute("terrain.create_mnt", &json!({ "selection": "auto" })).expect("should succeed");
        assert_eq!(out["layer"].as_str(), Some("MNT"));
        let d = s.doc().unwrap();
        assert_eq!(d.layer("MNT").map(|l| l.color), Some(cadcraft_color::Color::Index(3)));
        assert!(d.model.iter().any(|e| e.common.layer.eq_ignore_ascii_case("MNT") && matches!(e.kind, EntityKind::Line(_))));
    }

    #[test]
    fn create_mnt_uses_the_given_layer_without_overriding_its_existing_color() {
        let mut s = session_with_doc();
        s.doc_mut().unwrap().ensure_layer("TN");
        s.doc_mut().unwrap().layer_mut("TN").unwrap().color = cadcraft_color::Color::Index(5);
        import_tcpoint(&mut s, "1", 0.0, 0.0, 10.0);
        import_tcpoint(&mut s, "2", 10.0, 0.0, 12.0);
        import_tcpoint(&mut s, "3", 5.0, 10.0, 15.0);
        let out = s.execute("terrain.create_mnt", &json!({ "selection": "auto", "layer": "TN" })).expect("should succeed");
        assert_eq!(out["layer"].as_str(), Some("TN"));
        assert_eq!(s.doc().unwrap().layer("TN").map(|l| l.color), Some(cadcraft_color::Color::Index(5)));
    }

    // Every corner has a real (non-zero) altitude, since `ignoreNullAltitude` defaults to true.
    fn tilted_square_points(s: &mut Session) {
        import_tcpoint(s, "1", 0.0, 0.0, 1.0);
        import_tcpoint(s, "2", 10.0, 0.0, 10.0);
        import_tcpoint(s, "3", 10.0, 10.0, 10.0);
        import_tcpoint(s, "4", 0.0, 10.0, 1.0);
    }

    #[test]
    fn create_contours_draws_each_order_into_its_own_layer() {
        let mut s = session_with_doc();
        tilted_square_points(&mut s);
        let out = s
            .execute("terrain.create_contours", &json!({ "orders": [{ "step": 2.0, "layer": "C2" }, { "step": 5.0, "layer": "C5" }] }))
            .expect("should succeed");
        let orders = out["orders"].as_array().expect("orders array");
        assert_eq!(orders.len(), 2);
        assert!(orders[0]["polylineCount"].as_u64().unwrap_or(0) > 0);
        assert!(orders[1]["polylineCount"].as_u64().unwrap_or(0) > 0);
        let d = s.doc().unwrap();
        assert!(d.model.iter().any(|e| e.common.layer.eq_ignore_ascii_case("C2") && matches!(e.kind, EntityKind::Polyline3d(_))));
        assert!(d.model.iter().any(|e| e.common.layer.eq_ignore_ascii_case("C5") && matches!(e.kind, EntityKind::Polyline3d(_))));
    }

    #[test]
    fn create_contours_clear_layers_avoids_piling_up_duplicates() {
        let mut s = session_with_doc();
        tilted_square_points(&mut s);
        let params = json!({ "orders": [{ "step": 2.0, "layer": "C2" }] });
        s.execute("terrain.create_contours", &params).expect("first run should succeed");
        let after_first = s.doc().unwrap().model.iter().filter(|e| e.common.layer.eq_ignore_ascii_case("C2")).count();
        s.execute("terrain.create_contours", &params).expect("second run should succeed");
        let after_second = s.doc().unwrap().model.iter().filter(|e| e.common.layer.eq_ignore_ascii_case("C2")).count();
        assert_eq!(after_first, after_second, "clearLayers defaults to true, so re-running shouldn't double the lines");
    }

    #[test]
    fn create_contours_rejects_empty_orders() {
        let mut s = session_with_doc();
        tilted_square_points(&mut s);
        let out = s.execute("terrain.create_contours", &json!({ "orders": [] }));
        assert!(out.is_err());
    }

    #[test]
    fn create_contours_uses_a_true_colour_when_given_one() {
        let mut s = session_with_doc();
        tilted_square_points(&mut s);
        s.execute("terrain.create_contours", &json!({ "orders": [{ "step": 2.0, "layer": "C2", "colorRgb": [255, 165, 0] }] }))
            .expect("should succeed");
        let d = s.doc().unwrap();
        assert_eq!(d.layer("C2").map(|l| l.color), Some(cadcraft_color::Color::True(cadcraft_color::Rgb(255, 165, 0))));
    }

    #[test]
    fn create_contours_with_smooth_false_keeps_the_raw_triangle_edge_count() {
        let mut s = session_with_doc();
        tilted_square_points(&mut s);
        let smoothed = s.execute("terrain.create_contours", &json!({ "orders": [{ "step": 2.0, "layer": "SMOOTH" }], "smooth": true })).unwrap();
        let raw = s.execute("terrain.create_contours", &json!({ "orders": [{ "step": 2.0, "layer": "RAW" }], "smooth": false })).unwrap();
        let smoothed_n = smoothed["orders"][0]["vertexCount"].as_u64().unwrap_or(0);
        let raw_n = raw["orders"][0]["vertexCount"].as_u64().unwrap_or(0);
        assert!(smoothed_n > raw_n, "smoothing should subdivide each chained contour into more vertices ({smoothed_n} vs {raw_n})");
    }

    #[test]
    fn dimension_contours_regular_places_at_least_one_label_per_level() {
        let mut s = session_with_doc();
        tilted_square_points(&mut s);
        s.execute("terrain.create_contours", &json!({ "orders": [{ "step": 2.0, "layer": "C2" }] })).expect("should succeed");
        let out = s.execute("terrain.dimension_contours", &json!({ "layers": ["C2"], "mode": "regular", "interval": 1.0 })).expect("should succeed");
        assert!(out["labelCount"].as_u64().unwrap_or(0) > 0);
        let d = s.doc().unwrap();
        assert!(d.model.iter().any(|e| e.common.layer.eq_ignore_ascii_case("COTES_COURBES") && matches!(e.kind, EntityKind::Text(_))));
    }

    #[test]
    fn dimension_contours_axis_labels_only_where_the_axis_crosses() {
        let mut s = session_with_doc();
        // Three parallel horizontal contour lines at y = 0, 1, 2, each a distinct elevation.
        for (i, y) in [0.0, 1.0, 2.0].into_iter().enumerate() {
            s.add_entity(EntityKind::Line(Line { a: Vec3::new(0.0, y, i as f64), b: Vec3::new(10.0, y, i as f64) })).unwrap();
        }
        let out = s
            .execute("terrain.dimension_contours", &json!({ "layers": ["0"], "mode": "axis", "axis": { "a": [5.0, -1.0], "b": [5.0, 3.0] } }))
            .expect("should succeed");
        assert_eq!(out["labelCount"].as_u64(), Some(3));
    }

    #[test]
    fn dimension_contours_rejects_an_empty_layer_list() {
        let mut s = session_with_doc();
        let out = s.execute("terrain.dimension_contours", &json!({ "layers": [] }));
        assert!(out.is_err());
    }

    #[test]
    fn dimension_contours_axis_accepts_a_hand_drawn_polyline() {
        let mut s = session_with_doc();
        for (i, y) in [0.0, 1.0, 2.0].into_iter().enumerate() {
            s.add_entity(EntityKind::Line(Line { a: Vec3::new(0.0, y, i as f64), b: Vec3::new(10.0, y, i as f64) })).unwrap();
        }
        // A bent axis: straight from (-1,-1) to (5,1.5) then up to (5,3), still crossing all 3
        // lines exactly once each (the bend sits between y=1 and y=2, not on either line).
        let out = s
            .execute(
                "terrain.dimension_contours",
                &json!({ "layers": ["0"], "mode": "axis", "axis": { "points": [[-1.0, -1.0], [5.0, 1.5], [5.0, 3.0]] } }),
            )
            .expect("should succeed");
        assert_eq!(out["labelCount"].as_u64(), Some(3));
    }

    #[test]
    fn dimension_contours_axis_rejects_a_single_point_polyline() {
        let mut s = session_with_doc();
        let out = s.execute("terrain.dimension_contours", &json!({ "layers": ["0"], "mode": "axis", "axis": { "points": [[0.0, 0.0]] } }));
        assert!(out.is_err());
    }

    #[test]
    fn upright_rotation_keeps_a_direction_and_its_reverse_the_same_way_up() {
        let d = Vec2::new(-3.0, 1.0);
        assert!((upright_rotation(d) - upright_rotation(-d)).abs() < 1e-9);
        let rot = upright_rotation(d);
        assert!(rot > -std::f64::consts::FRAC_PI_2 && rot <= std::f64::consts::FRAC_PI_2);
    }

    #[test]
    fn dimension_contours_regular_labels_face_the_same_way_along_a_reversed_chain() {
        // Two chains covering the same geometry in opposite winding order should place labels at
        // the same rotation, since `upright_rotation` folds a direction and its reverse together.
        let chain_fwd = vec![Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.5)];
        let chain_rev = vec![Vec2::new(10.0, 0.5), Vec2::new(0.0, 0.0)];
        let fwd = labels_along_chain(&chain_fwd, 5.0);
        let rev = labels_along_chain(&chain_rev, 5.0);
        assert!(!fwd.is_empty() && !rev.is_empty());
        assert!((fwd[0].1 - rev[0].1).abs() < 1e-9, "fwd={:?} rev={:?}", fwd[0].1, rev[0].1);
    }

    #[test]
    fn draw_axis_rejected_headlessly_but_registered_as_interactive() {
        let mut s = session_with_doc();
        let out = s.execute("terrain.draw_axis", &json!({}));
        assert!(out.is_err(), "no params form - it only runs interactively");
    }

    #[test]
    fn draw_axis_collects_clicked_points_into_axis_pick_and_finishes_on_enter() {
        let mut s = session_with_doc();
        assert!(s.start("terrain.draw_axis").is_ok());
        assert!(s.input(Input::Point(Vec2::new(0.0, 0.0))).is_ok());
        assert!(s.input(Input::Point(Vec2::new(5.0, 5.0))).is_ok());
        assert!(s.input(Input::Point(Vec2::new(10.0, 0.0))).is_ok());
        assert!(s.input(Input::Enter).is_ok());
        let pts = s.axis_pick.clone().expect("axis_pick set");
        assert_eq!(pts, vec![Vec2::new(0.0, 0.0), Vec2::new(5.0, 5.0), Vec2::new(10.0, 0.0)]);
        assert!(s.running.is_none(), "command finished");
    }

    #[test]
    fn draw_axis_cancels_rather_than_finishing_with_fewer_than_two_points() {
        let mut s = session_with_doc();
        assert!(s.start("terrain.draw_axis").is_ok());
        assert!(s.input(Input::Point(Vec2::new(0.0, 0.0))).is_ok());
        let _ = s.input(Input::Enter);
        assert!(s.axis_pick.is_none());
    }
}
