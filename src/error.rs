use std::path::PathBuf;
use thiserror::Error;

/// All errors that can occur in the shp2geojson conversion pipeline.
#[derive(Debug, Error)]
pub enum AppError {
    /// A required sidecar file (`.dbf` or `.shx`) is missing alongside a `.shp`.
    ///
    /// Example: `missing sidecar .dbf for /data/counties.shp`
    #[error("missing sidecar {ext} for {shp}")]
    MissingSidecar { shp: PathBuf, ext: &'static str },

    /// The `shapefile` crate returned an error while reading geometry or records.
    #[error("shapefile read error in {path}: {source}")]
    ShapefileRead {
        path: PathBuf,
        #[source]
        source: shapefile::Error,
    },

    /// A general I/O error associated with a file path.
    #[error("I/O error for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// An error occurred while serializing a feature to GeoJSON.
    #[error("GeoJSON serialisation error in {path}: {reason}")]
    GeoJson { path: PathBuf, reason: String },

    /// Reprojection was requested but the CRS information is unavailable or invalid.
    #[error("projection unavailable for {path}: {reason}")]
    Projection { path: PathBuf, reason: String },

    /// The output file already exists and `--overwrite` was not specified.
    #[error("output already exists: {path} (use --overwrite to replace)")]
    OutputExists { path: PathBuf },

    /// An error occurred while reading or writing the checkpoint state file.
    #[error("checkpoint error: {reason}")]
    Checkpoint { reason: String },

    /// An error occurred while loading or parsing the configuration file.
    #[error("config error: {reason}")]
    Config { reason: String },
}
