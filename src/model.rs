//! Typed views over pgbot's JSON contracts. Only the fields the terminal
//! renders are declared; everything else passes through untouched (serde
//! ignores unknown keys), so a newer pgbot never breaks an older terminal.
//! Nothing here computes diagnostics — pgbot's output is the truth.

use serde::Deserialize;

use crate::sanitize::{ErrorKind, SafeError};

fn decode<'a, T: Deserialize<'a>>(json: &'a str, what: &str) -> Result<T, SafeError> {
    serde_json::from_str(json).map_err(|e| {
        SafeError::new(
            ErrorKind::BadOutput,
            &format!("{what}: {e} (is pgbot older than the terminal?)"),
            None,
        )
    })
}

/// `pgbot inspect --json` — the versioned Context document.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Context {
    pub schema_version: String,
    pub collected_at: Option<String>,
    pub server: Server,
    pub limits: Option<Limits>,
    pub health: Option<Health>,
    pub activity: Option<Activity>,
    pub locks: Option<Locks>,
    pub queries: Option<Queries>,
    pub tables: Option<Tables>,
    pub indexes: Option<Indexes>,
    pub replication: Option<Replication>,
    pub findings: Vec<Finding>,
}

impl Context {
    pub fn decode(json: &str) -> Result<Self, SafeError> {
        decode(json, "inspect context")
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Server {
    pub version_num: i64,
    pub version_text: String,
    pub database: String,
    pub has_pg_monitor: bool,
}

impl Server {
    /// "PostgreSQL 17" from version_num 170004.
    pub fn major(&self) -> i64 {
        self.version_num / 10_000
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Limits {
    pub connections_used: i64,
    pub connections_max: i64,
}

/// Mirrors model.CacheHitMinBlocks: below this block traffic the ratio is
/// noise and must not be graded or displayed as signal.
pub const CACHE_HIT_MIN_BLOCKS: i64 = 10_000;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Health {
    pub cache_hit_ratio: Option<f64>,
    #[serde(rename = "cache_blocks_sampled")]
    pub cache_blocks: Option<i64>,
}

impl Health {
    pub fn cache_hit_usable(&self) -> bool {
        matches!((self.cache_hit_ratio, self.cache_blocks), (Some(_), Some(b)) if b >= CACHE_HIT_MIN_BLOCKS)
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Activity {
    pub total: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Locks {
    pub blocked_count: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Queries {
    pub enabled: bool,
    pub reason: Option<String>,
    pub total_exec_ms: f64,
    pub top: Vec<QueryStat>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct QueryStat {
    pub queryid: i64,
    pub query: String,
    pub calls: i64,
    pub total_ms: f64,
    pub mean_ms: f64,
    pub max_ms: f64,
    pub rows: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Tables {
    pub db_size_bytes: i64,
    pub top: Vec<TableStat>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TableStat {
    pub schema: String,
    #[serde(rename = "table")]
    pub name: String,
    pub total_bytes: i64,
    pub live_tuples: i64,
    pub dead_ratio: f64,
    pub seq_scans: i64,
    pub index_scans: i64,
    pub last_vacuum: Option<String>,
    pub last_autovacuum: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Indexes {
    pub total: i64,
    pub unused: Vec<IndexStat>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct IndexStat {
    pub schema: String,
    pub table: String,
    #[serde(rename = "index")]
    pub name: String,
    pub scans: i64,
    pub bytes: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Replication {
    pub is_replica: bool,
    pub replicas: Vec<ReplicaRow>,
    pub receiver_lag_sec: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ReplicaRow {
    pub sync_state: String,
    pub replay_lag_sec: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Finding {
    pub id: String,
    pub severity: String,
    pub title: String,
    pub detail: String,
    pub object: Option<String>,
    pub evidence: Vec<String>,
    pub remediation: Option<String>,
    pub confidence: f64,
    pub caveats: Vec<String>,
    pub suppressed: bool,
    pub suppression_reason: Option<String>,
}

/// `pgbot indexes --json` — the graded correlation report. Rendered verbatim:
/// the confidence enum, do_not_drop, and the code-check instructions are
/// pgbot's grading, never ours.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct IndexesReport {
    pub fingerprint: String,
    pub cold_window: bool,
    pub stats_window_days: f64,
    pub indexes: Vec<IndexVerdict>,
    pub note: Option<String>,
}

impl IndexesReport {
    pub fn decode(json: &str) -> Result<Self, SafeError> {
        decode(json, "indexes report")
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct IndexVerdict {
    #[serde(rename = "index")]
    pub name: String,
    pub table: String,
    pub schema: String,
    pub size_bytes: i64,
    pub scans: i64,
    /// catalog_proven | needs_code_check | inconclusive
    pub confidence: String,
    pub reason: String,
    pub do_not_drop: bool,
    pub instruction: Option<String>,
    pub if_found: Option<String>,
    pub if_not_found: Option<String>,
    pub search_terms: Vec<String>,
}

/// `pgbot why --json` — offline causal chains from the baseline store.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct WhyReport {
    pub why_schema_version: String,
    pub database: String,
    pub snapshots: i64,
    pub analyzed_queries: i64,
    pub regressions_found: i64,
    pub chains: Vec<WhyChain>,
    pub notes: Vec<String>,
}

impl WhyReport {
    pub fn decode(json: &str) -> Result<Self, SafeError> {
        decode(json, "why report")
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct WhyChain {
    pub symptom: WhyHop,
    pub hops: Vec<WhyHop>,
    pub confidence: f64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct WhyHop {
    pub role: String,
    pub text: String,
    pub before: Option<f64>,
    pub after: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEALTHY: &str = include_str!("../tests/fixtures/context_healthy.json");
    const WARN: &str = include_str!("../tests/fixtures/context_warn.json");
    const CRITICAL: &str = include_str!("../tests/fixtures/context_critical.json");
    const INDEXES: &str = include_str!("../tests/fixtures/indexes_report.json");
    const WHY: &str = include_str!("../tests/fixtures/why_report.json");

    #[test]
    fn context_fixtures_decode() {
        let c = Context::decode(HEALTHY).unwrap();
        assert_eq!(c.schema_version, "1.2.0");
        assert_eq!(c.server.major(), 17);
        assert_eq!(c.limits.as_ref().unwrap().connections_used, 84);
        assert!(c.health.as_ref().unwrap().cache_hit_usable());
        assert_eq!(c.queries.as_ref().unwrap().top.len(), 2);
        assert_eq!(c.tables.as_ref().unwrap().top[0].name, "events");
        assert!(c.findings.is_empty());

        let w = Context::decode(WARN).unwrap();
        assert_eq!(w.findings.len(), 2);
        assert_eq!(w.indexes.as_ref().unwrap().unused.len(), 2);

        let cr = Context::decode(CRITICAL).unwrap();
        assert_eq!(cr.locks.as_ref().unwrap().blocked_count, 3);
        assert_eq!(cr.findings[0].severity, "critical");
    }

    #[test]
    fn unknown_fields_are_ignored_and_missing_sections_are_none() {
        let c =
            Context::decode(r#"{"schema_version":"9.9.9","future_section":{"x":1},"findings":[]}"#)
                .unwrap();
        assert!(c.limits.is_none());
        assert!(c.queries.is_none());
    }

    #[test]
    fn cache_hit_needs_enough_blocks() {
        let h = Health {
            cache_hit_ratio: Some(0.5),
            cache_blocks: Some(100),
        };
        assert!(!h.cache_hit_usable(), "100 blocks is noise, not signal");
    }

    #[test]
    fn indexes_report_grades_come_through_verbatim() {
        let r = IndexesReport::decode(INDEXES).unwrap();
        assert_eq!(r.indexes.len(), 3);
        assert_eq!(r.indexes[0].confidence, "needs_code_check");
        assert_eq!(r.indexes[1].confidence, "inconclusive");
        assert_eq!(r.indexes[2].confidence, "catalog_proven");
        assert!(r.indexes[2].do_not_drop);
    }

    #[test]
    fn why_report_decodes_chains() {
        let r = WhyReport::decode(WHY).unwrap();
        assert_eq!(r.regressions_found, 1);
        let chain = &r.chains[0];
        assert_eq!(chain.symptom.before, Some(8.0));
        assert_eq!(chain.symptom.after, Some(26.0));
        assert_eq!(chain.hops.len(), 3);
        assert!((chain.confidence - 0.8).abs() < 1e-9);
    }

    #[test]
    fn garbage_is_bad_output_not_a_panic() {
        let err = Context::decode("pgbot exploded").unwrap_err();
        assert_eq!(err.kind, crate::sanitize::ErrorKind::BadOutput);
    }
}
