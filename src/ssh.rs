//! Reaching a database through an SSH jump host.
//!
//! pgterm does not speak SSH itself — it runs the user's own `ssh` with `-W`,
//! which pipes the database connection over the child's stdin/stdout. That is
//! not an `ssh -L` port forward: no local port is opened, and the DSN keeps
//! naming the REAL database host all the way through, so `sslmode=verify-full`
//! still validates against that hostname and `.pgpass` still matches on it.
//!
//! Delegating to the binary is deliberate. `~/.ssh/config` (HostName, User,
//! Port, IdentityFile, ProxyJump, ControlMaster), the agent, hardware keys and
//! known_hosts all behave exactly the way the user's own `ssh` already does
//! for that host — none of it re-implemented, none of it subtly different.
//! BatchMode keeps a TUI-owned terminal safe: ssh fails with its reason on
//! stderr instead of prompting into a screen that cannot answer.
//!
//! The health checks never come through here: pgbot has native `--ssh-tunnel`
//! support, so the runner hands it the spec via PGBOT_SSH_TUNNEL instead.

use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use tokio::io::{AsyncBufReadExt as _, AsyncRead, AsyncWrite, ReadBuf};
use tokio::process::{Child, ChildStdin, ChildStdout};

use crate::sanitize::{ErrorKind, SafeError};

/// A validated `[user@]host[:port]` jump-host spec. A bare host may be an
/// ssh_config alias; ssh resolves it, not us.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spec {
    pub user: Option<String>,
    pub host: String,
    pub port: Option<u16>,
}

impl Spec {
    /// Parses and validates. The parts land in argv, so anything that could
    /// read as an ssh option or smuggle in whitespace is refused outright.
    pub fn parse(spec: &str) -> Result<Spec, String> {
        let raw = spec.trim();
        if raw.is_empty() {
            return Err("ssh spec is empty — want [user@]host[:port]".into());
        }
        if raw.contains("://") {
            return Err("ssh spec is [user@]host[:port], not a URL".into());
        }
        let (user, rest) = match raw.rsplit_once('@') {
            Some((u, r)) => (Some(u.to_string()), r),
            None => (None, raw),
        };
        // Bracketed IPv6 keeps its colons; a single colon splits off a port; a
        // bare IPv6 address has no unambiguous port syntax, so it is left alone.
        let (host, port) = if let Some(inner) = rest.strip_prefix('[') {
            match inner.split_once(']') {
                Some((h, "")) => (h.to_string(), None),
                Some((h, tail)) => match tail.strip_prefix(':') {
                    Some(p) => (h.to_string(), Some(p)),
                    None => return Err(format!("malformed ssh spec {raw:?}")),
                },
                None => return Err(format!("malformed ssh spec {raw:?}")),
            }
        } else if rest.matches(':').count() == 1 {
            let (h, p) = rest.split_once(':').expect("counted one");
            (h.to_string(), Some(p))
        } else {
            (rest.to_string(), None)
        };
        let port = match port {
            None => None,
            Some(p) => Some(
                p.parse::<u16>()
                    .ok()
                    .filter(|p| *p > 0)
                    .ok_or_else(|| format!("{p:?} is not a port number"))?,
            ),
        };
        for (what, s) in [("user", user.as_deref().unwrap_or("x")), ("host", &host)] {
            if s.is_empty() {
                return Err(format!("ssh spec has an empty {what}"));
            }
            if s.starts_with('-') {
                return Err(format!("ssh {what} may not start with '-'"));
            }
            if !s
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '%'))
            {
                return Err(format!(
                    "ssh {what} may only contain letters, digits, '.', '-', '_' and ':'"
                ));
            }
        }
        Ok(Spec { user, host, port })
    }
}

/// $PGTERM_SSH_BIN override → `ssh` on PATH. The same escape hatch pgbot and
/// pgrun binaries get.
fn ssh_bin() -> std::path::PathBuf {
    match std::env::var("PGTERM_SSH_BIN") {
        Ok(p) if !p.is_empty() => std::path::PathBuf::from(p),
        _ => std::path::PathBuf::from("ssh"),
    }
}

/// The database connection, riding an `ssh -W` child's stdio. Dropping the
/// stream kills the child (kill_on_drop), so a closed SQL tab leaves no ssh
/// behind.
pub struct SshStream {
    stdin: ChildStdin,
    stdout: ChildStdout,
    stderr: Arc<Mutex<String>>,
    _child: Child,
}

impl SshStream {
    /// Whatever ssh has said so far — "Permission denied (publickey)", "Host
    /// key verification failed" — for the error path. Empty means silence.
    pub fn stderr_handle(&self) -> Arc<Mutex<String>> {
        self.stderr.clone()
    }
}

/// Opens the tunnel: `ssh -W db_host:db_port [-l user] [-p port] -- host`.
/// The child is spawned, not awaited — a refused login surfaces as EOF on the
/// stream, with the reason in `stderr_handle`.
pub fn open(spec: &Spec, db_host: &str, db_port: u16) -> Result<SshStream, SafeError> {
    let bin = ssh_bin();
    let mut c = tokio::process::Command::new(&bin);
    // IPv6 database hosts are bracketed for -W, as ssh expects.
    let target = if db_host.contains(':') {
        format!("[{db_host}]:{db_port}")
    } else {
        format!("{db_host}:{db_port}")
    };
    c.arg("-W").arg(target);
    // BatchMode: fail with the reason rather than prompt into the TUI.
    // -W already implies -N, -T, ExitOnForwardFailure and ClearAllForwardings.
    for opt in [
        "BatchMode=yes",
        "ConnectTimeout=10",
        "ServerAliveInterval=30",
        "ServerAliveCountMax=3",
    ] {
        c.arg("-o").arg(opt);
    }
    if let Some(user) = &spec.user {
        c.arg("-l").arg(user);
    }
    if let Some(port) = spec.port {
        c.arg("-p").arg(port.to_string());
    }
    c.arg("--").arg(&spec.host);
    c.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    let mut child = c.spawn().map_err(|e| {
        SafeError::new(
            ErrorKind::ConnectionFailed,
            &format!(
                "cannot run {} for the ssh tunnel: {e} — an OpenSSH client is required (or set PGTERM_SSH_BIN)",
                bin.display()
            ),
            None,
        )
    })?;
    let stdin = child.stdin.take().expect("piped");
    let stdout = child.stdout.take().expect("piped");
    let err_pipe = child.stderr.take().expect("piped");
    let stderr = Arc::new(Mutex::new(String::new()));
    let sink = stderr.clone();
    // Line by line, not read_to_string: the reason must be there when the
    // stream fails, not only once ssh has exited and closed the pipe.
    tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(err_pipe).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Ok(mut s) = sink.lock() {
                if !s.is_empty() {
                    s.push('\n');
                }
                s.push_str(&line);
            }
        }
    });
    Ok(SshStream {
        stdin,
        stdout,
        stderr,
        _child: child,
    })
}

impl AsyncRead for SshStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stdout).poll_read(cx, buf)
    }
}

impl AsyncWrite for SshStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.stdin).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stdin).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stdin).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_parse_the_ssh_shapes() {
        assert_eq!(
            Spec::parse("bastion").unwrap(),
            Spec {
                user: None,
                host: "bastion".into(),
                port: None
            }
        );
        assert_eq!(
            Spec::parse("deploy@bastion.example.com:2222").unwrap(),
            Spec {
                user: Some("deploy".into()),
                host: "bastion.example.com".into(),
                port: Some(2222)
            }
        );
        assert_eq!(
            Spec::parse("[::1]:2222").unwrap(),
            Spec {
                user: None,
                host: "::1".into(),
                port: Some(2222)
            }
        );
        // Bare IPv6: the colons are the address, not a port.
        assert_eq!(Spec::parse("fe80::1").unwrap().port, None);
        // Whitespace around the spec is tolerated, inside it is not.
        assert!(Spec::parse("  bastion  ").is_ok());
    }

    #[test]
    fn hostile_specs_are_refused() {
        // Anything that could become an ssh option or extra argv.
        for bad in [
            "",
            "-oProxyCommand=evil",
            "-J other",
            "user@-host",
            "host extra",
            "host\targ",
            "ssh://host",
            "postgres://u:p@h/db",
            "host:notaport",
            "host:0",
            "host:99999",
            "@host",
            "user@",
            "[::1",
            "$(whoami)@host",
            "host;rm",
        ] {
            assert!(Spec::parse(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn user_and_port_survive_an_at_sign_in_passwords_never_seen_here() {
        // rsplit on '@': the LAST @ separates user from host, like ssh.
        let s = Spec::parse("we.ird-user@host").unwrap();
        assert_eq!(s.user.as_deref(), Some("we.ird-user"));
        assert_eq!(s.host, "host");
    }
}
