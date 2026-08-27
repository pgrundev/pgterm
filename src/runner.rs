//! The one place that spawns pgbot. Every diagnostic the UI shows comes
//! through `run_pgbot`: resolve the profile's env var in memory, hand the
//! secret to the child via ITS environment (never argv), enforce a deadline,
//! and map pgbot's exit-code contract (0/1/2 = JSON on stdout; 3 = failure on
//! stderr; 64 = our bug) into typed results with sanitized errors.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use crate::sanitize::{ErrorKind, SafeError};

/// Where a database's DSN comes from. `Env` is the persisted form (config
/// stores the variable's NAME); `Session` is a URL pasted into the running
/// UI — memory only, never written to disk, gone when pgterm exits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnSource {
    Env(String),
    Session(String),
}

impl ConnSource {
    /// Resolves the secret in memory at spawn time.
    pub fn resolve(&self) -> Result<String, SafeError> {
        match self {
            ConnSource::Env(name) => match std::env::var(name) {
                Ok(v) if !v.trim().is_empty() => Ok(v),
                _ => Err(SafeError::new(
                    ErrorKind::EnvMissing,
                    &format!(
                        "Environment variable {name} is not set — export it in the shell that launches pgterm, then restart."
                    ),
                    None,
                )),
            },
            ConnSource::Session(url) => Ok(url.clone()),
        }
    }

    /// What the UI may show for this source. Never the secret.
    pub fn label(&self) -> &str {
        match self {
            ConnSource::Env(name) => name,
            ConnSource::Session(_) => "pasted URL — this session only",
        }
    }
}

/// The closed set of pgbot operations the terminal may run. There is no
/// variant that could carry a shell string — `Ask`'s payload travels as a
/// single argv element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PgbotCommand {
    /// Background health sweep: lightest full inspection, no baseline writes.
    Monitor,
    /// User-requested inspection; writes the baseline store so `why` history
    /// accrues, exactly like running `pgbot inspect` by hand.
    InspectFull,
    /// The graded unused-index correlation report.
    Indexes,
    /// Offline causal analysis from the local baseline store.
    Why,
    /// Connection validation for `terminal add`: catalog-only, store-free,
    /// exit 0 unless the connection itself fails.
    Probe,
    /// `pgbot ask` (AI): the question is one argument, never interpolated.
    Ask(String),
}

pub fn args_for(cmd: &PgbotCommand) -> Vec<String> {
    let s = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    match cmd {
        PgbotCommand::Monitor => s(&[
            "inspect",
            "--json",
            "--ash-hz=0",
            "--interval=500ms",
            "--no-store",
            "--timeout=15s",
        ]),
        PgbotCommand::InspectFull => s(&[
            "inspect",
            "--json",
            "--ash-hz=0",
            "--interval=1s",
            "--timeout=30s",
        ]),
        PgbotCommand::Indexes => s(&["indexes", "--json", "--timeout=30s"]),
        PgbotCommand::Why => s(&["why", "--json"]),
        PgbotCommand::Probe => s(&[
            "inspect",
            "--json",
            "--profile=schema",
            "--no-store",
            "--fail-on=none",
            "--timeout=10s",
        ]),
        PgbotCommand::Ask(question) => {
            let mut v = s(&["ask", "--yes"]);
            v.push(question.clone());
            v
        }
    }
}

/// Wall-clock budget for the child: its own `--timeout` plus grace to
/// connect, render and exit. `why` is offline; `ask` waits on an AI vendor.
pub fn default_timeout(cmd: &PgbotCommand) -> Duration {
    match cmd {
        PgbotCommand::Monitor => Duration::from_secs(20),
        PgbotCommand::InspectFull | PgbotCommand::Indexes => Duration::from_secs(35),
        PgbotCommand::Why => Duration::from_secs(30),
        PgbotCommand::Probe => Duration::from_secs(15),
        PgbotCommand::Ask(_) => Duration::from_secs(120),
    }
}

/// Locates the pgbot binary: $PGBOT_BIN override → `pgbot` on PATH.
pub fn pgbot_bin() -> PathBuf {
    match std::env::var("PGBOT_BIN") {
        Ok(p) if !p.is_empty() => PathBuf::from(p),
        _ => PathBuf::from("pgbot"),
    }
}

#[derive(Debug, Clone)]
pub struct RunOutcome {
    pub stdout: String,
    /// pgbot's exit code: 0 clean, 1 warnings, 2 criticals — all with valid
    /// JSON on stdout.
    pub exit: i32,
}

pub async fn run_pgbot(
    pgbot_bin: &Path,
    source: &ConnSource,
    cmd: &PgbotCommand,
    timeout: Duration,
) -> Result<RunOutcome, SafeError> {
    let secret = source.resolve()?;

    let mut c = tokio::process::Command::new(pgbot_bin);
    c.args(args_for(cmd))
        .env("DATABASE_URL", &secret)
        // Lower-precedence fallback in pgbot; remove it so the child sees
        // exactly one connection source.
        .env_remove("PGBOT_DATABASE_URL")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let child = match c.spawn() {
        Ok(ch) => ch,
        Err(e) => {
            return Err(SafeError::new(
                ErrorKind::PgbotMissing,
                &format!("{}: {e}", pgbot_bin.display()),
                Some(&secret),
            ))
        }
    };

    let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            return Err(SafeError::new(
                ErrorKind::BadOutput,
                &e.to_string(),
                Some(&secret),
            ))
        }
        // The future holding the child is dropped here; kill_on_drop reaps it.
        Err(_) => {
            return Err(SafeError::new(
                ErrorKind::Timeout,
                &format!("no answer within {}s", timeout.as_secs()),
                Some(&secret),
            ))
        }
    };

    let stderr = String::from_utf8_lossy(&output.stderr);
    match output.status.code() {
        Some(code @ 0..=2) => Ok(RunOutcome {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            exit: code,
        }),
        Some(3) => Err(SafeError::new(
            ErrorKind::ConnectionFailed,
            strip_pgbot_prefix(&stderr),
            Some(&secret),
        )),
        Some(64) => Err(SafeError::new(
            ErrorKind::Usage,
            strip_pgbot_prefix(&stderr),
            Some(&secret),
        )),
        other => Err(SafeError::new(
            ErrorKind::BadOutput,
            &format!("exit {:?}: {}", other, strip_pgbot_prefix(&stderr)),
            Some(&secret),
        )),
    }
}

/// pgbot prefixes fatal errors with "pgbot: " — drop it, the UI adds its own
/// framing.
fn strip_pgbot_prefix(stderr: &str) -> &str {
    let line = stderr.trim();
    line.strip_prefix("pgbot: ").unwrap_or(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monitor_args_are_light_and_store_free() {
        assert_eq!(
            args_for(&PgbotCommand::Monitor),
            vec![
                "inspect",
                "--json",
                "--ash-hz=0",
                "--interval=500ms",
                "--no-store",
                "--timeout=15s"
            ]
        );
    }

    #[test]
    fn inspect_full_writes_the_store() {
        let args = args_for(&PgbotCommand::InspectFull);
        assert_eq!(args[0], "inspect");
        assert!(args.contains(&"--json".to_string()));
        assert!(
            !args.contains(&"--no-store".to_string()),
            "manual inspect must accrue history for `why`"
        );
    }

    #[test]
    fn indexes_and_why_use_their_own_json_contracts() {
        assert_eq!(
            args_for(&PgbotCommand::Indexes),
            vec!["indexes", "--json", "--timeout=30s"]
        );
        assert_eq!(args_for(&PgbotCommand::Why), vec!["why", "--json"]);
    }

    #[test]
    fn probe_is_schema_only_and_never_gates_on_findings() {
        let args = args_for(&PgbotCommand::Probe);
        assert!(args.contains(&"--profile=schema".to_string()));
        assert!(args.contains(&"--no-store".to_string()));
        assert!(args.contains(&"--fail-on=none".to_string()));
    }

    #[test]
    fn ask_payload_is_one_argv_element_never_interpolated() {
        let args = args_for(&PgbotCommand::Ask("why slow; rm -rf / $(whoami)".into()));
        assert_eq!(args, vec!["ask", "--yes", "why slow; rm -rf / $(whoami)"]);
        // The hostile string survives as a single inert argument.
    }

    #[test]
    fn no_command_ever_carries_a_connection_string_in_argv() {
        for cmd in [
            PgbotCommand::Monitor,
            PgbotCommand::InspectFull,
            PgbotCommand::Indexes,
            PgbotCommand::Why,
            PgbotCommand::Probe,
        ] {
            for a in args_for(&cmd) {
                assert!(!a.contains("://"), "{cmd:?} leaked a URL-shaped arg: {a}");
                assert!(!a.to_lowercase().contains("password"), "{cmd:?}: {a}");
            }
        }
    }

    #[test]
    fn timeouts_exceed_the_childs_own_budget() {
        // --timeout=15s child budget < our 20s deadline, and so on.
        assert!(default_timeout(&PgbotCommand::Monitor) > Duration::from_secs(15));
        assert!(default_timeout(&PgbotCommand::InspectFull) > Duration::from_secs(30));
        assert!(default_timeout(&PgbotCommand::Probe) > Duration::from_secs(10));
    }
}
