use std::path::Path;

use crate::error::AppError;

/// Wraps a PROJ transformation pipeline from source CRS to WGS84.
///
/// `Proj` is `Send` but NOT `Sync` — each worker thread needs its own instance.
/// Create one `Reprojector` per thread by calling [`Reprojector::from_prj`] inside
/// each thread.
pub struct Reprojector {
    proj: proj::Proj,
}

impl std::fmt::Debug for Reprojector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reprojector").finish_non_exhaustive()
    }
}

impl Reprojector {
    /// Creates a reprojector from a `.prj` file path.
    ///
    /// Returns `Ok(None)` if the CRS described by the file is already WGS84 (no
    /// reprojection needed). Returns `Ok(Some(Self))` with a valid PROJ pipeline
    /// otherwise. Returns `Err` on file read failure or if PROJ cannot parse the WKT.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::path::Path;
    /// use shp2geojson::reproject::Reprojector;
    ///
    /// let r = Reprojector::from_prj(Path::new("data/counties.prj")).unwrap();
    /// // r is None if the .prj is already WGS84
    /// ```
    pub fn from_prj(prj_path: &Path) -> Result<Option<Self>, AppError> {
        let wkt = std::fs::read_to_string(prj_path).map_err(|source| AppError::Io {
            path: prj_path.to_path_buf(),
            source,
        })?;

        let wkt = wkt.trim();
        if is_wgs84(wkt) {
            return Ok(None);
        }

        let proj = proj::Proj::new_known_crs(wkt, "EPSG:4326", None).map_err(|e| {
            AppError::Projection {
                path: prj_path.to_path_buf(),
                reason: e.to_string(),
            }
        })?;

        Ok(Some(Self { proj }))
    }

    /// Transform 2D coordinates.
    ///
    /// Returns `(longitude, latitude)` in WGS84 degrees.
    pub fn transform(&self, x: f64, y: f64, shp_path: &Path) -> Result<(f64, f64), AppError> {
        self.proj.convert((x, y)).map_err(|e| AppError::Projection {
            path: shp_path.to_path_buf(),
            reason: e.to_string(),
        })
    }

    /// Transform 3D coordinates.
    ///
    /// The Z value passes through unchanged; only X and Y are reprojected.
    pub fn transform_z(
        &self,
        x: f64,
        y: f64,
        z: f64,
        shp_path: &Path,
    ) -> Result<(f64, f64, f64), AppError> {
        let (lon, lat) = self.transform(x, y, shp_path)?;
        Ok((lon, lat, z))
    }
}

/// Heuristic: returns `true` if WKT describes a WGS84 geographic CRS.
///
/// Biased toward false negatives (safe: worst case is an unnecessary identity
/// transform). Projected CRS based on WGS84 are correctly identified as *not*
/// WGS84 so that the re-projection step is applied.
///
/// # Example
///
/// ```
/// use shp2geojson::reproject::is_wgs84;
///
/// assert!(is_wgs84("GEOGCS[\"GCS_WGS_1984\",...]"));
/// assert!(!is_wgs84("PROJCS[\"WGS_1984_UTM_Zone_32N\",...]"));
/// ```
pub fn is_wgs84(wkt: &str) -> bool {
    let upper = wkt.to_uppercase();
    // A projected CRS is NOT WGS84 geographic even if it's based on WGS84.
    if upper.starts_with("PROJCS[") || upper.starts_with("PROJCRS[") {
        return false;
    }
    // Must contain at least one WGS84 indicator.
    upper.contains("WGS_1984")
        || upper.contains("WGS 84")
        || upper.contains("WGS84")
        || upper.contains("GCS_WGS_1984")
        || upper.contains("WORLD GEODETIC SYSTEM 1984")
        || upper.contains("\"EPSG\",\"4326\"")
        || upper.contains("\"EPSG\",4326")
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── is_wgs84 ──────────────────────────────────────────────────────────────

    #[test]
    fn test_is_wgs84_returns_true_for_gcs_wgs_1984() {
        let wkt = r#"GEOGCS["GCS_WGS_1984",DATUM["D_WGS_1984",SPHEROID["WGS_1984",6378137.0,298.257223563]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]]"#;
        assert!(is_wgs84(wkt));
    }

    #[test]
    fn test_is_wgs84_returns_true_for_wgs84_shorthand() {
        assert!(is_wgs84("GEOGCS[\"WGS84\",...]"));
    }

    #[test]
    fn test_is_wgs84_returns_true_for_wgs_space_84() {
        assert!(is_wgs84("GEOGCS[\"WGS 84\",...]"));
    }

    #[test]
    fn test_is_wgs84_returns_false_for_utm_projected() {
        let wkt = r#"PROJCS["WGS_1984_UTM_Zone_32N",GEOGCS["GCS_WGS_1984",...],PROJECTION["Transverse_Mercator"],...]"#;
        assert!(!is_wgs84(wkt));
    }

    #[test]
    fn test_is_wgs84_returns_false_for_state_plane() {
        let wkt = r#"PROJCS["NAD83 / California zone 6",GEOGCS["NAD83",...],...]"#;
        assert!(!is_wgs84(wkt));
    }

    #[test]
    fn test_is_wgs84_returns_false_for_nad83() {
        let wkt = r#"GEOGCS["NAD83",DATUM["D_North_American_1983",...],...]"#;
        assert!(!is_wgs84(wkt));
    }

    #[test]
    fn test_is_wgs84_returns_false_for_empty_string() {
        assert!(!is_wgs84(""));
    }

    #[test]
    fn test_is_wgs84_case_insensitive() {
        // Lowercase should still match.
        assert!(is_wgs84("geogcs[\"gcs_wgs_1984\",...]"));
    }

    #[test]
    fn test_is_wgs84_projcrs_prefix_returns_false() {
        // ISO WKT2 uses PROJCRS[ instead of PROJCS[
        let wkt = r#"PROJCRS["WGS 84 / UTM zone 32N",...]"#;
        assert!(!is_wgs84(wkt));
    }

    // ── Reprojector::from_prj ─────────────────────────────────────────────────

    #[test]
    fn test_from_prj_returns_none_for_wgs84() {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        let wkt = r#"GEOGCS["GCS_WGS_1984",DATUM["D_WGS_1984",SPHEROID["WGS_1984",6378137.0,298.257223563]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]]"#;
        f.write_all(wkt.as_bytes()).unwrap();
        let result = Reprojector::from_prj(f.path()).unwrap();
        assert!(result.is_none(), "expected None for WGS84 .prj");
    }

    #[test]
    fn test_from_prj_returns_some_for_projected_crs() {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        // WGS84 UTM Zone 32N — a projected CRS
        let wkt = r#"PROJCS["WGS 84 / UTM zone 32N",GEOGCS["WGS 84",DATUM["WGS_1984",SPHEROID["WGS 84",6378137,298.257223563]],PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]],PROJECTION["Transverse_Mercator"],PARAMETER["latitude_of_origin",0],PARAMETER["central_meridian",9],PARAMETER["scale_factor",0.9996],PARAMETER["false_easting",500000],PARAMETER["false_northing",0],UNIT["metre",1]]"#;
        f.write_all(wkt.as_bytes()).unwrap();
        let result = Reprojector::from_prj(f.path()).unwrap();
        assert!(
            result.is_some(),
            "expected Some(Reprojector) for projected CRS"
        );
    }

    #[test]
    fn test_from_prj_missing_file_returns_error() {
        let path = std::path::Path::new("/nonexistent/path/to/file.prj");
        let result = Reprojector::from_prj(path);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("I/O") || err.contains("No such file"), "{err}");
    }

    #[test]
    fn test_transform_utm_to_wgs84() {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        let wkt = r#"PROJCS["WGS 84 / UTM zone 32N",GEOGCS["WGS 84",DATUM["WGS_1984",SPHEROID["WGS 84",6378137,298.257223563]],PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]],PROJECTION["Transverse_Mercator"],PARAMETER["latitude_of_origin",0],PARAMETER["central_meridian",9],PARAMETER["scale_factor",0.9996],PARAMETER["false_easting",500000],PARAMETER["false_northing",0],UNIT["metre",1]]"#;
        f.write_all(wkt.as_bytes()).unwrap();
        let reprojector = Reprojector::from_prj(f.path()).unwrap().unwrap();

        // UTM Zone 32N coordinates for approximately (lon=9.0, lat=51.0)
        // Easting: ~500000, Northing: ~5649740
        let (lon, lat) = reprojector
            .transform(500_000.0, 5_649_740.0, std::path::Path::new("test.shp"))
            .unwrap();

        // Allow ±0.01 degree tolerance
        assert!((lon - 9.0).abs() < 0.01, "lon={lon} expected ~9.0");
        assert!((lat - 51.0).abs() < 0.01, "lat={lat} expected ~51.0");
    }
}
