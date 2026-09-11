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

/// Opens a connection, through the profile's SSH jump host when it has one.
/// TLS follows the DSN's own `sslmode`, which tokio-postgres parses — pgterm
/// does not weaken it, tunneled or not.
pub async fn connect(source: &ConnSource, ssh: Option<&str>) -> Result<Client, SafeError> {
    let dsn = source.resolve()?;
    let fail = |e: tokio_postgres::Error| chain_error(e, &dsn);

    let config: tokio_postgres::Config = dsn.parse().map_err(|e: tokio_postgres::Error| {
        SafeError::new(ErrorKind::Usage, &e.to_string(), Some(&dsn))
    })?;

    if let Some(spec) = ssh {
        return connect_ssh(&config, &dsn, spec).await;
    }

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
            if dsn_demands_tls(&dsn) {
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

/// tokio-postgres' own Display is terse ("error connecting to server");
/// the reason lives in the source chain, and the reason is the useful part.
fn chain_error(e: tokio_postgres::Error, dsn: &str) -> SafeError {
    let mut msg = e.to_string();
    let mut src = std::error::Error::source(&e);
    while let Some(cause) = src {
        msg.push_str(&format!(": {cause}"));
        src = cause.source();
    }
    SafeError::new(ErrorKind::ConnectionFailed, &msg, Some(dsn))
}

fn dsn_demands_tls(dsn: &str) -> bool {
    dsn.contains("sslmode=require")
        || dsn.contains("sslmode=verify-ca")
        || dsn.contains("sslmode=verify-full")
}

/// `connect`, through an SSH jump host: the TCP leg is an `ssh -W` child, and
/// the Postgres startup (TLS negotiation included) runs over its stdio via
/// `connect_raw`. The DSN's hostname is what TLS verifies — the tunnel never
/// rewrites it to a loopback address.
async fn connect_ssh(
    config: &tokio_postgres::Config,
    dsn: &str,
    spec: &str,
) -> Result<Client, SafeError> {
    use tokio_postgres::config::Host;
    use tokio_postgres::tls::MakeTlsConnect;

    let spec = crate::ssh::Spec::parse(spec)
        .map_err(|e| SafeError::new(ErrorKind::Usage, &e, Some(dsn)))?;
    let host = match config.get_hosts().first() {
        Some(Host::Tcp(h)) => h.clone(),
        #[cfg(unix)]
        Some(Host::Unix(_)) => {
            return Err(SafeError::new(
                ErrorKind::Usage,
                "a unix-socket DSN cannot go through an SSH tunnel — name the host and port the jump host can reach",
                Some(dsn),
            ))
        }
        None => {
            return Err(SafeError::new(
                ErrorKind::Usage,
                "the DSN names no host to tunnel to",
                Some(dsn),
            ))
        }
    };
    let port = config.get_ports().first().copied().unwrap_or(5432);

    // A failed login or refused forward surfaces as EOF on the stream; ssh's
    // stderr has the actual reason, so it is folded into the error.
    let fail = |e: tokio_postgres::Error, stderr: &std::sync::Arc<std::sync::Mutex<String>>| {
        let mut err = chain_error(e, dsn);
        if let Ok(said) = stderr.lock() {
            let said = said.trim();
            if !said.is_empty() {
                err = SafeError::new(
                    err.kind,
                    &format!("{} — ssh: {said}", err.message),
                    Some(dsn),
                );
            }
        }
        err
    };

    let connect_tls = async {
        let stream = crate::ssh::open(&spec, &host, port)?;
        let stderr = stream.stderr_handle();
        let mut mk = tokio_postgres_rustls::MakeRustlsConnect::new(tls_config()?);
        let tls = <tokio_postgres_rustls::MakeRustlsConnect as MakeTlsConnect<
            crate::ssh::SshStream,
        >>::make_tls_connect(&mut mk, &host)
        .map_err(|e| SafeError::new(ErrorKind::ConnectionFailed, &e.to_string(), Some(dsn)))?;
        match config.connect_raw(stream, tls).await {
            Ok(pair) => Ok(pair),
            Err(e) => {
                // Give the stderr reader a beat to collect ssh's last words.
                tokio::time::sleep(Duration::from_millis(150)).await;
                Err(fail(e, &stderr))
            }
        }
    };
    let tls_attempt = match tokio::time::timeout(CONNECT_TIMEOUT, connect_tls).await {
        Ok(r) => r,
        Err(_) => Err(timeout_error()),
    };
    match tls_attempt {
        Ok((client, connection)) => {
            tokio::spawn(async move {
                let _ = connection.await;
            });
            Ok(client)
        }
        Err(tls_err) => {
            // Same fallback contract as the direct path: plain only when the
            // DSN did not demand TLS — over a fresh tunnel, the first ssh died
            // with its stream.
            if dsn_demands_tls(dsn) {
                return Err(tls_err);
            }
            let connect_plain = async {
                let stream = crate::ssh::open(&spec, &host, port)?;
                let stderr = stream.stderr_handle();
                match config.connect_raw(stream, NoTls).await {
                    Ok(pair) => Ok(pair),
                    Err(e) => {
                        tokio::time::sleep(Duration::from_millis(150)).await;
                        Err(fail(e, &stderr))
                    }
                }
            };
            match tokio::time::timeout(CONNECT_TIMEOUT, connect_plain).await {
                Ok(Ok((client, connection))) => {
                    tokio::spawn(async move {
                        let _ = connection.await;
                    });
                    Ok(client)
                }
                Ok(Err(e)) => Err(e),
                Err(_) => Err(timeout_error()),
            }
        }
    }
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
        // relkind and friends: Postgres' internal one-byte "char", which is
        // an i8 on the wire and would otherwise read as NULL.
        Type::CHAR => row
            .try_get::<_, Option<i8>>(i)
            .ok()
            .flatten()
            .map(|v| (v as u8 as char).to_string()),
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

/// The Data browser's queries. The schema and table names come from the
/// catalog, not from typing, but they still reach SQL as text — so they are
/// quoted here by the same rules Postgres uses, with tests, rather than
/// interpolated raw.
/// A SQL string literal: single quotes doubled, wrapped in single quotes.
pub fn quote_literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// A SQL identifier: double quotes doubled, wrapped in double quotes. Always
/// quoted, so a name that is a keyword or has capitals still works.
pub fn quote_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

pub const SCHEMAS_SQL: &str = "\
SELECT n.nspname AS schema,
       count(c.oid) FILTER (WHERE c.relkind IN ('r','p')) AS tables
  FROM pg_namespace n
  LEFT JOIN pg_class c ON c.relnamespace = n.oid
 WHERE n.nspname NOT LIKE 'pg\\_%' AND n.nspname <> 'information_schema'
 GROUP BY n.nspname
 ORDER BY n.nspname";

/// The tables of one schema, largest first.
pub fn tables_sql(schema: &str) -> String {
    format!(
        "SELECT c.relname AS table,
                CASE WHEN c.reltuples < 0 THEN '?'
                     ELSE c.reltuples::bigint::text END AS est_rows,
                pg_size_pretty(pg_total_relation_size(c.oid)) AS size
           FROM pg_class c
           JOIN pg_namespace n ON n.oid = c.relnamespace
          WHERE n.nspname = {} AND c.relkind IN ('r','p')
          ORDER BY pg_total_relation_size(c.oid) DESC",
        quote_literal(schema)
    )
}

/// One page of a table's rows. Capped at the server as well as in the reader.
pub fn rows_sql(schema: &str, table: &str, offset: usize) -> String {
    format!(
        "SELECT * FROM {}.{} LIMIT {} OFFSET {}",
        quote_ident(schema),
        quote_ident(table),
        PAGE_ROWS,
        offset
    )
}

/// Rows the Data browser pulls per page.
pub const PAGE_ROWS: usize = 100;

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
    fn quoting_follows_postgres_rules_and_survives_hostile_names() {
        assert_eq!(quote_literal("public"), "'public'");
        assert_eq!(quote_literal("it's"), "'it''s'");
        assert_eq!(
            quote_literal("'; DROP TABLE users --"),
            "'''; DROP TABLE users --'"
        );
        assert_eq!(quote_ident("orders"), "\"orders\"");
        assert_eq!(quote_ident("Odd Name"), "\"Odd Name\"");
        assert_eq!(quote_ident("we\"ird"), "\"we\"\"ird\"");

        // A hostile name cannot break out of either position.
        let sql = tables_sql("'; DROP TABLE users --");
        assert!(sql.contains("'''; DROP TABLE users --'"), "{sql}");
        let sql = rows_sql("pu\"blic", "or\"ders", 200);
        assert!(sql.contains("\"pu\"\"blic\".\"or\"\"ders\""), "{sql}");
        assert!(
            sql.contains("OFFSET 200") && sql.contains("LIMIT 100"),
            "{sql}"
        );
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

    /// Both halves mutate PGTERM_SSH_BIN, and lib tests share the process
    /// environment — one test, sequential, so they cannot race each other.
    #[cfg(unix)]
    #[test]
    fn ssh_tunnel_failures_say_what_actually_went_wrong() {
        use std::io::Write as _;
        use std::os::unix::fs::PermissionsExt;

        let rt = tokio::runtime::Runtime::new().unwrap();
        let dsn = "postgres://u:pw@db.internal:5432/app?sslmode=disable";

        // No ssh at all: the error names the requirement and the override.
        std::env::set_var("PGTERM_SSH_BIN", "/nonexistent/pgterm-test-ssh");
        let err =
            rt.block_on(async { connect(&ConnSource::Session(dsn.into()), Some("bastion")).await });
        let err = err.expect_err("no ssh, no tunnel");
        assert!(err.to_string().contains("OpenSSH"), "{err}");

        // A stand-in ssh that refuses the way a real one does: reason on
        // stderr, nothing on stdout. The error the SQL tab shows must carry
        // that reason, not just "unexpected EOF".
        let dir = std::env::temp_dir().join(format!("pgterm-ssh-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("ssh");
        let mut f = std::fs::File::create(&fake).unwrap();
        f.write_all(b"#!/bin/sh\necho 'Permission denied (publickey).' >&2\nexit 255\n")
            .unwrap();
        drop(f);
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var("PGTERM_SSH_BIN", &fake);

        let err = rt.block_on(async {
            connect(&ConnSource::Session(dsn.into()), Some("deploy@bastion")).await
        });
        std::env::remove_var("PGTERM_SSH_BIN");
        let _ = std::fs::remove_dir_all(&dir);
        let err = err.expect_err("a refused ssh login cannot connect");
        assert!(
            err.to_string().contains("Permission denied"),
            "ssh's reason is missing: {err}"
        );
        assert!(!err.to_string().contains(":pw@"), "password leaked: {err}");
    }

    #[test]
    fn ssh_tunnel_refuses_a_unix_socket_dsn() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let err = rt.block_on(async {
            let source =
                ConnSource::Session("postgres://u:pw@%2Fvar%2Frun%2Fpostgresql/app".into());
            connect(&source, Some("bastion")).await
        });
        let err = err.expect_err("a socket path cannot be tunneled");
        assert!(err.to_string().contains("unix-socket"), "{err}");
    }

    /// Against a real database:
    /// `PGTERM_TEST_DATABASE_URL=postgres://... cargo test --lib live_ -- --ignored --nocapture`
    /// Add `PGTERM_TEST_SSH_TUNNEL=[user@]host[:port]` to run the tunnel test.
    fn live_source() -> Option<ConnSource> {
        std::env::var("PGTERM_TEST_DATABASE_URL")
            .ok()
            .filter(|u| !u.is_empty())
            .map(ConnSource::Session)
    }

    #[test]
    #[ignore]
    fn live_ssh_tunnel_runs_a_query() {
        let (Some(source), Ok(spec)) = (live_source(), std::env::var("PGTERM_TEST_SSH_TUNNEL"))
        else {
            println!("set PGTERM_TEST_DATABASE_URL and PGTERM_TEST_SSH_TUNNEL to run this");
            return;
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut c = connect(&source, Some(&spec))
                .await
                .expect("tunneled connect");
            let r = run_sql(&mut c, "SELECT 1 AS one", WritePolicy::ReadOnly)
                .await
                .expect("select over the tunnel");
            assert_eq!(r.rows[0], vec!["1"]);
        });
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
            let mut c = connect(&source, None).await.expect("connect");

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

            let r = run_sql(
                &mut c,
                "SELECT relkind FROM pg_class LIMIT 1",
                WritePolicy::ReadOnly,
            )
            .await
            .expect("relkind");
            println!("relkind: {:?} ({:?})", r.rows[0], r.types);
            assert_ne!(
                r.rows[0][0], "NULL",
                "Postgres' internal char type must render, not read as NULL"
            );

            let r = run_sql(&mut c, &tables_sql("public"), WritePolicy::ReadOnly)
                .await
                .expect("tables");
            println!("tables (2): {:?}", &r.rows[..r.rows.len().min(2)]);
            for row in &r.rows {
                assert_ne!(row[1], "-1", "never-analyzed must read as ?, not -1 rows");
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
