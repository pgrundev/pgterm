//! Health derivation from a Context: the overall tab status, the 0–100 score
//! (mirroring pgbot's computeHealthScore in internal/render/dashboard.go),
//! and the seven category rows of the dashboard. Statuses come from pgbot's
//! findings — this module maps, it never diagnoses.

use std::time::SystemTime;

use crate::format;
use crate::model::{Context, Finding};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    Healthy,
    Warning,
    Critical,
    Checking,
    Unavailable,
}

/// Worst non-suppressed finding severity → tab status.
pub fn overall(ctx: &Context) -> HealthStatus {
    let mut worst = HealthStatus::Healthy;
    for f in active(&ctx.findings) {
        match f.severity.as_str() {
            "critical" => return HealthStatus::Critical,
            "warn" => worst = HealthStatus::Warning,
            _ => {}
        }
    }
    worst
}

/// Mirror of computeHealthScore: 100 − (10·critical + 3·warn + 1·other) over
/// non-suppressed findings, floored at 0.
pub fn score(ctx: &Context) -> u32 {
    let mut penalty: i64 = 0;
    for f in active(&ctx.findings) {
        penalty += match f.severity.as_str() {
            "critical" => 10,
            "warn" => 3,
            _ => 1,
        };
    }
    (100 - penalty).max(0) as u32
}

fn active(findings: &[Finding]) -> impl Iterator<Item = &Finding> {
    findings.iter().filter(|f| !f.suppressed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowStatus {
    Ok,
    Warn,
    Fail,
    Unknown,
}

impl RowStatus {
    pub fn label(self) -> &'static str {
        match self {
            RowStatus::Ok => "OK",
            RowStatus::Warn => "WARN",
            RowStatus::Fail => "FAIL",
            RowStatus::Unknown => "—",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CategoryRow {
    pub name: &'static str,
    pub status: RowStatus,
    pub metric: String,
}

const CATEGORIES: [&str; 7] = [
    "Connections",
    "Cache",
    "Locks",
    "Queries",
    "Indexes",
    "Vacuum",
    "Replication",
];

/// Stable finding-id → category mapping. Ids come from pgbot's findings
/// catalogue (docs/findings/); unmapped ids never invent a row — they are
/// returned as a count the dashboard mentions alongside the table.
fn category_of(id: &str) -> Option<&'static str> {
    if id.starts_with("autovacuum_") {
        return Some("Vacuum");
    }
    if id.starts_with("replica_") || id.starts_with("archiving_") {
        return Some("Replication");
    }
    match id {
        "connection_saturation"
        | "connections_overprovisioned"
        | "idle_in_transaction"
        | "long_running_transaction"
        | "prepared_xact_abandoned" => Some("Connections"),
        "low_cache_hit" | "checkpoints_forced" | "io_timing_off" | "wait_io_bound" => Some("Cache"),
        "blocking_chains"
        | "wait_lock_contention"
        | "wait_lwlock_pressure"
        | "recovery_conflicts" => Some("Locks"),
        "query_slowdown"
        | "seq_scan_heavy"
        | "partition_seq_scan_heavy"
        | "high_rollback_ratio"
        | "pg_stat_statements_missing"
        | "pgss_entries_evicted"
        | "statement_timeout_unset"
        | "work_mem_low"
        | "work_mem_overcommit"
        | "random_page_cost_high" => Some("Queries"),
        "unused_indexes"
        | "redundant_indexes"
        | "index_invalid"
        | "fk_unindexed"
        | "low_hot_update_ratio" => Some("Indexes"),
        "table_bloat"
        | "table_never_vacuumed"
        | "never_analyzed"
        | "stale_statistics"
        | "stale_stats_window"
        | "vacuum_horizon_blocked"
        | "txid_wraparound"
        | "mxid_wraparound"
        | "sequence_exhaustion" => Some("Vacuum"),
        "replication_slot_inactive" | "subscription_worker_down" | "sync_rep_degraded" => {
            Some("Replication")
        }
        _ => None,
    }
}

/// The dashboard's seven rows plus the count of findings that fit none of
/// them (still reflected in `overall` and `score`).
pub fn categories(ctx: &Context, now: SystemTime) -> (Vec<CategoryRow>, usize) {
    let mut status: std::collections::HashMap<&str, RowStatus> =
        CATEGORIES.iter().map(|c| (*c, RowStatus::Ok)).collect();
    let mut unmapped = 0usize;
    for f in active(&ctx.findings) {
        let Some(cat) = category_of(&f.id) else {
            unmapped += 1;
            continue;
        };
        let s = status.get_mut(cat).expect("category table is closed");
        match f.severity.as_str() {
            "critical" => *s = RowStatus::Fail,
            "warn" if *s != RowStatus::Fail => *s = RowStatus::Warn,
            _ => {}
        }
    }

    let rows = CATEGORIES
        .iter()
        .map(|&name| CategoryRow {
            name,
            status: status[name],
            metric: metric_for(name, ctx, now),
        })
        .collect();
    (rows, unmapped)
}

fn metric_for(category: &str, ctx: &Context, now: SystemTime) -> String {
    match category {
        "Connections" => match &ctx.limits {
            Some(l) if l.connections_max > 0 => {
                format!("{} / {}", l.connections_used, l.connections_max)
            }
            _ => ctx
                .activity
                .as_ref()
                .map(|a| a.total.to_string())
                .unwrap_or_else(|| "—".into()),
        },
        "Cache" => match &ctx.health {
            Some(h) if h.cache_hit_usable() => {
                format!("{:.1}%", h.cache_hit_ratio.unwrap_or_default() * 100.0)
            }
            _ => "—".into(),
        },
        "Locks" => match &ctx.locks {
            Some(l) => format!("{} blocked", l.blocked_count),
            None => "—".into(),
        },
        "Queries" => {
            let regressions = active(&ctx.findings)
                .filter(|f| f.id == "query_slowdown")
                .count();
            if regressions > 0 {
                // pgbot reports one aggregate finding; its evidence rows are
                // the individual regressed queries.
                let n = active(&ctx.findings)
                    .filter(|f| f.id == "query_slowdown")
                    .map(|f| f.evidence.len().max(1))
                    .sum::<usize>();
                format!("{n} regressions")
            } else {
                match &ctx.queries {
                    Some(q) if q.enabled => format!("{} tracked", q.top.len()),
                    Some(_) => "pg_stat_statements off".into(),
                    None => "—".into(),
                }
            }
        }
        "Indexes" => match &ctx.indexes {
            Some(ix) if !ix.unused.is_empty() => {
                let bytes: i64 = ix.unused.iter().map(|u| u.bytes).sum();
                format!(
                    "{} unused · {}",
                    ix.unused.len(),
                    format::human_bytes(bytes)
                )
            }
            Some(_) => "none unused".into(),
            None => "—".into(),
        },
        "Vacuum" => {
            let latest = ctx
                .tables
                .iter()
                .flat_map(|t| t.top.iter())
                .flat_map(|t| [t.last_autovacuum.as_deref(), t.last_vacuum.as_deref()])
                .flatten()
                .filter_map(format::parse_rfc3339)
                .max();
            match latest.and_then(|t| now.duration_since(t).ok()) {
                Some(d) => format::ago(d),
                None => "—".into(),
            }
        }
        "Replication" => match &ctx.replication {
            Some(r) if r.is_replica => match r.receiver_lag_sec {
                Some(lag) => lag_str(lag),
                None => "replica".into(),
            },
            Some(r) if !r.replicas.is_empty() => {
                let max = r
                    .replicas
                    .iter()
                    .filter_map(|x| x.replay_lag_sec)
                    .fold(0.0f64, f64::max);
                lag_str(max)
            }
            Some(_) => "none".into(),
            None => "—".into(),
        },
        _ => "—".into(),
    }
}

fn lag_str(sec: f64) -> String {
    if sec < 1.0 {
        format!("{:.0} ms", sec * 1000.0)
    } else {
        format!("{sec:.1} s")
    }
}

/// "1 failing · 2 warnings · 5 healthy" over the category rows.
pub fn summary_line(rows: &[CategoryRow]) -> String {
    let fail = rows.iter().filter(|r| r.status == RowStatus::Fail).count();
    let warn = rows.iter().filter(|r| r.status == RowStatus::Warn).count();
    let ok = rows.iter().filter(|r| r.status == RowStatus::Ok).count();
    let mut parts = Vec::new();
    if fail > 0 {
        parts.push(format!("{fail} failing"));
    }
    if warn > 0 {
        parts.push(format!(
            "{warn} warning{}",
            if warn == 1 { "" } else { "s" }
        ));
    }
    parts.push(format!("{ok} healthy"));
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Context;
    use std::time::Duration;

    const HEALTHY: &str = include_str!("../tests/fixtures/context_healthy.json");
    const WARN: &str = include_str!("../tests/fixtures/context_warn.json");
    const CRITICAL: &str = include_str!("../tests/fixtures/context_critical.json");

    fn now() -> SystemTime {
        // Fixtures are stamped 2026-08-26T10:00; "now" is 12s later.
        crate::format::parse_rfc3339("2026-08-26T10:00:12Z").unwrap()
    }

    #[test]
    fn overall_and_score_track_severities() {
        let h = Context::decode(HEALTHY).unwrap();
        assert_eq!(overall(&h), HealthStatus::Healthy);
        assert_eq!(score(&h), 100);

        let w = Context::decode(WARN).unwrap();
        assert_eq!(overall(&w), HealthStatus::Warning);
        assert_eq!(score(&w), 100 - 3 - 3);

        let c = Context::decode(CRITICAL).unwrap();
        assert_eq!(overall(&c), HealthStatus::Critical);
        assert_eq!(score(&c), 100 - 10 - 3 - 3);
    }

    #[test]
    fn suppressed_findings_move_nothing() {
        let mut c = Context::decode(WARN).unwrap();
        for f in &mut c.findings {
            f.suppressed = true;
        }
        assert_eq!(overall(&c), HealthStatus::Healthy);
        assert_eq!(score(&c), 100);
        let (rows, unmapped) = categories(&c, now());
        assert!(rows.iter().all(|r| r.status == RowStatus::Ok));
        assert_eq!(unmapped, 0);
    }

    #[test]
    fn warn_fixture_maps_to_indexes_and_queries_rows() {
        let c = Context::decode(WARN).unwrap();
        let (rows, unmapped) = categories(&c, now());
        assert_eq!(unmapped, 0);
        let row = |n: &str| rows.iter().find(|r| r.name == n).unwrap();
        assert_eq!(row("Indexes").status, RowStatus::Warn);
        assert_eq!(row("Indexes").metric, "2 unused · 20 GiB");
        assert_eq!(row("Queries").status, RowStatus::Warn);
        assert_eq!(row("Queries").metric, "2 regressions");
        assert_eq!(row("Connections").status, RowStatus::Ok);
        assert_eq!(row("Connections").metric, "84 / 300");
        assert_eq!(row("Cache").metric, "99.2%");
        assert_eq!(row("Vacuum").metric, "3m ago");
        assert_eq!(row("Replication").metric, "210 ms");
    }

    #[test]
    fn critical_fixture_fails_the_locks_row() {
        let c = Context::decode(CRITICAL).unwrap();
        let (rows, _) = categories(&c, now());
        let locks = rows.iter().find(|r| r.name == "Locks").unwrap();
        assert_eq!(locks.status, RowStatus::Fail);
        assert_eq!(locks.metric, "3 blocked");
        assert_eq!(summary_line(&rows), "1 failing · 2 warnings · 4 healthy");
    }

    #[test]
    fn unknown_finding_id_counts_as_unmapped_not_a_row() {
        let mut c = Context::decode(HEALTHY).unwrap();
        c.findings.push(crate::model::Finding {
            id: "finding_from_the_future".into(),
            severity: "warn".into(),
            ..Default::default()
        });
        let (rows, unmapped) = categories(&c, now());
        assert_eq!(unmapped, 1);
        assert!(rows.iter().all(|r| r.status == RowStatus::Ok));
        assert_eq!(
            overall(&c),
            HealthStatus::Warning,
            "unmapped still gates the tab"
        );
    }

    #[test]
    fn healthy_summary_counts_all_seven() {
        let c = Context::decode(HEALTHY).unwrap();
        let (rows, _) = categories(&c, now());
        assert_eq!(summary_line(&rows), "7 healthy");
        // And the vacuum recency comes from the freshest table timestamp.
        assert_eq!(
            rows.iter().find(|r| r.name == "Vacuum").unwrap().metric,
            "3m ago"
        );
        let _ = Duration::ZERO;
    }
}
