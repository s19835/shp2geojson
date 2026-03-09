use std::sync::Arc;

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

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
    /// # Example
    ///
    /// ```
    /// use shp2geojson::progress::Progress;
    /// let p = Progress::new(true, 10);
    /// // Returns Noop when not a TTY (e.g. in tests).
    /// ```
    pub fn new(is_human: bool, total_files: u64) -> Self {
        if !is_human || !console::Term::stderr().is_term() {
            return Self::Noop;
        }

        let mp = Arc::new(MultiProgress::new());
        let overall = mp.add(ProgressBar::new(total_files));
        overall.set_style(
            ProgressStyle::with_template(
                "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} files ({eta} remaining)"
            )
            .unwrap()
            .progress_chars("█▓░"),
        );
        overall.enable_steady_tick(std::time::Duration::from_millis(100));

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
    pub fn add_worker_bar(&self, worker_index: usize) -> WorkerProgress {
        match self {
            Self::Noop => WorkerProgress::Noop,
            Self::Live { mp, .. } => {
                let bar = mp.add(ProgressBar::new(0));
                bar.set_style(
                    ProgressStyle::with_template(
                        &format!("  W{worker_index}: {{spinner:.yellow}} {{msg:.dim}} [{{bar:30.green/black}}] {{pos}}/{{len}} records")
                    )
                    .unwrap()
                    .progress_chars("━╸─"),
                );
                bar.set_message("idle");
                bar.enable_steady_tick(std::time::Duration::from_millis(100));
                WorkerProgress::Live(bar)
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
        if let Self::Live { overall, .. } = self {
            overall.finish_with_message("done");
        }
    }

    /// Returns true if this is a Live progress (for suppressing human emit).
    pub fn is_live(&self) -> bool {
        matches!(self, Self::Live { .. })
    }
}

/// Per-worker progress bar handle. Cheap to clone (Arc internally in indicatif).
pub enum WorkerProgress {
    Noop,
    Live(ProgressBar),
}

impl WorkerProgress {
    /// Called when a worker starts a new file. Sets the bar length and message.
    pub fn start_file(&self, filename: &str, total_records: u64) {
        if let Self::Live(bar) = self {
            bar.set_length(total_records);
            bar.set_position(0);
            bar.set_message(filename.to_string());
        }
    }

    /// Called per-record to tick progress.
    pub fn inc(&self) {
        if let Self::Live(bar) = self {
            bar.inc(1);
        }
    }

    /// Called when a file is done. Resets bar to idle.
    ///
    /// Uses `reset()` instead of `finish_and_clear()` so the bar remains in
    /// `InProgress` status and can be reused for the next file.
    pub fn finish_file(&self) {
        if let Self::Live(bar) = self {
            bar.reset();
            bar.set_length(0);
            bar.set_message("idle");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_progress_new_returns_noop_when_not_human() {
        let p = Progress::new(false, 10);
        assert!(!p.is_live());
    }

    #[test]
    fn test_progress_new_returns_noop_on_non_tty() {
        // In test environments stderr is not a TTY, so is_human=true still gives Noop.
        let p = Progress::new(true, 10);
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
