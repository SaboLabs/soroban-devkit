//! Process executor with strict stdout/stderr separation.
//!
//! The agent never merges streams (`2>&1` is deliberately absent), never
//! retries, and always records the exit code — including the case where no
//! exit code exists because the process hit the timeout.

use std::io::Read;
use std::os::raw::c_int;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use std::{env, thread};

/// Environment override for the binary under test, so tests can point the
/// executor at a fake `sdkt` without touching the real CLI.
pub const BIN_ENV: &str = "SDKT_AGENT_BIN";

/// Name of the binary the agent drives by default.
pub const DEFAULT_BIN: &str = "sdkt";

/// Optional fixed arguments placed before the caller's argv. The test
/// fixtures need it because on Windows the fake CLI runs as
/// `powershell.exe -File <script>`: the program alone is not enough.
/// Unset in normal use, so real invocations are unaffected.
pub const BIN_ARGS_ENV: &str = "SDKT_AGENT_BIN_ARGS";

/// Default wall-clock budget for one execution.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);

/// Raw outcome of one process run. Streams are kept apart on purpose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawExecution {
    /// Exit code, or `None` when the process was killed at the timeout.
    pub exit_code: Option<c_int>,
    /// Standard output, verbatim. Never contaminated by stderr.
    pub stdout: String,
    /// Standard error, kept as diagnostic evidence.
    pub stderr: String,
    /// True when the timeout fired and `exit_code` is `None`.
    pub timed_out: bool,
    /// True when the process could not be spawned at all.
    pub spawn_failed: bool,
}

impl RawExecution {
    fn spawned(status: Option<c_int>, stdout: String, stderr: String, timed_out: bool) -> Self {
        Self {
            exit_code: status,
            stdout,
            stderr,
            timed_out,
            spawn_failed: false,
        }
    }

    fn spawn_error(message: String) -> Self {
        Self {
            exit_code: None,
            stdout: String::new(),
            stderr: message,
            timed_out: false,
            spawn_failed: true,
        }
    }
}

/// Runs one command line against a fixed program with a fixed timeout.
#[derive(Debug, Clone)]
pub struct Executor {
    program: String,
    timeout: Duration,
}

impl Executor {
    /// Executor for the configured `sdkt` binary.
    pub fn for_sdkt() -> Self {
        Self {
            program: env::var(BIN_ENV).unwrap_or_else(|_| DEFAULT_BIN.to_string()),
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Fixed prefix arguments, when [`BIN_ARGS_ENV`] is set.
    fn prefix_args() -> Vec<String> {
        env::var(BIN_ARGS_ENV)
            .unwrap_or_default()
            .split('|')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    }

    /// Executor for an arbitrary program — used by tests with fake binaries.
    pub fn with_program(program: impl Into<String>, timeout: Duration) -> Self {
        Self {
            program: program.into(),
            timeout,
        }
    }

    pub fn program(&self) -> &str {
        &self.program
    }

    /// Execute `program args…`, capturing each stream separately.
    ///
    /// No retry: one attempt, one result. A failed spawn is reported as a
    /// spawn error rather than silently becoming a success.
    pub fn run(&self, args: &[String]) -> RawExecution {
        let mut child = match Command::new(&self.program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                return RawExecution::spawn_error(format!("could not run {}: {e}", self.program))
            }
        };

        // Both pipes are drained on their own threads before waiting, so a
        // chatty child cannot deadlock on a full pipe buffer while we poll.
        let out_reader = child.stdout.take().map(spawn_reader);
        let err_reader = child.stderr.take().map(spawn_reader);

        let deadline = Instant::now() + self.timeout;
        let mut timed_out = false;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) => {
                    if Instant::now() >= deadline {
                        // Kill and reap; the exit code is then meaningless, so
                        // it is reported as absent rather than guessed.
                        let _ = child.kill();
                        let _ = child.wait();
                        timed_out = true;
                        break None;
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(e) => {
                    let _ = child.kill();
                    return RawExecution::spawn_error(format!("wait failed: {e}"));
                }
            }
        };

        let stdout = join_reader(out_reader);
        let stderr = join_reader(err_reader);
        RawExecution::spawned(status.and_then(|s| s.code()), stdout, stderr, timed_out)
    }
}

/// Convenience wrapper: run with the default `sdkt` binary.
pub fn execute(args: &[String]) -> RawExecution {
    let mut full = Executor::prefix_args();
    full.extend_from_slice(args);
    Executor::for_sdkt().run(&full)
}

fn spawn_reader<R: Read + Send + 'static>(mut reader: R) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut buf = String::new();
        // A partial read is still evidence; never discard what we did get.
        let _ = reader.read_to_string(&mut buf);
        buf
    })
}

fn join_reader(handle: Option<thread::JoinHandle<String>>) -> String {
    match handle {
        Some(h) => h.join().unwrap_or_default(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // One source of truth for how a fake `sdkt` behaves, shared with the
    // integration tests. Included rather than declared as a module so the
    // helper is compiled once per crate that needs it.
    include!("../tests/common/fixture.rs");

    /// Path to a binary that exists on every platform, used to prove a
    /// spawn failure is reported. A relative path with no directory is
    /// resolved against PATH, and no PATH entry provides this name.
    const NONEXISTENT_BIN: &str = "sdkt-agent-nonexistent-binary-for-tests";

    #[test]
    fn exit_code_is_recorded_not_swallowed() {
        let tmp = tempfile::tempdir().unwrap();
        let fx = FakeCli {
            stdout: "ok\n".into(),
            exit_code: 3,
            ..FakeCli::ok()
        }
        .write(tmp.path());
        let argv = fx.argv_for(&[]);
        let raw = Executor::with_program(fx.program, Duration::from_secs(30)).run(&argv);
        assert_eq!(raw.exit_code, Some(3));
        assert_eq!(raw.stdout, "ok\n");
        assert!(raw.stderr.is_empty(), "stderr: {:?}", raw.stderr);
        assert!(!raw.timed_out);
        assert!(!raw.spawn_failed, "stderr: {:?}", raw.stderr);
    }

    #[test]
    fn stdout_and_stderr_never_mix() {
        let tmp = tempfile::tempdir().unwrap();
        // stdout: pure JSON. stderr: human noise, written to the other stream.
        let fx = FakeCli {
            stdout: "{\"health\":\"critical\"}\n".into(),
            stderr: "warning: something on stderr\nError: actionable failure\n".into(),
            exit_code: 1,
            ..FakeCli::ok()
        }
        .write(tmp.path());
        let argv = fx.argv_for(&[]);
        let raw = Executor::with_program(fx.program, Duration::from_secs(30)).run(&argv);
        assert_eq!(raw.exit_code, Some(1));
        assert!(
            raw.stdout.trim().parse::<serde_json::Value>().is_ok(),
            "stdout must stay pure JSON, got {:?}",
            raw.stdout
        );
        assert!(raw.stderr.contains("warning: something on stderr"));
        assert!(raw.stderr.contains("Error: actionable failure"));
        assert!(
            !raw.stdout.contains("stderr"),
            "stderr leaked into stdout: {}",
            raw.stdout
        );
    }

    #[test]
    fn timeout_kills_and_reports_no_exit_code() {
        let tmp = tempfile::tempdir().unwrap();
        // Sleep well past the budget so the kill path is genuinely exercised.
        // Kept short: after the kill the reader threads can only return once
        // the pipe closes, so a long sleep would make the test wait for it.
        let fx = FakeCli {
            sleep_ms: 2_000,
            ..FakeCli::ok()
        }
        .write(tmp.path());
        let argv = fx.argv_for(&[]);
        let raw = Executor::with_program(fx.program, Duration::from_millis(300)).run(&argv);
        assert!(raw.timed_out, "expected a timeout");
        assert_eq!(raw.exit_code, None, "a killed run has no usable code");
    }

    #[test]
    fn large_output_on_both_streams_does_not_deadlock() {
        let tmp = tempfile::tempdir().unwrap();
        // ~128 KiB on each stream, well past the 64 KiB pipe buffer. Distinct
        // payloads prove both pipes were drained and stayed separate.
        let fx = FakeCli {
            big_bytes: 128 * 1024,
            ..FakeCli::ok()
        }
        .write(tmp.path());
        let argv = fx.argv_for(&[]);
        let raw = Executor::with_program(fx.program, Duration::from_secs(60)).run(&argv);
        assert_eq!(raw.exit_code, Some(0));
        assert!(
            raw.stdout.len() >= 128 * 1024,
            "stdout truncated: {}",
            raw.stdout.len()
        );
        assert!(
            raw.stderr.len() >= 128 * 1024,
            "stderr truncated: {}",
            raw.stderr.len()
        );
        assert!(
            raw.stdout.chars().all(|c| c == 'a'),
            "stdout carried stderr payload"
        );
        assert!(
            raw.stderr.chars().all(|c| c == 'b'),
            "stderr carried stdout payload"
        );
    }

    #[test]
    fn failed_spawn_is_reported_not_silenced() {
        // A name that exists on no PATH entry, so the spawn fails on every
        // platform for the same reason.
        let raw = Executor::with_program(NONEXISTENT_BIN, Duration::from_secs(5))
            .run(&["health".to_string()]);
        assert!(raw.spawn_failed);
        assert_eq!(raw.exit_code, None);
        assert!(!raw.stderr.is_empty(), "spawn error must carry a message");
    }

    #[test]
    fn argv_is_passed_through_unchanged() {
        let tmp = tempfile::tempdir().unwrap();
        let fx = FakeCli {
            echo_args: true,
            ..FakeCli::ok()
        }
        .write(tmp.path());
        let argv = fx.argv_for(&["health", "--contract", "CABC", "--format", "json"]);
        let raw = Executor::with_program(fx.program, Duration::from_secs(30)).run(&argv);
        // On any mismatch, report the whole process outcome (not just the
        // stdout bytes): an empty string alone cannot distinguish "no argv
        // seen" from "script never ran".
        assert_eq!(
            raw.stdout, "health|--contract|CABC|--format|json|",
            "spawn_failed={} exit={:?} stderr={:?}",
            raw.spawn_failed, raw.exit_code, raw.stderr
        );
        assert_eq!(raw.exit_code, Some(0));
    }

    #[test]
    fn bin_env_selects_the_program() {
        // The override is what lets integration tests use a fake CLI.
        std::env::set_var(BIN_ENV, NONEXISTENT_BIN);
        std::env::remove_var(BIN_ARGS_ENV);
        let raw = execute(&["health".to_string()]);
        std::env::remove_var(BIN_ENV);
        assert!(raw.spawn_failed);
    }
}
