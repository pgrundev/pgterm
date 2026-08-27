//! Acceptance tests for `pgterm add|list|remove`, run against the
//! real binary with a fake pgbot as PGBOT_BIN. Each test gets its own config
//! file and a scrubbed child environment — no process-env races, no real
//! PostgreSQL.
#![cfg(unix)]

mod common;

use std::path::Path;
use std::process::{Command, Output};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_pgterm")
}

struct Setup {
    dir: common::TempDir,
    fake: std::path::PathBuf,
}

impl Setup {
    fn new(tag: &str) -> Setup {
        let dir = common::TempDir::new(tag);
        let fake = common::write_fake_pgbot(dir.path());
        Setup { dir, fake }
    }

    fn config_path(&self) -> std::path::PathBuf {
        self.dir.path().join("terminal.toml")
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(bin());
        c.args(args)
            .env_remove("DATABASE_URL")
            .env_remove("PGBOT_DATABASE_URL")
            .env("PGTERM_CONFIG", self.config_path())
            .env("PGBOT_BIN", &self.fake);
        c
    }
}

fn run(c: &mut Command) -> Output {
    c.output().expect("spawn pgterm")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn config_text(s: &Setup) -> String {
    std::fs::read_to_string(s.config_path()).unwrap_or_default()
}

#[test]
fn add_discovers_database_url_and_persists_the_name_only() {
    let s = Setup::new("add-discover");
    let out = run(s
        .cmd(&["add", "production"])
        .env("DATABASE_URL", common::dsn("healthy")));
    let (so, se) = (stdout(&out), stderr(&out));
    assert_eq!(out.status.code(), Some(0), "stdout={so} stderr={se}");
    assert!(so.contains("✓ Found DATABASE_URL"), "{so}");
    assert!(so.contains("Testing production..."), "{so}");
    assert!(so.contains("✓ PostgreSQL 17"), "{so}");
    assert!(so.contains("✓ Connection successful"), "{so}");
    assert!(so.contains("✓ Read-only diagnostics available"), "{so}");
    assert!(so.contains("Added production"), "{so}");
    assert!(so.contains("Open with:"), "{so}");
    assert!(so.contains("pgterm"), "{so}");

    let cfg = config_text(&s);
    assert!(cfg.contains("name = \"production\""), "{cfg}");
    assert!(cfg.contains("env = \"DATABASE_URL\""), "{cfg}");
    assert!(!cfg.contains("postgres://"), "config leaked the DSN: {cfg}");
    assert!(
        !cfg.contains("hunter2"),
        "config leaked the password: {cfg}"
    );
}

#[test]
fn add_with_explicit_env_behaves_identically() {
    let s = Setup::new("add-env");
    let out = run(s
        .cmd(&["add", "production", "--env", "PROD_DATABASE_URL"])
        .env("PROD_DATABASE_URL", common::dsn("healthy")));
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let so = stdout(&out);
    assert!(
        !so.contains("✓ Found"),
        "explicit --env is not a discovery: {so}"
    );
    let cfg = config_text(&s);
    assert!(cfg.contains("env = \"PROD_DATABASE_URL\""), "{cfg}");
    assert!(!cfg.contains("postgres://"), "{cfg}");
}

#[test]
fn add_refuses_an_unset_env_var_and_saves_nothing() {
    let s = Setup::new("add-unset");
    let out = run(&mut s.cmd(&["add", "production", "--env", "DOES_NOT_EXIST"]));
    assert_eq!(out.status.code(), Some(2));
    let se = stderr(&out);
    assert!(
        se.contains("Environment variable DOES_NOT_EXIST is not set."),
        "{se}"
    );
    assert!(se.contains("Nothing was saved."), "{se}");
    assert!(!s.config_path().exists(), "config must not be created");
}

#[test]
fn add_without_any_source_prints_the_help() {
    let s = Setup::new("add-nosource");
    let out = run(&mut s.cmd(&["add", "production"]));
    assert_eq!(out.status.code(), Some(2));
    let se = stderr(&out);
    assert!(se.contains("No PostgreSQL connection found."), "{se}");
    assert!(se.contains("--env PROD_DATABASE_URL"), "{se}");
    assert!(!s.config_path().exists());
}

#[test]
fn add_failure_saves_nothing_and_leaks_nothing() {
    let s = Setup::new("add-refused");
    let out = run(s
        .cmd(&["add", "production", "--env", "REFUSED_URL"])
        .env("REFUSED_URL", common::dsn("refuse")));
    assert_eq!(out.status.code(), Some(1));
    let se = stderr(&out);
    assert!(se.contains("Unable to add production."), "{se}");
    assert!(se.contains("Connection failed:"), "{se}");
    assert!(se.contains("connection refused"), "{se}");
    assert!(se.contains("Nothing was saved."), "{se}");
    // Neither the DSN we resolved nor the one the fake echoed may appear.
    assert!(!se.contains("hunter2"), "{se}");
    assert!(!se.contains("sekret-pw"), "{se}");
    assert!(!s.config_path().exists());
}

#[test]
fn duplicate_add_fails_before_probing() {
    let s = Setup::new("add-dup");
    let ok = run(s
        .cmd(&["add", "prod", "--env", "A_URL"])
        .env("A_URL", common::dsn("healthy")));
    assert_eq!(ok.status.code(), Some(0), "{}", stderr(&ok));
    let before = config_text(&s);

    let dup = run(s
        .cmd(&["add", "prod", "--env", "B_URL"])
        .env("B_URL", common::dsn("healthy")));
    assert_eq!(dup.status.code(), Some(1));
    assert!(stderr(&dup).contains("already exists"), "{}", stderr(&dup));
    assert!(
        stderr(&dup).contains("Nothing was saved."),
        "{}",
        stderr(&dup)
    );
    assert_eq!(config_text(&s), before, "config must be untouched");
}

#[test]
fn list_shows_names_and_env_names_never_values() {
    let s = Setup::new("list");
    run(s
        .cmd(&["add", "prod", "--env", "PROD_URL"])
        .env("PROD_URL", common::dsn("healthy")));
    run(s
        .cmd(&["add", "staging", "--env", "STAGING_URL"])
        .env("STAGING_URL", common::dsn("warn")));

    let out = run(s.cmd(&["list"]).env("PROD_URL", common::dsn("healthy"))); // STAGING_URL unset here
    assert_eq!(out.status.code(), Some(0));
    let so = stdout(&out);
    assert!(so.contains("NAME"), "{so}");
    assert!(
        so.contains("prod") && so.contains("PROD_URL") && so.contains("configured"),
        "{so}"
    );
    assert!(
        so.contains("staging") && so.contains("STAGING_URL") && so.contains("env not set"),
        "{so}"
    );
    assert!(!so.contains("postgres://"), "list leaked a value: {so}");
}

#[test]
fn remove_deletes_the_profile_and_says_postgres_was_untouched() {
    let s = Setup::new("remove");
    run(s
        .cmd(&["add", "staging", "--env", "S_URL"])
        .env("S_URL", common::dsn("healthy")));
    let out = run(&mut s.cmd(&["remove", "staging"]));
    assert_eq!(out.status.code(), Some(0));
    let so = stdout(&out);
    assert!(so.contains("Removed local profile \"staging\"."), "{so}");
    assert!(so.contains("PostgreSQL was not modified."), "{so}");
    assert!(!config_text(&s).contains("staging"));

    let again = run(&mut s.cmd(&["remove", "staging"]));
    assert_eq!(again.status.code(), Some(1));
}

#[test]
fn unknown_flags_exit_64() {
    let s = Setup::new("usage");
    for args in [
        &["add", "p", "--url", "postgres://x"][..],
        &["frobnicate"][..],
    ] {
        let out = run(&mut s.cmd(args));
        assert_eq!(out.status.code(), Some(64), "args={args:?}");
        assert!(stderr(&out).contains("usage:"), "{}", stderr(&out));
    }
}

#[test]
fn probe_uses_pgbot_bin_not_path() {
    let s = Setup::new("pgbot-bin");
    // PATH cleared of pgbot: only PGBOT_BIN can reach the fake.
    let out = run(s
        .cmd(&["add", "prod", "--env", "P_URL"])
        .env("P_URL", common::dsn("healthy"))
        .env("PATH", "/usr/bin:/bin"));
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let log = std::fs::read_to_string(s.dir.path().join("invocations.log")).unwrap();
    assert!(
        log.contains("--profile=schema"),
        "probe must be the schema profile: {log}"
    );
    assert!(Path::new(&s.fake).exists());
}
