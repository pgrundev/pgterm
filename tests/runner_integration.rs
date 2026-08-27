//! Runner behavior against a deterministic fake pgbot — no real PostgreSQL.
#![cfg(unix)]

mod common;

use std::time::{Duration, Instant};

use pgterm::runner::{run_pgbot, PgbotCommand};
use pgterm::sanitize::ErrorKind;

#[tokio::test]
async fn healthy_run_returns_json_stdout() {
    let _env = common::env_lock();
    let dir = common::TempDir::new("run-healthy");
    let bin = common::write_fake_pgbot(dir.path());
    std::env::set_var("IT_HEALTHY_URL", common::dsn("healthy"));

    let out = run_pgbot(
        &bin,
        "IT_HEALTHY_URL",
        &PgbotCommand::Monitor,
        Duration::from_secs(10),
    )
    .await
    .expect("healthy run");
    assert_eq!(out.exit, 0);
    let v: serde_json::Value = serde_json::from_str(&out.stdout).expect("stdout is JSON");
    assert_eq!(v["schema_version"], "1.2.0");
    assert_eq!(v["server"]["database"], "app");
}

#[tokio::test]
async fn warn_exit_one_still_carries_json() {
    let _env = common::env_lock();
    let dir = common::TempDir::new("run-warn");
    let bin = common::write_fake_pgbot(dir.path());
    std::env::set_var("IT_WARN_URL", common::dsn("warn"));

    let out = run_pgbot(
        &bin,
        "IT_WARN_URL",
        &PgbotCommand::Monitor,
        Duration::from_secs(10),
    )
    .await
    .expect("exit 1 is a successful run in pgbot's contract");
    assert_eq!(out.exit, 1);
    let v: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(v["findings"][0]["severity"], "warn");
}

#[tokio::test]
async fn refused_connection_is_sanitized() {
    let _env = common::env_lock();
    let dir = common::TempDir::new("run-refuse");
    let bin = common::write_fake_pgbot(dir.path());
    std::env::set_var("IT_REFUSE_URL", common::dsn("refuse"));

    let err = run_pgbot(
        &bin,
        "IT_REFUSE_URL",
        &PgbotCommand::Monitor,
        Duration::from_secs(10),
    )
    .await
    .expect_err("exit 3 must surface as ConnectionFailed");
    assert_eq!(err.kind, ErrorKind::ConnectionFailed);
    // Neither the secret we resolved nor the DSN pgbot echoed may survive.
    assert!(!err.message.contains("hunter2-refuse"), "{}", err.message);
    assert!(!err.message.contains("sekret-pw"), "{}", err.message);
    assert!(
        err.message.contains("connection refused"),
        "{}",
        err.message
    );
}

#[tokio::test]
async fn hang_is_killed_at_the_deadline() {
    let _env = common::env_lock();
    let dir = common::TempDir::new("run-hang");
    let bin = common::write_fake_pgbot(dir.path());
    std::env::set_var("IT_HANG_URL", common::dsn("hang"));

    let started = Instant::now();
    let err = run_pgbot(
        &bin,
        "IT_HANG_URL",
        &PgbotCommand::Monitor,
        Duration::from_millis(300),
    )
    .await
    .expect_err("must time out");
    assert_eq!(err.kind, ErrorKind::Timeout);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "timeout took {:?} — the child was not killed promptly",
        started.elapsed()
    );
}

#[tokio::test]
async fn missing_env_never_spawns_pgbot() {
    let _env = common::env_lock();
    let dir = common::TempDir::new("run-noenv");
    let bin = common::write_fake_pgbot(dir.path());
    std::env::remove_var("IT_DOES_NOT_EXIST");

    let err = run_pgbot(
        &bin,
        "IT_DOES_NOT_EXIST",
        &PgbotCommand::Monitor,
        Duration::from_secs(5),
    )
    .await
    .expect_err("unset env must fail fast");
    assert_eq!(err.kind, ErrorKind::EnvMissing);
    assert!(err.message.contains("IT_DOES_NOT_EXIST"));
    assert!(
        !dir.path().join("invocations.log").exists(),
        "pgbot was spawned despite the missing env var"
    );
}

#[tokio::test]
async fn missing_binary_reports_pgbot_missing() {
    let _env = common::env_lock();
    std::env::set_var("IT_BIN_URL", common::dsn("healthy"));
    let err = run_pgbot(
        std::path::Path::new("/nonexistent/pgbot"),
        "IT_BIN_URL",
        &PgbotCommand::Monitor,
        Duration::from_secs(5),
    )
    .await
    .expect_err("nonexistent binary");
    assert_eq!(err.kind, ErrorKind::PgbotMissing);
}

#[tokio::test]
async fn indexes_and_why_reach_their_own_reports() {
    let _env = common::env_lock();
    let dir = common::TempDir::new("run-reports");
    let bin = common::write_fake_pgbot(dir.path());
    std::env::set_var("IT_REPORTS_URL", common::dsn("healthy"));

    let idx = run_pgbot(
        &bin,
        "IT_REPORTS_URL",
        &PgbotCommand::Indexes,
        Duration::from_secs(10),
    )
    .await
    .unwrap();
    let v: serde_json::Value = serde_json::from_str(&idx.stdout).unwrap();
    assert_eq!(v["indexes"][0]["confidence"], "needs_code_check");

    let why = run_pgbot(
        &bin,
        "IT_REPORTS_URL",
        &PgbotCommand::Why,
        Duration::from_secs(10),
    )
    .await
    .unwrap();
    let v: serde_json::Value = serde_json::from_str(&why.stdout).unwrap();
    assert_eq!(v["why_schema_version"], "1.0.0");
}

#[tokio::test]
async fn dsn_travels_by_env_not_argv() {
    let _env = common::env_lock();
    let dir = common::TempDir::new("run-envonly");
    let bin = common::write_fake_pgbot(dir.path());
    std::env::set_var("IT_ENVONLY_URL", common::dsn("healthy"));

    run_pgbot(
        &bin,
        "IT_ENVONLY_URL",
        &PgbotCommand::Monitor,
        Duration::from_secs(10),
    )
    .await
    .unwrap();
    let log = std::fs::read_to_string(dir.path().join("invocations.log")).unwrap();
    // The fake logs "argv url=$DATABASE_URL": argv (before " url=") must not
    // contain the DSN; the env must.
    let (argv, env_url) = log.trim().rsplit_once(" url=").unwrap();
    assert!(!argv.contains("://"), "DSN leaked into argv: {argv}");
    assert!(
        env_url.contains("mode-healthy"),
        "child env missing DATABASE_URL: {env_url}"
    );
}
