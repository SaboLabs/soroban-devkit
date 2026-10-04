// Cross-platform fake CLI for the agent's executor tests.
//
// The executor's contract is about process behaviour — separate streams, an
// exact exit code, a real timeout, argv passed through unchanged. Proving
// that needs a child process the test fully controls, on every platform.
//
// A `#!/bin/sh` script cannot do that on Windows: `.sh` is not a runnable
// image there, so `CreateProcess` fails and the test observes a spawn error
// instead of the behaviour under test. Instead, one declarative [`FakeCli`]
// spec is emitted as a native script per platform:
//
// | platform | program | script |
// |----------|---------|--------|
// | unix     | the script itself (mode 0755) | `#!/bin/sh` + `printf` |
// | windows  | `powershell.exe` | `.ps1` using `[Console]::Out/Error` |
//
// Both emit **byte-identical** stdout/stderr for the same spec, so every
// assertion downstream is platform-independent — including the exact argv
// echo, which is why PowerShell is used rather than `cmd.exe` (cmd cannot
// reproduce `printf '%s|' "$@"` faithfully).
//
// Payloads are embedded as literal strings and escaped per platform, so the
// spec is written once and never duplicated as shell syntax.
//
// This file is shared by the unit tests in `src/executor.rs` (via `include!`)
// and by `tests/agent_e2e.rs` (via `#[path]`), so there is one source of
// truth for how a fake `sdkt` behaves.

use std::fs;
use std::path::{Path, PathBuf};

/// Declarative description of what the fake CLI should do when invoked.
#[derive(Debug, Clone, Default)]
pub struct FakeCli {
    /// Bytes written to stdout, verbatim.
    pub stdout: String,
    /// Bytes written to stderr, verbatim.
    pub stderr: String,
    /// Process exit code.
    pub exit_code: i32,
    /// Milliseconds to sleep before writing anything (for timeout tests).
    pub sleep_ms: u64,
    /// Emit this many bytes on EACH stream (`a` on stdout, `b` on stderr),
    /// beyond the OS pipe buffer, to prove the executor drains both pipes
    /// concurrently and keeps the payloads separate.
    pub big_bytes: usize,
    /// Echo the received argv as `arg|arg|` instead of using `stdout`.
    pub echo_args: bool,
    /// Create this file when invoked, so a test can prove no execution
    /// happened by asserting the file is absent.
    pub marker: Option<PathBuf>,
}

/// A prepared fake CLI: the program to run plus any fixed leading arguments.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Fixture {
    /// Program to hand to the executor.
    pub program: String,
    /// Arguments that must precede the test's own argv.
    pub prefix: Vec<String>,
}

impl Fixture {
    /// Full argument vector for a test's own args: the fixture's fixed leading
    /// arguments (empty on unix) followed by the args under test.
    ///
    /// Used by the executor unit tests; the integration tests drive the agent
    /// binary instead, which applies the prefix itself.
    #[allow(dead_code)]
    pub fn argv_for(&self, args: &[&str]) -> Vec<String> {
        self.prefix
            .iter()
            .cloned()
            .chain(args.iter().map(|a| (*a).to_string()))
            .collect()
    }
}

impl FakeCli {
    /// A fake that exits 0 having done nothing.
    pub fn ok() -> Self {
        Self::default()
    }

    /// Write the script for this spec into `dir` and return how to run it.
    pub fn write(&self, dir: &Path) -> Fixture {
        if cfg!(windows) {
            self.write_powershell(dir)
        } else {
            self.write_sh(dir)
        }
    }

    /// Unix: a `#!/bin/sh` script with the executable bit set.
    fn write_sh(&self, dir: &Path) -> Fixture {
        let path = dir.join("fake-sdkt.sh");
        let mut body = String::from("#!/bin/sh\n");

        if let Some(marker) = &self.marker {
            body.push_str(&format!(": > '{}'\n", sh_quote(&marker.to_string_lossy())));
        }
        if self.sleep_ms > 0 {
            // `sleep` takes seconds; sub-second waits use a fractional value.
            body.push_str(&format!("sleep {}\n", self.sleep_ms as f64 / 1000.0));
        }

        if self.echo_args {
            // `printf '%s|'` per argument: no trailing newline, no separators
            // beyond the pipe, exactly as received.
            body.push_str("printf '%s|' \"$@\"\n");
        } else if self.big_bytes > 0 {
            let out = "a".repeat(64);
            let err = "b".repeat(64);
            let reps = self.big_bytes.div_ceil(out.len());
            body.push_str(&format!(
                "i=0\nwhile [ $i -lt {reps} ]; do printf '%s' '{out}'; i=$((i+1)); done\n"
            ));
            body.push_str(&format!(
                "i=0\nwhile [ $i -lt {reps} ]; do printf '%s' '{err}' 1>&2; i=$((i+1)); done\n"
            ));
        } else {
            if !self.stdout.is_empty() {
                body.push_str(&format!("printf '%s' {}\n", sh_quote(&self.stdout)));
            }
            if !self.stderr.is_empty() {
                // `1>&2` redirects this command's stdout to stderr; it never
                // merges the two streams into one.
                body.push_str(&format!("printf '%s' {} 1>&2\n", sh_quote(&self.stderr)));
            }
        }

        body.push_str(&format!("exit {}\n", self.exit_code));
        fs::write(&path, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        Fixture {
            program: path.to_string_lossy().into_owned(),
            prefix: Vec::new(),
        }
    }

    /// Windows: a PowerShell script, invoked through `powershell.exe`.
    ///
    /// PowerShell is used instead of `cmd.exe` because it can write exact
    /// bytes with no trailing newline and join `$args` the same way the Unix
    /// fixture does, keeping every assertion platform-independent.
    fn write_powershell(&self, dir: &Path) -> Fixture {
        let path = dir.join("fake-sdkt.ps1");
        let mut body = String::new();
        // Windows PowerShell 5.1 defaults the console to UTF-16, which would
        // double every byte and break the exact-output assertions. Force
        // UTF-8 without a BOM, and flush explicitly because [Console]::Error
        // is autoflush while [Console]::Out is not.
        body.push_str("$OutputEncoding = [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding $false\n");

        if let Some(marker) = &self.marker {
            body.push_str(&format!(
                "[IO.File]::WriteAllText('{}', '')\n",
                ps_quote(&marker.to_string_lossy())
            ));
        }
        if self.sleep_ms > 0 {
            body.push_str(&format!("Start-Sleep -Milliseconds {}\n", self.sleep_ms));
        }

        if self.echo_args {
            body.push_str("[Console]::Out.Write(($args -join '|') + '|')\n");
        } else if self.big_bytes > 0 {
            // Repeat a 64-byte chunk so both streams exceed the pipe buffer.
            let out = "a".repeat(64);
            let err = "b".repeat(64);
            let reps = self.big_bytes.div_ceil(out.len());
            body.push_str(&format!(
                "[Console]::Out.Write('{out}' * {reps})\n[Console]::Error.Write('{err}' * {reps})\n"
            ));
        } else {
            if !self.stdout.is_empty() {
                body.push_str(&format!(
                    "[Console]::Out.Write('{}')\n",
                    ps_quote(&self.stdout)
                ));
            }
            if !self.stderr.is_empty() {
                body.push_str(&format!(
                    "[Console]::Error.Write('{}')\n",
                    ps_quote(&self.stderr)
                ));
            }
        }

        body.push_str("[Console]::Out.Flush()\n");
        body.push_str(&format!("exit {}\n", self.exit_code));
        fs::write(&path, body).unwrap();
        Fixture {
            program: "powershell.exe".to_string(),
            prefix: vec![
                "-NoProfile".to_string(),
                "-NonInteractive".to_string(),
                "-ExecutionPolicy".to_string(),
                "Bypass".to_string(),
                "-File".to_string(),
                path.to_string_lossy().into_owned(),
            ],
        }
    }
}

/// Single-quote a string for `sh`, escaping embedded single quotes.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Single-quote a string for PowerShell.
///
/// Doubles embedded single quotes and doubles backticks, because PowerShell
/// treats the backtick as an escape character even inside a single-quoted
/// string. Without this, a payload containing `` ` `` would silently write
/// different bytes on Windows than on unix.
fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''").replace('`', "``"))
}
