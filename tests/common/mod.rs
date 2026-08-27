//! Shared test scaffolding: a deterministic fake pgbot binary (a POSIX shell
//! script — this is a test fixture, not product code; the product itself never
//! touches a shell) plus an env-mutation lock, since Rust tests share one
//! process and `std::env::set_var` is not thread-safe.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Serializes tests that mutate process env. Hold the guard for the whole test.
pub fn env_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    match LOCK.get_or_init(|| Mutex::new(())).lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> TempDir {
        let p = std::env::temp_dir().join(format!(
            "pgterm-it-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).expect("temp dir");
        TempDir(p)
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Writes the fake pgbot. Behavior is selected by the DSN it receives via
/// $DATABASE_URL (`postgres://mode-warn@x/db` → warn), mirroring how the real
/// pgbot reads its connection from the child environment:
///   healthy → context_healthy.json, exit 0
///   warn → context_warn.json, exit 1
///   critical → context_critical.json, exit 2
///   refuse → connection-refused stderr (with a DSN in it), exit 3
///   hang → sleep 60
/// `indexes`/`why` subcommands emit their own reports. Every invocation
/// appends a line to invocations.log; while running, a `running.<pid>` marker
/// exists so tests can measure peak concurrency.
pub fn write_fake_pgbot(dir: &Path) -> PathBuf {
    let fixtures = fixtures_dir();
    let bin = dir.join("fake-pgbot");
    let log = dir.join("invocations.log");
    let script = format!(
        r#"#!/bin/sh
FIX="{fixtures}"
DIR="{dir}"
case "$1" in
  --version|-v) echo "pgbot version 0.9.9"; exit 0;;
esac
echo "$* url=$DATABASE_URL" >> "{log}"
touch "$DIR/running.$$"
finish() {{ rm -f "$DIR/running.$$"; exit "$1"; }}
n=$(ls "$DIR" | grep -c '^running\.')
[ "$n" -gt "${{PEAK:-0}}" ] && echo "$n" >> "$DIR/peaks.log"
mode=other
case "$DATABASE_URL" in
  *mode-healthy*) mode=healthy;;
  *mode-warn*) mode=warn;;
  *mode-critical*) mode=critical;;
  *mode-refuse*) mode=refuse;;
  *mode-hang*) mode=hang;;
esac
case "$1" in
  indexes) cat "$FIX/indexes_report.json"; finish 0;;
  why) cat "$FIX/why_report.json"; finish 0;;
esac
case "$mode" in
  healthy) sleep "${{FAKE_PGBOT_DELAY:-0}}"; cat "$FIX/context_healthy.json"; finish 0;;
  warn) sleep "${{FAKE_PGBOT_DELAY:-0}}"; cat "$FIX/context_warn.json"; finish 1;;
  critical) cat "$FIX/context_critical.json"; finish 2;;
  refuse) echo "pgbot: connect postgres://alex:sekret-pw@db.internal:5432/app: connection refused" >&2; finish 3;;
  hang) sleep 60; finish 0;;
  *) echo "pgbot: no connection string (pass one or set \$DATABASE_URL)" >&2; finish 3;;
esac
"#,
        fixtures = fixtures.display(),
        dir = dir.display(),
        log = log.display(),
    );
    std::fs::write(&bin, script).expect("write fake pgbot");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    bin
}

/// The DSN the fake understands, mode baked in. Contains a fake password so
/// leak assertions have something to catch.
pub fn dsn(mode: &str) -> String {
    format!("postgres://tester:hunter2-{mode}@mode-{mode}.example:5432/app")
}
