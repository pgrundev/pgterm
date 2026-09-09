# pgterm redesign — sidebar shell, tabs, SQL, Data, Branches, Book (2026-09-08)

pgterm grows from "pgbot's health tabs" into a workbench for the databases
you care about, modelled on the desktop mock (sidebar of databases with
environment badges; Overview / PgBot / SQL / Data / Branches tabs; stat
tiles; a findings summary with confidence; a command palette). This document
is the approved design. Slice 1 (the shell) is specified to the level an
implementation plan needs; slices 2–4 are specified to the level of the
decisions already taken, and each gets its own plan when it starts.

Approved decisions (2026-09-08):

- Scope: everything in the mock, including SQL and Data.
- SQL write policy: **read-only by default, opt-in writes per database**; on
  a PROD-badged database a write additionally requires a typed confirmation.
  The Data browser is read-only regardless.
- Order: shell → connection+SQL+Data → Branches → Book. Each slice is a
  release.

## Architecture

pgterm becomes a shell over four sources, each isolated in its own module
with its own fake binary (or test database) for tests:

| Source | Module | What it feeds | Slice |
|---|---|---|---|
| pgbot (child process, unchanged) | `runner.rs`, `model.rs` | health, findings, PgBot tab | today |
| direct Postgres connection (tokio-postgres + rustls) | `db.rs` | latency/tables/schemas tiles, SQL, Data | 2 |
| pgrun-cli (child process) | `pgrun.rs` | Branches | 3 |
| pgbook (child process) | `book.rs` | Book | 4 |

Rules that hold across every slice:

- Secrets: config stores environment-variable **names**. Every source
  resolves the URL in memory at use time; children get it via their
  environment, the direct connection gets it in memory; it never reaches
  argv, disk, logs, or the screen. Children that do not need a DSN (pgrun,
  pgbook) get `DATABASE_URL`/`PGBOT_DATABASE_URL` removed from their env.
- Every error passes `sanitize.rs`.
- Closed argv sets for every child; no shell anywhere.
- State mutation happens only in `App::update`; the runtime performs
  `Effect`s and feeds results back as `Action`s.
- Nothing pgterm shows is computed from diagnostics it did not get from
  pgbot; pgbot's grading renders verbatim.
- `cargo test`, `cargo fmt --check`, `clippy -D warnings` green at every
  commit; Windows stays in the CI matrix.

### Config (`config.rs`)

```toml
version = 1

[settings]
interval_seconds = 60
max_concurrent_checks = 3

[[databases]]
name = "production"
env = "PROD_DATABASE_URL"      # the VARIABLE name (unchanged)
stage = "prod"                 # slice 1 — badge: prod | staging | dev | local
writes = false                 # slice 2 — SQL tab may write (default false)
pgrun_project = "acme-api"     # slice 3 — pgrun project slug for Branches
```

All three new fields are optional; a v1 file without them loads unchanged.
`stage` absent → inferred from the name (contains `prod` → prod, `stag` →
staging, `local` → local, `dev` → dev, else no badge); explicit always wins.
Unknown `stage` values are a config error naming the four allowed.

## Slice 1 — the shell (v0.2.0)

### Layout

Wide (≥ 100 columns):

```
 pgterm   ▸ Production  PROD                                   ^K commands  ? help
─────────────────────────┬────────────────────────────────────────────────────────
 DATABASES               │ 1 Overview   2 PgBot                ● PostgreSQL 17 · 12s ago
 ▸ ● Production    PROD  │────────────────────────────────────────────────────────
   ● Staging    STAGING  │ Production  PROD                          r refresh  2 pgbot
   ● Local        LOCAL  │ ● Connected · PostgreSQL 17.4 · RDS · up 12d · checked 12s ago
   ! Analytics      DEV  │
   + Add database        │ ┌ PostgreSQL ┐┌ Connections ┐┌ Active ┐┌ Cache hit ┐┌ Size ┐┌ Uptime ┐
                         │ │ 17.4       ││ 84 / 300    ││ 12     ││ 99.2%     ││ 18 GB││ 12d    │
                         │ └────────────┘└─────────────┘└────────┘└───────────┘└──────┘└────────┘
                         │
                         │ PGBOT   2 findings need attention
                         │ ⚠ 27 unused indexes · 43 GiB                        confidence HIGH
                         │ ⚠ 2 query regressions since baseline             confidence MEDIUM
                         │ ✓ Locks        none blocked
                         │ ✓ Replication  210 ms lag
                         │ ✓ Vacuum       3m ago
                         │ ✓ Cache        99.2%
─────────────────────────┴────────────────────────────────────────────────────────
 production > _
```

Rows: top bar (1), body, command bar (1). The body splits into the sidebar
(26 columns including its border) and the main pane. The main pane is: tab
row (1), rule (1), tab body. There is no separate shortcut row any more; the
tab row carries the numbers and the tab body's header carries the keys that
matter there.

Narrow (< 100 columns): the sidebar is gone and today's top tab strip
(`[ production ● ] [ staging ● ] [ + Add DB ]`) sits between the top bar and
the tab row. Everything else is identical. Minimum size stays 80×24.

### Sidebar

- Header `DATABASES`. One row per database: cursor `▸` (only when the
  sidebar has focus), health glyph in its tone (`●` healthy, `!` warning /
  critical, `○` unavailable, `◌` checking — unchanged), name, badge
  right-aligned. The selected database is REVERSED whether or not the
  sidebar has focus; `attention` bolds the row as today.
- Badges: `PROD` yellow, `STAGING` cyan, `DEV` green, `LOCAL` dark gray, all
  bold; no badge when stage is unknown. Text, not colour, carries the meaning.
- `+ Add database` row (Hit::OpenAdd).
- A `BRANCHES` section appears in slice 3.

### Tabs (`Tab` enum on `App`, one current tab per database)

`Overview`, `PgBot` in slice 1; `Sql`, `Data` (slice 2), `Branches` (3),
`Book` (4). Only implemented tabs render — no placeholders. Number keys
`1`..`n` select tabs; the tab row shows `1 Overview   2 PgBot …`, current
REVERSED, each a `Hit::SetTab`. The right end of the tab row shows the
server and freshness: `● PostgreSQL 17 · 12s ago` (glyph = health tone).

The current tab is per database (like `view` today) so switching databases
returns you where you were.

### Overview tab

From the cached inspect Context only (slice 2 adds the direct-connection
tiles). Sections top to bottom:

1. **Header**: name bold + badge; right: `r refresh  2 pgbot`.
2. **Status line**: `● Connected · PostgreSQL 17.4 · RDS · up 12d · checked
   12s ago`. Version = digits of `server.version_text` (fallback
   `short_version()`); provider from `server.provider` when non-empty
   (rendered upper-case: RDS, AURORA, CLOUDSQL, AZURE, SUPABASE, NEON);
   uptime from `server.uptime_seconds` via `format::duration_short` (`12d`,
   `3h`, `41m`). Unavailable: `○ Unavailable · <sanitized error> · r retry`.
   Checking with no cache: `◌ Checking…`.
3. **Tiles**: `PostgreSQL`, `Connections` (`84 / 300` from limits),
   `Active` (`activity.active`), `Cache hit` (`99.2%`, or `—` below
   `CACHE_HIT_MIN_BLOCKS`), `Size` (`tables.db_size_bytes`, `format::bytes`),
   `Uptime`. Each tile is a 3-row bordered box: dim label, bold value.
   Tiles flow left to right and wrap to a second row when the pane is
   narrower than the sum of their widths; a section that has no data shows
   `—`. (Slice 2 swaps `Uptime` for `Latency` and adds `Tables`, `Schemas`.)
4. **Findings summary**: `PGBOT   N findings need attention` where N counts
   non-suppressed `warning` + `critical` findings; `no findings` when zero.
   Then up to five finding rows, critical first: glyph (`✗` red for
   critical, `⚠` yellow for warning), title, and right-aligned
   `confidence HIGH` (≥ 0.8) / `MEDIUM` (≥ 0.5) / `LOW`. When more than five:
   `… and 3 more — 2 pgbot`. Then the `Ok` rows from
   `health::categories` as `✓ <Category>  <detail>` (green ✓, dim detail).
   `Enter` on the summary (or `2`) opens the PgBot tab on Inspect.

### PgBot tab

Sub-tab row `Inspect  Queries  Indexes  Tables  Why` (+ `Ask` while ask
output exists), current REVERSED, mouse `Hit::SetView`. Body = the existing
screens, untouched. `←`/`→` (and `h`/`l`) cycle sub-tabs, as today. The
command-bar verbs `inspect`, `queries`, … switch to this tab and its
sub-tab. `j`/`k` scroll the body as today.

### Focus and keys

`Focus::Main` gains a pane: `Pane::Sidebar | Pane::Main`. Other focus states
(`CommandBar`, `Popup`, `Help`, and the new `Palette`) are unchanged in
kind.

```
Tab / Shift+Tab   toggle focus sidebar ↔ main          [  /  ]   previous / next database
j k ↑ ↓           sidebar: move + select; main: scroll  1..n     switch tab
← → h l           PgBot sub-tabs                         Enter    sidebar: focus main; overview: open PgBot
^K  or  :         command palette                       /        command bar (verbs + ask …)
a                 add database        r  refresh        ?  help   q / ^C  quit
```

`Tab` no longer switches databases; `[`/`]` do, from anywhere in Main
focus. The README gets a "what moved" note. Mouse: sidebar rows select,
tabs and sub-tabs switch, `+ Add database` opens the popup, wheel scrolls
the main body.

### Command palette (`Focus::Palette`, `src/palette.rs`)

Centered overlay, 60 columns wide, an input line and up to 10 matches.
Items: every `UserCommand` verb (`refresh`, `inspect`, `queries`, `indexes`,
`tables`, `why`, `overview`, `pgbot`), `add database`, `help`, `quit`, and
one `switch to <name>` per database. Typing filters with a dependency-free
subsequence matcher (consecutive-run and word-start bonuses; case-
insensitive); `↑`/`↓` move, `Enter` runs, `Esc` closes. `ask <text>` typed
into the palette is passed through to the command bar's parser, so the
palette is a superset of the bar. The parser gains the verbs `overview` and
`pgbot`.

### Add-database popup and CLI

- Popup gains a `Stage` field after Name: `←`/`→`/Space cycle
  `auto · prod · staging · dev · local`; `auto` = infer from name. Tab order
  Name → Stage → Connection.
- `pgterm add <name> [--env VAR] [--stage prod|staging|dev|local] [--open]`;
  `pgterm list` shows the stage column.
- Session-only tabs (pasted URLs) infer stage from the name.

### Model additions (`model.rs`)

`Server.provider: String`, `Server.uptime_seconds: i64`,
`Activity.active: i64` — all `#[serde(default)]`; fixtures gain the fields.
`format.rs` gains `duration_short` and `bytes` if missing.

### Tests (slice 1)

- `config.rs`: stage round-trips; invalid stage is an error naming the four
  values; a pre-0.2 file loads with `stage = None`; inference table.
- `cli.rs`: `--stage` parsing and rejection.
- `app.rs` (`update`): Tab toggles pane and back; `[`/`]` cycle and wrap;
  `j`/`k` in the sidebar select; digits switch tabs and are inert in the
  popup/bar/palette; `←`/`→` cycle sub-tabs only on the PgBot tab; `Enter`
  on Overview opens PgBot/Inspect; palette open, filter, select, execute,
  esc; `switch to` item selects the database; hitmap dispatch for sidebar
  rows, tabs, sub-tabs; per-database tab survives switching.
- `palette.rs`: matcher scoring (prefix beats scattered; `sw st` → switch
  to staging; no match → empty).
- `ui.rs` (TestBackend): wide layout has the sidebar with badges and no
  shortcut row; narrow layout has the strip and no sidebar; Overview renders
  tiles from the healthy fixture with the expected values; findings summary
  shows critical first with confidence labels; unavailable status line.
- `screens/overview.rs`: pure helpers (`confidence_label`, version digits,
  finding ordering) unit-tested.
- Existing tests updated for the key changes; nothing else regresses.

### Files (slice 1)

New: `src/palette.rs`, `src/screens/overview.rs`, `src/screens/sidebar.rs`,
`src/screens/tabs.rs`. Changed: `action.rs` (Tab, Pane, Hit variants,
Palette actions), `app.rs` (pane/tab/palette state and keys), `ui.rs`
(layout), `config.rs`, `cli.rs`, `parser.rs`, `model.rs`, `format.rs`,
`screens/mod.rs`, `screens/states.rs` (welcome mentions the palette),
README, help overlay. Version 0.2.0.

## Slice 2 — direct connection, SQL, Data (v0.3.0)

Decisions taken; details go in that slice's plan.

- **Driver**: `tokio-postgres` 0.7 with `tokio-postgres-rustls` and
  `rustls-native-certs` (system roots, so RDS and friends verify). The
  URL's `sslmode` is honoured as tokio-postgres parses it: `disable`,
  `prefer` (default), `require`, `verify-ca`, `verify-full`. Linux builds
  stay static musl; rustls has no C dependency.
- **Connection lifecycle** (`db.rs`): one connection per database, opened
  lazily on first need (a tile refresh, the SQL or Data tab), dropped after
  five idle minutes, reopened transparently. Never opened for a database
  that is only being monitored by pgbot. `application_name = pgterm`.
- **Tiles**: `Latency` = round trip of `SELECT 1` measured on each monitor
  tick once a connection exists; `Tables` = count of `pg_class` relkinds
  r/p outside system schemas; `Schemas` = `pg_namespace` minus system. The
  Overview row becomes PostgreSQL · Latency · Tables · Schemas · Connections
  · Size (Uptime and Active move into the status line).
- **SQL tab**: a small built-in multi-line editor (insert, delete, newline,
  cursor keys, Home/End, word-wrap off) — `tui-textarea` still targets
  ratatui 0.29, so we own ~200 lines. `F5` / `Ctrl-Enter` runs the buffer;
  `Esc` returns focus to the shell. Execution: simple-query protocol inside
  `BEGIN READ ONLY; SET LOCAL statement_timeout = '30s'`, then the buffer,
  then `COMMIT`. Results: a grid with typed headers, `j`/`k`/`PgUp`/`PgDn`
  and horizontal scroll, rows capped at 1,000 per run with a visible
  "truncated — add LIMIT" note, execution time and row count in the footer.
  Errors show Postgres's message and position. History: the last 50
  statements, in memory only. Nothing is written to disk.
- **Writes**: profile `writes = true` (or `pgterm add --writes`) lifts
  `READ ONLY`. On a `prod`-stage profile with writes enabled, a run whose
  buffer contains a write keyword (INSERT, UPDATE, DELETE, MERGE, ALTER,
  DROP, CREATE, TRUNCATE, GRANT, REVOKE, VACUUM, COPY … FROM) first asks the
  user to type the database name. The status line shows `writes on` in
  yellow whenever it applies. Everything else stays read-only at the server,
  which is the real guarantee — the keyword scan only decides when to ask.
- **Data tab**: three-level browser — schemas → tables (with `reltuples`
  estimates and sizes) → a paged row grid (`LIMIT 100 OFFSET n`, sortable by
  column, identifiers quoted with `quote_ident` semantics), every query in
  a READ ONLY transaction with the same timeout. No editing.
- **Tests**: unit tests for URL/sslmode mapping, the write-keyword scan,
  identifier quoting, grid paging; integration tests behind
  `PGTERM_TEST_DATABASE_URL` running on Linux CI against a `postgres:17`
  service container (READ ONLY really rejects writes; timeout fires;
  results truncate at the cap; TLS `require` against the service fails
  clearly). macOS and Windows skip them.

## Slice 3 — Branches via pgrun-cli (v0.4.0)

- `pgrun.rs` runner: `$PGRUN_BIN` → `pgrun` on PATH; closed argv:
  `branch list <slug> --json`, `branch get <slug> <name> --json`,
  `branch create <slug> --name <n> --ttl <t> --wait --json`,
  `branch delete <slug> <name> --json`. DSN variables scrubbed from the
  child env; pgrun's own auth (`~/.config/pgrun/config.json`,
  `PGRUN_API_TOKEN`) is untouched.
- Profile `pgrun_project`; without it the Branches section shows
  `set pgrun_project = "<slug>" for this database`; without pgrun on PATH
  or logged in, the section shows the install/login hint.
- Sidebar `BRANCHES` section under the selected database (name, age or
  status), Branches tab (name, status, parent, Postgres version, created,
  expires), Overview `BRANCHES · N` list.
- Keys: `Enter` opens the branch as a **session-only** database tab from
  `connection_url` (memory only, exactly like a pasted URL); `n` creates
  (name + TTL prompt, `--wait`, progress shown); `d` deletes with
  type-the-name confirmation. Listing refreshes on the monitor tick while
  the tab or section is visible.

## Slice 4 — Book (v0.5.0)

`docs/book-design.md` applies with one change: the Book is a top-level tab
in the main pane rather than a `Focus::Book` mode; the two-pane reader lives
in the tab body, `Esc` in the reader returns to the list, and `b` jumps to
the tab. pgbook v0.2.0 (`--json`) ships first as that spec describes.

## Packaging and docs

- README rewritten for the shell: the wide-layout figure, tabs, keys, the
  "what moved" note, per-slice sections as they ship.
- Homebrew formula caveats list pgbot, pgrun and pgbook as companions;
  README install line names them; no same-tap `depends_on`
  (`docs/releasing.md`).
- `install.sh` bootstraps pgbot today; pgrun and pgbook bootstraps arrive
  with their slices, each with a `PGTERM_NO_<TOOL>=1` opt-out.

## Out of scope

Light/dark theme switching (terminal palette rules), window chrome, query
editing of rows, saved queries, per-database notes.
