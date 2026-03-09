use std::sync::Arc;

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

// ANSI color codes matching the mockup palette
const C_CORAL: &str = "\x1b[38;5;209;1m"; // coral bold — app name
const C_GREEN: &str = "\x1b[38;5;78m"; // green — success/active
const C_BLUE: &str = "\x1b[38;5;69m"; // blue — worker ids
const _C_CYAN: &str = "\x1b[38;5;80m"; // cyan — values (reserved)
const C_YELLOW: &str = "\x1b[38;5;220m"; // yellow — ETA/warnings
const C_MUTED: &str = "\x1b[38;5;60m"; // muted — labels/dividers
const C_DIM: &str = "\x1b[38;5;103m"; // dim — secondary
const C_RESET: &str = "\x1b[0m";

const DIVIDER: &str = "────────────────────────────────────────────────────────────────────────";

/// Top-level progress handle. `Noop` when output is JSON or stderr is not a TTY.
pub enum Progress {
    Noop,
    Live {
        mp: Arc<MultiProgress>,
        overall: ProgressBar,
    },
}

impl Progress {
    /// Create a `Live` progress if stderr is a TTY and format is Human; otherwise `Noop`.
    ///
    /// Prints the startup banner and section labels as part of the live TUI layout.
    ///
    /// # Example
    ///
    /// ```
    /// use shp2geojson::progress::Progress;
    /// let p = Progress::new(true, 10, 4, false);
    /// // Returns Noop when not a TTY (e.g. in tests).
    /// ```
    pub fn new(is_human: bool, total_files: u64, worker_count: usize, resume: bool) -> Self {
        if !is_human || !console::Term::stderr().is_term() {
            return Self::Noop;
        }

        let mp = Arc::new(MultiProgress::new());

        // ── Banner ──────────────────────────────────────────────────────────
        let resume_str = if resume {
            format!("{C_GREEN}✓{C_RESET}")
        } else {
            format!("{C_DIM}–{C_RESET}")
        };
        let _ = mp.println(format!("{C_MUTED}{DIVIDER}{C_RESET}"));
        let _ = mp.println(format!(
            "{C_CORAL}shp2geojson{C_RESET}  {C_DIM}v{}{C_RESET}   {C_MUTED}workers:{C_RESET} {C_BLUE}{worker_count}{C_RESET}   {C_MUTED}resume:{C_RESET} {resume_str}",
            env!("CARGO_PKG_VERSION"),
        ));
        let _ = mp.println(format!("{C_MUTED}{DIVIDER}{C_RESET}"));
        let _ = mp.println("");

        // ── "Overall" section label ─────────────────────────────────────────
        let _ = mp.println(format!("  {C_MUTED}Overall{C_RESET}"));

        // ── Overall progress bar ────────────────────────────────────────────
        let overall = mp.add(ProgressBar::new(total_files));
        overall.set_style(
            ProgressStyle::with_template(
                &format!(
                    "  {{spinner:.green}} {C_DIM}[{{elapsed_precise}}]{C_RESET} [{{bar:40.cyan/blue}}] {{pos}}/{{len}} files  {{msg}}  {C_YELLOW}({{eta}} remaining){C_RESET}"
                )
            )
            .unwrap()
            .progress_chars("━╸─"),
        );
        overall.set_message("");
        overall.enable_steady_tick(std::time::Duration::from_millis(80));

        // ── "Workers" section label (as a static bar) ───────────────────────
        let spacer = mp.add(ProgressBar::new(0));
        spacer.set_style(
            ProgressStyle::with_template(&format!("\n  {C_MUTED}Workers{C_RESET}")).unwrap(),
        );
        spacer.tick();

        Self::Live { mp, overall }
    }

    /// Returns an `Arc<MultiProgress>` for routing output through when live, or `None` when Noop.
    pub fn multi_progress(&self) -> Option<Arc<MultiProgress>> {
        match self {
            Self::Live { mp, .. } => Some(Arc::clone(mp)),
            Self::Noop => None,
        }
    }

    /// Prints a message above the progress bars without corrupting the layout.
    ///
    /// When live, routes through `MultiProgress::println`. When Noop, writes to stderr.
    pub fn println(&self, msg: impl AsRef<str>) {
        match self {
            Self::Live { mp, .. } => {
                let _ = mp.println(msg.as_ref());
            }
            Self::Noop => {
                eprintln!("{}", msg.as_ref());
            }
        }
    }

    /// Create a per-worker progress bar (returned for use in worker threads).
    ///
    /// Each worker bar shows: `Worker N  ⠋ filename.shp  [━━━━━━━━━━╸───] 63/100  ▲ active`
    /// When idle: `Worker N  · (idle — waiting for job)`
    pub fn add_worker_bar(&self, worker_index: usize) -> WorkerProgress {
        match self {
            Self::Noop => WorkerProgress::Noop,
            Self::Live { mp, .. } => {
                let bar = mp.add(ProgressBar::new(0));
                bar.set_style(idle_style(worker_index));
                bar.set_message("(idle — waiting for job)");
                bar.enable_steady_tick(std::time::Duration::from_millis(80));
                WorkerProgress::Live(WorkerBar {
                    bar,
                    index: worker_index,
                })
            }
        }
    }

    /// Increment the overall file counter by 1.
    pub fn inc_overall(&self) {
        if let Self::Live { overall, .. } = self {
            overall.inc(1);
        }
    }

    /// Mark overall progress as finished.
    pub fn finish(&self) {
        if let Self::Live { overall, mp, .. } = self {
            overall.finish_with_message(format!("{C_GREEN}done ✓{C_RESET}"));
            let _ = mp.println("");
            let _ = mp.println(format!("{C_MUTED}{DIVIDER}{C_RESET}"));
        }
    }

    /// Returns true if this is a Live progress (for suppressing human emit).
    pub fn is_live(&self) -> bool {
        matches!(self, Self::Live { .. })
    }
}

/// The active-file style for a worker bar.
fn active_style(worker_index: usize) -> ProgressStyle {
    ProgressStyle::with_template(&format!(
        "    {C_MUTED}Worker{C_RESET} {C_BLUE}{worker_index}{C_RESET}  {{spinner:.yellow}} {{msg}} [{{bar:25.green/black}}] {{pos}}/{{len}}  {C_GREEN}▲ active{C_RESET}"
    ))
    .unwrap()
    .progress_chars("━╸─")
}

/// The idle style for a worker bar.
fn idle_style(worker_index: usize) -> ProgressStyle {
    ProgressStyle::with_template(&format!(
        "    {C_MUTED}Worker{C_RESET} {C_BLUE}{worker_index}{C_RESET}  {C_DIM}{{msg}}{C_RESET}"
    ))
    .unwrap()
}

/// Per-worker progress bar handle. Cheap to clone (Arc internally in indicatif).
pub enum WorkerProgress {
    Noop,
    Live(WorkerBar),
}

/// Inner state for a live worker progress bar.
pub struct WorkerBar {
    bar: ProgressBar,
    index: usize,
}

impl WorkerBar {
    /// Returns a clone of the inner `ProgressBar` for use in per-record callbacks.
    pub fn progress_bar(&self) -> ProgressBar {
        self.bar.clone()
    }
}

impl WorkerProgress {
    /// Called when a worker starts a new file. Switches to the active style and sets bar length.
    pub fn start_file(&self, filename: &str, total_records: u64) {
        if let Self::Live(wb) = self {
            wb.bar.set_style(active_style(wb.index));
            wb.bar.set_length(total_records);
            wb.bar.set_position(0);
            wb.bar.set_message(filename.to_string());
        }
    }

    /// Called per-record to tick progress.
    pub fn inc(&self) {
        if let Self::Live(wb) = self {
            wb.bar.inc(1);
        }
    }

    /// Called when a file is done. Switches back to the idle style.
    ///
    /// Uses `reset()` instead of `finish_and_clear()` so the bar remains in
    /// `InProgress` status and can be reused for the next file.
    pub fn finish_file(&self) {
        if let Self::Live(wb) = self {
            wb.bar.reset();
            wb.bar.set_style(idle_style(wb.index));
            wb.bar.set_length(0);
            wb.bar.set_message("(idle — waiting for job)");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_progress_new_returns_noop_when_not_human() {
        let p = Progress::new(false, 10, 4, false);
        assert!(!p.is_live());
    }

    #[test]
    fn test_progress_new_returns_noop_on_non_tty() {
        // In test environments stderr is not a TTY, so is_human=true still gives Noop.
        let p = Progress::new(true, 10, 4, false);
        assert!(!p.is_live());
    }

    #[test]
    fn test_noop_add_worker_bar_returns_noop() {
        let p = Progress::Noop;
        let wp = p.add_worker_bar(0);
        assert!(matches!(wp, WorkerProgress::Noop));
    }

    #[test]
    fn test_noop_inc_overall_does_not_panic() {
        let p = Progress::Noop;
        p.inc_overall();
    }

    #[test]
    fn test_noop_finish_does_not_panic() {
        let p = Progress::Noop;
        p.finish();
    }

    #[test]
    fn test_worker_progress_noop_start_file_does_not_panic() {
        let wp = WorkerProgress::Noop;
        wp.start_file("test.shp", 100);
    }

    #[test]
    fn test_worker_progress_noop_inc_does_not_panic() {
        let wp = WorkerProgress::Noop;
        wp.inc();
    }

    #[test]
    fn test_worker_progress_noop_finish_file_does_not_panic() {
        let wp = WorkerProgress::Noop;
        wp.finish_file();
    }
}
