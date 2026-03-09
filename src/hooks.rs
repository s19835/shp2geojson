use std::collections::HashMap;
use std::process::{Command, Stdio};

use crate::config::HooksConfig;

/// Replaces `{{key}}` placeholders in `template` with shell-quoted values
/// from `vars`.
///
/// Values are wrapped in single quotes with internal single quotes escaped
/// (`'` → `'\''`), preventing shell injection from adversarial filenames.
///
/// Uses single-pass scanning to avoid non-deterministic substitution order
/// when HashMap iteration interacts with overlapping keys.
///
/// Unknown placeholders are left as-is (not an error).
///
/// # Examples
///
/// ```
/// use std::collections::HashMap;
/// use shp2geojson::hooks::substitute_template;
///
/// let mut vars = HashMap::new();
/// vars.insert("file", "/data/foo.shp".to_string());
/// let result = substitute_template("Processing {{file}}", &vars);
/// assert_eq!(result, "Processing '/data/foo.shp'");
/// ```
pub fn substitute_template(template: &str, vars: &HashMap<&str, String>) -> String {
    let mut result = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find("{{") {
        result.push_str(&rest[..open]);
        rest = &rest[open + 2..];
        if let Some(close) = rest.find("}}") {
            let key = &rest[..close];
            if let Some(val) = vars.get(key) {
                result.push_str(&shell_quote(val));
            } else {
                result.push_str("{{");
                result.push_str(key);
                result.push_str("}}");
            }
            rest = &rest[close + 2..];
        } else {
            // Unclosed `{{` — emit literally.
            result.push_str("{{");
        }
    }
    result.push_str(rest);
    result
}

/// Shell-quotes a value by wrapping in single quotes and escaping internal
/// single quotes: `'` → `'\''`.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Fires a hook command asynchronously (fire-and-forget).
///
/// Spawns `sh -c <command>` (or `cmd /C <command>` on Windows) in a background
/// thread. Failures are logged to stderr but never block the pipeline. Returns
/// immediately after spawning the thread.
pub fn fire_hook(hook_name: &str, command: &str) {
    let hook_name = hook_name.to_string();
    let command = command.to_string();

    std::thread::spawn(move || {
        let result = if cfg!(target_os = "windows") {
            Command::new("cmd")
                .args(["/C", &command])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
        } else {
            Command::new("sh")
                .args(["-c", &command])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
        };

        match result {
            Ok(output) => {
                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    eprintln!(
                        "HOOK ERR  {hook_name}  exit {}  — {}",
                        output.status.code().unwrap_or(-1),
                        stderr.trim()
                    );
                }
            }
            Err(e) => {
                eprintln!("HOOK ERR  {hook_name}  spawn failed: {e}");
            }
        }
    });
}

/// Convenience: substitute template vars and fire if the hook is configured.
///
/// Does nothing if `hooks` is `None`, or if the named hook has no command
/// configured, or if `hook_name` is not a recognised hook.
pub fn fire_hook_if_configured(
    hooks: &Option<HooksConfig>,
    hook_name: &str,
    vars: &HashMap<&str, String>,
) {
    let hooks = match hooks {
        Some(h) => h,
        None => return,
    };

    let template = match hook_name {
        "on_file_complete" => hooks.on_file_complete.as_deref(),
        "on_file_failed" => hooks.on_file_failed.as_deref(),
        "on_batch_done" => hooks.on_batch_done.as_deref(),
        "on_pause" => hooks.on_pause.as_deref(),
        _ => None,
    };

    if let Some(tmpl) = template {
        let cmd = substitute_template(tmpl, vars);
        fire_hook(hook_name, &cmd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── substitute_template ───────────────────────────────────────────────────

    #[test]
    fn test_substitute_template_basic() {
        let mut vars = HashMap::new();
        vars.insert("file", "/data/foo.shp".to_string());
        let result = substitute_template("Processing {{file}}", &vars);
        assert_eq!(result, "Processing '/data/foo.shp'");
    }

    #[test]
    fn test_substitute_template_multiple_vars() {
        let mut vars = HashMap::new();
        vars.insert("file", "/data/foo.shp".to_string());
        vars.insert("features", "42".to_string());
        let result = substitute_template("{{file}} has {{features}} features", &vars);
        assert_eq!(result, "'/data/foo.shp' has '42' features");
    }

    #[test]
    fn test_substitute_template_unknown_placeholder_left_intact() {
        let vars: HashMap<&str, String> = HashMap::new();
        let result = substitute_template("echo {{unknown}}", &vars);
        assert_eq!(result, "echo {{unknown}}");
    }

    #[test]
    fn test_substitute_template_empty_template() {
        let mut vars = HashMap::new();
        vars.insert("file", "/data/foo.shp".to_string());
        let result = substitute_template("", &vars);
        assert_eq!(result, "");
    }

    #[test]
    fn test_substitute_template_no_placeholders() {
        let mut vars = HashMap::new();
        vars.insert("file", "/data/foo.shp".to_string());
        let result = substitute_template("echo hello world", &vars);
        assert_eq!(result, "echo hello world");
    }

    #[test]
    fn test_substitute_template_escapes_single_quotes() {
        let mut vars = HashMap::new();
        vars.insert("file", "it's a file.shp".to_string());
        let result = substitute_template("echo {{file}}", &vars);
        assert_eq!(result, "echo 'it'\\''s a file.shp'");
    }

    #[test]
    fn test_substitute_template_prevents_injection() {
        let mut vars = HashMap::new();
        vars.insert("file", "payload; rm -rf /".to_string());
        let result = substitute_template("echo {{file}}", &vars);
        // Value is safely quoted — semicolon is inside single quotes.
        assert_eq!(result, "echo 'payload; rm -rf /'");
    }

    // ── fire_hook_if_configured ───────────────────────────────────────────────

    #[test]
    fn test_fire_hook_if_configured_none_hooks_does_nothing() {
        // Must not panic.
        let vars: HashMap<&str, String> = HashMap::new();
        fire_hook_if_configured(&None, "on_file_complete", &vars);
    }

    #[test]
    fn test_fire_hook_if_configured_unknown_hook_does_nothing() {
        let hooks = Some(crate::config::HooksConfig {
            on_file_complete: Some("echo done".to_string()),
            on_file_failed: None,
            on_batch_done: None,
            on_pause: None,
        });
        let vars: HashMap<&str, String> = HashMap::new();
        // "on_mysterious_event" is not a known hook; must not panic.
        fire_hook_if_configured(&hooks, "on_mysterious_event", &vars);
    }

    #[test]
    fn test_fire_hook_runs_command() {
        // Spawn a trivial shell command and verify the function returns without panicking.
        // We do NOT wait for the background thread; the test just checks no panic occurs.
        fire_hook("test_hook", "echo shp2geojson_hook_test");
        // Small sleep to give the background thread a chance to run (not required for
        // correctness but helps surface spawn errors in CI).
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}
