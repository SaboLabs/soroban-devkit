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
    Executor::for_sdkt().run(args)
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
    use std::fs;
    use std::path::Path;

    /// Write an executable shell script and return its path.
    fn fake_script(dir: &Path, body: &str) -> String {
        let path = dir.join("fake-sdkt.sh");
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn exit_code_is_recorded_not_swallowed() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = fake_script(tmp.path(), "echo ok\nexit 3\n");
        let raw = Executor::with_program(bin, Duration::from_secs(5)).run(&[]);
        assert_eq!(raw.exit_code, Some(3));
        assert_eq!(raw.stdout, "ok\n");
        assert!(raw.stderr.is_empty());
        assert!(!raw.timed_out);
    }

    #[test]
    fn stdout_and_stderr_never_mix() {
        let tmp = tempfile::tempdir().unwrap();
        // stdout: pure JSON. stderr: human noise + shell warnings.
        let body = r#"printf '{"health":"critical"}\n'
echo "warning: something on stderr" 1>&2
echo "Error: actionable failure" 1>&2
exit 1"#;
        let bin = fake_script(tmp.path(), body);
        let raw = Executor::with_program(bin, Duration::from_secs(5)).run(&[]);
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
        let bin = fake_script(tmp.path(), "sleep 5\n");
        let raw = Executor::with_program(bin, Duration::from_millis(150)).run(&[]);
        assert!(raw.timed_out, "expected a timeout");
        assert_eq!(raw.exit_code, None, "a killed run has no usable code");
    }

    #[test]
    fn large_output_on_both_streams_does_not_deadlock() {
        let tmp = tempfile::tempdir().unwrap();
        // ~128 KiB on each stream, well past the 64 KiB pipe buffer.
        let body = "i=0\nwhile [ $i -lt 2000 ]; do echo \"line $i aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"; echo \"err $i bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\" 1>&2; i=$((i+1)); done";
        let bin = fake_script(tmp.path(), body);
        let raw = Executor::with_program(bin, Duration::from_secs(20)).run(&[]);
        assert_eq!(raw.exit_code, Some(0));
        assert!(raw.stdout.contains("line 1999"));
        assert!(raw.stderr.contains("err 1999"));
    }

    #[test]
    fn failed_spawn_is_reported_not_silenced() {
        let raw = Executor::with_program(
            "/nonexistent/sdkt-binary-does-not-exist",
            Duration::from_secs(5),
        )
        .run(&["health".to_string()]);
        assert!(raw.spawn_failed);
        assert_eq!(raw.exit_code, None);
        assert!(!raw.stderr.is_empty(), "spawn error must carry a message");
    }

    #[test]
    fn argv_is_passed_through_unchanged() {
        let tmp = tempfile::tempdir().unwrap();
        // Echo the received arguments so the test can assert exact forwarding.
        let bin = fake_script(tmp.path(), "printf '%s|' \"$@\"");
        let raw = Executor::with_program(bin, Duration::from_secs(5)).run(&[
            "health".to_string(),
            "--contract".to_string(),
            "CABC".to_string(),
            "--format".to_string(),
            "json".to_string(),
        ]);
        assert_eq!(raw.stdout, "health|--contract|CABC|--format|json|");
        assert_eq!(raw.exit_code, Some(0));
    }

    #[test]
    fn bin_env_selects_the_program() {
        // The override is what lets integration tests use a fake CLI.
        std::env::set_var(BIN_ENV, "/tmp/definitely-not-sdkt");
        let raw = execute(&["health".to_string()]);
        std::env::remove_var(BIN_ENV);
        assert!(raw.spawn_failed);
    }
}
