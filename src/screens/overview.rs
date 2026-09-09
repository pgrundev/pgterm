//! The Overview tab: status line, stat tiles, pgbot's gauge strip, and the
//! findings summary. This half is pure — everything derives from the cached
//! Context and is unit-tested without a terminal. The gauge rules mirror
//! pgbot's own default view (its internal/render/gauges.go) so the two
//! surfaces never disagree about the same database.

use crate::format;
use crate::model::{Context, Finding, CACHE_HIT_MIN_BLOCKS};

/// pgbot's confidence buckets, as words rather than a bare number.
pub fn confidence_label(c: f64) -> &'static str {
    if c >= 0.8 {
        "HIGH"
    } else if c >= 0.5 {
        "MEDIUM"
    } else {
        "LOW"
    }
}

/// "17.4" out of "PostgreSQL 17.4 on x86_64…": the first digits-and-dots run.
pub fn version_digits(text: &str) -> Option<String> {
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let run: String = text[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let run = run.trim_end_matches('.').to_string();
    if run.is_empty() {
        None
    } else {
        Some(run)
    }
}

/// The managed platform pgbot detected, upper-cased for the status line.
pub fn provider_label(p: &str) -> Option<String> {
    let p = p.trim();
    if p.is_empty() || p == "unknown" {
        None
    } else {
        Some(p.to_ascii_uppercase())
    }
}

fn severity_rank(s: &str) -> u8 {
    match s {
        "critical" => 0,
        "warn" => 1,
        _ => 2,
    }
}

/// Non-suppressed findings that need a human, worst first.
pub fn attention_findings(ctx: &Context) -> Vec<&Finding> {
    let mut v: Vec<&Finding> = ctx
        .findings
        .iter()
        .filter(|f| !f.suppressed && matches!(f.severity.as_str(), "critical" | "warn"))
        .collect();
    v.sort_by_key(|f| severity_rank(&f.severity));
    v
}

/// Label/value pairs for the stat tiles. A section pgbot did not collect
/// shows an em dash rather than a zero that looks like a measurement.
pub fn tiles(ctx: &Context) -> Vec<(&'static str, String)> {
    let dash = || "—".to_string();
    let version = version_digits(&ctx.server.version_text)
        .or_else(|| (ctx.server.major() > 0).then(|| ctx.server.major().to_string()))
        .unwrap_or_else(dash);
    let conns = ctx
        .limits
        .as_ref()
        .filter(|l| l.connections_max > 0)
        .map(|l| format!("{} / {}", l.connections_used, l.connections_max))
        .unwrap_or_else(dash);
    let active = ctx
        .activity
        .as_ref()
        .map(|a| a.active.to_string())
        .unwrap_or_else(dash);
    let size = ctx
        .tables
        .as_ref()
        .filter(|t| t.db_size_bytes > 0)
        .map(|t| format::human_bytes(t.db_size_bytes))
        .unwrap_or_else(dash);
    let uptime = if ctx.server.uptime_seconds > 0 {
        format::duration_short(ctx.server.uptime_seconds)
    } else {
        dash()
    };
    vec![
        ("PostgreSQL", version),
        ("Connections", conns),
        ("Active", active),
        ("Size", size),
        ("Uptime", uptime),
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GaugeKind {
    Ok,
    Watch,
    Bad,
    Info,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Gauge {
    pub label: &'static str,
    pub share: f64,
    pub value: String,
    pub status: String,
    pub kind: GaugeKind,
    pub measurable: bool,
}

pub const GAUGE_WIDTH: usize = 20;

fn not_measurable(label: &'static str, why: &str) -> Gauge {
    Gauge {
        label,
        share: 0.0,
        value: "—".into(),
        status: why.into(),
        kind: GaugeKind::Info,
        measurable: false,
    }
}

/// Nearest cell, with a one-cell floor so a small but real signal is visible.
pub fn gauge_cells(share: f64) -> usize {
    if share <= 0.0 {
        return 0;
    }
    let s = share.min(1.0);
    ((s * GAUGE_WIDTH as f64 + 0.5) as usize).clamp(1, GAUGE_WIDTH)
}

pub fn gauge_bar(share: f64) -> String {
    let n = gauge_cells(share);
    "█".repeat(n) + &"░".repeat(GAUGE_WIDTH - n)
}

fn fired(ctx: &Context, id: &str) -> bool {
    ctx.findings.iter().any(|f| f.id == id)
}

/// Cache hit ratio, graded by pgbot's low_cache_hit finding. Too little block
/// traffic to judge is stated as such, never shown as a low ratio.
pub fn cache_hit_gauge(ctx: &Context) -> Gauge {
    let Some(h) = ctx.health.as_ref() else {
        return not_measurable("cache hit", "not measurable");
    };
    let Some(ratio) = h.cache_hit_ratio else {
        return not_measurable("cache hit", "not measurable");
    };
    if !matches!(h.cache_blocks, Some(b) if b >= CACHE_HIT_MIN_BLOCKS) {
        return not_measurable("cache hit", "thin sample");
    }
    let mut g = Gauge {
        label: "cache hit",
        share: ratio,
        value: format::pct(ratio),
        status: "ok".into(),
        kind: GaugeKind::Ok,
        measurable: true,
    };
    if fired(ctx, "low_cache_hit") {
        g.status = "low".into();
        g.kind = GaugeKind::Bad;
    }
    g
}

/// Blocked sessions. pgterm runs pgbot with wait sampling off, so there is
/// never a Lock share to fill the bar with: the value stays "—" and the status
/// carries the meaning — exactly what pgbot renders without a wait profile.
pub fn lock_wait_gauge(ctx: &Context) -> Gauge {
    let Some(locks) = ctx.locks.as_ref() else {
        return not_measurable("lock wait", "not measurable");
    };
    let mut g = Gauge {
        label: "lock wait",
        share: 0.0,
        value: "—".into(),
        status: "ok".into(),
        kind: GaugeKind::Ok,
        measurable: true,
    };
    if locks.blocked_count > 0 {
        g.status = format!("{} blocked", locks.blocked_count);
        g.kind = GaugeKind::Bad;
    }
    g
}

/// Rolled-back share of transactions, graded by high_rollback_ratio.
pub fn rollbacks_gauge(ctx: &Context) -> Gauge {
    let Some(ratio) = ctx.health.as_ref().and_then(|h| h.rollback_ratio) else {
        return not_measurable("rollbacks", "not measurable");
    };
    let mut g = Gauge {
        label: "rollbacks",
        share: ratio,
        value: format::pct(ratio),
        status: "ok".into(),
        kind: GaugeKind::Ok,
        measurable: true,
    };
    if fired(ctx, "high_rollback_ratio") {
        g.status = "watch".into();
        g.kind = GaugeKind::Watch;
    }
    g
}

/// Bytes held by zero-scan indexes; the bar is that size over the database,
/// so it answers "how much of my storage is dead weight". Meaningless in a
/// cold stats window, exactly like the finding that grades it.
pub fn idle_index_gauge(ctx: &Context) -> Gauge {
    if ctx.window.as_ref().is_some_and(|w| w.cold()) {
        return not_measurable("idle idx", "window < 15m");
    }
    let Some(ix) = ctx.indexes.as_ref() else {
        return not_measurable("idle idx", "not measurable");
    };
    let idle: i64 = ix
        .unused
        .iter()
        .filter(|i| i.scans == 0)
        .map(|i| i.bytes)
        .sum();
    let share = match ctx.tables.as_ref().filter(|t| t.db_size_bytes > 0) {
        Some(t) => idle as f64 / t.db_size_bytes as f64,
        None => 0.0,
    };
    let mut g = Gauge {
        label: "idle idx",
        share,
        value: format::human_bytes(idle),
        status: "ok".into(),
        kind: GaugeKind::Ok,
        measurable: true,
    };
    if fired(ctx, "unused_indexes") {
        g.status = "review".into();
        g.kind = GaugeKind::Watch;
    }
    g
}

pub fn gauges(ctx: &Context) -> [Gauge; 4] {
    [
        cache_hit_gauge(ctx),
        lock_wait_gauge(ctx),
        rollbacks_gauge(ctx),
        idle_index_gauge(ctx),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEALTHY: &str = include_str!("../../tests/fixtures/context_healthy.json");
    const WARN: &str = include_str!("../../tests/fixtures/context_warn.json");
    const CRITICAL: &str = include_str!("../../tests/fixtures/context_critical.json");

    #[test]
    fn confidence_buckets() {
        assert_eq!(confidence_label(0.95), "HIGH");
        assert_eq!(confidence_label(0.8), "HIGH");
        assert_eq!(confidence_label(0.5), "MEDIUM");
        assert_eq!(confidence_label(0.49), "LOW");
    }

    #[test]
    fn version_and_provider_labels() {
        assert_eq!(
            version_digits("PostgreSQL 17.4 on x86_64-pc-linux-gnu").as_deref(),
            Some("17.4")
        );
        assert_eq!(version_digits("PostgreSQL 16beta1").as_deref(), Some("16"));
        assert_eq!(version_digits("weird"), None);
        assert_eq!(provider_label("rds").as_deref(), Some("RDS"));
        assert_eq!(provider_label(""), None);
        assert_eq!(provider_label("unknown"), None);
    }

    #[test]
    fn attention_findings_are_critical_first_and_skip_suppressed() {
        let ctx = Context::decode(CRITICAL).unwrap();
        let f = attention_findings(&ctx);
        assert!(!f.is_empty());
        assert_eq!(f[0].severity, "critical");
        assert!(f
            .windows(2)
            .all(|w| severity_rank(&w[0].severity) <= severity_rank(&w[1].severity)));
        assert!(f.iter().all(|x| !x.suppressed && x.severity != "info"));
    }

    #[test]
    fn tiles_come_from_the_context_or_show_a_dash() {
        let ctx = Context::decode(HEALTHY).unwrap();
        let t = tiles(&ctx);
        let get = |k: &str| {
            t.iter()
                .find(|(l, _)| *l == k)
                .map(|(_, v)| v.clone())
                .unwrap()
        };
        assert_eq!(get("PostgreSQL"), "17.4");
        assert_eq!(get("Connections"), "84 / 300");
        assert_eq!(get("Active"), "5");
        assert!(get("Size").ends_with("GiB"), "{}", get("Size"));
        assert_eq!(get("Uptime"), "10d");
        let empty = Context::decode("{}").unwrap();
        assert!(tiles(&empty).iter().all(|(_, v)| v == "—"));
    }

    #[test]
    fn gauge_cells_round_to_nearest_with_a_one_cell_minimum() {
        for (share, cells) in [
            (0.0, 0),
            (0.001, 1),
            (0.024, 1),
            (0.026, 1),
            (0.074, 1),
            (0.076, 2),
            (0.5, 10),
            (0.974, 19),
            (0.976, 20),
            (1.0, 20),
            (1.4, 20),
        ] {
            assert_eq!(gauge_cells(share), cells, "share {share}");
        }
        assert_eq!(gauge_bar(0.5).chars().filter(|c| *c == '█').count(), 10);
        assert_eq!(gauge_bar(0.3).chars().count(), GAUGE_WIDTH);
    }

    #[test]
    fn gauges_follow_pgbots_rules() {
        let healthy = Context::decode(HEALTHY).unwrap();
        let g = cache_hit_gauge(&healthy);
        assert!(g.measurable && g.status == "ok" && g.kind == GaugeKind::Ok);
        assert_eq!(g.value, "99.2%");
        assert_eq!(lock_wait_gauge(&healthy).status, "ok");

        let warn = Context::decode(WARN).unwrap();
        let r = rollbacks_gauge(&warn);
        assert_eq!(
            (r.value.as_str(), r.status.as_str(), r.kind),
            ("12.0%", "watch", GaugeKind::Watch)
        );
        let ix = idle_index_gauge(&warn);
        assert_eq!(ix.status, "review");
        assert_eq!(ix.kind, GaugeKind::Watch);
        assert!(ix.share > 0.0 && ix.value.contains("GiB"), "{ix:?}");

        let crit = Context::decode(CRITICAL).unwrap();
        let lw = lock_wait_gauge(&crit);
        assert_eq!(
            lw.value, "—",
            "pgterm runs pgbot without wait sampling, so there is no share"
        );
        assert_eq!(lw.status, "3 blocked");
        assert_eq!(lw.kind, GaugeKind::Bad);

        let empty = Context::decode("{}").unwrap();
        for g in gauges(&empty) {
            assert!(!g.measurable && g.value == "—", "{g:?}");
        }
        let cold =
            Context::decode(r#"{"indexes":{"unused":[]},"window":{"window_age_seconds":10}}"#)
                .unwrap();
        assert_eq!(idle_index_gauge(&cold).status, "window < 15m");
        let thin =
            Context::decode(r#"{"health":{"cache_hit_ratio":0.5,"cache_blocks_sampled":10}}"#)
                .unwrap();
        assert_eq!(cache_hit_gauge(&thin).status, "thin sample");
        assert!(!cache_hit_gauge(&thin).measurable);
    }
}
