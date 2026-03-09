use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use geojson::{Feature, FeatureWriter, Geometry};
use serde_json::{Map, Value};
use shapefile::dbase::FieldValue;
use shapefile::{PolygonRing, Shape};

use crate::discover::ShapefileEntry;
use crate::error::AppError;

/// Statistics produced by a single successful conversion.
#[derive(Debug, Clone)]
pub struct ConversionStats {
    /// Path to the source `.shp` file.
    pub input: PathBuf,
    /// Path to the output GeoJSON / GeoJSONL file.
    pub output: PathBuf,
    /// Number of GeoJSON features written.
    pub features_written: u64,
    /// Number of records skipped (e.g., Multipatch shapes).
    pub records_skipped: u64,
    /// Wall-clock duration of the conversion.
    pub duration: std::time::Duration,
}

/// Options controlling a single conversion run.
pub struct ConvertOptions {
    /// When `true`, write one Feature per line (GeoJSONL) instead of a FeatureCollection.
    pub geojsonl: bool,
    /// When `true`, overwrite an existing output file; otherwise return an error.
    pub overwrite: bool,
    /// Path to `.prj` file for reprojection to WGS84 (None = passthrough).
    pub reproject_from_prj: Option<PathBuf>,
    /// Optional callback invoked once per written record (used by progress bars).
    pub on_record: Option<Box<dyn Fn() + Send>>,
}

// ── Coordinate transform ──────────────────────────────────────────────────────

/// Describes how to transform (x, y) coordinates during feature construction.
///
/// `Passthrough` leaves coordinates unchanged. `Proj` wraps a live PROJ pipeline.
enum CoordTransform {
    /// No transformation — coordinates are passed through as-is.
    Passthrough,
    #[cfg(feature = "reproject")]
    /// Active PROJ pipeline transforming to WGS84.
    Proj(crate::reproject::Reprojector),
}

/// Resolves the coordinate transform to use for a shapefile.
///
/// If `prj_path` is `None`, or if the `reproject` feature is disabled, returns
/// `CoordTransform::Passthrough`. Otherwise attempts to build a PROJ pipeline from
/// the `.prj` WKT; returns `Passthrough` if the CRS is already WGS84.
fn resolve_transform(prj_path: &Option<PathBuf>) -> Result<CoordTransform, AppError> {
    match prj_path {
        None => Ok(CoordTransform::Passthrough),
        #[cfg(feature = "reproject")]
        Some(prj) => match crate::reproject::Reprojector::from_prj(prj)? {
            Some(r) => Ok(CoordTransform::Proj(r)),
            None => Ok(CoordTransform::Passthrough),
        },
        #[cfg(not(feature = "reproject"))]
        Some(_) => Ok(CoordTransform::Passthrough),
    }
}

/// Transforms 2D coordinates, returning `[x, y]` or `[lon, lat]`.
///
/// Returns an error if coordinates are non-finite (NaN/infinity) — RFC 7946 §3.1.1
/// requires all coordinate values to be finite numbers.
fn apply_2d(
    x: f64,
    y: f64,
    transform: &CoordTransform,
    #[cfg_attr(not(feature = "reproject"), allow(unused_variables))] shp_path: &Path,
) -> Result<Vec<f64>, AppError> {
    if !x.is_finite() || !y.is_finite() {
        return Err(AppError::GeoJson {
            path: shp_path.to_path_buf(),
            reason: format!("non-finite coordinate: x={x}, y={y}"),
        });
    }
    match transform {
        CoordTransform::Passthrough => Ok(vec![x, y]),
        #[cfg(feature = "reproject")]
        CoordTransform::Proj(r) => {
            let (lon, lat) = r.transform(x, y, shp_path)?;
            Ok(vec![lon, lat])
        }
    }
}

/// Transforms 3D coordinates, returning `[x, y, z]` or `[lon, lat, z]`.
///
/// The Z value always passes through unchanged. Returns an error if any
/// coordinate is non-finite (NaN/infinity).
fn apply_3d(
    x: f64,
    y: f64,
    z: f64,
    transform: &CoordTransform,
    #[cfg_attr(not(feature = "reproject"), allow(unused_variables))] shp_path: &Path,
) -> Result<Vec<f64>, AppError> {
    if !x.is_finite() || !y.is_finite() || !z.is_finite() {
        return Err(AppError::GeoJson {
            path: shp_path.to_path_buf(),
            reason: format!("non-finite coordinate: x={x}, y={y}, z={z}"),
        });
    }
    match transform {
        CoordTransform::Passthrough => Ok(vec![x, y, z]),
        #[cfg(feature = "reproject")]
        CoordTransform::Proj(r) => {
            let (lon, lat, z) = r.transform_z(x, y, z, shp_path)?;
            Ok(vec![lon, lat, z])
        }
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Converts a single shapefile entry to GeoJSON or GeoJSONL.
///
/// Writes to a temporary file (`{output}.tmp`) first and renames it atomically on
/// success. The temporary file is removed on failure.
///
/// # Errors
///
/// Returns [`AppError::OutputExists`] if the output already exists and
/// `options.overwrite` is `false`.
///
/// Returns [`AppError::ShapefileRead`] for shapefile parsing failures.
///
/// Returns [`AppError::GeoJson`] for serialisation failures.
///
/// Returns [`AppError::Projection`] if reprojection is requested but the CRS
/// pipeline cannot be constructed or a coordinate transform fails.
pub fn convert(
    entry: &ShapefileEntry,
    output_path: &Path,
    options: &ConvertOptions,
) -> Result<ConversionStats, AppError> {
    let start = Instant::now();

    // Reject early if the output already exists and overwrite is not allowed.
    if output_path.exists() && !options.overwrite {
        return Err(AppError::OutputExists {
            path: output_path.to_path_buf(),
        });
    }

    // Ensure parent directory exists.
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).map_err(|source| AppError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    // Write to a temporary file; rename on success, delete on failure.
    let tmp_path = output_path.with_extension(
        output_path
            .extension()
            .map(|e| format!("{}.tmp", e.to_string_lossy()))
            .unwrap_or_else(|| "tmp".to_string()),
    );

    let result = write_features(entry, &tmp_path, options);

    match result {
        Ok((features_written, records_skipped)) => {
            // Atomic rename.
            fs::rename(&tmp_path, output_path).map_err(|source| AppError::Io {
                path: output_path.to_path_buf(),
                source,
            })?;
            Ok(ConversionStats {
                input: entry.shp.clone(),
                output: output_path.to_path_buf(),
                features_written,
                records_skipped,
                duration: start.elapsed(),
            })
        }
        Err(e) => {
            // Best-effort cleanup — ignore error if the tmp file never existed.
            let _ = fs::remove_file(&tmp_path);
            Err(e)
        }
    }
}

/// Core writing logic. Opens the shapefile reader and streams features to the
/// temporary output file.
fn write_features(
    entry: &ShapefileEntry,
    tmp_path: &Path,
    options: &ConvertOptions,
) -> Result<(u64, u64), AppError> {
    // Resolve the coordinate transform once per file (PROJ pipeline creation is
    // non-trivial, so we do it here rather than per-feature).
    let transform = resolve_transform(&options.reproject_from_prj)?;

    let mut reader =
        shapefile::Reader::from_path(&entry.shp).map_err(|source| AppError::ShapefileRead {
            path: entry.shp.clone(),
            source,
        })?;

    let file = fs::File::create(tmp_path).map_err(|source| AppError::Io {
        path: tmp_path.to_path_buf(),
        source,
    })?;
    let buf_writer = BufWriter::new(file);

    let mut features_written: u64 = 0;
    let mut records_skipped: u64 = 0;

    if options.geojsonl {
        let mut writer = buf_writer;
        for result in reader.iter_shapes_and_records() {
            let (shape, record) = result.map_err(|source| AppError::ShapefileRead {
                path: entry.shp.clone(),
                source,
            })?;

            if matches!(shape, Shape::Multipatch(_)) {
                records_skipped += 1;
                if let Some(ref cb) = options.on_record {
                    cb();
                }
                continue;
            }

            let feature = build_feature(&shape, &record, &entry.shp, &transform)?;
            serde_json::to_writer(&mut writer, &feature).map_err(|e| AppError::GeoJson {
                path: entry.shp.clone(),
                reason: e.to_string(),
            })?;
            writer.write_all(b"\n").map_err(|source| AppError::Io {
                path: tmp_path.to_path_buf(),
                source,
            })?;
            features_written += 1;
            if let Some(ref cb) = options.on_record {
                cb();
            }
        }
        writer.flush().map_err(|source| AppError::Io {
            path: tmp_path.to_path_buf(),
            source,
        })?;
    } else {
        let mut feature_writer = FeatureWriter::from_writer(buf_writer);
        for result in reader.iter_shapes_and_records() {
            let (shape, record) = result.map_err(|source| AppError::ShapefileRead {
                path: entry.shp.clone(),
                source,
            })?;

            if matches!(shape, Shape::Multipatch(_)) {
                records_skipped += 1;
                if let Some(ref cb) = options.on_record {
                    cb();
                }
                continue;
            }

            let feature = build_feature(&shape, &record, &entry.shp, &transform)?;
            feature_writer
                .write_feature(&feature)
                .map_err(|e| AppError::GeoJson {
                    path: entry.shp.clone(),
                    reason: e.to_string(),
                })?;
            features_written += 1;
            if let Some(ref cb) = options.on_record {
                cb();
            }
        }
        feature_writer.finish().map_err(|e| AppError::GeoJson {
            path: entry.shp.clone(),
            reason: e.to_string(),
        })?;
        // Explicit flush to surface any buffered I/O errors before rename.
        feature_writer.flush().map_err(|e| AppError::GeoJson {
            path: tmp_path.to_path_buf(),
            reason: e.to_string(),
        })?;
    }

    Ok((features_written, records_skipped))
}

/// Constructs a GeoJSON `Feature` from a shapefile shape and its attribute record.
fn build_feature(
    shape: &Shape,
    record: &shapefile::dbase::Record,
    shp_path: &Path,
    transform: &CoordTransform,
) -> Result<Feature, AppError> {
    let geometry = shape_to_geometry(shape, shp_path, transform)?;
    let properties = record_to_properties(record);

    Ok(Feature {
        bbox: None,
        geometry,
        id: None,
        properties: Some(properties),
        foreign_members: None,
    })
}

/// Maps a `shapefile::Shape` to an optional `geojson::Geometry`.
///
/// Returns `None` for `NullShape` (represented as `"geometry": null` in GeoJSON).
/// Returns an error for shapes that cannot be converted due to invalid coordinate data.
fn shape_to_geometry(
    shape: &Shape,
    shp_path: &Path,
    transform: &CoordTransform,
) -> Result<Option<Geometry>, AppError> {
    let value = match shape {
        Shape::NullShape => return Ok(None),

        Shape::Point(p) => geojson::Value::Point(apply_2d(p.x, p.y, transform, shp_path)?),
        Shape::PointM(p) => geojson::Value::Point(apply_2d(p.x, p.y, transform, shp_path)?),
        Shape::PointZ(p) => geojson::Value::Point(apply_3d(p.x, p.y, p.z, transform, shp_path)?),

        Shape::Polyline(pl) => polyline_parts_to_value(pl.parts(), transform, shp_path)?,
        Shape::PolylineM(pl) => polyline_parts_m_to_value(pl.parts(), transform, shp_path)?,
        Shape::PolylineZ(pl) => polyline_parts_z_to_value(pl.parts(), transform, shp_path)?,

        Shape::Polygon(pg) => polygon_rings_to_value(pg.rings(), shp_path, transform)?,
        Shape::PolygonM(pg) => polygon_rings_m_to_value(pg.rings(), shp_path, transform)?,
        Shape::PolygonZ(pg) => polygon_rings_z_to_value(pg.rings(), shp_path, transform)?,

        Shape::Multipoint(mp) => {
            let coords: Result<Vec<Vec<f64>>, AppError> = mp
                .points()
                .iter()
                .map(|p| apply_2d(p.x, p.y, transform, shp_path))
                .collect();
            geojson::Value::MultiPoint(coords?)
        }
        Shape::MultipointM(mp) => {
            let coords: Result<Vec<Vec<f64>>, AppError> = mp
                .points()
                .iter()
                .map(|p| apply_2d(p.x, p.y, transform, shp_path))
                .collect();
            geojson::Value::MultiPoint(coords?)
        }
        Shape::MultipointZ(mp) => {
            let coords: Result<Vec<Vec<f64>>, AppError> = mp
                .points()
                .iter()
                .map(|p| apply_3d(p.x, p.y, p.z, transform, shp_path))
                .collect();
            geojson::Value::MultiPoint(coords?)
        }

        // Multipatch is skipped at the caller level; this branch should not be reached.
        Shape::Multipatch(_) => {
            return Err(AppError::GeoJson {
                path: shp_path.to_path_buf(),
                reason: "Multipatch shape should have been filtered before geometry conversion"
                    .to_string(),
            })
        }
    };

    Ok(Some(Geometry::new(value)))
}

// ── Polyline helpers ──────────────────────────────────────────────────────────

fn polyline_parts_to_value(
    parts: &[Vec<shapefile::Point>],
    transform: &CoordTransform,
    shp_path: &Path,
) -> Result<geojson::Value, AppError> {
    if parts.len() == 1 {
        let coords = points_to_coords(&parts[0], transform, shp_path)?;
        Ok(geojson::Value::LineString(coords))
    } else {
        let lines: Result<Vec<Vec<Vec<f64>>>, AppError> = parts
            .iter()
            .map(|pts| points_to_coords(pts, transform, shp_path))
            .collect();
        Ok(geojson::Value::MultiLineString(lines?))
    }
}

fn polyline_parts_m_to_value(
    parts: &[Vec<shapefile::PointM>],
    transform: &CoordTransform,
    shp_path: &Path,
) -> Result<geojson::Value, AppError> {
    if parts.len() == 1 {
        let coords: Result<Vec<Vec<f64>>, AppError> = parts[0]
            .iter()
            .map(|p| apply_2d(p.x, p.y, transform, shp_path))
            .collect();
        Ok(geojson::Value::LineString(coords?))
    } else {
        let lines: Result<Vec<Vec<Vec<f64>>>, AppError> = parts
            .iter()
            .map(|pts| {
                pts.iter()
                    .map(|p| apply_2d(p.x, p.y, transform, shp_path))
                    .collect()
            })
            .collect();
        Ok(geojson::Value::MultiLineString(lines?))
    }
}

fn polyline_parts_z_to_value(
    parts: &[Vec<shapefile::PointZ>],
    transform: &CoordTransform,
    shp_path: &Path,
) -> Result<geojson::Value, AppError> {
    if parts.len() == 1 {
        let coords: Result<Vec<Vec<f64>>, AppError> = parts[0]
            .iter()
            .map(|p| apply_3d(p.x, p.y, p.z, transform, shp_path))
            .collect();
        Ok(geojson::Value::LineString(coords?))
    } else {
        let lines: Result<Vec<Vec<Vec<f64>>>, AppError> = parts
            .iter()
            .map(|pts| {
                pts.iter()
                    .map(|p| apply_3d(p.x, p.y, p.z, transform, shp_path))
                    .collect()
            })
            .collect();
        Ok(geojson::Value::MultiLineString(lines?))
    }
}

fn points_to_coords(
    pts: &[shapefile::Point],
    transform: &CoordTransform,
    shp_path: &Path,
) -> Result<Vec<Vec<f64>>, AppError> {
    pts.iter()
        .map(|p| apply_2d(p.x, p.y, transform, shp_path))
        .collect()
}

// ── Polygon helpers ───────────────────────────────────────────────────────────
//
// ESRI shapefiles use CW outer rings / CCW inner rings.
// RFC 7946 GeoJSON requires CCW outer rings / CW inner rings.
// We must reverse each ring's coordinate array.
//
// Strategy: collect rings into groups separated by Outer rings. Each group forms
// one polygon (one exterior + zero or more interiors). Emit Polygon when there is
// exactly one exterior ring group; emit MultiPolygon otherwise.

fn polygon_rings_to_value(
    rings: &[PolygonRing<shapefile::Point>],
    shp_path: &Path,
    transform: &CoordTransform,
) -> Result<geojson::Value, AppError> {
    let polygons = group_rings(
        rings,
        |ring| ring_point_coords_reversed(ring.points(), transform, shp_path),
        shp_path,
    )?;
    Ok(polygons_to_value(polygons))
}

fn polygon_rings_m_to_value(
    rings: &[PolygonRing<shapefile::PointM>],
    shp_path: &Path,
    transform: &CoordTransform,
) -> Result<geojson::Value, AppError> {
    let polygons = group_rings(
        rings,
        |ring| ring_pointm_coords_reversed(ring.points(), transform, shp_path),
        shp_path,
    )?;
    Ok(polygons_to_value(polygons))
}

fn polygon_rings_z_to_value(
    rings: &[PolygonRing<shapefile::PointZ>],
    shp_path: &Path,
    transform: &CoordTransform,
) -> Result<geojson::Value, AppError> {
    let polygons = group_rings(
        rings,
        |ring| ring_pointz_coords_reversed(ring.points(), transform, shp_path),
        shp_path,
    )?;
    Ok(polygons_to_value(polygons))
}

/// Emit `Polygon` for a single exterior ring (with optional holes),
/// or `MultiPolygon` when there are multiple exterior rings.
fn polygons_to_value(mut polygons: Vec<Vec<Vec<Vec<f64>>>>) -> geojson::Value {
    if polygons.len() == 1 {
        geojson::Value::Polygon(polygons.remove(0))
    } else {
        geojson::Value::MultiPolygon(polygons)
    }
}

/// Groups polygon rings into (exterior, holes) groups, each forming one polygon.
///
/// An `Outer` ring starts a new polygon group. `Inner` rings are appended to the
/// most recent group. Rings are coordinate-reversed to comply with RFC 7946.
fn group_rings<PointType, F>(
    rings: &[PolygonRing<PointType>],
    coord_fn: F,
    shp_path: &Path,
) -> Result<Vec<Vec<Vec<Vec<f64>>>>, AppError>
where
    F: Fn(&PolygonRing<PointType>) -> Result<Vec<Vec<f64>>, AppError>,
{
    if rings.is_empty() {
        return Err(AppError::GeoJson {
            path: shp_path.to_path_buf(),
            reason: "polygon has no rings".to_string(),
        });
    }

    // Each element: Vec of rings (first = exterior, rest = holes).
    let mut polygons: Vec<Vec<Vec<Vec<f64>>>> = Vec::new();

    for ring in rings {
        let coords = coord_fn(ring)?;
        match ring {
            PolygonRing::Outer(_) => {
                polygons.push(vec![coords]);
            }
            PolygonRing::Inner(_) => {
                if let Some(poly) = polygons.last_mut() {
                    poly.push(coords);
                } else {
                    // Inner ring without a preceding outer ring — treat as new polygon.
                    polygons.push(vec![coords]);
                }
            }
        }
    }

    Ok(polygons)
}

fn ring_point_coords_reversed(
    pts: &[shapefile::Point],
    transform: &CoordTransform,
    shp_path: &Path,
) -> Result<Vec<Vec<f64>>, AppError> {
    let mut coords: Vec<Vec<f64>> = pts
        .iter()
        .map(|p| apply_2d(p.x, p.y, transform, shp_path))
        .collect::<Result<_, _>>()?;
    coords.reverse();
    Ok(coords)
}

fn ring_pointm_coords_reversed(
    pts: &[shapefile::PointM],
    transform: &CoordTransform,
    shp_path: &Path,
) -> Result<Vec<Vec<f64>>, AppError> {
    let mut coords: Vec<Vec<f64>> = pts
        .iter()
        .map(|p| apply_2d(p.x, p.y, transform, shp_path))
        .collect::<Result<_, _>>()?;
    coords.reverse();
    Ok(coords)
}

fn ring_pointz_coords_reversed(
    pts: &[shapefile::PointZ],
    transform: &CoordTransform,
    shp_path: &Path,
) -> Result<Vec<Vec<f64>>, AppError> {
    let mut coords: Vec<Vec<f64>> = pts
        .iter()
        .map(|p| apply_3d(p.x, p.y, p.z, transform, shp_path))
        .collect::<Result<_, _>>()?;
    coords.reverse();
    Ok(coords)
}

// ── Record → properties ───────────────────────────────────────────────────────

/// Converts a dbase `Record` (map of field name → `FieldValue`) into a
/// `serde_json::Map` suitable for GeoJSON properties.
fn record_to_properties(record: &shapefile::dbase::Record) -> Map<String, Value> {
    use std::collections::HashMap;
    let hash_map: &HashMap<String, FieldValue> = record.as_ref();
    let mut map = Map::new();
    for (name, field_value) in hash_map {
        let json_value = field_value_to_json(field_value);
        map.insert(name.clone(), json_value);
    }
    map
}

/// Maps a `shapefile::dbase::FieldValue` to a `serde_json::Value`.
fn field_value_to_json(fv: &FieldValue) -> Value {
    match fv {
        FieldValue::Character(Some(s)) => Value::String(s.clone()),
        FieldValue::Character(None) => Value::Null,

        FieldValue::Numeric(Some(f)) => json_number_f64(*f),
        FieldValue::Numeric(None) => Value::Null,

        FieldValue::Float(Some(f)) => json_number_f64(f64::from(*f)),
        FieldValue::Float(None) => Value::Null,

        FieldValue::Double(f) => json_number_f64(*f),

        FieldValue::Integer(i) => Value::Number((*i).into()),

        FieldValue::Logical(Some(b)) => Value::Bool(*b),
        FieldValue::Logical(None) => Value::Null,

        FieldValue::Date(Some(d)) => {
            Value::String(format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day()))
        }
        FieldValue::Date(None) => Value::Null,

        FieldValue::DateTime(dt) => {
            let d = dt.date();
            let t = dt.time();
            Value::String(format!(
                "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
                d.year(),
                d.month(),
                d.day(),
                t.hours(),
                t.minutes(),
                t.seconds(),
            ))
        }

        FieldValue::Memo(s) => Value::String(s.clone()),

        FieldValue::Currency(f) => json_number_f64(*f),
    }
}

/// Converts an `f64` to a `serde_json::Value::Number`.
///
/// Falls back to `Value::Null` for values that cannot be represented as JSON
/// numbers (NaN, infinity).
fn json_number_f64(f: f64) -> Value {
    serde_json::Number::from_f64(f)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}

/// Computes the output path for a given `.shp` file, mirroring the source
/// directory structure under `output_root`.
///
/// # Example
///
/// ```
/// use std::path::PathBuf;
/// use shp2geojson::convert::output_path_for;
///
/// let shp = PathBuf::from("/data/regions/europe.shp");
/// let input_root = PathBuf::from("/data");
/// let output_root = PathBuf::from("/out");
///
/// let out = output_path_for(&shp, &input_root, &output_root, false).unwrap();
/// assert_eq!(out, PathBuf::from("/out/regions/europe.geojson"));
/// ```
pub fn output_path_for(
    shp: &Path,
    input_root: &Path,
    output_root: &Path,
    geojsonl: bool,
) -> Result<PathBuf, AppError> {
    let relative = shp
        .strip_prefix(input_root)
        .map_err(|_| AppError::GeoJson {
            path: shp.to_path_buf(),
            reason: format!(
                "could not make {} relative to {}",
                shp.display(),
                input_root.display()
            ),
        })?;

    let extension = if geojsonl { "geojsonl" } else { "geojson" };
    let out = output_root.join(relative).with_extension(extension);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ── output_path_for ───────────────────────────────────────────────────────

    #[test]
    fn test_output_path_for_geojson() {
        let shp = PathBuf::from("/data/regions/europe.shp");
        let input = PathBuf::from("/data");
        let output = PathBuf::from("/out");
        let result = output_path_for(&shp, &input, &output, false).unwrap();
        assert_eq!(result, PathBuf::from("/out/regions/europe.geojson"));
    }

    #[test]
    fn test_output_path_for_geojsonl() {
        let shp = PathBuf::from("/data/regions/europe.shp");
        let input = PathBuf::from("/data");
        let output = PathBuf::from("/out");
        let result = output_path_for(&shp, &input, &output, true).unwrap();
        assert_eq!(result, PathBuf::from("/out/regions/europe.geojsonl"));
    }

    #[test]
    fn test_output_path_for_root_level() {
        let shp = PathBuf::from("/data/file.shp");
        let input = PathBuf::from("/data");
        let output = PathBuf::from("/out");
        let result = output_path_for(&shp, &input, &output, false).unwrap();
        assert_eq!(result, PathBuf::from("/out/file.geojson"));
    }

    #[test]
    fn test_output_path_for_wrong_root_returns_error() {
        let shp = PathBuf::from("/other/file.shp");
        let input = PathBuf::from("/data");
        let output = PathBuf::from("/out");
        assert!(output_path_for(&shp, &input, &output, false).is_err());
    }

    // ── field_value_to_json ───────────────────────────────────────────────────

    #[test]
    fn test_field_value_character_some() {
        let fv = FieldValue::Character(Some("hello".to_string()));
        assert_eq!(field_value_to_json(&fv), Value::String("hello".to_string()));
    }

    #[test]
    fn test_field_value_character_none() {
        let fv = FieldValue::Character(None);
        assert_eq!(field_value_to_json(&fv), Value::Null);
    }

    #[test]
    fn test_field_value_numeric_some() {
        let fv = FieldValue::Numeric(Some(3.14));
        match field_value_to_json(&fv) {
            Value::Number(n) => assert!((n.as_f64().unwrap() - 3.14).abs() < 1e-10),
            other => panic!("expected Number, got {other:?}"),
        }
    }

    #[test]
    fn test_field_value_numeric_none() {
        let fv = FieldValue::Numeric(None);
        assert_eq!(field_value_to_json(&fv), Value::Null);
    }

    #[test]
    fn test_field_value_integer() {
        let fv = FieldValue::Integer(42);
        assert_eq!(field_value_to_json(&fv), Value::Number(42.into()));
    }

    #[test]
    fn test_field_value_logical_true() {
        let fv = FieldValue::Logical(Some(true));
        assert_eq!(field_value_to_json(&fv), Value::Bool(true));
    }

    #[test]
    fn test_field_value_logical_none() {
        let fv = FieldValue::Logical(None);
        assert_eq!(field_value_to_json(&fv), Value::Null);
    }

    #[test]
    fn test_field_value_date_some() {
        let fv = FieldValue::Date(Some(shapefile::dbase::Date::new(8, 3, 2026)));
        assert_eq!(
            field_value_to_json(&fv),
            Value::String("2026-03-08".to_string())
        );
    }

    #[test]
    fn test_field_value_date_none() {
        let fv = FieldValue::Date(None);
        assert_eq!(field_value_to_json(&fv), Value::Null);
    }

    #[test]
    fn test_field_value_memo() {
        let fv = FieldValue::Memo("long text".to_string());
        assert_eq!(
            field_value_to_json(&fv),
            Value::String("long text".to_string())
        );
    }

    #[test]
    fn test_field_value_nan_becomes_null() {
        let fv = FieldValue::Numeric(Some(f64::NAN));
        assert_eq!(field_value_to_json(&fv), Value::Null);
    }

    // ── shape_to_geometry ─────────────────────────────────────────────────────

    #[test]
    fn test_null_shape_produces_none_geometry() {
        let t = CoordTransform::Passthrough;
        let result = shape_to_geometry(&Shape::NullShape, Path::new("test.shp"), &t).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_point_shape() {
        let p = shapefile::Point { x: 10.0, y: 20.0 };
        let t = CoordTransform::Passthrough;
        let result = shape_to_geometry(&Shape::Point(p), Path::new("test.shp"), &t)
            .unwrap()
            .unwrap();
        assert!(matches!(result.value, geojson::Value::Point(_)));
        if let geojson::Value::Point(coords) = result.value {
            assert_eq!(coords, vec![10.0, 20.0]);
        }
    }

    #[test]
    fn test_pointz_includes_z_coordinate() {
        let p = shapefile::PointZ {
            x: 1.0,
            y: 2.0,
            z: 3.0,
            m: 0.0,
        };
        let t = CoordTransform::Passthrough;
        let result = shape_to_geometry(&Shape::PointZ(p), Path::new("test.shp"), &t)
            .unwrap()
            .unwrap();
        if let geojson::Value::Point(coords) = result.value {
            assert_eq!(coords, vec![1.0, 2.0, 3.0]);
        } else {
            panic!("expected Point");
        }
    }

    #[test]
    fn test_polyline_single_part_is_linestring() {
        let pl = shapefile::Polyline::new(vec![
            shapefile::Point { x: 0.0, y: 0.0 },
            shapefile::Point { x: 1.0, y: 1.0 },
        ]);
        let t = CoordTransform::Passthrough;
        let result = shape_to_geometry(&Shape::Polyline(pl), Path::new("test.shp"), &t)
            .unwrap()
            .unwrap();
        assert!(matches!(result.value, geojson::Value::LineString(_)));
    }

    #[test]
    fn test_polyline_multi_part_is_multilinestring() {
        let pl = shapefile::Polyline::with_parts(vec![
            vec![
                shapefile::Point { x: 0.0, y: 0.0 },
                shapefile::Point { x: 1.0, y: 1.0 },
            ],
            vec![
                shapefile::Point { x: 2.0, y: 2.0 },
                shapefile::Point { x: 3.0, y: 3.0 },
            ],
        ]);
        let t = CoordTransform::Passthrough;
        let result = shape_to_geometry(&Shape::Polyline(pl), Path::new("test.shp"), &t)
            .unwrap()
            .unwrap();
        assert!(matches!(result.value, geojson::Value::MultiLineString(_)));
    }

    #[test]
    fn test_polygon_single_ring_is_polygon() {
        let ring = shapefile::PolygonRing::Outer(vec![
            shapefile::Point { x: 0.0, y: 0.0 },
            shapefile::Point { x: 1.0, y: 0.0 },
            shapefile::Point { x: 1.0, y: 1.0 },
            shapefile::Point { x: 0.0, y: 1.0 },
            shapefile::Point { x: 0.0, y: 0.0 },
        ]);
        let polygon = shapefile::Polygon::new(ring);
        let t = CoordTransform::Passthrough;
        let result = shape_to_geometry(&Shape::Polygon(polygon), Path::new("test.shp"), &t)
            .unwrap()
            .unwrap();
        assert!(matches!(result.value, geojson::Value::Polygon(_)));
    }

    #[test]
    fn test_multipoint_shape() {
        let mp = shapefile::Multipoint::new(vec![
            shapefile::Point { x: 1.0, y: 2.0 },
            shapefile::Point { x: 3.0, y: 4.0 },
        ]);
        let t = CoordTransform::Passthrough;
        let result = shape_to_geometry(&Shape::Multipoint(mp), Path::new("test.shp"), &t)
            .unwrap()
            .unwrap();
        assert!(matches!(result.value, geojson::Value::MultiPoint(_)));
    }
}
