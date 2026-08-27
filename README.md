# pgterm

**htop for all your Postgres databases** — an interactive terminal UI that
monitors every database you care about in one place, powered by
[pgbot](https://pgbot.dev)'s read-only diagnostics.

```
┌──────────────────────────────────────────────────────────────────────┐
│ [ production ● ] [ staging ● ] [ analytics ! ] [ + Add DB ]          │
├──────────────────────────────────────────────────────────────────────┤
│ production                                       last check: 12s ago │
│                                                                      │
│ DATABASE HEALTH                                       94 / 100       │
│                                                                      │
│ Connections        OK          84 / 300                              │
│ Cache              OK          99.2%                                 │
│ Locks              FAIL        3 blocked                             │
│ Queries            WARN        2 regressions                         │
│ Indexes            WARN        27 unused · 43 GiB                    │
│ Vacuum             OK          3m ago                                │
│ Replication        OK          210 ms                                │
│                                                                      │
│ 1 failing · 2 warnings · 4 healthy                                   │
├──────────────────────────────────────────────────────────────────────┤
│ 1 Inspect   2 Queries   3 Indexes   4 Tables   5 Why                 │
├──────────────────────────────────────────────────────────────────────┤
│ production > _                                                       │
└──────────────────────────────────────────────────────────────────────┘
```

Each tab is one PostgreSQL database. pgterm checks them all in the
background and flags the tab that needs attention — without stealing focus
from the one you're looking at.

## Try it in 10 seconds (no database needed)

```bash
cargo build --release
./demo/run.sh
```

Three pretend databases — healthy, warnings, and a blocked-locks incident —
served by a fake pgbot from the test fixtures. Everything works: tabs,
views 1–5, `r`, the command bar (`/ ask why did checkout get slower?`).
The demo keeps its config under `$TMPDIR/pgterm-demo`, so your real
configuration is untouched.

## Quickstart

pgterm drives [pgbot](https://github.com/pgrundev/pgbot), so install that
first:

```bash
curl -fsSL https://pgbot.dev/install | sh
```

Then add your database and open the terminal:

```bash
export DATABASE_URL='postgresql://...'

pgterm add production
pgterm
```

More databases, each from its own environment variable:

```bash
pgterm add staging   --env STAGING_DATABASE_URL
pgterm add analytics --env ANALYTICS_DATABASE_URL --open
```

`add` tests the connection before saving anything; a broken profile is never
persisted.

## Security posture

- The config (`~/.config/pgterm/config.toml`) stores **environment-variable
  names, never connection strings**. No password ever touches disk, logs, or
  the screen; every error is scrubbed of credentials.
- In the add-database popup you may also paste a `postgres://` URL directly:
  it is masked on screen, kept **in memory for that session only**, and never
  written anywhere — the tab disappears when pgterm exits. Use an env-var
  reference for databases you want to keep.
- Connection strings reach pgbot through the child process **environment,
  never argv** — nothing shows up in `ps` or shell history.
- Strictly read-only: pgterm runs only whitelisted pgbot diagnostics. There
  is no SQL console, no shell, no "fix it" button, and command-bar input is
  parsed against a closed set of verbs — never handed to a shell.

## Commands

| Command | What it does |
|---|---|
| `pgterm` | Open the terminal UI |
| `pgterm add <name>` | Add the database from `DATABASE_URL` (validates first) |
| `pgterm add <name> --env <VAR>` | Add a database by env-var reference |
| `pgterm add <name> --env <VAR> --open` | Add, then open the UI on it |
| `pgterm list` | List configured databases (names only, never values) |
| `pgterm remove <name>` | Remove the local profile (PostgreSQL untouched) |
| `pgterm --interval 30s` | Background check cadence (default 60s) |
| `pgterm --no-monitor` | Disable background checks |

## Keys

```
Tab / Shift+Tab    switch database          /    command bar
1..5               inspect · queries ·      r    refresh
  ← / →            indexes · tables · why   a    add database
?                  help                     q    quit
```

## Configuration

`~/.config/pgterm/config.toml` (or `$XDG_CONFIG_HOME/pgterm/config.toml`):

```toml
version = 1

[settings]
interval_seconds = 60
max_concurrent_checks = 3

[[databases]]
name = "production"
env = "PROD_DATABASE_URL"
```

## Building

```bash
cargo build --release   # → target/release/pgterm
cargo test
```

pgterm finds pgbot on `PATH`, or wherever `PGBOT_BIN` points.

## License

Apache-2.0
