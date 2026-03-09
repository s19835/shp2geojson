use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clap::Parser;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use shp2geojson::checkpoint::{done_set, generate_run_id, relative_shp_path, CheckpointState};
use shp2geojson::cli::Cli;
use shp2geojson::convert::{output_path_for, ConvertOptions};
use shp2geojson::discover::{discover, EntryStatus};
use shp2geojson::interactive::PauseFlag;
use shp2geojson::output::{emit, format_bytes, OutputEvent};
use shp2geojson::progress::Progress;
use shp2geojson::queue::{Job, JobResult, WorkQueue};
use shp2geojson::worker::{worker_loop, WorkerFlags};
use shp2geojson::{config, hooks};

// ── Tracing MakeWriter that routes through MultiProgress ────────────────────

/// Shared slot populated once the TUI `MultiProgress` is created.
type MpSlot = Arc<Mutex<Option<Arc<indicatif::MultiProgress>>>>;

/// Factory for [`MpWriter`] instances — implements `tracing_subscriber::fmt::MakeWriter`.
#[derive(Clone)]
struct ProgressMakeWriter {
    mp_slot: MpSlot,
}

/// Per-event writer that buffers output and, on drop, routes through
/// `MultiProgress::println` (if live) or falls back to stderr.
struct MpWriter {
    mp_slot: MpSlot,
    buf: Vec<u8>,
}

impl Write for MpWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for MpWriter {
    fn drop(&mut self) {
        if self.buf.is_empty() {
            return;
        }
        let msg = String::from_utf8_lossy(&self.buf);
        let msg = msg.trim_end();
        let guard = self.mp_slot.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(mp) = guard.as_ref() {
            let _ = mp.println(msg);
        } else {
            eprintln!("{msg}");
        }
    }
}

impl<'a> fmt::MakeWriter<'a> for ProgressMakeWriter {
    type Writer = MpWriter;

    fn make_writer(&'a self) -> Self::Writer {
        MpWriter {
            mp_slot: Arc::clone(&self.mp_slot),
            buf: Vec::new(),
        }
    }
}

/// Initialise the `tracing` subscriber with an env-filter, stderr layer
/// (routing through `MultiProgress` when live), and an optional file layer.
fn init_tracing(mp_slot: MpSlot, log_path: Option<&Path>) -> anyhow::Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));

    let stderr_layer = fmt::layer()
        .with_target(false)
        .with_writer(ProgressMakeWriter { mp_slot })
        .with_ansi(console::Term::stderr().is_term());

    let file_layer = if let Some(path) = log_path {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Some(
            fmt::layer()
                .with_target(false)
                .with_ansi(false)
                .with_writer(Mutex::new(file)),
        )
    } else {
        None
    };

    tracing_subscriber::registry()
        .with(filter)
        .with(stderr_layer)
        .with(file_layer)
        .init();

    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

/// A live worker thread handle with its control flags.
struct WorkerHandle {
    thread: std::thread::JoinHandle<()>,
    exit: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
}

fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // ── Shell completions ──────────────────────────────────────────────────────
    if let Some(shell) = cli.completions {
        let mut cmd = <Cli as clap::CommandFactory>::command();
        clap_complete::generate(shell, &mut cmd, "shp2geojson", &mut std::io::stdout());
        return Ok(());
    }

    // --input is required unless --completions
    let input = cli
        .input
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("--input is required"))?
        .clone();

    // --output is required unless --dry-run
    if cli.output.is_none() && !cli.dry_run {
        anyhow::bail!("--output is required unless --dry-run is specified");
    }

    // ── Config loading ────────────────────────────────────────────────────────
    let (config_path, config_explicit) = config::resolve_config_path(&cli.config);
    let app_config = config::load_config(&config_path, config_explicit)?;

    // ── Discovery ─────────────────────────────────────────────────────────────
    let report = discover(&input)?;

    // ── Dry-run: report and exit ──────────────────────────────────────────────
    if cli.dry_run {
        let estimated_input_gb = report.total_input_bytes as f64 / 1_073_741_824.0;
        let estimated_output_gb = report.estimated_output_bytes as f64 / 1_073_741_824.0;

        eprintln!("Discovery report:");
        eprintln!("  Total .shp files found   : {}", report.entries.len());
        eprintln!("  Valid (all sidecars OK)  : {}", report.valid_count);
        eprintln!("  Invalid (missing sidecar): {}", report.invalid_count);
        eprintln!(
            "  Total input size         : {}",
            format_bytes(report.total_input_bytes)
        );
        eprintln!(
            "  Estimated output size    : {}",
            format_bytes(report.estimated_output_bytes)
        );

        // Only show GB lines if the numbers are worth it (suppress for tiny test data).
        if estimated_input_gb >= 0.01 || estimated_output_gb >= 0.01 {
            eprintln!("                             ({estimated_input_gb:.1} GB → {estimated_output_gb:.1} GB)");
        }

        eprintln!();
        for entry in &report.entries {
            match &entry.status {
                EntryStatus::Valid => {
                    eprintln!("  OK       {}", entry.shp.display());
                }
                EntryStatus::Invalid(missing) => {
                    eprintln!(
                        "  INVALID  {} — missing {}",
                        entry.shp.display(),
                        missing.join(", ")
                    );
                }
            }
        }
        return Ok(());
    }

    // ── Normal conversion run ─────────────────────────────────────────────────
    let output_root = cli.output.as_ref().expect("checked above");
    fs::create_dir_all(output_root)?;

    // ── Merge config → CLI (CLI wins over config) ─────────────────────────────
    // geojsonl: cli flag wins; config can set output_format = "geojsonl"
    let geojsonl = cli.geojsonl
        || app_config
            .conversion
            .as_ref()
            .and_then(|c| c.output_format.as_deref())
            .map(|f| f == "geojsonl")
            .unwrap_or(false);

    // overwrite: cli flag wins
    let overwrite = cli.overwrite
        || app_config
            .conversion
            .as_ref()
            .and_then(|c| c.overwrite)
            .unwrap_or(false);

    // reproject: CLI explicit flags win; if neither --reproject nor --no-reproject
    // was passed, fall back to config, then default to true.
    let reproject = if cli.reproject {
        true // explicitly requested
    } else if cli.no_reproject {
        false // explicitly disabled
    } else {
        // Neither flag given — config decides, defaulting to true.
        app_config
            .conversion
            .as_ref()
            .and_then(|c| c.reproject)
            .unwrap_or(true)
    };

    // jobs: cli wins if provided
    let jobs = cli.jobs.or_else(|| {
        app_config
            .conversion
            .as_ref()
            .and_then(|c| c.jobs)
            .filter(|&j| j > 0)
    });

    // log path: cli wins
    let log_path: PathBuf = cli
        .log
        .clone()
        .or_else(|| app_config.output.as_ref().and_then(|o| o.log_file.clone()))
        .unwrap_or_else(|| output_root.join("conversion_errors.log"));

    // ── Tracing init ─────────────────────────────────────────────────────────
    let mp_slot: MpSlot = Arc::new(Mutex::new(None));
    init_tracing(Arc::clone(&mp_slot), Some(&log_path))?;

    #[cfg(not(feature = "reproject"))]
    if reproject {
        tracing::warn!("binary built without reprojection support; --reproject ignored");
    }

    // ── Checkpoint setup ──────────────────────────────────────────────────────
    let state_path = output_root.join(".shp2geojson_state.json");

    let mut checkpoint = if cli.resume && state_path.exists() {
        CheckpointState::load(&state_path)?
    } else {
        CheckpointState::new(
            generate_run_id(),
            iso_timestamp_now(),
            input.clone(),
            output_root.clone(),
        )
    };

    // When resuming, warn if the input root has changed, clean up partial outputs
    // from previously failed entries, and clear the failed list for retry.
    if cli.resume && state_path.exists() {
        if checkpoint.input_root != input {
            tracing::warn!(
                checkpoint_root = %checkpoint.input_root.display(),
                cli_input = %input.display(),
                "--resume: checkpoint input_root differs from --input; done-file lookup may be incorrect"
            );
        }
        for rel_shp in &checkpoint.failed {
            let shp_path = input.join(rel_shp);
            if let Ok(out) = output_path_for(&shp_path, &input, output_root, geojsonl) {
                let _ = fs::remove_file(&out);
                // Remove stale .tmp variant as well.
                let tmp = out.with_extension(
                    out.extension()
                        .map(|e| format!("{}.tmp", e.to_string_lossy()))
                        .unwrap_or_else(|| "tmp".to_string()),
                );
                let _ = fs::remove_file(&tmp);
            }
        }
        checkpoint.failed.clear();
    }

    let already_done = done_set(&checkpoint);

    let batch_start = Instant::now();
    let mut converted: u64 = 0;
    let mut failed: u64 = 0;
    let mut total_bytes_processed: u64 = 0;

    // Early progress stub — used before job count is known. Replaced after enqueueing.
    let noop_progress = Progress::Noop;

    // Save report totals before entries are consumed.
    let report_total_files = report.entries.len();
    let report_total_bytes = report.total_input_bytes;

    // Emit start event.
    emit(
        &OutputEvent::Start {
            total_files: report_total_files,
            total_bytes: report_total_bytes,
            timestamp: iso_timestamp_now(),
        },
        &cli.output_format,
        &noop_progress,
    );

    // Log invalid entries.
    for entry in report.entries.iter().filter(|e| !e.is_valid()) {
        if let EntryStatus::Invalid(ref missing) = entry.status {
            let reason = format!("missing sidecar(s): {}", missing.join(", "));
            log_error(&log_path, "INVALID", &entry.shp, &reason);
            failed += 1;
            emit(
                &OutputEvent::FileFailed {
                    file: entry.shp.display().to_string(),
                    reason,
                },
                &cli.output_format,
                &noop_progress,
            );
        }
    }

    // ── Step B: Enqueue valid jobs ────────────────────────────────────────────
    let queue = WorkQueue::new();
    let mut jobs_enqueued: u64 = 0;

    for entry in report.entries.into_iter().filter(|e| e.is_valid()) {
        let rel = match relative_shp_path(&entry.shp, &input) {
            Some(r) => r,
            None => {
                tracing::warn!(
                    file = %entry.shp.display(),
                    "could not relativize path, skipping checkpoint entry"
                );
                String::new()
            }
        };

        // Skip files already completed in a prior run.
        if cli.resume && already_done.contains(&rel) {
            emit(
                &OutputEvent::FileSkipped {
                    file: entry.shp.display().to_string(),
                    reason: "already completed in previous run".to_string(),
                },
                &cli.output_format,
                &noop_progress,
            );
            log_error(
                &log_path,
                "SKIPPED",
                &entry.shp,
                "output exists, --resume active",
            );
            continue;
        }

        // Warn when the caller asked for reprojection but there is no .prj to read.
        if reproject && entry.prj.is_none() {
            log_error(
                &log_path,
                "WARN",
                &entry.shp,
                "no .prj found, CRS unknown, coordinates passed through unchanged",
            );
        }

        let out_path = match output_path_for(&entry.shp, &input, output_root, geojsonl) {
            Ok(p) => p,
            Err(e) => {
                let reason = e.to_string();
                failed += 1;
                log_error(&log_path, "FAILED", &entry.shp, &reason);
                emit(
                    &OutputEvent::FileFailed {
                        file: entry.shp.display().to_string(),
                        reason,
                    },
                    &cli.output_format,
                    &noop_progress,
                );
                continue;
            }
        };

        let options = ConvertOptions {
            geojsonl,
            overwrite,
            reproject_from_prj: if reproject { entry.prj.clone() } else { None },
            on_record: None,
        };

        // Record entry as pending in checkpoint before dispatch.
        checkpoint.pending.push(rel);

        let _ = queue.job_tx.send(Job {
            entry,
            output_path: out_path,
            options,
        });
        jobs_enqueued += 1;
    }

    // Save checkpoint with all pending entries recorded before workers start.
    if let Err(e) = checkpoint.save(&state_path) {
        tracing::warn!("checkpoint save failed: {e}");
    }

    // Signal workers: no more jobs are coming (via job_tx drop after spawning).
    drop(queue.job_tx);

    // ── Step C: Create progress + spawn worker threads ────────────────────────
    // Progress is created here so `jobs_enqueued` is the accurate total.
    let worker_count = jobs.unwrap_or_else(num_cpus::get).max(1);
    let progress = Progress::new(
        matches!(cli.output_format, shp2geojson::cli::OutputFormat::Human),
        jobs_enqueued,
        worker_count,
        cli.resume,
        report_total_files,
        report_total_bytes,
    );

    // Wire tracing output through the live MultiProgress (if active).
    if let Some(mp) = progress.multi_progress() {
        *mp_slot.lock().unwrap() = Some(mp);
    }

    // Shared state for interactive commands.
    let pause_flag = PauseFlag::new();
    let skip_set: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
    let input_root = Arc::new(input.clone());

    let mut worker_handles: Vec<WorkerHandle> = Vec::with_capacity(worker_count);

    // Keep cloneable handles for scale-up via /workers.
    // `scale_result_tx` is an extra clone held so new workers can be given senders;
    // it must be dropped after the select loop so `result_rx` disconnects when
    // all workers finish.
    let scale_job_rx = queue.job_rx.clone();
    let scale_result_tx = queue.result_tx.clone();
    // Drop main thread's direct copies.
    drop(queue.job_rx);
    drop(queue.result_tx); // CRITICAL: main thread must not hold result_tx permanently

    for i in 0..worker_count {
        let rx = scale_job_rx.clone();
        let tx = scale_result_tx.clone();
        let wp = progress.add_worker_bar(i);
        let flags = WorkerFlags::new(
            pause_flag.arc(),
            Arc::clone(&skip_set),
            Arc::clone(&input_root),
        );
        let exit = Arc::clone(&flags.exit);
        let done = Arc::clone(&flags.done);
        worker_handles.push(WorkerHandle {
            thread: std::thread::spawn(move || worker_loop(rx, tx, wp, flags)),
            exit,
            done,
        });
    }

    // ── Step D: Ctrl+C graceful shutdown ───────────────────────────────────────
    // Ctrl+C handler: signal workers to exit gracefully.
    let ctrlc_flag = Arc::new(AtomicBool::new(false));
    {
        let ctrlc_flag = Arc::clone(&ctrlc_flag);
        let pause_arc = pause_flag.arc();
        let _ = ctrlc::set_handler(move || {
            if ctrlc_flag
                .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
                .is_err()
            {
                // Already set — second Ctrl+C, hard exit.
                std::process::exit(130);
            }
            // First Ctrl+C — unpause workers so they can drain and exit.
            pause_arc.store(false, Ordering::Relaxed);
            eprintln!(
                "\nCtrl+C received — finishing in-flight files. Press Ctrl+C again to force quit."
            );
        });
    }

    let tick_rx = crossbeam_channel::tick(Duration::from_millis(200));

    // ── Step E: Drain results via crossbeam select! ───────────────────────────
    let mut quit_requested = false;
    let mut results_received: u64 = 0;

    loop {
        crossbeam_channel::select! {
            recv(queue.result_rx) -> msg => {
                match msg {
                    Ok(JobResult::Done(stats)) => {
                        results_received += 1;
                        converted += 1;
                        progress.inc_overall();
                        if let Ok(meta) = std::fs::metadata(&stats.input) {
                            total_bytes_processed += meta.len();
                        }
                        emit(
                            &OutputEvent::FileDone {
                                file: stats.input.display().to_string(),
                                output: stats.output.display().to_string(),
                                duration_ms: stats.duration.as_millis() as u64,
                                features: stats.features_written,
                            },
                            &cli.output_format,
                            &progress,
                        );

                        // Fire on_file_complete hook.
                        let mut hook_vars = std::collections::HashMap::new();
                        hook_vars.insert("file", stats.input.display().to_string());
                        hook_vars.insert("output", stats.output.display().to_string());
                        hook_vars.insert("features", stats.features_written.to_string());
                        hook_vars.insert("duration_ms", stats.duration.as_millis().to_string());
                        hooks::fire_hook_if_configured(&app_config.hooks, "on_file_complete", &hook_vars);

                        // Update checkpoint after each successful job.
                        if let Some(rel) = relative_shp_path(&stats.input, &input) {
                            checkpoint.mark_done(&rel);
                            if let Err(e) = checkpoint.save(&state_path) {
                                tracing::warn!("checkpoint save failed: {e}");
                            }
                        }
                    }
                    Ok(JobResult::Failed { shp, reason }) => {
                        results_received += 1;
                        failed += 1;
                        log_error(&log_path, "FAILED", &shp, &reason);
                        emit(
                            &OutputEvent::FileFailed {
                                file: shp.display().to_string(),
                                reason: reason.clone(),
                            },
                            &cli.output_format,
                            &progress,
                        );

                        // Fire on_file_failed hook.
                        let mut hook_vars = std::collections::HashMap::new();
                        hook_vars.insert("file", shp.display().to_string());
                        hook_vars.insert("reason", reason.clone());
                        hooks::fire_hook_if_configured(&app_config.hooks, "on_file_failed", &hook_vars);

                        // Update checkpoint after each failed job.
                        if let Some(rel) = relative_shp_path(&shp, &input) {
                            checkpoint.mark_failed(&rel);
                            if let Err(e) = checkpoint.save(&state_path) {
                                tracing::warn!("checkpoint save failed: {e}");
                            }
                        }
                    }
                    Ok(JobResult::Skipped { shp: _ }) => {
                        results_received += 1;
                        progress.inc_overall();
                        // Checkpoint was already updated in the /skip handler.
                    }
                    Err(_) => {
                        // All worker result_tx clones dropped — channel disconnected.
                        break;
                    }
                }

                // Count-based termination: all enqueued jobs accounted for.
                if results_received >= jobs_enqueued {
                    break;
                }
            },
            recv(tick_rx) -> _ => {
                // Periodic tick — prune completed worker handles.
                worker_handles.retain(|h| !h.done.load(Ordering::Relaxed));

                // Check Ctrl+C flag — initiate graceful shutdown.
                if ctrlc_flag.load(Ordering::Relaxed) && !quit_requested {
                    quit_requested = true;
                    // Signal all workers to exit after their current job.
                    for h in worker_handles.iter() {
                        h.exit.store(true, Ordering::Relaxed);
                    }
                    // Save checkpoint.
                    if let Err(e) = checkpoint.save(&state_path) {
                        tracing::warn!("checkpoint save failed: {e}");
                    }
                }
            },
        }
    }

    // Drop the scale handles; once all worker threads drop their clones, the
    // result channel is fully disconnected.
    drop(scale_result_tx);
    drop(scale_job_rx);

    progress.finish();

    // ── Step F: Join workers and emit batch summary ───────────────────────────
    for wh in worker_handles {
        if let Err(e) = wh.thread.join() {
            tracing::error!("worker thread panicked: {e:?}");
        }
    }

    let elapsed_s = batch_start.elapsed().as_secs_f64();

    // Fire on_batch_done hook before emitting the BatchDone event.
    let summary = serde_json::json!({
        "converted": converted,
        "failed": failed,
        "elapsed_s": elapsed_s,
        "gb_processed": total_bytes_processed as f64 / 1_073_741_824.0
    });
    let mut hook_vars = std::collections::HashMap::new();
    hook_vars.insert("converted", converted.to_string());
    hook_vars.insert("failed", failed.to_string());
    hook_vars.insert("elapsed_s", format!("{elapsed_s:.1}"));
    hook_vars.insert(
        "gb_processed",
        format!("{:.3}", total_bytes_processed as f64 / 1_073_741_824.0),
    );
    hook_vars.insert("summary_json", summary.to_string());
    hooks::fire_hook_if_configured(&app_config.hooks, "on_batch_done", &hook_vars);

    emit(
        &OutputEvent::BatchDone {
            converted,
            failed,
            elapsed_s,
            gb_processed: total_bytes_processed as f64 / 1_073_741_824.0,
        },
        &cli.output_format,
        &progress,
    );

    Ok(())
}

/// Appends a single error/warning line to the error log file.
///
/// Format: `[YYYY-MM-DD HH:MM:SS] LEVEL   path — reason`
fn log_error(log_path: &Path, level: &str, shp: &Path, reason: &str) {
    // Ensure parent directory exists.
    if let Some(parent) = log_path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    let timestamp = simple_timestamp();
    let line = format!(
        "[{}] {:<8} {}  — {}\n",
        timestamp,
        level,
        shp.display(),
        reason
    );

    match OpenOptions::new().create(true).append(true).open(log_path) {
        Ok(mut f) => {
            let _ = f.write_all(line.as_bytes());
        }
        Err(e) => {
            tracing::warn!(
                path = %log_path.display(),
                "could not write to error log: {e}"
            );
        }
    }
}

/// Returns a simple `YYYY-MM-DD HH:MM:SS` timestamp using only `std`.
///
/// Uses `std::time::SystemTime` converted to seconds since UNIX epoch, then
/// performs integer arithmetic for calendar conversion. This avoids a `chrono`
/// dependency.
fn simple_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    unix_secs_to_datetime_str(secs, false)
}

/// Returns an ISO 8601 UTC timestamp string (`YYYY-MM-DDTHH:MM:SSZ`).
fn iso_timestamp_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    unix_secs_to_datetime_str(secs, true)
}

/// Converts Unix seconds to a datetime string.
///
/// When `iso` is `true`, produces `YYYY-MM-DDTHH:MM:SSZ`.
/// When `false`, produces `YYYY-MM-DD HH:MM:SS`.
///
/// Algorithm: https://howardhinnant.github.io/date_algorithms.html
fn unix_secs_to_datetime_str(secs: u64, iso: bool) -> String {
    let z = (secs / 86400) as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    let time_of_day = secs % 86400;
    let hh = time_of_day / 3600;
    let mm = (time_of_day % 3600) / 60;
    let ss = time_of_day % 60;

    if iso {
        format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
    } else {
        format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}:{ss:02}")
    }
}
