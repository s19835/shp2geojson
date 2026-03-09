use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::AppError;

/// Top-level config file structure for `.shp2geojson.toml`.
#[derive(Debug, Default, Deserialize)]
pub struct AppConfig {
    pub conversion: Option<ConversionConfig>,
    pub output: Option<OutputConfig>,
    pub hooks: Option<HooksConfig>,
}

/// Conversion-related settings that can be specified in the config file.
///
/// All fields are optional; CLI flags always take precedence.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionConfig {
    pub reproject: Option<bool>,
    /// Output format: `"geojson"` or `"geojsonl"`.
    pub output_format: Option<String>,
    pub overwrite: Option<bool>,
    pub jobs: Option<usize>,
}

/// Output-related settings that can be specified in the config file.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputConfig {
    pub log_file: Option<PathBuf>,
    /// Reserved; directory mirroring is always enabled. Accepted for
    /// forward-compatibility with the spec.
    pub mirror_structure: Option<bool>,
}

/// Shell hook commands fired on lifecycle events.
///
/// Each field is an optional shell command template. Template variables use
/// `{{name}}` syntax and are substituted before execution.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HooksConfig {
    /// Fired after each file is converted successfully.
    ///
    /// Available vars: `{{file}}`, `{{output}}`, `{{features}}`, `{{duration_ms}}`.
    pub on_file_complete: Option<String>,
    /// Fired after each file fails to convert.
    ///
    /// Available vars: `{{file}}`, `{{reason}}`.
    pub on_file_failed: Option<String>,
    /// Fired once after the entire batch finishes.
    ///
    /// Available vars: `{{converted}}`, `{{failed}}`, `{{elapsed_s}}`,
    /// `{{gb_processed}}`, `{{summary_json}}`.
    pub on_batch_done: Option<String>,
    /// Fired when the user pauses the run via `/pause`.
    pub on_pause: Option<String>,
}

/// Resolves the config file path: uses `--config` if provided, otherwise looks
/// for `.shp2geojson.toml` in the current directory.
///
/// Returns `(path, explicit)` where `explicit` is `true` when `--config` was used.
///
/// # Examples
///
/// ```
/// use std::path::PathBuf;
/// use shp2geojson::config::resolve_config_path;
///
/// let (path, explicit) = resolve_config_path(&None);
/// assert_eq!(path, PathBuf::from(".shp2geojson.toml"));
/// assert!(!explicit);
///
/// let cli_path = PathBuf::from("/etc/my.toml");
/// let (path, explicit) = resolve_config_path(&Some(cli_path.clone()));
/// assert_eq!(path, cli_path);
/// assert!(explicit);
/// ```
pub fn resolve_config_path(cli_config: &Option<PathBuf>) -> (PathBuf, bool) {
    match cli_config {
        Some(p) => (p.clone(), true),
        None => (PathBuf::from(".shp2geojson.toml"), false),
    }
}

/// Loads and parses the config file.
///
/// If `explicit` is `true` (user passed `--config`), a missing file is an error.
/// If `false` (default path), a missing file returns [`AppConfig::default()`].
///
/// # Errors
///
/// Returns [`AppError::Config`] if:
/// - `explicit` is `true` and the file does not exist, or
/// - the file cannot be read for any reason, or
/// - the TOML content fails to parse.
pub fn load_config(path: &Path, explicit: bool) -> Result<AppConfig, AppError> {
    match std::fs::read_to_string(path) {
        Ok(contents) => toml::from_str(&contents).map_err(|e| AppError::Config {
            reason: format!("failed to parse {}: {e}", path.display()),
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !explicit => Ok(AppConfig::default()),
        Err(e) => Err(AppError::Config {
            reason: format!("cannot read {}: {e}", path.display()),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    // ── resolve_config_path ───────────────────────────────────────────────────

    #[test]
    fn test_resolve_config_path_default() {
        let (path, explicit) = resolve_config_path(&None);
        assert_eq!(path, PathBuf::from(".shp2geojson.toml"));
        assert!(!explicit);
    }

    #[test]
    fn test_resolve_config_path_with_cli_flag() {
        let cli_path = PathBuf::from("/custom/path/config.toml");
        let (path, explicit) = resolve_config_path(&Some(cli_path.clone()));
        assert_eq!(path, cli_path);
        assert!(explicit);
    }

    // ── load_config ───────────────────────────────────────────────────────────

    #[test]
    fn test_load_config_missing_file_not_explicit_returns_default() {
        let path = PathBuf::from("/nonexistent/path/config.toml");
        let result = load_config(&path, false);
        assert!(result.is_ok());
        let cfg = result.unwrap();
        assert!(cfg.conversion.is_none());
        assert!(cfg.output.is_none());
        assert!(cfg.hooks.is_none());
    }

    #[test]
    fn test_load_config_missing_file_explicit_returns_error() {
        let path = PathBuf::from("/nonexistent/path/config.toml");
        let result = load_config(&path, true);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("config error"));
    }

    #[test]
    fn test_load_config_valid_toml_parses() {
        let mut tmp = NamedTempFile::new().unwrap();
        let toml_content = r#"
[conversion]
reproject = true
output_format = "geojsonl"
overwrite = false
jobs = 4

[output]
log_file = "/tmp/errors.log"

[hooks]
on_file_complete = "echo done {{file}}"
on_file_failed = "echo failed {{file}}"
on_batch_done = "echo batch done"
"#;
        write!(tmp, "{toml_content}").unwrap();

        let result = load_config(tmp.path(), true);
        assert!(result.is_ok(), "parse error: {:?}", result.unwrap_err());
        let cfg = result.unwrap();

        let conv = cfg.conversion.as_ref().unwrap();
        assert_eq!(conv.reproject, Some(true));
        assert_eq!(conv.output_format.as_deref(), Some("geojsonl"));
        assert_eq!(conv.overwrite, Some(false));
        assert_eq!(conv.jobs, Some(4));

        let out = cfg.output.as_ref().unwrap();
        assert_eq!(out.log_file, Some(PathBuf::from("/tmp/errors.log")));

        let hooks = cfg.hooks.as_ref().unwrap();
        assert_eq!(
            hooks.on_file_complete.as_deref(),
            Some("echo done {{file}}")
        );
        assert_eq!(
            hooks.on_file_failed.as_deref(),
            Some("echo failed {{file}}")
        );
        assert_eq!(hooks.on_batch_done.as_deref(), Some("echo batch done"));
    }

    #[test]
    fn test_load_config_invalid_toml_returns_error() {
        let mut tmp = NamedTempFile::new().unwrap();
        write!(tmp, "this is [ not valid toml = !!").unwrap();

        let result = load_config(tmp.path(), true);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("config error"));
    }

    #[test]
    fn test_load_config_unknown_field_in_section_errors() {
        let mut tmp = NamedTempFile::new().unwrap();
        write!(
            tmp,
            r#"
[conversion]
unknown_field = "oops"
"#
        )
        .unwrap();

        let result = load_config(tmp.path(), true);
        assert!(
            result.is_err(),
            "expected error for unknown field, got: {:?}",
            result.unwrap()
        );
    }

    #[test]
    fn test_load_config_all_fields_optional() {
        let mut tmp = NamedTempFile::new().unwrap();
        // Empty file — all sections are absent.
        write!(tmp, "").unwrap();

        let result = load_config(tmp.path(), true);
        assert!(result.is_ok());
        let cfg = result.unwrap();
        assert!(cfg.conversion.is_none());
        assert!(cfg.output.is_none());
        assert!(cfg.hooks.is_none());
    }
}
