//! Shared test scaffolding: the fake pgbot binary plus an env-mutation lock,
//! since Rust tests share one process and `set_var` is not thread-safe.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

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

/// Copies the built `fake-pgbot` binary into `dir`; it keeps its scratch state
/// (invocations.log, running.<pid> markers, peaks.log) next to itself.
pub fn write_fake_pgbot(dir: &Path) -> PathBuf {
    let built = PathBuf::from(env!("CARGO_BIN_EXE_fake-pgbot"));
    let bin = dir.join(format!("fake-pgbot{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(&built, &bin)
        .unwrap_or_else(|e| panic!("copying {} -> {}: {e}", built.display(), bin.display()));
    bin
}

/// The DSN the fake understands, mode baked in. Contains a fake password so
/// leak assertions have something to catch.
pub fn dsn(mode: &str) -> String {
    format!("postgres://tester:hunter2-{mode}@mode-{mode}.example:5432/app")
}
