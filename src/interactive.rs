use std::io::BufRead;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender};

/// A command entered interactively via stdin during a conversion run.
#[derive(Debug, Clone)]
pub enum SlashCommand {
    /// Print worker status and queue depth.
    Status,
    /// Pause all workers and write a checkpoint.
    Pause,
    /// Resume paused workers.
    Resume,
    /// Scale worker count to N.
    Workers(usize),
    /// Skip a pending file by path fragment.
    Skip(String),
    /// Show last 20 lines of the error log.
    Log,
    /// List remaining pending files.
    DryRun,
    /// Save checkpoint and exit after in-flight files complete.
    Quit,
    /// Show this help.
    Help,
}

/// A shareable, atomic pause flag.
///
/// Wraps an `Arc<AtomicBool>` so the flag can be shared between the main
/// thread (which sets/clears it) and worker threads (which poll it).
pub struct PauseFlag(Arc<AtomicBool>);

impl PauseFlag {
    /// Creates a new flag initialised to `false` (not paused).
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    /// Returns `true` if the flag is currently set (paused).
    pub fn is_paused(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// Sets the flag (workers will idle).
    pub fn set_paused(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Clears the flag (workers resume).
    pub fn clear(&self) {
        self.0.store(false, Ordering::Relaxed);
    }

    /// Returns an `Arc<AtomicBool>` clone suitable for sharing with workers.
    pub fn arc(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.0)
    }
}

impl Default for PauseFlag {
    fn default() -> Self {
        Self::new()
    }
}

/// Help text printed by `/help`, styled with ANSI colors matching the mockup.
pub const HELP_TEXT: &str = "\
\x1b[38;5;60m──── Available Commands ─────────────────────────────────────────────\x1b[0m
  \x1b[38;5;69;1m/status\x1b[0m      \x1b[38;5;103mPrint worker status and queue depth\x1b[0m
  \x1b[38;5;69;1m/pause\x1b[0m       \x1b[38;5;103mPause all workers, write checkpoint\x1b[0m
  \x1b[38;5;69;1m/resume\x1b[0m      \x1b[38;5;103mResume paused workers\x1b[0m
  \x1b[38;5;69;1m/workers\x1b[0m \x1b[38;5;80mN\x1b[0m  \x1b[38;5;103mDynamically scale worker count up or down\x1b[0m
  \x1b[38;5;69;1m/skip\x1b[0m \x1b[38;5;80mFILE\x1b[0m  \x1b[38;5;103mSkip a pending file, mark as SKIPPED\x1b[0m
  \x1b[38;5;69;1m/log\x1b[0m         \x1b[38;5;103mTail conversion_errors.log inline\x1b[0m
  \x1b[38;5;69;1m/dry-run\x1b[0m     \x1b[38;5;103mPreview remaining pending files\x1b[0m
  \x1b[38;5;69;1m/quit\x1b[0m        \x1b[38;5;103mCheckpoint state and exit cleanly\x1b[0m
  \x1b[38;5;69;1m/help\x1b[0m        \x1b[38;5;103mShow this help\x1b[0m
\x1b[38;5;60m───────────────────────────────────────────────────────────────────────\x1b[0m
  \x1b[38;5;103mCtrl+C — pause and checkpoint  ·  Ctrl+C×2 — force quit\x1b[0m";

/// Starts a background thread that reads `/command` lines from stdin and sends
/// parsed [`SlashCommand`]s on the returned receiver.
///
/// Non-slash input is silently ignored. Unrecognised slash commands print an
/// error message. Help text for `/help` is printed immediately inside the
/// reader thread so it bypasses the result channel.
///
/// `mp` — if `Some`, routes printed messages through `MultiProgress::println`
/// to avoid corrupting the progress bar layout.
pub fn start_stdin_reader(mp: Option<Arc<indicatif::MultiProgress>>) -> Receiver<SlashCommand> {
    let (tx, rx): (Sender<SlashCommand>, Receiver<SlashCommand>) = crossbeam_channel::unbounded();

    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(line) => {
                    if let Some(cmd) = parse_command(&line) {
                        if matches!(cmd, SlashCommand::Help) {
                            if let Some(ref mp) = mp {
                                let _ = mp.println(HELP_TEXT);
                            } else {
                                eprintln!("{HELP_TEXT}");
                            }
                        }
                        if tx.send(cmd).is_err() {
                            break; // main thread dropped receiver
                        }
                    } else {
                        let trimmed = line.trim();
                        if trimmed.starts_with('/') {
                            let msg =
                                format!("Unknown command: {trimmed}. Type /help for commands.");
                            if let Some(ref mp) = mp {
                                let _ = mp.println(&msg);
                            } else {
                                eprintln!("{msg}");
                            }
                        }
                    }
                }
                Err(_) => break, // stdin error or EOF
            }
        }
    });

    rx
}

/// Parses a single line into a [`SlashCommand`].
///
/// Returns `None` if the line does not start with `/`, is an unknown command,
/// or has invalid arguments (e.g., `/workers abc`).
///
/// Leading and trailing whitespace on the line is stripped before parsing.
/// The command name is case-insensitive.
///
/// # Examples
///
/// ```
/// use shp2geojson::interactive::parse_command;
///
/// assert!(matches!(parse_command("/status"), Some(_)));
/// assert!(parse_command("not a command").is_none());
/// assert!(parse_command("/workers abc").is_none());
/// ```
pub fn parse_command(line: &str) -> Option<SlashCommand> {
    let trimmed = line.trim();
    if !trimmed.starts_with('/') {
        return None;
    }

    let parts: Vec<&str> = trimmed[1..].splitn(2, char::is_whitespace).collect();
    let cmd = parts[0].to_lowercase();
    let arg = parts.get(1).map(|s| s.trim());

    match cmd.as_str() {
        "status" => Some(SlashCommand::Status),
        "pause" => Some(SlashCommand::Pause),
        "resume" => Some(SlashCommand::Resume),
        "workers" => arg?.parse::<usize>().ok().map(SlashCommand::Workers),
        "skip" => {
            let path = arg?;
            if path.is_empty() {
                return None;
            }
            Some(SlashCommand::Skip(path.to_string()))
        }
        "log" => Some(SlashCommand::Log),
        "dry-run" => Some(SlashCommand::DryRun),
        "quit" => Some(SlashCommand::Quit),
        "help" => Some(SlashCommand::Help),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_status(cmd: &Option<SlashCommand>) -> bool {
        matches!(cmd, Some(SlashCommand::Status))
    }

    fn is_pause(cmd: &Option<SlashCommand>) -> bool {
        matches!(cmd, Some(SlashCommand::Pause))
    }

    fn is_resume(cmd: &Option<SlashCommand>) -> bool {
        matches!(cmd, Some(SlashCommand::Resume))
    }

    fn is_log(cmd: &Option<SlashCommand>) -> bool {
        matches!(cmd, Some(SlashCommand::Log))
    }

    fn is_dry_run(cmd: &Option<SlashCommand>) -> bool {
        matches!(cmd, Some(SlashCommand::DryRun))
    }

    fn is_quit(cmd: &Option<SlashCommand>) -> bool {
        matches!(cmd, Some(SlashCommand::Quit))
    }

    fn is_help(cmd: &Option<SlashCommand>) -> bool {
        matches!(cmd, Some(SlashCommand::Help))
    }

    #[test]
    fn test_parse_command_status() {
        assert!(is_status(&parse_command("/status")));
    }

    #[test]
    fn test_parse_command_pause() {
        assert!(is_pause(&parse_command("/pause")));
    }

    #[test]
    fn test_parse_command_resume() {
        assert!(is_resume(&parse_command("/resume")));
    }

    #[test]
    fn test_parse_command_workers_valid() {
        let cmd = parse_command("/workers 4");
        assert!(matches!(cmd, Some(SlashCommand::Workers(4))));
    }

    #[test]
    fn test_parse_command_workers_zero() {
        // Zero is a valid usize — the caller clamps it to 1.
        let cmd = parse_command("/workers 0");
        assert!(matches!(cmd, Some(SlashCommand::Workers(0))));
    }

    #[test]
    fn test_parse_command_workers_invalid() {
        // Non-numeric argument fails to parse.
        let cmd = parse_command("/workers abc");
        assert!(cmd.is_none());
    }

    #[test]
    fn test_parse_command_skip() {
        let cmd = parse_command("/skip data/foo.shp");
        assert!(matches!(cmd, Some(SlashCommand::Skip(ref s)) if s == "data/foo.shp"));
    }

    #[test]
    fn test_parse_command_skip_no_arg() {
        // `/skip` with no argument returns None.
        let cmd = parse_command("/skip");
        assert!(cmd.is_none());
    }

    #[test]
    fn test_parse_command_quit() {
        assert!(is_quit(&parse_command("/quit")));
    }

    #[test]
    fn test_parse_command_dry_run() {
        assert!(is_dry_run(&parse_command("/dry-run")));
    }

    #[test]
    fn test_parse_command_log() {
        assert!(is_log(&parse_command("/log")));
    }

    #[test]
    fn test_parse_command_help() {
        assert!(is_help(&parse_command("/help")));
    }

    #[test]
    fn test_parse_command_non_slash() {
        assert!(parse_command("status").is_none());
        assert!(parse_command("hello world").is_none());
    }

    #[test]
    fn test_parse_command_empty() {
        assert!(parse_command("").is_none());
    }

    #[test]
    fn test_parse_command_bare_slash() {
        // A lone `/` has an empty command name — not recognised.
        assert!(parse_command("/").is_none());
    }

    #[test]
    fn test_parse_command_unknown() {
        assert!(parse_command("/unknown").is_none());
        assert!(parse_command("/xyzzy").is_none());
    }

    #[test]
    fn test_parse_command_case_insensitive() {
        assert!(is_status(&parse_command("/STATUS")));
        assert!(is_pause(&parse_command("/PAUSE")));
        assert!(is_resume(&parse_command("/RESUME")));
        assert!(is_quit(&parse_command("/QUIT")));
        let cmd = parse_command("/WORKERS 8");
        assert!(matches!(cmd, Some(SlashCommand::Workers(8))));
    }

    #[test]
    fn test_parse_command_leading_space() {
        // Leading whitespace on the line is stripped.
        assert!(is_status(&parse_command("  /status")));
        assert!(is_pause(&parse_command("  /pause")));
    }

    #[test]
    fn test_pause_flag_default_not_paused() {
        let flag = PauseFlag::new();
        assert!(!flag.is_paused());
    }

    #[test]
    fn test_pause_flag_set_and_clear() {
        let flag = PauseFlag::new();
        flag.set_paused();
        assert!(flag.is_paused());
        flag.clear();
        assert!(!flag.is_paused());
    }

    #[test]
    fn test_pause_flag_arc_shares_state() {
        let flag = PauseFlag::new();
        let shared = flag.arc();
        flag.set_paused();
        assert!(shared.load(Ordering::Relaxed));
    }
}
