use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::AppError;

/// Persistent state for resumable conversion runs.
///
/// Saved atomically to `.shp2geojson_state.json` in the output root after each
/// completed job. When `--resume` is passed on a subsequent invocation, this
/// file is loaded and already-completed files are skipped.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointState {
    /// Unique identifier for this run (8 hex chars).
    pub run_id: String,
    /// ISO 8601 timestamp when the run started.
    pub started: String,
    /// Absolute path to the input root directory.
    pub input_root: PathBuf,
    /// Absolute path to the output root directory.
    pub output_root: PathBuf,
    /// Relative `.shp` paths (relative to `input_root`) that completed successfully.
    pub done: Vec<String>,
    /// Relative `.shp` paths that failed during conversion.
    pub failed: Vec<String>,
    /// Relative `.shp` paths that are still pending.
    pub pending: Vec<String>,
}

impl CheckpointState {
    /// Creates a new empty `CheckpointState` for a fresh run.
    ///
    /// # Example
    ///
    /// ```
    /// use std::path::PathBuf;
    /// use shp2geojson::checkpoint::CheckpointState;
    ///
    /// let state = CheckpointState::new(
    ///     "deadbeef".to_string(),
    ///     "2026-03-09T00:00:00Z".to_string(),
    ///     PathBuf::from("/data"),
    ///     PathBuf::from("/out"),
    /// );
    /// assert!(state.done.is_empty());
    /// assert!(state.failed.is_empty());
    /// assert!(state.pending.is_empty());
    /// ```
    pub fn new(run_id: String, started: String, input_root: PathBuf, output_root: PathBuf) -> Self {
        Self {
            run_id,
            started,
            input_root,
            output_root,
            done: Vec::new(),
            failed: Vec::new(),
            pending: Vec::new(),
        }
    }

    /// Loads a `CheckpointState` from a JSON file at `path`.
    ///
    /// Returns [`AppError::Checkpoint`] if the file cannot be read or parsed.
    pub fn load(path: &Path) -> Result<Self, AppError> {
        let bytes = std::fs::read(path).map_err(|source| AppError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        serde_json::from_slice(&bytes).map_err(|e| AppError::Checkpoint {
            reason: format!("failed to parse {}: {e}", path.display()),
        })
    }

    /// Atomically saves this `CheckpointState` to `path`.
    ///
    /// Writes to a temporary file (`{path}.tmp`) first, then renames it to the
    /// final path. This ensures the state file is never partially written.
    pub fn save(&self, path: &Path) -> Result<(), AppError> {
        let tmp = path.with_extension("json.tmp");
        let json = serde_json::to_string_pretty(self).map_err(|e| AppError::Checkpoint {
            reason: e.to_string(),
        })?;
        std::fs::write(&tmp, json.as_bytes()).map_err(|source| AppError::Io {
            path: tmp.clone(),
            source,
        })?;
        std::fs::rename(&tmp, path).map_err(|source| AppError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(())
    }

    /// Moves `relative_shp` from the `pending` list to `done`.
    ///
    /// If the entry is not found in `pending`, it is still added to `done`
    /// (idempotent for recovery scenarios).
    pub fn mark_done(&mut self, relative_shp: &str) {
        self.pending.retain(|p| p != relative_shp);
        if !self.done.contains(&relative_shp.to_string()) {
            self.done.push(relative_shp.to_string());
        }
    }

    /// Moves `relative_shp` from the `pending` list to `failed`.
    ///
    /// If the entry is not found in `pending`, it is still added to `failed`
    /// (idempotent for recovery scenarios).
    pub fn mark_failed(&mut self, relative_shp: &str) {
        self.pending.retain(|p| p != relative_shp);
        if !self.failed.contains(&relative_shp.to_string()) {
            self.failed.push(relative_shp.to_string());
        }
    }
}

/// Generates a short 8-character hex run ID from the current system time.
///
/// Uses the lower 32 bits of nanoseconds since UNIX epoch, formatted as 8
/// lowercase hex digits. Suitable as a human-readable run identifier.
pub fn generate_run_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id() as u128;
    let id_bits = ((nanos ^ (pid << 16)) & 0xFFFF_FFFF) as u32;
    format!("{:08x}", id_bits)
}

/// Returns the path of `shp` relative to `input_root` as a `String`.
///
/// Returns `None` if `shp` is not under `input_root`.
pub fn relative_shp_path(shp: &Path, input_root: &Path) -> Option<String> {
    shp.strip_prefix(input_root)
        .ok()
        .map(|p| p.to_string_lossy().to_string())
}

/// Returns the set of paths in `done` as a `HashSet<String>` for fast lookup.
pub fn done_set(state: &CheckpointState) -> HashSet<String> {
    state.done.iter().cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_state() -> CheckpointState {
        CheckpointState::new(
            "deadbeef".to_string(),
            "2026-03-09T00:00:00Z".to_string(),
            PathBuf::from("/data"),
            PathBuf::from("/out"),
        )
    }

    // ── new ───────────────────────────────────────────────────────────────────

    #[test]
    fn test_new_has_empty_lists() {
        let s = make_state();
        assert_eq!(s.run_id, "deadbeef");
        assert_eq!(s.started, "2026-03-09T00:00:00Z");
        assert_eq!(s.input_root, PathBuf::from("/data"));
        assert_eq!(s.output_root, PathBuf::from("/out"));
        assert!(s.done.is_empty());
        assert!(s.failed.is_empty());
        assert!(s.pending.is_empty());
    }

    // ── mark_done ─────────────────────────────────────────────────────────────

    #[test]
    fn test_mark_done_moves_from_pending_to_done() {
        let mut s = make_state();
        s.pending.push("foo/bar.shp".to_string());
        s.mark_done("foo/bar.shp");
        assert!(s.done.contains(&"foo/bar.shp".to_string()));
        assert!(!s.pending.contains(&"foo/bar.shp".to_string()));
    }

    #[test]
    fn test_mark_done_idempotent() {
        let mut s = make_state();
        s.mark_done("a.shp");
        s.mark_done("a.shp");
        assert_eq!(s.done.iter().filter(|d| *d == "a.shp").count(), 1);
    }

    // ── mark_failed ───────────────────────────────────────────────────────────

    #[test]
    fn test_mark_failed_moves_from_pending_to_failed() {
        let mut s = make_state();
        s.pending.push("bad.shp".to_string());
        s.mark_failed("bad.shp");
        assert!(s.failed.contains(&"bad.shp".to_string()));
        assert!(!s.pending.contains(&"bad.shp".to_string()));
    }

    #[test]
    fn test_mark_failed_idempotent() {
        let mut s = make_state();
        s.mark_failed("b.shp");
        s.mark_failed("b.shp");
        assert_eq!(s.failed.iter().filter(|f| *f == "b.shp").count(), 1);
    }

    // ── save / load round-trip ────────────────────────────────────────────────

    #[test]
    fn test_save_and_load_round_trip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(".shp2geojson_state.json");

        let mut original = make_state();
        original.pending.push("regions/europe.shp".to_string());
        original.done.push("regions/asia.shp".to_string());
        original.failed.push("regions/bad.shp".to_string());

        original.save(&path).unwrap();

        let loaded = CheckpointState::load(&path).unwrap();
        assert_eq!(loaded.run_id, original.run_id);
        assert_eq!(loaded.started, original.started);
        assert_eq!(loaded.input_root, original.input_root);
        assert_eq!(loaded.output_root, original.output_root);
        assert_eq!(loaded.done, original.done);
        assert_eq!(loaded.failed, original.failed);
        assert_eq!(loaded.pending, original.pending);
    }

    #[test]
    fn test_save_is_atomic_tmp_file_not_left_behind() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("state.json");

        let s = make_state();
        s.save(&path).unwrap();

        // The .tmp file must have been renamed away.
        let tmp = path.with_extension("json.tmp");
        assert!(!tmp.exists(), "stale tmp file found after save");
        assert!(path.exists(), "final state file missing");
    }

    #[test]
    fn test_load_returns_error_for_missing_file() {
        let result = CheckpointState::load(Path::new("/nonexistent/state.json"));
        assert!(result.is_err());
    }

    #[test]
    fn test_load_returns_error_for_invalid_json() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path, b"not valid json").unwrap();
        let result = CheckpointState::load(&path);
        assert!(result.is_err());
    }

    // ── generate_run_id ───────────────────────────────────────────────────────

    #[test]
    fn test_generate_run_id_length() {
        let id = generate_run_id();
        assert_eq!(id.len(), 8, "run_id must be exactly 8 chars, got: {id}");
    }

    #[test]
    fn test_generate_run_id_hex_chars_only() {
        let id = generate_run_id();
        assert!(
            id.chars().all(|c| c.is_ascii_hexdigit()),
            "run_id contains non-hex character: {id}"
        );
    }

    // ── relative_shp_path ─────────────────────────────────────────────────────

    #[test]
    fn test_relative_shp_path_strips_prefix() {
        let shp = Path::new("/data/regions/europe.shp");
        let root = Path::new("/data");
        assert_eq!(
            relative_shp_path(shp, root),
            Some("regions/europe.shp".to_string())
        );
    }

    #[test]
    fn test_relative_shp_path_returns_none_when_not_under_root() {
        let shp = Path::new("/other/file.shp");
        let root = Path::new("/data");
        assert_eq!(relative_shp_path(shp, root), None);
    }

    // ── done_set ──────────────────────────────────────────────────────────────

    #[test]
    fn test_done_set_contains_all_done_entries() {
        let mut s = make_state();
        s.done.push("a.shp".to_string());
        s.done.push("b/c.shp".to_string());
        let set = done_set(&s);
        assert!(set.contains("a.shp"));
        assert!(set.contains("b/c.shp"));
        assert!(!set.contains("z.shp"));
    }
}
