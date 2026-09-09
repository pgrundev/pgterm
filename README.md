# pgterm

**htop for all your Postgres databases** — an interactive terminal UI that
monitors every database you care about in one place, powered by
[pgbot](https://pgbot.dev)'s read-only diagnostics.

```
 pgterm   ▸ production  PROD                                                                        ^K commands  ? help
 DATABASES               │  1 Overview   2 PgBot                                               ! PostgreSQL 17 · 0s ago
                         │──────────────────────────────────────────────────────────────────────────────────────────────
  ! production      PROD │ production  PROD                                                        r refresh   2 pgbot
    2 indexes with zero  │ ● Connected · PostgreSQL 17.4 · RDS · up 10d · checked 0s ago
  ● staging      STAGING │
    checked 0s ago       │ ┌ PostgreSQL ─┐ ┌ Connections ┐ ┌ Active ─────┐ ┌ Size ───────┐ ┌ Uptime ─────┐
  ◌ analytics            │ │ 17.4        │ │ 84 / 300    │ │ 5           │ │ 140 GiB     │ │ 10d         │
    checking…            │ └─────────────┘ └─────────────┘ └─────────────┘ └─────────────┘ └─────────────┘
 + Add database          │
                         │  cache hit  [████████████████████]  99.2%     ok
                         │  lock wait  [░░░░░░░░░░░░░░░░░░░░]  —         ok
                         │  rollbacks  [██░░░░░░░░░░░░░░░░░░]  12.0%     watch
                         │  idle idx   [███░░░░░░░░░░░░░░░░░]  20 GiB    review
                         │
                         │ PGBOT   3 findings need attention
                         │  ⚠ 2 indexes with zero scans in the observed window                        confidence MEDIUM
                         │  ⚠ 2 queries regressed vs baseline                                           confidence HIGH
                         │  ⚠ rollbacks 12% of transactions                                           confidence MEDIUM
                         │  ✓ Connections   84 / 300
                         │  ✓ Cache         99.2%
                         │  ✓ Locks         0 blocked
                         │  ✓ Vacuum        13d ago
                         │  ✓ Replication   210 ms
```

One row per database in the sidebar, badged by environment. pgterm checks
them all in the background and flags the one that needs attention without
stealing focus from the one you're looking at. Overview answers "is this
database healthy" at a glance; the PgBot tab has the full diagnostics.

Below 100 columns the sidebar collapses to a tab strip and everything else
stays put, so pgterm still works in a split pane.

## Try it in 10 seconds (no database needed)

```bash
cargo build --release
./demo/run.sh
```

Three pretend databases — healthy, warnings, and a blocked-locks incident —
served by a fake pgbot from the test fixtures. Everything works: the sidebar
and its badges, both tabs, the gauges, the palette (`:`), the pgbot views,
and the command bar (`/ ask why did checkout get slower?`). The demo keeps
its config under `$TMPDIR/pgterm-demo`, so your real configuration is
untouched.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/pgrundev/pgterm/main/install.sh | sh
```

Downloads the latest release for your platform (linux/macOS, amd64/arm64),
verifies its checksum, and installs to `/usr/local/bin` (override with
`PGTERM_INSTALL_DIR`). pgterm drives [pgbot](https://github.com/pgrundev/pgbot)
— the diagnostic engine — so if pgbot isn't on your PATH the installer fetches
it too, through pgbot's own checksum-verified installer (skip that with
`PGTERM_NO_PGBOT=1`). `https://pgterm.dev/install.sh` works too (it
redirects here — keep the `-L` flag).

Or with [Homebrew](https://brew.sh) on macOS or Linux — name both formulae,
pgterm and the pgbot it drives, so Homebrew trusts both from the tap:

```bash
brew install pgrundev/tap/pgterm pgrundev/tap/pgbot
```

On Windows, in PowerShell (install pgbot separately from
[pgbot.dev](https://pgbot.dev)):

```powershell
irm https://pgterm.dev/install.ps1 | iex
```

Or build from source: `cargo build --release`.

## Quickstart

One line installs pgterm, and pgbot with it when missing:

```bash
curl -fsSL https://raw.githubusercontent.com/pgrundev/pgterm/main/install.sh | sh
```

Point pgterm at your database and open it:

```bash
export DATABASE_URL='postgresql://user:password@host:5432/dbname'

pgterm add production   # validates the connection, then saves the profile
pgterm                  # opens the UI
```

`add` tests the connection before saving anything; a broken profile is never
persisted.

### Adding more databases

Give each database its own environment variable and reference it by name:

```bash
export STAGING_DATABASE_URL='postgresql://...'
export ANALYTICS_DATABASE_URL='postgresql://...'

pgterm add staging   --env STAGING_DATABASE_URL
pgterm add analytics --env ANALYTICS_DATABASE_URL --open   # --open jumps straight in
```

pgterm resolves variables when it starts — export first, then launch. To make
a variable survive new terminals, add its `export` line to your shell profile
(`~/.zshrc` or `~/.bashrc`):

```bash
echo "export STAGING_DATABASE_URL='postgresql://...'" >> ~/.zshrc
```

`pgterm list` shows every profile and whether its variable is currently set.

### Adding from inside the UI

Press `a` (or click `+ Add database`). Name, then Stage (`←`/`→` cycles
auto · prod · staging · dev · local), then Connection, which accepts any of:

| You type or paste | What happens |
|---|---|
| `STAGING_DATABASE_URL` | references the exported variable — persisted to config |
| `postgresql://user:pass@host/db` | connects now, **session-only**, never saved |
| `STAGING_DATABASE_URL='postgresql://...'` | connects now with the URL, saves only the **name** |

Whatever you paste is masked on screen; connection strings never touch disk.

## Security posture

- The config (`~/.config/pgterm/config.toml`) stores **environment-variable
  names, never connection strings**. No password ever touches disk, logs, or
  the screen; every error is scrubbed of credentials.
- In the add-database popup you may also paste a `postgres://` URL directly:
  it is masked on screen, kept **in memory for that session only**, and never
  written anywhere — the tab disappears when pgterm exits. Use an env-var
  reference for databases you want to keep.
- Best of both: paste the whole export line —
  `STAGING_DATABASE_URL='postgresql://...'` — and pgterm connects with the
  URL now (memory only) while saving just the **variable name** to config,
  so the tab returns on the next launch once the variable is exported.
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
| `pgterm add <name> --stage prod` | Add with an environment badge |
| `pgterm add <name> --env <VAR> --open` | Add, then open the UI on it |
| `pgterm list` | List configured databases (names only, never values) |
| `pgterm remove <name>` | Remove the local profile (PostgreSQL untouched) |
| `pgterm --interval 30s` | Background check cadence (default 60s) |
| `pgterm --no-monitor` | Disable background checks |
| `pgterm --default-config` | Print an annotated default config |

## Keys

```
NAVIGATION
Tab / S-Tab        focus sidebar / main pane
[                  previous database
]                  next database
1                  overview tab
2                  pgbot tab
3                  sql tab
4                  data tab
5                  branches tab
C-k / :            command palette
/                  command bar (verbs, ask …)
a                  add database
r                  refresh
j / Down           scroll down / move down
k / Up             scroll up / move up

SIDEBAR
Enter              open the selected database

OVERVIEW
Enter              open pgbot findings

SQL TAB
F5                 run the query (Ctrl-Enter too)

DATA TAB
Enter              open the schema, table, or rows
Esc                back up one level

BRANCHES TAB
Enter              open the branch as a tab

PGBOT TAB
Left / h           previous pgbot view
Right / l          next pgbot view

GENERAL
q                  quit
?                  help
```

`?` shows this same list inside pgterm — it is generated from the keymap, so
it can never describe a binding that does not exist. A test checks this table
against it too.

### What moved in 0.2

- `Tab` / `Shift+Tab` now move between the sidebar and the main pane. `[` and
  `]` switch databases.
- Number keys pick tabs (`1` Overview, `2` PgBot, `3` SQL, `4` Data, `5` Branches). Inside PgBot, `←`/`→` or
  `h`/`l` step through Inspect · Queries · Indexes · Tables · Why.
- `Ctrl-K` (or `:`) opens the command palette. `/` is still the command bar.

## The Overview tab

Six tiles, then the same four gauges pgbot's own `inspect` shows, computed
from the same JSON by the same rules so the two never disagree:

| Gauge | What it measures | When it is not shown |
|---|---|---|
| cache hit | sampled cache-hit ratio; `low` when pgbot flags it | `thin sample` below 10,000 blocks |
| lock wait | sessions blocked right now | `not measurable` without lock data |
| rollbacks | rolled-back share of transactions; `watch` when flagged | `not measurable` without the counter |
| idle idx | bytes in zero-scan indexes, as a share of the database | `window < 15m` in a cold stats window |

Under them, the findings that need attention with pgbot's own confidence, then
a `✓` line per subsystem that came back clean. Press `Enter` (or `2`) for the
full report.

## SQL and Data

The SQL tab runs a query against the database and shows the rows. The Data
tab browses schemas, then tables with size and row estimates, then a page of
rows.

Both use pgterm's own connection, and both are fenced:

- **Every statement runs in a transaction that is `READ ONLY`** unless the
  profile opts in with `writes = true`. The server refuses the write, not a
  keyword check of ours, so there is nothing to trick.
- On a **PROD**-badged database with writes enabled, a statement that looks
  like a write asks you to type the database name first.
- `statement_timeout` is 30s and rows are capped, so a stray `SELECT *`
  cannot hang the UI or pull a billion rows into it.
- The Data browser is read-only whatever the profile allows. Browsing is
  never a way to change something.

```toml
[[databases]]
name = "staging"
env = "STAGING_DATABASE_URL"
writes = true    # lift READ ONLY for this database only
```

TLS follows the connection string's own `sslmode`; pgterm never weakens it.
Nothing you type is written to disk.

## Branches

When a database has a pgrun project, its branches appear in the sidebar and
on the Branches tab:

```toml
[[databases]]
name = "production"
env = "PROD_DATABASE_URL"
pgrun_project = "acme-api"   # `pgrun project list` shows your projects
```

`Enter` on a branch opens it as its own tab. The connection URL comes from
pgrun at that moment, lives in memory for the session, and is never written
to config — the tab is gone when you quit. Creating and deleting branches
stays in the pgrun CLI, where the confirmations already live.

Needs [pgrun](https://github.com/pgrundev/pgrun-cli) on your PATH (or
`PGRUN_BIN`), logged in. Without it the tab says so instead of failing.

## Stages and badges

Each database carries an environment badge — `PROD`, `STAGING`, `DEV`,
`LOCAL` — shown in the sidebar and the header. Set it explicitly, or let
pgterm infer it from the name:

```bash
pgterm add production --env PROD_DATABASE_URL --stage prod
```

Without `--stage`, a name containing `prod`, `stag`, `local` or `dev` picks
its own badge; anything else gets none. The badge is a word first, so it
survives a monochrome terminal.

## Configuration

`~/.config/pgterm/config.toml` (or `$XDG_CONFIG_HOME/pgterm/config.toml`).
`pgterm --default-config` prints an annotated copy of the defaults:

```toml
version = 1

[settings]
interval_seconds = 60
max_concurrent_checks = 3

[ui]
sidebar_detail = true   # a second, dim line per database in the sidebar
bell = false            # ring the terminal bell with a toast

[[databases]]
name = "production"
env = "PROD_DATABASE_URL"
stage = "prod"          # prod | staging | dev | local — inferred when absent
```

When a database you are *not* looking at turns critical or unavailable, a
one-line toast appears at the right of the command bar for five seconds
(and rings the bell if you asked for one). Nothing interrupts the database
you are actually reading.

## Building

```bash
cargo build --release   # → target/release/pgterm
cargo test
```

pgterm finds pgbot on `PATH`, or wherever `PGBOT_BIN` points.

## License

Apache-2.0
