//! End-to-end monitoring: App::update drives real pgbot subprocess runs
//! (the deterministic fake) under the bounded-concurrency semaphore, and the
//! per-database states stay independent.
#![cfg(unix)]

mod common;

use std::sync::Arc;

use pgterm::action::{Action, Effect};
use pgterm::app::{run_effect, App};
use pgterm::config::TerminalConfig;
use pgterm::health::HealthStatus;
use tokio::sync::Semaphore;

fn three_db_app() -> App {
    let mut cfg = TerminalConfig::default();
    cfg.add("alpha", "MON_A_URL").unwrap();
    cfg.add("bravo", "MON_B_URL").unwrap();
    cfg.add("charlie", "MON_C_URL").unwrap();
    App::new(&cfg, None, false, None)
}

/// Runs one full monitor sweep: collect effects, execute them concurrently
/// through the real runner, feed results back.
async fn sweep(app: &mut App, bin: &std::path::Path, permits: usize) {
    let effects = app.update(Action::MonitorTick);
    let sem = Arc::new(Semaphore::new(permits));
    let mut joins = Vec::new();
    for e in effects {
        if let Effect::Spawn { db, cmd, kind } = e {
            let env = app.dbs[db].profile.env.clone();
            joins.push(tokio::spawn(run_effect(
                bin.to_path_buf(),
                env,
                db,
                cmd,
                kind,
                sem.clone(),
            )));
        }
    }
    for j in joins {
        let action = j.await.expect("task");
        app.update(action);
    }
}

#[tokio::test]
async fn three_databases_monitor_independently() {
    let _env = common::env_lock();
    let dir = common::TempDir::new("mon-independent");
    let bin = common::write_fake_pgbot(dir.path());
    std::env::set_var("MON_A_URL", common::dsn("healthy"));
    std::env::set_var("MON_B_URL", common::dsn("warn"));
    std::env::set_var("MON_C_URL", common::dsn("refuse"));

    let mut app = three_db_app();
    sweep(&mut app, &bin, 2).await;

    assert_eq!(app.dbs[0].health, HealthStatus::Healthy);
    assert_eq!(app.dbs[1].health, HealthStatus::Warning);
    assert_eq!(app.dbs[2].health, HealthStatus::Unavailable);

    // Independence: each db kept its own evidence.
    assert!(app.dbs[0].ctx.as_ref().unwrap().findings.is_empty());
    assert_eq!(app.dbs[1].ctx.as_ref().unwrap().findings.len(), 2);
    assert!(app.dbs[2].ctx.is_none());
    let err = app.dbs[2]
        .error
        .as_ref()
        .expect("charlie must carry its error");
    assert!(
        !err.message.contains("sekret-pw"),
        "sanitized: {}",
        err.message
    );
    assert!(
        !err.message.contains("hunter2"),
        "sanitized: {}",
        err.message
    );

    // The user is looking at alpha; the OTHER troubled tabs are flagged.
    assert!(!app.dbs[0].attention);
    assert!(app.dbs[1].attention, "warning tab must ask for attention");
    assert!(
        app.dbs[2].attention,
        "unavailable tab must ask for attention"
    );

    // A second sweep still works and states remain per-database.
    std::env::set_var("MON_C_URL", common::dsn("healthy"));
    sweep(&mut app, &bin, 2).await;
    assert_eq!(
        app.dbs[2].health,
        HealthStatus::Healthy,
        "charlie recovered"
    );
    assert_eq!(app.dbs[1].health, HealthStatus::Warning, "bravo unchanged");
}

#[tokio::test]
async fn concurrency_stays_under_the_semaphore_cap() {
    let _env = common::env_lock();
    let dir = common::TempDir::new("mon-cap");
    let bin = common::write_fake_pgbot(dir.path());
    for v in ["CAP_A_URL", "CAP_B_URL", "CAP_C_URL"] {
        std::env::set_var(v, common::dsn("healthy"));
    }
    std::env::set_var("FAKE_PGBOT_DELAY", "0.4");

    let mut cfg = TerminalConfig::default();
    cfg.add("a", "CAP_A_URL").unwrap();
    cfg.add("b", "CAP_B_URL").unwrap();
    cfg.add("c", "CAP_C_URL").unwrap();
    let mut app = App::new(&cfg, None, false, None);
    sweep(&mut app, &bin, 2).await;
    std::env::remove_var("FAKE_PGBOT_DELAY");

    let peaks = std::fs::read_to_string(dir.path().join("peaks.log")).unwrap_or_default();
    let max = peaks
        .lines()
        .filter_map(|l| l.trim().parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    assert!(
        max >= 1,
        "the fake never observed itself running: {peaks:?}"
    );
    assert!(
        max <= 2,
        "semaphore(2) breached — peak {max} concurrent pgbots"
    );
    assert!(app.dbs.iter().all(|d| d.health == HealthStatus::Healthy));
}
