use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crossbeam_channel::{Receiver, Sender};

use crate::progress::WorkerProgress;
use crate::queue::{Job, JobResult};

/// Shared control flags for a worker thread.
///
/// All fields are cheaply cloneable via `Arc` — cloning produces a new handle
/// pointing to the same underlying state.
#[derive(Clone)]
pub struct WorkerFlags {
    /// When `true`, the worker idles in a spin-wait loop instead of pulling jobs.
    pub pause: Arc<AtomicBool>,
    /// When `true`, the worker exits after its current job completes.
    pub exit: Arc<AtomicBool>,
    /// Set of relative `.shp` paths that should be skipped (populated by `/skip`).
    pub skip_set: Arc<Mutex<HashSet<String>>>,
    /// The input root directory; used to compute relative paths for skip lookups.
    pub input_root: Arc<PathBuf>,
    /// Set to `true` once the worker's loop has exited.
    pub done: Arc<AtomicBool>,
}

impl WorkerFlags {
    /// Creates a new `WorkerFlags` with the given shared pause flag and skip set.
    ///
    /// `exit` and `done` are initialised to `false`.
    pub fn new(
        pause: Arc<AtomicBool>,
        skip_set: Arc<Mutex<HashSet<String>>>,
        input_root: Arc<PathBuf>,
    ) -> Self {
        Self {
            pause,
            exit: Arc::new(AtomicBool::new(false)),
            skip_set,
            input_root,
            done: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Returns `true` if the given `.shp` path's relative form is in the skip set.
    pub fn should_skip(&self, shp: &std::path::Path) -> bool {
        if let Ok(rel) = shp.strip_prefix(self.input_root.as_ref()) {
            let rel_str = rel.to_string_lossy();
            self.skip_set.lock().unwrap().contains(rel_str.as_ref())
        } else {
            false
        }
    }
}

/// Runs the worker loop: pulls jobs from `job_rx` and sends results on `result_tx`.
///
/// The loop:
/// 1. Spin-waits while `flags.pause` is set.
/// 2. Exits early if `flags.exit` is set.
/// 3. Receives the next job; exits if the channel is disconnected.
/// 4. Checks if the job should be skipped; sends `JobResult::Skipped` if so.
/// 5. Otherwise performs the conversion and sends the result.
///
/// Sets `flags.done` to `true` before returning.
///
/// Each call to [`crate::convert::convert`] constructs its own PROJ pipeline
/// internally — `proj::Proj` is `Send` but not `Sync`, so it must not be shared
/// across threads.
///
/// # Panics
///
/// Does not panic. Any conversion error is captured as [`JobResult::Failed`].
pub fn worker_loop(
    job_rx: Receiver<Job>,
    result_tx: Sender<JobResult>,
    wp: WorkerProgress,
    flags: WorkerFlags,
) {
    loop {
        // Pause spin-wait — idle until the pause flag is cleared.
        while flags.pause.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        // Per-worker exit signal — stop pulling jobs.
        if flags.exit.load(Ordering::Relaxed) {
            break;
        }

        // Claim next job; exit cleanly when the channel is disconnected.
        let mut job = match job_rx.recv() {
            Ok(j) => j,
            Err(_) => break,
        };

        // Skip check — user issued `/skip` for this file.
        if flags.should_skip(&job.entry.shp) {
            if result_tx
                .send(JobResult::Skipped {
                    shp: job.entry.shp.clone(),
                })
                .is_err()
            {
                break;
            }
            wp.finish_file();
            continue;
        }

        // Extract a short filename for the progress bar label.
        let filename = job
            .entry
            .shp
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| job.entry.shp.display().to_string());

        // Read record count from .dbf header (~32 bytes) for accurate progress bar.
        let num_records = shapefile::dbase::Reader::from_path(&job.entry.dbf)
            .map(|r| r.header().num_records as u64)
            .unwrap_or(0);
        wp.start_file(&filename, num_records);

        // Build a per-record callback that ticks the worker progress bar once.
        job.options.on_record = Some(wp_ref_for_record(&wp));

        let shp_path = job.entry.shp.clone();
        let result = match crate::convert::convert(&job.entry, &job.output_path, &job.options) {
            Ok(stats) => JobResult::Done(stats),
            Err(e) => JobResult::Failed {
                shp: shp_path,
                reason: e.to_string(),
            },
        };

        wp.finish_file();

        if result_tx.send(result).is_err() {
            // Main thread disconnected — stop processing.
            break;
        }
    }

    flags.done.store(true, Ordering::Relaxed);
}

/// Returns a boxed `Send` closure that ticks `wp` by one record.
///
/// `WorkerProgress::Noop` is cheap — the closure is a no-op.
/// `WorkerProgress::Live` wraps a `ProgressBar` which is `Send + Clone` (Arc internally).
fn wp_ref_for_record(wp: &WorkerProgress) -> Box<dyn Fn() + Send> {
    match wp {
        WorkerProgress::Noop => Box::new(|| {}),
        WorkerProgress::Live(bar) => {
            let bar = bar.clone();
            Box::new(move || bar.inc(1))
        }
    }
}

/// Creates a `WorkerFlags` with all flags in default (inactive) state.
///
/// Useful in tests where no pause/skip/exit behaviour is needed.
pub fn default_flags(input_root: PathBuf) -> WorkerFlags {
    WorkerFlags::new(
        Arc::new(AtomicBool::new(false)),
        Arc::new(Mutex::new(HashSet::new())),
        Arc::new(input_root),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::WorkQueue;

    /// Verify that `worker_loop` exits cleanly when the job channel is closed
    /// before any jobs are enqueued.
    #[test]
    fn test_worker_loop_exits_on_empty_closed_channel() {
        let queue = WorkQueue::new();

        // Drop the sender immediately — no jobs will arrive.
        drop(queue.job_tx);

        let flags = default_flags(PathBuf::from("/"));
        let rx = queue.job_rx;
        let tx = queue.result_tx;
        let handle = std::thread::spawn(move || worker_loop(rx, tx, WorkerProgress::Noop, flags));

        handle.join().expect("worker thread panicked");

        // result_rx should be empty — no results produced.
        assert!(queue.result_rx.try_recv().is_err());
    }

    /// Verify that a failed job produces a `JobResult::Failed` on the result channel.
    #[test]
    fn test_worker_loop_sends_failed_for_nonexistent_shp() {
        use crate::convert::ConvertOptions;
        use crate::discover::{EntryStatus, ShapefileEntry};

        let queue = WorkQueue::new();

        // Build a job pointing to a non-existent shapefile.
        let entry = ShapefileEntry {
            shp: PathBuf::from("/nonexistent/file.shp"),
            dbf: PathBuf::from("/nonexistent/file.dbf"),
            shx: PathBuf::from("/nonexistent/file.shx"),
            prj: None,
            cpg: None,
            status: EntryStatus::Valid,
        };
        let job = Job {
            output_path: PathBuf::from("/tmp/out.geojson"),
            options: ConvertOptions {
                geojsonl: false,
                overwrite: true,
                reproject_from_prj: None,
                on_record: None,
            },
            entry,
        };

        queue.job_tx.send(job).unwrap();
        drop(queue.job_tx); // Signal end of jobs.

        let flags = default_flags(PathBuf::from("/nonexistent"));
        let rx = queue.job_rx;
        // The worker takes ownership of result_tx. Once the worker finishes,
        // result_tx is dropped inside the thread, closing the result channel.
        let handle = std::thread::spawn(move || {
            worker_loop(rx, queue.result_tx, WorkerProgress::Noop, flags)
        });
        handle.join().expect("worker thread panicked");

        let result = queue.result_rx.recv().expect("expected one result");
        match result {
            JobResult::Failed { shp, reason: _ } => {
                assert_eq!(shp, PathBuf::from("/nonexistent/file.shp"));
            }
            JobResult::Done(_) => panic!("expected Failed, got Done"),
            JobResult::Skipped { .. } => panic!("expected Failed, got Skipped"),
        }
    }

    /// Verify that a job with its path in the skip set produces `JobResult::Skipped`.
    #[test]
    fn test_worker_loop_sends_skipped_for_skip_set_entry() {
        use crate::convert::ConvertOptions;
        use crate::discover::{EntryStatus, ShapefileEntry};

        let queue = WorkQueue::new();
        let input_root = PathBuf::from("/data");

        let entry = ShapefileEntry {
            shp: PathBuf::from("/data/sub/file.shp"),
            dbf: PathBuf::from("/data/sub/file.dbf"),
            shx: PathBuf::from("/data/sub/file.shx"),
            prj: None,
            cpg: None,
            status: EntryStatus::Valid,
        };
        let shp_path = entry.shp.clone();
        let job = Job {
            output_path: PathBuf::from("/tmp/out.geojson"),
            options: ConvertOptions {
                geojsonl: false,
                overwrite: true,
                reproject_from_prj: None,
                on_record: None,
            },
            entry,
        };

        queue.job_tx.send(job).unwrap();
        drop(queue.job_tx);

        let skip_set = Arc::new(Mutex::new(HashSet::new()));
        // Insert the relative path that should be skipped.
        skip_set.lock().unwrap().insert("sub/file.shp".to_string());

        let flags = WorkerFlags::new(
            Arc::new(AtomicBool::new(false)),
            Arc::clone(&skip_set),
            Arc::new(input_root),
        );

        let rx = queue.job_rx;
        let handle = std::thread::spawn(move || {
            worker_loop(rx, queue.result_tx, WorkerProgress::Noop, flags)
        });
        handle.join().expect("worker thread panicked");

        let result = queue.result_rx.recv().expect("expected one result");
        match result {
            JobResult::Skipped { shp } => {
                assert_eq!(shp, shp_path);
            }
            JobResult::Done(_) => panic!("expected Skipped, got Done"),
            JobResult::Failed { .. } => panic!("expected Skipped, got Failed"),
        }
    }

    /// Verify that `WorkerFlags::done` is set to true after the loop exits.
    #[test]
    fn test_worker_flags_done_set_after_exit() {
        let queue = WorkQueue::new();
        drop(queue.job_tx);

        let flags = default_flags(PathBuf::from("/"));
        let done_arc = Arc::clone(&flags.done);
        let rx = queue.job_rx;
        let handle = std::thread::spawn(move || {
            worker_loop(rx, queue.result_tx, WorkerProgress::Noop, flags)
        });
        handle.join().expect("worker thread panicked");

        assert!(done_arc.load(Ordering::Relaxed), "done flag should be set");
    }
}
