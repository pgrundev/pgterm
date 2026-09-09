//! The pgrun CLI, for branches. The second sibling binary pgterm drives (after
//! pgbot), and the same rules apply: a closed argv set built directly, no
//! shell, sanitized errors, and a child environment with every database URL
//! removed — pgrun talks to the pgrun API, never to a database, so a DSN has
//! no business reaching it.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;

use crate::sanitize::{ErrorKind, SafeError};

/// The closed set of pgrun operations the terminal may run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PgrunCommand {
    /// Every branch of a project.
    List(String),
    /// One branch, which is also the only way to learn its connection URL.
    Get { project: String, branch: String },
}

/// Names come from pgrun's own API, but they travel as argv, so they are
/// checked before anything is spawned rather than trusted.
fn valid_slug(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        // A leading dash would be read as a flag, not a name.
        && !s.starts_with('-')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn args_for(cmd: &PgrunCommand) -> Option<Vec<String>> {
    let v = |parts: &[&str]| parts.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    match cmd {
        PgrunCommand::List(project) if valid_slug(project) => {
            Some(v(&["branch", "list", project, "--json"]))
        }
        PgrunCommand::Get { project, branch } if valid_slug(project) && valid_slug(branch) => {
            Some(v(&["branch", "get", project, branch, "--json"]))
        }
        _ => None,
    }
}

pub fn default_timeout(cmd: &PgrunCommand) -> Duration {
    match cmd {
        PgrunCommand::List(_) => Duration::from_secs(20),
        PgrunCommand::Get { .. } => Duration::from_secs(20),
    }
}

/// Locates pgrun: $PGRUN_BIN override → `pgrun` on PATH.
pub fn pgrun_bin() -> PathBuf {
    match std::env::var("PGRUN_BIN") {
        Ok(p) if !p.is_empty() => PathBuf::from(p),
        _ => PathBuf::from("pgrun"),
    }
}

/// One branch, as pgrun reports it. Unknown fields are ignored so a newer
/// pgrun never breaks an older pgterm.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Branch {
    pub id: String,
    pub name: String,
    /// ready | creating | snapshotting | provisioning | starting | deleting | failed | …
    pub status: String,
    pub is_base: bool,
    pub parent_branch_id: Option<String>,
    pub postgres_version: String,
    pub created_at: Option<String>,
    pub expires_at: Option<String>,
    /// Only `branch get` returns this. It is a secret: it reaches a session
    /// tab in memory and is never written or displayed.
    pub connection_url: Option<String>,
}

impl Branch {
    /// Still moving: pgterm should keep polling rather than treat it as final.
    pub fn in_progress(&self) -> bool {
        matches!(
            self.status.as_str(),
            "creating" | "snapshotting" | "provisioning" | "starting" | "deleting"
        )
    }

    pub fn failed(&self) -> bool {
        self.status == "failed"
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct BranchList {
    #[serde(deserialize_with = "crate::model::null_as_empty")]
    branches: Vec<Branch>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct BranchEnvelope {
    branch: Option<Branch>,
}

pub fn decode_list(json: &str) -> Result<Vec<Branch>, SafeError> {
    serde_json::from_str::<BranchList>(json)
        .map(|l| l.branches)
        .map_err(|e| {
            SafeError::new(
                ErrorKind::BadOutput,
                &format!("branch list: {e} (is pgrun older than pgterm?)"),
                None,
            )
        })
}

/// `branch get` may answer with the branch at the top level or wrapped in a
/// `branch` key; accept both so a shape change is not an outage.
pub fn decode_branch(json: &str) -> Result<Branch, SafeError> {
    if let Ok(env) = serde_json::from_str::<BranchEnvelope>(json) {
        if let Some(b) = env.branch {
            if !b.name.is_empty() {
                return Ok(b);
            }
        }
    }
    serde_json::from_str::<Branch>(json).map_err(|e| {
        SafeError::new(
            ErrorKind::BadOutput,
            &format!("branch: {e} (is pgrun older than pgterm?)"),
            None,
        )
    })
}

#[derive(Debug, Clone)]
pub struct PgrunOutcome {
    pub stdout: String,
}

pub async fn run_pgrun(
    bin: &Path,
    cmd: &PgrunCommand,
    timeout: Duration,
) -> Result<PgrunOutcome, SafeError> {
    let Some(args) = args_for(cmd) else {
        return Err(SafeError::new(
            ErrorKind::Usage,
            "refusing to run pgrun with an unexpected project or branch name",
            None,
        ));
    };

    let mut c = tokio::process::Command::new(bin);
    c.args(args)
        // pgrun reads its own token from ~/.config/pgrun/config.json or
        // $PGRUN_API_TOKEN; a database URL would only be a leak.
        .env_remove("DATABASE_URL")
        .env_remove("PGBOT_DATABASE_URL")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let child = match c.spawn() {
        Ok(ch) => ch,
        Err(e) => {
            return Err(SafeError::new(
                ErrorKind::PgrunMissing,
                &format!("{}: {e}", bin.display()),
                None,
            ))
        }
    };

    let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Err(SafeError::new(ErrorKind::BadOutput, &e.to_string(), None)),
        Err(_) => {
            return Err(SafeError::new(
                ErrorKind::Timeout,
                &format!("pgrun gave no answer within {}s", timeout.as_secs()),
                None,
            ))
        }
    };

    if output.status.success() {
        return Ok(PgrunOutcome {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        });
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let msg = stderr
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("pgrun failed")
        .trim()
        .strip_prefix("pgrun: ")
        .unwrap_or_else(|| {
            stderr
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("pgrun failed")
                .trim()
        })
        .to_string();
    Err(SafeError::new(ErrorKind::PgrunFailed, &msg, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_is_closed_and_names_are_checked_before_spawning() {
        assert_eq!(
            args_for(&PgrunCommand::List("jobsgpt".into())).unwrap(),
            vec!["branch", "list", "jobsgpt", "--json"]
        );
        assert_eq!(
            args_for(&PgrunCommand::Get {
                project: "jobsgpt".into(),
                branch: "test32".into()
            })
            .unwrap(),
            vec!["branch", "get", "jobsgpt", "test32", "--json"]
        );
        for bad in [
            "",
            "a b",
            "--force",
            "x;rm -rf /",
            "$(whoami)",
            "a/b",
            "'quoted'",
            &"x".repeat(65),
        ] {
            assert!(
                args_for(&PgrunCommand::List(bad.to_string())).is_none(),
                "project {bad:?} must be refused"
            );
            assert!(
                args_for(&PgrunCommand::Get {
                    project: "ok".into(),
                    branch: bad.to_string()
                })
                .is_none(),
                "branch {bad:?} must be refused"
            );
        }
    }

    #[test]
    fn list_decodes_pgruns_real_shape() {
        let json = r#"{"branches":[
            {"id":"branch_1","name":"main","status":"ready","is_base":true,
             "parent_branch_id":null,"postgres_version":"18",
             "created_at":"2026-09-03T18:19:26Z","expires_at":null,"ready_at":null},
            {"id":"branch_14","name":"test32","status":"creating","is_base":false,
             "parent_branch_id":"branch_1","postgres_version":"18",
             "created_at":"2026-09-09T02:15:31Z","expires_at":null}
        ]}"#;
        let bs = decode_list(json).unwrap();
        assert_eq!(bs.len(), 2);
        assert!(bs[0].is_base && !bs[0].in_progress());
        assert_eq!(bs[0].parent_branch_id, None, "null decodes as absent");
        assert!(bs[1].in_progress() && !bs[1].failed());
        assert_eq!(bs[1].parent_branch_id.as_deref(), Some("branch_1"));
        assert!(bs[1].connection_url.is_none(), "list never carries the URL");

        assert!(decode_list(r#"{"branches":null}"#).unwrap().is_empty());
        assert!(decode_list("{}").unwrap().is_empty());
        assert_eq!(
            decode_list("not json").unwrap_err().kind,
            ErrorKind::BadOutput
        );
    }

    #[test]
    fn get_decodes_bare_or_wrapped_and_carries_the_url() {
        let bare = r#"{"id":"branch_14","name":"test32","status":"ready",
            "connection_url":"postgres://u:pw@h/db"}"#;
        let b = decode_branch(bare).unwrap();
        assert_eq!(b.name, "test32");
        assert_eq!(b.connection_url.as_deref(), Some("postgres://u:pw@h/db"));

        let wrapped = format!(r#"{{"branch":{bare}}}"#);
        let b = decode_branch(&wrapped).unwrap();
        assert_eq!(b.name, "test32");
        assert!(b.connection_url.is_some());

        assert_eq!(
            decode_branch("not json").unwrap_err().kind,
            ErrorKind::BadOutput
        );
    }

    #[test]
    fn failed_and_in_progress_cover_pgruns_statuses() {
        let with = |s: &str| Branch {
            status: s.into(),
            ..Default::default()
        };
        for s in [
            "creating",
            "snapshotting",
            "provisioning",
            "starting",
            "deleting",
        ] {
            assert!(with(s).in_progress(), "{s}");
        }
        assert!(with("failed").failed());
        assert!(!with("ready").in_progress() && !with("ready").failed());
    }
}
