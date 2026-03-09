use std::fmt;

use console::Style;
use serde::Serialize;

use crate::cli::OutputFormat;
use crate::progress::Progress;

// ── Mockup color palette ────────────────────────────────────────────────────
// These match the CSS variables in shp2geojson-mockup.html:
//   coral (#ff7c5c), green (#56d18a), red (#f06b6b), yellow (#f0c040),
//   blue (#5b8fff), cyan (#4ecdc4), purple (#a78bfa), dim (#6b7499)

fn s_coral() -> Style {
    Style::new().color256(209).bold()
}
fn s_green() -> Style {
    Style::new().color256(78)
}
fn s_red() -> Style {
    Style::new().color256(203)
}
fn s_yellow() -> Style {
    Style::new().color256(220)
}
fn s_blue() -> Style {
    Style::new().color256(69)
}
fn s_cyan() -> Style {
    Style::new().color256(80)
}
fn s_purple() -> Style {
    Style::new().color256(141)
}
fn s_dim() -> Style {
    Style::new().color256(103)
}
fn s_muted() -> Style {
    Style::new().color256(60)
}
fn s_text() -> Style {
    Style::new().color256(189)
}

/// The horizontal divider line matching the mockup.
pub const DIVIDER: &str =
    "────────────────────────────────────────────────────────────────────────";

/// A lifecycle event emitted during a conversion run.
///
/// In JSON mode (`--output-format json`) events are serialised as
/// newline-delimited JSON objects to **stdout**.  In human mode they are
/// formatted for readability and written to **stderr**.
///
/// The `event` field is the serde tag value (snake_case variant name).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum OutputEvent {
    /// Emitted once before any conversion work begins.
    Start {
        /// Total number of shapefile entries discovered (valid + invalid).
        total_files: usize,
        /// Sum of `.shp` file sizes for valid entries (bytes).
        total_bytes: u64,
        /// ISO 8601 UTC timestamp when the run started.
        timestamp: String,
    },
    /// Emitted after a file is converted successfully.
    FileDone {
        /// Path to the source `.shp` file.
        file: String,
        /// Path to the output GeoJSON / GeoJSONL file.
        output: String,
        /// Wall-clock time taken to convert this file (milliseconds).
        duration_ms: u64,
        /// Number of GeoJSON features written.
        features: u64,
    },
    /// Emitted when a file fails to convert (invalid sidecars or read errors).
    FileFailed {
        /// Path to the source `.shp` file.
        file: String,
        /// Human-readable description of why conversion failed.
        reason: String,
    },
    /// Emitted when a file is skipped because it was already completed in a prior run.
    FileSkipped {
        /// Path to the source `.shp` file.
        file: String,
        /// Human-readable reason for skipping (e.g., "already completed in previous run").
        reason: String,
    },
    /// Emitted once after all files have been processed.
    BatchDone {
        /// Number of files converted successfully.
        converted: u64,
        /// Number of files that failed.
        failed: u64,
        /// Wall-clock time for the entire batch (seconds).
        elapsed_s: f64,
        /// Total gigabytes of `.shp` data processed.
        gb_processed: f64,
    },
    /// Emitted when the user pauses the run via `/pause`.
    Paused {
        /// Number of files converted so far.
        converted: u64,
        /// Number of files failed so far.
        failed: u64,
        /// Number of files still pending in the queue.
        pending: usize,
    },
    /// Emitted when the user resumes the run via `/resume`.
    Resumed,
    /// Emitted when the user changes the worker count via `/workers N`.
    WorkersChanged {
        /// Previous active worker count.
        from: usize,
        /// New requested worker count.
        to: usize,
    },
    /// Emitted when a file is skipped because the user issued `/skip`.
    FileSkippedByUser {
        /// Relative path to the `.shp` file that was skipped.
        file: String,
    },
}

impl fmt::Display for OutputEvent {
    /// Plain-text Display (used in non-TTY / tests). Color version is in [`format_styled`].
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OutputEvent::Start {
                total_files,
                total_bytes,
                timestamp,
            } => {
                write!(
                    f,
                    "Starting conversion: {total_files} file(s), {} input, {timestamp}",
                    format_bytes(*total_bytes),
                )
            }
            OutputEvent::FileDone {
                file,
                output,
                duration_ms,
                features,
            } => {
                write!(
                    f,
                    "  ✓ converted  {file} → {output}  ({features} features, {duration_ms}ms)",
                )
            }
            OutputEvent::FileFailed { file, reason } => {
                write!(f, "  ✗ FAILED  {file} — {reason}")
            }
            OutputEvent::FileSkipped { file, reason } => {
                write!(f, "  ⚠ SKIPPED  {file} — {reason}")
            }
            OutputEvent::BatchDone {
                converted,
                failed,
                elapsed_s,
                gb_processed,
            } => {
                write!(
                    f,
                    "\nDone: {converted} converted, {failed} failed, {elapsed_s:.1}s elapsed, {gb_processed:.3} GB processed",
                )
            }
            OutputEvent::Paused {
                converted,
                failed,
                pending,
            } => {
                write!(
                    f,
                    "PAUSED — {converted} done, {failed} failed, {pending} pending",
                )
            }
            OutputEvent::Resumed => {
                write!(f, "RESUMED — workers are running")
            }
            OutputEvent::WorkersChanged { from, to } => {
                write!(f, "Workers: {from} → {to}")
            }
            OutputEvent::FileSkippedByUser { file } => {
                write!(f, "  ⚠ SKIPPED (user)  {file}")
            }
        }
    }
}

/// Formats an [`OutputEvent`] with ANSI colors matching the mockup palette.
///
/// Used only when stderr is a TTY in human mode.
pub fn format_styled(event: &OutputEvent) -> String {
    match event {
        OutputEvent::Start {
            total_files,
            total_bytes,
            timestamp,
        } => {
            format!(
                "{}\n{}\n  {} {} file(s)   {} {}   {} {}",
                s_muted().apply_to(DIVIDER),
                s_coral().apply_to(format_args!("shp2geojson  {}", env!("CARGO_PKG_VERSION"))),
                s_muted().apply_to("files:"),
                s_text().apply_to(total_files),
                s_muted().apply_to("input:"),
                s_cyan().apply_to(format_bytes(*total_bytes)),
                s_muted().apply_to("started:"),
                s_dim().apply_to(timestamp),
            )
        }
        OutputEvent::FileDone {
            file,
            output,
            duration_ms,
            features,
        } => {
            // Extract just filenames for cleaner display
            let shp_name = std::path::Path::new(file)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| file.clone());
            let out_name = std::path::Path::new(output)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| output.clone());
            format!(
                "  {} {}  {}  {}  {} {} {}",
                s_green().apply_to("✓"),
                s_text().apply_to(&shp_name),
                s_dim().apply_to("→"),
                s_cyan().apply_to(&out_name),
                s_dim().apply_to(format_args!("({features} features,")),
                s_dim().apply_to(format_args!("{duration_ms}ms")),
                s_dim().apply_to(")"),
            )
        }
        OutputEvent::FileFailed { file, reason } => {
            let shp_name = std::path::Path::new(file)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| file.clone());
            format!(
                "  {} {}  {}  {}",
                s_red().apply_to("✗ FAILED"),
                s_text().apply_to(&shp_name),
                s_dim().apply_to("—"),
                s_red().apply_to(reason),
            )
        }
        OutputEvent::FileSkipped { file, reason } => {
            let shp_name = std::path::Path::new(file)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| file.clone());
            format!(
                "  {} {}  {}  {}",
                s_yellow().apply_to("⚠ SKIPPED"),
                s_text().apply_to(&shp_name),
                s_dim().apply_to("—"),
                s_yellow().apply_to(reason),
            )
        }
        OutputEvent::BatchDone {
            converted,
            failed,
            elapsed_s,
            gb_processed,
        } => {
            format!(
                "{}\n  {} {}   {} {}   {} {}   {} {}\n{}",
                s_muted().apply_to(DIVIDER),
                s_green().apply_to("✓ converted"),
                s_green().bold().apply_to(converted),
                s_red().apply_to("✗ failed"),
                s_red().bold().apply_to(failed),
                s_purple().apply_to("elapsed"),
                s_purple().bold().apply_to(format_args!("{elapsed_s:.1}s")),
                s_purple().apply_to("processed"),
                s_purple()
                    .bold()
                    .apply_to(format_args!("{gb_processed:.3} GB")),
                s_muted().apply_to(DIVIDER),
            )
        }
        OutputEvent::Paused {
            converted,
            failed,
            pending,
        } => {
            format!(
                "  {} {} {} done  {} failed  {} pending",
                s_yellow().bold().apply_to("⏸ PAUSED"),
                s_dim().apply_to("—"),
                s_green().apply_to(converted),
                s_red().apply_to(failed),
                s_cyan().apply_to(pending),
            )
        }
        OutputEvent::Resumed => {
            format!(
                "  {} workers are running",
                s_green().bold().apply_to("▶ RESUMED —"),
            )
        }
        OutputEvent::WorkersChanged { from, to } => {
            format!(
                "  {} {} {} {}",
                s_blue().apply_to("Workers:"),
                s_dim().apply_to(from),
                s_cyan().apply_to("→"),
                s_cyan().bold().apply_to(to),
            )
        }
        OutputEvent::FileSkippedByUser { file } => {
            format!(
                "  {} {}",
                s_yellow().apply_to("⚠ SKIPPED (user)"),
                s_text().apply_to(file),
            )
        }
    }
}

/// Emits an [`OutputEvent`] to the appropriate output stream.
///
/// - [`OutputFormat::Json`]: serialises to JSON and prints to **stdout**.
/// - [`OutputFormat::Human`]: formats for humans and prints to **stderr**.
///   When a live TUI is active, routes through `MultiProgress::println()` to
///   avoid corrupting the progress bar layout.
///
/// This is the single point responsible for the stdout/stderr routing decision.
/// Callers must never write directly to stdout or stderr for event output.
pub fn emit(event: &OutputEvent, format: &OutputFormat, progress: &Progress) {
    match format {
        OutputFormat::Json => {
            let json = serde_json::to_string(event).unwrap_or_else(|e| {
                format!("{{\"event\":\"error\",\"reason\":\"serialisation failed: {e}\"}}")
            });
            println!("{json}");
        }
        OutputFormat::Human => {
            let is_tty = console::Term::stderr().is_term();
            let msg = if is_tty {
                format_styled(event)
            } else {
                format!("{event}")
            };
            match progress {
                Progress::Live { mp, .. } => {
                    let _ = mp.println(&msg);
                }
                Progress::Noop => {
                    eprintln!("{msg}");
                }
            }
        }
    }
}

/// Formats a byte count as a human-readable string (B / KB / MB / GB).
pub fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1_024;
    const MB: u64 = 1_024 * KB;
    const GB: u64 = 1_024 * MB;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Display formatting ────────────────────────────────────────────────────

    #[test]
    fn test_display_start() {
        let event = OutputEvent::Start {
            total_files: 10,
            total_bytes: 1_073_741_824, // 1 GB
            timestamp: "2026-03-09T00:00:00Z".to_string(),
        };
        let s = event.to_string();
        assert!(s.contains("10 file(s)"));
        assert!(s.contains("1.0 GB"));
        assert!(s.contains("2026-03-09T00:00:00Z"));
    }

    #[test]
    fn test_display_file_done() {
        let event = OutputEvent::FileDone {
            file: "/data/a.shp".to_string(),
            output: "/out/a.geojson".to_string(),
            duration_ms: 42,
            features: 100,
        };
        let s = event.to_string();
        assert!(s.contains("/data/a.shp"));
        assert!(s.contains("/out/a.geojson"));
        assert!(s.contains("100 features"));
        assert!(s.contains("42ms"));
    }

    #[test]
    fn test_display_file_failed() {
        let event = OutputEvent::FileFailed {
            file: "/data/b.shp".to_string(),
            reason: "missing .dbf".to_string(),
        };
        let s = event.to_string();
        assert!(s.contains("FAILED"));
        assert!(s.contains("/data/b.shp"));
        assert!(s.contains("missing .dbf"));
    }

    #[test]
    fn test_display_file_skipped() {
        let event = OutputEvent::FileSkipped {
            file: "/data/c.shp".to_string(),
            reason: "already completed in previous run".to_string(),
        };
        let s = event.to_string();
        assert!(s.contains("SKIPPED"));
        assert!(s.contains("/data/c.shp"));
        assert!(s.contains("already completed in previous run"));
    }

    #[test]
    fn test_display_batch_done() {
        let event = OutputEvent::BatchDone {
            converted: 8,
            failed: 2,
            elapsed_s: 3.5,
            gb_processed: 0.123,
        };
        let s = event.to_string();
        assert!(s.contains("8 converted"));
        assert!(s.contains("2 failed"));
        assert!(s.contains("3.5s"));
    }

    // ── JSON serialisation ────────────────────────────────────────────────────

    #[test]
    fn test_json_start_event_tag() {
        let event = OutputEvent::Start {
            total_files: 5,
            total_bytes: 0,
            timestamp: "2026-03-09T00:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""event":"start""#));
        assert!(json.contains(r#""total_files":5"#));
    }

    #[test]
    fn test_json_file_done_event_tag() {
        let event = OutputEvent::FileDone {
            file: "x.shp".to_string(),
            output: "x.geojson".to_string(),
            duration_ms: 1,
            features: 0,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""event":"file_done""#));
    }

    #[test]
    fn test_json_file_failed_event_tag() {
        let event = OutputEvent::FileFailed {
            file: "y.shp".to_string(),
            reason: "oops".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""event":"file_failed""#));
    }

    #[test]
    fn test_json_file_skipped_event_tag() {
        let event = OutputEvent::FileSkipped {
            file: "z.shp".to_string(),
            reason: "already done".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""event":"file_skipped""#));
    }

    #[test]
    fn test_json_batch_done_event_tag() {
        let event = OutputEvent::BatchDone {
            converted: 1,
            failed: 0,
            elapsed_s: 0.1,
            gb_processed: 0.0,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""event":"batch_done""#));
    }

    // ── format_bytes ─────────────────────────────────────────────────────────

    #[test]
    fn test_format_bytes_small() {
        assert_eq!(format_bytes(500), "500 B");
    }

    #[test]
    fn test_format_bytes_kb() {
        assert_eq!(format_bytes(2_048), "2.0 KB");
    }

    #[test]
    fn test_format_bytes_mb() {
        assert_eq!(format_bytes(5 * 1_024 * 1_024), "5.0 MB");
    }

    #[test]
    fn test_format_bytes_gb() {
        assert_eq!(format_bytes(2 * 1_024 * 1_024 * 1_024), "2.0 GB");
    }

    #[test]
    fn test_format_bytes_zero() {
        assert_eq!(format_bytes(0), "0 B");
    }
}
