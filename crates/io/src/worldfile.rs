//! Esri "worldfile" (`.jgw`/`.tfw`/`.pgw`/`.bpw`/`.gfw`, generically `.wld`) parsing: the plain
//! six-line text sidecar that places a raster image in world (drawing) coordinates, from the
//! public Esri world file specification. No image bytes are touched here - see [`crate::image_header`]
//! for reading an image's pixel size without decoding it.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorldFile {
    /// Pixel size in the X direction (world units per pixel column).
    pub pixel_size_x: f64,
    /// Rotation about the Y axis (usually 0).
    pub rotation_y: f64,
    /// Rotation about the X axis (usually 0).
    pub rotation_x: f64,
    /// Pixel size in the Y direction - conventionally **negative**, since image row 0 is the top
    /// while world Y increases upward.
    pub pixel_size_y: f64,
    /// World X of the centre of the top-left pixel.
    pub origin_x: f64,
    /// World Y of the centre of the top-left pixel.
    pub origin_y: f64,
}

#[derive(Debug, Clone, thiserror::Error, PartialEq)]
pub enum WorldFileError {
    #[error("worldfile must have 6 lines (pixel size X, rotation, rotation, pixel size Y, origin X, origin Y); found {0}")]
    WrongLineCount(usize),
    #[error("worldfile line {0} (\"{1}\") is not a finite number")]
    NotANumber(usize, String),
    #[error("worldfile pixel size must not be zero (image would have no extent)")]
    ZeroPixelSize,
}

/// Parses a worldfile's text. Blank trailing lines are tolerated; anything else that isn't
/// exactly 6 numeric lines is reported rather than guessed at.
pub fn parse(text: &str) -> Result<WorldFile, WorldFileError> {
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    if lines.len() != 6 {
        return Err(WorldFileError::WrongLineCount(lines.len()));
    }
    let mut n = [0.0f64; 6];
    for (i, line) in lines.iter().enumerate() {
        n[i] = line.parse::<f64>().ok().filter(|v| v.is_finite()).ok_or_else(|| WorldFileError::NotANumber(i + 1, line.to_string()))?;
    }
    if n[0] == 0.0 || n[3] == 0.0 {
        return Err(WorldFileError::ZeroPixelSize);
    }
    Ok(WorldFile { pixel_size_x: n[0], rotation_y: n[1], rotation_x: n[2], pixel_size_y: n[3], origin_x: n[4], origin_y: n[5] })
}

impl WorldFile {
    /// The world-space position and axis vectors for a `width` x `height` pixel image, suitable
    /// for a CADCraft `IMAGE` entity (`insert`, `u` per-pixel across, `v` per-pixel up). Rotation
    /// terms are folded into `u`/`v` so a rotated worldfile still places the image correctly.
    pub fn placement(&self, width_px: u32, height_px: u32) -> ImagePlacement {
        // Worldfile convention: top-left pixel CENTRE is (origin_x, origin_y). The image's
        // top-left CORNER - CADCraft's `insert` point for an IMAGE entity - sits half a pixel
        // up-and-left of that along the file's own row/column axes.
        let u = cadcraft_geom::Vec3::new(self.pixel_size_x, self.rotation_x, 0.0);
        let v = cadcraft_geom::Vec3::new(self.rotation_y, -self.pixel_size_y, 0.0);
        let corner_x = self.origin_x - 0.5 * u.x - 0.5 * (-v.x);
        let corner_y = self.origin_y - 0.5 * u.y - 0.5 * (-v.y);
        ImagePlacement {
            insert: cadcraft_geom::Vec3::new(corner_x, corner_y, 0.0),
            u,
            v,
            size: cadcraft_geom::Vec2::new(width_px as f64, height_px as f64),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImagePlacement {
    pub insert: cadcraft_geom::Vec3,
    pub u: cadcraft_geom::Vec3,
    pub v: cadcraft_geom::Vec3,
    pub size: cadcraft_geom::Vec2,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_plain_unrotated_worldfile() {
        let wf = parse("0.5\n0.0\n0.0\n-0.5\n500000.25\n6500000.25\n").expect("should parse");
        assert_eq!(wf.pixel_size_x, 0.5);
        assert_eq!(wf.pixel_size_y, -0.5);
        assert_eq!(wf.origin_x, 500000.25);
        assert_eq!(wf.origin_y, 6500000.25);
    }

    #[test]
    fn tolerates_trailing_blank_lines() {
        let wf = parse("1\n0\n0\n-1\n0\n0\n\n\n").expect("should parse");
        assert_eq!(wf.pixel_size_x, 1.0);
    }

    #[test]
    fn wrong_line_count_is_an_error_not_a_panic() {
        assert_eq!(parse("1\n2\n3\n"), Err(WorldFileError::WrongLineCount(3)));
    }

    #[test]
    fn non_numeric_line_is_an_error_not_a_panic() {
        assert!(matches!(parse("x\n0\n0\n-1\n0\n0\n"), Err(WorldFileError::NotANumber(1, _))));
    }

    #[test]
    fn zero_pixel_size_is_rejected() {
        assert_eq!(parse("0\n0\n0\n-1\n0\n0\n"), Err(WorldFileError::ZeroPixelSize));
    }

    #[test]
    fn placement_offsets_insert_by_half_a_pixel_from_the_centre_origin() {
        let wf = parse("2.0\n0\n0\n-2.0\n100.0\n200.0\n").expect("should parse");
        let pl = wf.placement(10, 20);
        // Top-left pixel centre (100, 200) minus half a pixel each way (1.0, 1.0 world units).
        assert!((pl.insert.x - 99.0).abs() < 1e-9);
        assert!((pl.insert.y - 201.0).abs() < 1e-9);
        assert_eq!(pl.size, cadcraft_geom::Vec2::new(10.0, 20.0));
    }
}
