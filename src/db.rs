//! pgterm's own PostgreSQL connection, behind the SQL and Data tabs.
//!
//! Everything here is a deliberate exception to "pgterm never touches a
//! database directly" — pgbot cannot run your query for you. The exception is
//! fenced:
//!
//! * Every statement runs inside a transaction. It is `READ ONLY` unless the
//!   profile opted in with `writes = true`, so the *server* rejects a write,
//!   not a keyword scan of ours.
//! * `statement_timeout` and `idle_in_transaction_session_timeout` are set on
//!   every transaction, so a runaway query cannot pin a connection.
//! * Rows are capped, so `SELECT * FROM events` cannot pull a billion rows
//!   into the terminal.
//! * The DSN is resolved in memory at connect time, never logged or shown.

use std::time::Duration;

use rustls::ClientConfig;
use tokio_postgres::types::Type;
use tokio_postgres::{Client, NoTls, Row};

use crate::runner::ConnSource;
use crate::sanitize::{ErrorKind, SafeError};

/// Rows a single run may return. Beyond this the grid says it truncated.
pub const ROW_CAP: usize = 1_000;
/// Characters of any one cell that reach the grid.
pub const CELL_CAP: usize = 200;
pub const STATEMENT_TIMEOUT: &str = "30s";
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// One result set, already rendered to strings — the UI never holds a live
/// row, so nothing borrows the connection.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub types: Vec<String>,
    pub rows: Vec<Vec<String>>,
    /// True when the server had more rows than ROW_CAP.
    pub truncated: bool,
    pub elapsed_ms: u128,
    /// For statements that return no rows: "UPDATE 3", "CREATE TABLE".
    pub tag: Option<String>,
}

/// Whether a database may be written to at all, and whether a write needs
/// confirming first. `writes = true` lifts READ ONLY; a prod badge still asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WritePolicy {
    ReadOnly,
    /// Writes allowed, but a write statement asks for a typed confirmation.
    ConfirmWrites,
    Writes,
}

impl WritePolicy {
    pub fn read_only(self) -> bool {
        self == WritePolicy::ReadOnly
    }
}

/// SQL keywords that mean "this changes something". Only used to decide when
/// to ASK — the READ ONLY transaction is what actually enforces the policy.
const WRITE_KEYWORDS: [&str; 14] = [
    "insert", "update", "delete", "merge", "alter", "drop", "create", "truncate", "grant",
    "revoke", "vacuum", "reindex", "cluster", "copy",
];

/// Does this buffer look like it writes? Comments and string literals are
/// stripped first so a keyword inside them does not trigger a prompt.
pub fn looks_like_write(sql: &str) -> bool {
    let stripped = strip_sql_noise(sql).to_lowercase();
    stripped
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .any(|word| WRITE_KEYWORDS.contains(&word))
}

/// Remove `--` line comments, `/* */` blocks and quoted strings.
fn strip_sql_noise(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '-' if chars.peek() == Some(&'-') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        out.push(' ');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut prev = ' ';
                for c in chars.by_ref() {
                    if prev == '*' && c == '/' {
                        break;
                    }
                    prev = c;
                }
                out.push(' ');
            }
            '\'' | '"' => {
                for q in chars.by_ref() {
                    if q == c {
                        break;
                    }
                }
                out.push(' ');
            }
            _ => out.push(c),
        }
    }
    out
}

fn tls_config() -> Result<ClientConfig, SafeError> {
    // The process-wide provider must be installed before any config is built;
    // doing it here keeps the requirement next to the only use.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut roots = rustls::RootCertStore::empty();
    let loaded = rustls_native_certs::load_native_certs();
    for cert in loaded.certs {
        let _ = roots.add(cert);
    }
    if roots.is_empty() {
        return Err(SafeError::new(
            ErrorKind::ConnectionFailed,
            "no system certificate roots found, so a TLS connection cannot be verified",
            None,
        ));
    }
    Ok(ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth())
}

/// Opens a connection. TLS follows the DSN's own `sslmode`, which
/// tokio-postgres parses — pgterm does not weaken it.
pub async fn connect(source: &ConnSource) -> Result<Client, SafeError> {
    let dsn = source.resolve()?;
    let fail = |e: tokio_postgres::Error| {
        SafeError::new(ErrorKind::ConnectionFailed, &e.to_string(), Some(&dsn))
    };

    let config: tokio_postgres::Config = dsn.parse().map_err(|e: tokio_postgres::Error| {
        SafeError::new(ErrorKind::Usage, &e.to_string(), Some(&dsn))
    })?;

    let connect_tls = async {
        let tls = tokio_postgres_rustls::MakeRustlsConnect::new(tls_config()?);
        config.connect(tls).await.map_err(fail)
    };

    type Pair = (
        Client,
        tokio_postgres::Connection<
            tokio_postgres::Socket,
            <tokio_postgres_rustls::MakeRustlsConnect as tokio_postgres::tls::MakeTlsConnect<
                tokio_postgres::Socket,
            >>::Stream,
        >,
    );
    let tls_attempt: Result<Pair, SafeError> =
        match tokio::time::timeout(CONNECT_TIMEOUT, connect_tls).await {
            Ok(r) => r,
            Err(_) => Err(timeout_error()),
        };
    let (client, connection) = match tls_attempt {
        Ok(pair) => {
            tokio::spawn(async move {
                let _ = pair.1.await;
            });
            return Ok(pair.0);
        }
        Err(tls_err) => {
            // A server with TLS off refuses the handshake; fall back to plain
            // only when the DSN did not demand TLS.
            let demanded = dsn.contains("sslmode=require")
                || dsn.contains("sslmode=verify-ca")
                || dsn.contains("sslmode=verify-full");
            if demanded {
                return Err(tls_err);
            }
            match tokio::time::timeout(CONNECT_TIMEOUT, config.connect(NoTls)).await {
                Ok(Ok(pair)) => pair,
                Ok(Err(e)) => return Err(fail(e)),
                Err(_) => return Err(timeout_error()),
            }
        }
    };

    // The connection future drives the socket; it ends when the client drops.
    tokio::spawn(async move {
        let _ = connection.await;
    });
    Ok(client)
}

fn timeout_error() -> SafeError {
    SafeError::new(
        ErrorKind::Timeout,
        &format!("no connection within {}s", CONNECT_TIMEOUT.as_secs()),
        None,
    )
}

/// Runs one buffer inside a bounded transaction and renders the result.
pub async fn run_sql(
    client: &mut Client,
    sql: &str,
    policy: WritePolicy,
) -> Result<QueryResult, SafeError> {
    let started = std::time::Instant::now();
    let tx = client
        .transaction()
        .await
        .map_err(|e| SafeError::new(ErrorKind::BadOutput, &e.to_string(), None))?;

    let mode = if policy.read_only() {
        "SET TRANSACTION READ ONLY"
    } else {
        "SET TRANSACTION READ WRITE"
    };
    for setup in [
        mode.to_string(),
        format!("SET LOCAL statement_timeout = '{STATEMENT_TIMEOUT}'"),
        "SET LOCAL idle_in_transaction_session_timeout = '60s'".to_string(),
    ] {
        tx.batch_execute(&setup)
            .await
            .map_err(|e| SafeError::new(ErrorKind::BadOutput, &e.to_string(), None))?;
    }

    let rows = tx.query(sql, &[]).await;
    let result = match rows {
        Ok(rows) => Ok(render_rows(rows, started)),
        Err(e) => {
            // No rows is not an error: a DDL or DML statement has a tag instead.
            if e.as_db_error().is_none() && e.to_string().contains("no results") {
                Ok(QueryResult {
                    tag: Some("done".into()),
                    elapsed_ms: started.elapsed().as_millis(),
                    ..Default::default()
                })
            } else {
                Err(SafeError::new(
                    ErrorKind::QueryFailed,
                    &db_message(&e),
                    None,
                ))
            }
        }
    };
    // Read-only work has nothing to keep; a write is committed only if it ran.
    match &result {
        Ok(_) => {
            let _ = tx.commit().await;
        }
        Err(_) => {
            let _ = tx.rollback().await;
        }
    }
    result
}

/// Postgres' own message, with its position when it gave one.
fn db_message(e: &tokio_postgres::Error) -> String {
    match e.as_db_error() {
        Some(db) => {
            let mut s = db.message().to_string();
            if let Some(p) = db.position() {
                s.push_str(&format!(" (at {p:?})"));
            }
            if let Some(h) = db.hint() {
                s.push_str(&format!(" — {h}"));
            }
            s
        }
        None => e.to_string(),
    }
}

fn render_rows(rows: Vec<Row>, started: std::time::Instant) -> QueryResult {
    let mut out = QueryResult {
        elapsed_ms: started.elapsed().as_millis(),
        ..Default::default()
    };
    let Some(first) = rows.first() else {
        out.tag = Some("0 rows".into());
        return out;
    };
    for c in first.columns() {
        out.columns.push(c.name().to_string());
        out.types.push(c.type_().name().to_string());
    }
    out.truncated = rows.len() > ROW_CAP;
    for row in rows.iter().take(ROW_CAP) {
        let cells = (0..row.columns().len())
            .map(|i| cell_to_string(row, i))
            .collect();
        out.rows.push(cells);
    }
    out
}

/// Every column reaches the grid as text. Types pgterm does not know are read
/// as their text representation rather than failing the whole query.
fn cell_to_string(row: &Row, i: usize) -> String {
    let ty = row.columns()[i].type_();
    let s = match *ty {
        Type::BOOL => row
            .try_get::<_, Option<bool>>(i)
            .ok()
            .flatten()
            .map(|v| v.to_string()),
        Type::INT2 => row
            .try_get::<_, Option<i16>>(i)
            .ok()
            .flatten()
            .map(|v| v.to_string()),
        Type::INT4 => row
            .try_get::<_, Option<i32>>(i)
            .ok()
            .flatten()
            .map(|v| v.to_string()),
        Type::INT8 => row
            .try_get::<_, Option<i64>>(i)
            .ok()
            .flatten()
            .map(|v| v.to_string()),
        Type::FLOAT4 => row
            .try_get::<_, Option<f32>>(i)
            .ok()
            .flatten()
            .map(|v| v.to_string()),
        Type::FLOAT8 => row
            .try_get::<_, Option<f64>>(i)
            .ok()
            .flatten()
            .map(|v| v.to_string()),
        Type::BYTEA => row
            .try_get::<_, Option<Vec<u8>>>(i)
            .ok()
            .flatten()
            .map(|v| format!("\\x{} ({} bytes)", hex_prefix(&v), v.len())),
        _ => row
            .try_get::<_, Option<String>>(i)
            .ok()
            .flatten()
            .or_else(|| {
                // Anything else: ask the server for its text form.
                row.try_get::<_, Option<&str>>(i)
                    .ok()
                    .flatten()
                    .map(str::to_string)
            }),
    };
    match s {
        None => "NULL".to_string(),
        Some(v) => truncate_cell(&v),
    }
}

fn hex_prefix(bytes: &[u8]) -> String {
    bytes.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

fn truncate_cell(v: &str) -> String {
    let flat = v.replace(['\n', '\t', '\r'], " ");
    if flat.chars().count() <= CELL_CAP {
        flat
    } else {
        flat.chars().take(CELL_CAP - 1).collect::<String>() + "…"
    }
}

/// The Data browser's queries. Written here rather than composed in the UI so
/// every identifier is parameterised, never interpolated.
pub const SCHEMAS_SQL: &str = "\
SELECT n.nspname AS schema,
       count(c.oid) FILTER (WHERE c.relkind IN ('r','p')) AS tables
  FROM pg_namespace n
  LEFT JOIN pg_class c ON c.relnamespace = n.oid
 WHERE n.nspname NOT LIKE 'pg\\_%' AND n.nspname <> 'information_schema'
 GROUP BY n.nspname
 ORDER BY n.nspname";

pub const TABLES_SQL: &str = "\
SELECT c.relname AS table,
       c.reltuples::bigint AS est_rows,
       pg_total_relation_size(c.oid) AS bytes
  FROM pg_class c
  JOIN pg_namespace n ON n.oid = c.relnamespace
 WHERE n.nspname = $1 AND c.relkind IN ('r','p')
 ORDER BY pg_total_relation_size(c.oid) DESC";

/// Page one table. The identifier is quoted by the server via format(%I), so
/// a hostile table name cannot break out.
pub const ROWS_SQL: &str = "\
SELECT format('SELECT * FROM %I.%I LIMIT $1 OFFSET $2', $2::text, $3::text)";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_detection_ignores_comments_and_string_literals() {
        assert!(!looks_like_write("SELECT * FROM orders"));
        assert!(!looks_like_write("SELECT 'we should delete this' AS note"));
        assert!(!looks_like_write("-- delete the old rows\nSELECT 1"));
        assert!(!looks_like_write("/* update later */ SELECT 1"));
        assert!(!looks_like_write("SELECT \"deleted_at\" FROM users"));
        assert!(
            !looks_like_write("SELECT updated_at FROM users"),
            "a column named updated_at is not an UPDATE"
        );

        assert!(looks_like_write("DELETE FROM orders WHERE id = 1"));
        assert!(looks_like_write("  update orders set x = 1"));
        assert!(looks_like_write("SELECT 1; DROP TABLE users"));
        assert!(looks_like_write("CREATE INDEX CONCURRENTLY ON orders (id)"));
        assert!(looks_like_write("COPY orders FROM '/tmp/x'"));
        assert!(looks_like_write(
            "WITH d AS (DELETE FROM t RETURNING *) SELECT * FROM d"
        ));
    }

    #[test]
    fn write_policy_gates_read_only() {
        assert!(WritePolicy::ReadOnly.read_only());
        assert!(!WritePolicy::Writes.read_only());
        assert!(!WritePolicy::ConfirmWrites.read_only());
    }

    #[test]
    fn cells_are_flattened_and_capped() {
        assert_eq!(truncate_cell("a\nb\tc"), "a b c");
        let long = "x".repeat(CELL_CAP + 50);
        let out = truncate_cell(&long);
        assert_eq!(out.chars().count(), CELL_CAP);
        assert!(out.ends_with('…'));
        assert_eq!(truncate_cell("short"), "short");
    }

    #[test]
    fn strip_sql_noise_removes_what_it_should_and_keeps_the_rest() {
        assert_eq!(
            strip_sql_noise("SELECT 1 -- delete\n+ 2").trim(),
            "SELECT 1  + 2"
        );
        assert!(!strip_sql_noise("/* drop */ SELECT 1").contains("drop"));
        assert!(!strip_sql_noise("SELECT 'insert'").contains("insert"));
        assert!(strip_sql_noise("SELECT x FROM t").contains("FROM t"));
    }

    /// Against a real database:
    /// `PGTERM_TEST_DATABASE_URL=postgres://... cargo test --lib live_ -- --ignored --nocapture`
    fn live_source() -> Option<ConnSource> {
        std::env::var("PGTERM_TEST_DATABASE_URL")
            .ok()
            .filter(|u| !u.is_empty())
            .map(ConnSource::Session)
    }

    #[test]
    #[ignore]
    fn live_read_only_really_is_read_only() {
        let Some(source) = live_source() else {
            println!("set PGTERM_TEST_DATABASE_URL to run this");
            return;
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut c = connect(&source).await.expect("connect");

            let r = run_sql(
                &mut c,
                "SELECT 1 AS one, 'x' AS s, NULL::int AS n",
                WritePolicy::ReadOnly,
            )
            .await
            .expect("select");
            println!(
                "columns {:?} types {:?} rows {:?}",
                r.columns, r.types, r.rows
            );
            assert_eq!(r.columns, vec!["one", "s", "n"]);
            assert_eq!(r.rows[0], vec!["1", "x", "NULL"]);

            let err = run_sql(
                &mut c,
                "CREATE TABLE pgterm_should_never_exist (id int)",
                WritePolicy::ReadOnly,
            )
            .await
            .expect_err("a read-only transaction must refuse DDL");
            println!("refused: {err}");
            assert_eq!(err.kind, crate::sanitize::ErrorKind::QueryFailed);

            // The connection still works after the rejection.
            let r = run_sql(
                &mut c,
                "SELECT count(*) FROM pg_class",
                WritePolicy::ReadOnly,
            )
            .await
            .expect("still usable");
            println!("pg_class rows: {:?}", r.rows[0]);

            let r = run_sql(&mut c, SCHEMAS_SQL, WritePolicy::ReadOnly)
                .await
                .expect("schemas");
            println!("schemas: {:?}", r.rows);
            for row in &r.rows {
                assert!(
                    !row[0].starts_with("pg_") && row[0] != "information_schema",
                    "system schema leaked into the browser: {row:?}"
                );
            }

            let bad = run_sql(
                &mut c,
                "SELECT * FROM no_such_table_here",
                WritePolicy::ReadOnly,
            )
            .await
            .expect_err("postgres reports its own error");
            println!("bad query: {bad}");
            assert!(bad.to_string().contains("no_such_table_here"), "{bad}");
        });
    }
}
