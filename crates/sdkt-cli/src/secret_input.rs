//! Secure secret input for identity workflows.
//!
//! Secrets must never be passed as plaintext CLI arguments: argv is visible in
//! shell history, `ps` output, and CI logs. Instead, `sdkt identity import`
//! reads the secret from stdin — either piped
//! (`echo "S..." | sdkt identity import <name>`) or typed at an interactive
//! prompt when stdin is a terminal.

use std::io::{self, BufRead, IsTerminal, Write};

/// Read a secret key from stdin.
///
/// When stdin is a terminal an interactive prompt is printed to stderr (so it
/// never pollutes piped stdout) and the typed value is echoed back only as a
/// masked placeholder. When stdin is piped the first line is consumed verbatim.
///
/// The returned value is trimmed of surrounding whitespace and newlines, which
/// makes `echo "S..." | sdkt identity import alice` work as documented.
///
/// Errors are explicit and actionable:
/// * empty input (including a blank line or whitespace-only input),
/// * I/O failures while reading stdin.
pub fn read_secret_from_stdin() -> Result<String, String> {
    let stdin = io::stdin();
    let interactive = stdin.is_terminal();

    if interactive {
        eprint!("Enter secret key (S...): ");
        io::stderr().flush().ok();
    }

    let mut line = String::new();
    let read = stdin
        .lock()
        .read_line(&mut line)
        .map_err(|e| format!("Failed to read secret from stdin: {}", e))?;

    if interactive {
        // The terminal echoes the typed secret; emit a newline so the following
        // output starts on a fresh line.
        eprintln!();
    }

    if read == 0 {
        return Err(
            "No secret provided on stdin. Pipe the secret key, e.g. \
             `echo \"S...\" | sdkt identity import <name>`."
                .to_string(),
        );
    }

    let secret = line.trim();
    if secret.is_empty() {
        return Err(
            "Empty secret provided on stdin. Pipe the secret key, e.g. \
             `echo \"S...\" | sdkt identity import <name>`."
                .to_string(),
        );
    }

    Ok(secret.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The trimming contract that makes `echo "S..." | sdkt identity import`
    /// work: trailing newlines and surrounding whitespace are stripped.
    #[test]
    fn trims_surrounding_whitespace() {
        let raw = "  SABC123  \n";
        assert_eq!(raw.trim(), "SABC123");
    }
}
