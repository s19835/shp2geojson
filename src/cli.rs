use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use clap_complete::Shell;

/// Output format for progress and status messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    /// Rich human-readable terminal output (default).
    Human,
    /// Newline-delimited JSON events for CI/CD pipelines.
    Json,
}

/// High-performance ESRI Shapefile to GeoJSON converter.
///
/// Recursively discovers `.shp` files under `--input`, validates their sidecar
/// files, and converts each one to GeoJSON (or GeoJSONL) under `--output`,
/// mirroring the source directory structure.
#[derive(Debug, Parser)]
#[command(name = "shp2geojson", version, about)]
pub struct Cli {
    /// Source folder (scanned recursively for `.shp` files).
    ///
    /// Required unless `--completions` is specified.
    #[arg(long, value_name = "PATH")]
    pub input: Option<PathBuf>,

    /// Output root directory (mirrors source structure).
    ///
    /// Required unless `--dry-run` is specified.
    #[arg(long, value_name = "PATH")]
    pub output: Option<PathBuf>,

    /// Number of parallel worker threads.
    ///
    /// Defaults to the number of logical CPUs when omitted.
    #[arg(long, value_name = "N")]
    pub jobs: Option<usize>,

    /// Skip files that were already successfully converted in a prior run.
    ///
    /// Reads `.shp2geojson_state.json` from the output root.
    #[arg(long, default_value_t = false)]
    pub resume: bool,

    /// Discover and validate shapefiles without writing any output.
    #[arg(long, default_value_t = false)]
    pub dry_run: bool,

    /// Format for status and progress output.
    #[arg(long, value_name = "FORMAT", default_value = "human")]
    pub output_format: OutputFormat,

    /// Automatically reproject geometries to WGS84 (EPSG:4326) when a `.prj` is present.
    #[arg(long, overrides_with = "no_reproject")]
    pub reproject: bool,

    /// Disable reprojection — pass geometry coordinates through unchanged.
    #[arg(long, overrides_with = "reproject")]
    pub no_reproject: bool,

    /// Write one GeoJSON Feature per line (GeoJSONL) instead of a FeatureCollection.
    #[arg(long, default_value_t = false)]
    pub geojsonl: bool,

    /// Overwrite existing output files.
    ///
    /// Without this flag, the tool fails if an output file already exists.
    #[arg(long, default_value_t = false)]
    pub overwrite: bool,

    /// Path for the error log file.
    ///
    /// Defaults to `{output}/conversion_errors.log`.
    #[arg(long, value_name = "PATH")]
    pub log: Option<PathBuf>,

    /// Path to the project configuration file.
    ///
    /// Defaults to `.shp2geojson.toml` in the current directory.
    #[arg(long, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Generate shell completions for the given shell and print to stdout.
    ///
    /// Example: `shp2geojson --completions bash > /usr/local/etc/bash_completion.d/shp2geojson`
    #[arg(long, value_name = "SHELL")]
    pub completions: Option<Shell>,
}
