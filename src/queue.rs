use std::path::PathBuf;

use crossbeam_channel::{Receiver, Sender};

use crate::convert::{ConversionStats, ConvertOptions};
use crate::discover::ShapefileEntry;

/// A single unit of work dispatched to a worker thread.
///
/// Contains everything the worker needs to perform a conversion without
/// holding references back to the main thread.
pub struct Job {
    /// The validated shapefile entry to convert.
    pub entry: ShapefileEntry,
    /// Where the output GeoJSON / GeoJSONL file should be written.
    pub output_path: PathBuf,
    /// Conversion options (format, overwrite, reprojection).
    pub options: ConvertOptions,
}

/// The outcome of a single job, sent back to the main thread over the result channel.
pub enum JobResult {
    /// The conversion succeeded; carries per-file statistics.
    Done(ConversionStats),
    /// The conversion failed; carries the source path and a human-readable reason.
    Failed {
        /// Path to the `.shp` file that could not be converted.
        shp: PathBuf,
        /// Human-readable description of the failure.
        reason: String,
    },
    /// The file was skipped because the user requested it via `/skip`.
    Skipped {
        /// Path to the `.shp` file that was skipped.
        shp: PathBuf,
    },
}

/// Holds the crossbeam channel pairs used for job dispatch and result collection.
///
/// After construction:
/// - Enqueue jobs on `job_tx`, then drop it to signal completion.
/// - Spawn workers that consume `job_rx` and send results on `result_tx`.
/// - Drop `result_tx` and `job_rx` on the main thread after spawning workers.
/// - Drain `result_rx` on the main thread until the channel is empty.
pub struct WorkQueue {
    /// Send end of the job channel — enqueue [`Job`]s here.
    pub job_tx: Sender<Job>,
    /// Receive end of the job channel — workers pull from this.
    pub job_rx: Receiver<Job>,
    /// Send end of the result channel — workers push [`JobResult`]s here.
    pub result_tx: Sender<JobResult>,
    /// Receive end of the result channel — main thread drains this.
    pub result_rx: Receiver<JobResult>,
}

impl WorkQueue {
    /// Creates a new `WorkQueue` with two unbounded crossbeam channels.
    pub fn new() -> Self {
        let (job_tx, job_rx) = crossbeam_channel::unbounded();
        let (result_tx, result_rx) = crossbeam_channel::unbounded();
        Self {
            job_tx,
            job_rx,
            result_tx,
            result_rx,
        }
    }
}

impl Default for WorkQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_work_queue_new_channels_are_empty() {
        let queue = WorkQueue::new();
        // Channels start empty — try_recv returns Err(Empty).
        assert!(queue.job_rx.try_recv().is_err());
        assert!(queue.result_rx.try_recv().is_err());
    }

    #[test]
    fn test_work_queue_default_equals_new() {
        // Default must not panic; channels must be usable.
        let queue = WorkQueue::default();
        assert!(queue.job_rx.try_recv().is_err());
        assert!(queue.result_rx.try_recv().is_err());
    }

    #[test]
    fn test_job_result_failed_carries_shp_and_reason() {
        let result = JobResult::Failed {
            shp: PathBuf::from("/data/bad.shp"),
            reason: "missing .dbf".to_string(),
        };
        match result {
            JobResult::Failed { shp, reason } => {
                assert_eq!(shp, PathBuf::from("/data/bad.shp"));
                assert_eq!(reason, "missing .dbf");
            }
            JobResult::Done(_) => panic!("expected Failed"),
            JobResult::Skipped { .. } => panic!("expected Failed"),
        }
    }
}
