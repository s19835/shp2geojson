use std::fs;
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::error::AppError;

/// Indicates whether a discovered shapefile entry has all required sidecar files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryStatus {
    /// All required sidecars (`.dbf`, `.shx`) are present.
    Valid,
    /// One or more required sidecars are missing. Contains the missing extensions.
    Invalid(Vec<&'static str>),
}

/// Represents a single discovered shapefile dataset with its sidecar file paths.
///
/// A valid shapefile dataset consists of `.shp` + `.dbf` + `.shx`.
/// The `.prj` (CRS) and `.cpg` (encoding) sidecars are optional.
#[derive(Debug, Clone)]
pub struct ShapefileEntry {
    /// Path to the `.shp` geometry file.
    pub shp: PathBuf,
    /// Path to the `.dbf` attribute table.
    pub dbf: PathBuf,
    /// Path to the `.shx` spatial index.
    pub shx: PathBuf,
    /// Path to the `.prj` CRS definition (optional).
    pub prj: Option<PathBuf>,
    /// Path to the `.cpg` character encoding hint (optional).
    pub cpg: Option<PathBuf>,
    /// Whether this entry has all required sidecars.
    pub status: EntryStatus,
}

impl ShapefileEntry {
    /// Returns `true` if this entry is ready to be converted.
    pub fn is_valid(&self) -> bool {
        self.status == EntryStatus::Valid
    }
}

/// Summary statistics produced by the discovery phase.
///
/// Contains all discovered entries along with aggregate counts and size
/// estimates used to inform the user before conversion begins.
#[derive(Debug)]
pub struct DiscoveryReport {
    /// All discovered shapefile entries (both valid and invalid).
    pub entries: Vec<ShapefileEntry>,
    /// Number of entries where all required sidecars are present.
    pub valid_count: usize,
    /// Number of entries with one or more missing required sidecars.
    pub invalid_count: usize,
    /// Sum of `.shp` file sizes for valid entries (bytes).
    pub total_input_bytes: u64,
    /// Estimated GeoJSON output size: `total_input_bytes * 3.7`.
    pub estimated_output_bytes: u64,
}

/// Recursively discovers all `.shp` files under `input_root` and validates
/// their required sidecar files.
///
/// `.dbf` and `.shx` must exist alongside the `.shp` for an entry to be
/// `Valid`. Missing either produces an `Invalid` entry listing the missing
/// extensions.
///
/// `.prj` and `.cpg` are stored when present but are not required.
///
/// # Errors
///
/// Returns [`AppError`] only if `walkdir` itself fails (e.g., permission denied
/// on the root directory). Individual missing sidecars produce `Invalid` entries,
/// not errors.
pub fn discover(input_root: &Path) -> Result<DiscoveryReport, AppError> {
    let mut entries = Vec::new();

    for dir_entry in WalkDir::new(input_root)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter()
        .filter_map(|result| match result {
            Ok(e) => Some(e),
            Err(e) => {
                eprintln!("warning: directory walk error: {e}");
                None
            }
        })
        .filter(|e| e.file_type().is_file())
    {
        let path = dir_entry.path();

        // Case-sensitive: only match lowercase ".shp" extension
        let extension = match path.extension().and_then(|e| e.to_str()) {
            Some(ext) => ext,
            None => continue,
        };

        if extension != "shp" {
            continue;
        }

        let entry = check_entry(path);
        entries.push(entry);
    }

    let valid_count = entries.iter().filter(|e| e.is_valid()).count();
    let invalid_count = entries.len() - valid_count;

    // Sum .shp file sizes for valid entries only.
    let total_input_bytes: u64 = entries
        .iter()
        .filter(|e| e.is_valid())
        .map(|e| fs::metadata(&e.shp).map(|m| m.len()).unwrap_or(0))
        .sum();

    // GeoJSON text is approximately 3.7× larger than the binary shapefile.
    let estimated_output_bytes = total_input_bytes * 37 / 10;

    Ok(DiscoveryReport {
        entries,
        valid_count,
        invalid_count,
        total_input_bytes,
        estimated_output_bytes,
    })
}

/// Builds a `ShapefileEntry` for a given `.shp` path by checking for sidecars.
fn check_entry(shp: &Path) -> ShapefileEntry {
    let dbf = shp.with_extension("dbf");
    let shx = shp.with_extension("shx");
    let prj_path = shp.with_extension("prj");
    let cpg_path = shp.with_extension("cpg");

    let mut missing: Vec<&'static str> = Vec::new();
    if !dbf.exists() {
        missing.push(".dbf");
    }
    if !shx.exists() {
        missing.push(".shx");
    }

    let status = if missing.is_empty() {
        EntryStatus::Valid
    } else {
        EntryStatus::Invalid(missing)
    };

    ShapefileEntry {
        shp: shp.to_path_buf(),
        dbf,
        shx,
        prj: if prj_path.exists() {
            Some(prj_path)
        } else {
            None
        },
        cpg: if cpg_path.exists() {
            Some(cpg_path)
        } else {
            None
        },
        status,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Creates a minimal shapefile set (touch-only — zero-byte files are enough for
    /// the discovery phase, which only checks existence, not content).
    fn touch(dir: &Path, name: &str) {
        fs::write(dir.join(name), b"").unwrap();
    }

    #[test]
    fn test_discover_finds_valid_entry() {
        let dir = TempDir::new().unwrap();
        touch(dir.path(), "region.shp");
        touch(dir.path(), "region.dbf");
        touch(dir.path(), "region.shx");

        let report = discover(dir.path()).unwrap();
        assert_eq!(report.entries.len(), 1);
        assert_eq!(report.entries[0].status, EntryStatus::Valid);
        assert_eq!(report.valid_count, 1);
        assert_eq!(report.invalid_count, 0);
    }

    #[test]
    fn test_discover_skips_missing_dbf() {
        let dir = TempDir::new().unwrap();
        touch(dir.path(), "region.shp");
        touch(dir.path(), "region.shx");
        // No .dbf

        let report = discover(dir.path()).unwrap();
        assert_eq!(report.entries.len(), 1);
        assert!(matches!(report.entries[0].status, EntryStatus::Invalid(_)));
        if let EntryStatus::Invalid(ref missing) = report.entries[0].status {
            assert!(missing.contains(&".dbf"));
        }
        assert_eq!(report.valid_count, 0);
        assert_eq!(report.invalid_count, 1);
    }

    #[test]
    fn test_discover_skips_missing_shx() {
        let dir = TempDir::new().unwrap();
        touch(dir.path(), "region.shp");
        touch(dir.path(), "region.dbf");
        // No .shx

        let report = discover(dir.path()).unwrap();
        assert_eq!(report.entries.len(), 1);
        if let EntryStatus::Invalid(ref missing) = report.entries[0].status {
            assert!(missing.contains(&".shx"));
        }
    }

    #[test]
    fn test_discover_both_sidecars_missing() {
        let dir = TempDir::new().unwrap();
        touch(dir.path(), "region.shp");
        // No .dbf, no .shx

        let report = discover(dir.path()).unwrap();
        assert_eq!(report.entries.len(), 1);
        if let EntryStatus::Invalid(ref missing) = report.entries[0].status {
            assert_eq!(missing.len(), 2);
        }
    }

    #[test]
    fn test_discover_optional_prj_and_cpg() {
        let dir = TempDir::new().unwrap();
        touch(dir.path(), "region.shp");
        touch(dir.path(), "region.dbf");
        touch(dir.path(), "region.shx");
        touch(dir.path(), "region.prj");
        touch(dir.path(), "region.cpg");

        let report = discover(dir.path()).unwrap();
        assert_eq!(report.entries.len(), 1);
        assert_eq!(report.entries[0].status, EntryStatus::Valid);
        assert!(report.entries[0].prj.is_some());
        assert!(report.entries[0].cpg.is_some());
    }

    #[test]
    fn test_discover_ignores_non_shp_files() {
        let dir = TempDir::new().unwrap();
        touch(dir.path(), "readme.txt");
        touch(dir.path(), "data.SHP"); // uppercase — should be ignored (case-sensitive)
        touch(dir.path(), "data.geojson");

        let report = discover(dir.path()).unwrap();
        assert_eq!(report.entries.len(), 0);
        assert_eq!(report.valid_count, 0);
        assert_eq!(report.invalid_count, 0);
    }

    #[test]
    fn test_discover_recursive() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("sub").join("nested");
        fs::create_dir_all(&sub).unwrap();
        touch(&sub, "deep.shp");
        touch(&sub, "deep.dbf");
        touch(&sub, "deep.shx");

        let report = discover(dir.path()).unwrap();
        assert_eq!(report.entries.len(), 1);
        assert_eq!(report.entries[0].status, EntryStatus::Valid);
    }

    #[test]
    fn test_discover_empty_directory() {
        let dir = TempDir::new().unwrap();
        let report = discover(dir.path()).unwrap();
        assert_eq!(report.entries.len(), 0);
        assert_eq!(report.valid_count, 0);
        assert_eq!(report.invalid_count, 0);
        assert_eq!(report.total_input_bytes, 0);
        assert_eq!(report.estimated_output_bytes, 0);
    }

    #[test]
    fn test_discover_multiple_files() {
        let dir = TempDir::new().unwrap();
        for name in &["a", "b", "c"] {
            touch(dir.path(), &format!("{name}.shp"));
            touch(dir.path(), &format!("{name}.dbf"));
            touch(dir.path(), &format!("{name}.shx"));
        }

        let report = discover(dir.path()).unwrap();
        assert_eq!(report.entries.len(), 3);
        assert!(report
            .entries
            .iter()
            .all(|e| e.status == EntryStatus::Valid));
        assert_eq!(report.valid_count, 3);
        assert_eq!(report.invalid_count, 0);
    }

    #[test]
    fn test_discover_report_counts_valid_and_invalid() {
        let dir = TempDir::new().unwrap();
        // Two valid entries.
        touch(dir.path(), "good1.shp");
        touch(dir.path(), "good1.dbf");
        touch(dir.path(), "good1.shx");
        touch(dir.path(), "good2.shp");
        touch(dir.path(), "good2.dbf");
        touch(dir.path(), "good2.shx");
        // One invalid (missing .dbf).
        touch(dir.path(), "bad.shp");
        touch(dir.path(), "bad.shx");

        let report = discover(dir.path()).unwrap();
        assert_eq!(report.entries.len(), 3);
        assert_eq!(report.valid_count, 2);
        assert_eq!(report.invalid_count, 1);
    }

    #[test]
    fn test_discover_report_estimated_output_bytes_ratio() {
        let dir = TempDir::new().unwrap();
        // Write 10 bytes to each .shp so we get a predictable size.
        fs::write(dir.path().join("x.shp"), b"0123456789").unwrap();
        touch(dir.path(), "x.dbf");
        touch(dir.path(), "x.shx");

        let report = discover(dir.path()).unwrap();
        assert_eq!(report.total_input_bytes, 10);
        // 10 * 37 / 10 = 37
        assert_eq!(report.estimated_output_bytes, 37);
    }
}
