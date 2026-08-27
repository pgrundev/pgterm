# pgterm — multi-database terminal UI for PostgreSQL (design)

Condensed from the product brief (2026-08-26), updated for the standalone
pivot: pgterm is its own application and repository, not a companion binary
inside the pgbot repo. The brief's product requirements otherwise stand.

## Product

`pgterm` opens an interactive, keyboard-first terminal UI that monitors
several PostgreSQL databases at once — one top tab per database — and lets the
user drill into pgbot's diagnostics (inspect, queries, indexes, tables, why)
without leaving the interface. "htop for all my Postgres databases, powered by
pgbot." It is NOT a web dashboard, SQL IDE, terminal emulator, or Electron app.

```
[ production ● ] [ staging ● ] [ analytics ● ] [ + Add DB ]
Connections  OK    84 / 300
Cache        OK    99.2%
Locks        FAIL  3 blocked
...
1 Inspect  2 Queries  3 Indexes  4 Tables  5 Why
production > _
```

## Architecture

pgbot (Go, installed separately — https://pgbot.dev) remains the only
diagnostic engine. pgterm (Rust: ratatui 0.30, crossterm 0.29, tokio, serde,
toml, anyhow, dirs) is a presentation/orchestration layer that shells out to
pgbot and renders its JSON contracts:

- `pgbot inspect --json` — the versioned Context document (schema 1.2.0).
  Feeds background health monitoring and the Inspect/Queries/Tables views
  (pgbot's own `queries`/`tables` commands are text renderings of the same
  document; its footer says "pgbot inspect --json for the full set").
- `pgbot indexes --json` — the graded correlation report (confidence enum
  `catalog_proven | needs_code_check | inconclusive`, do_not_drop, safety).
- `pgbot why --json` — offline causal chains from pgbot's baseline store.

pgbot is located via `$PGBOT_BIN` (override) or `pgbot` on PATH. There is no
recursion and no shell anywhere: one runner builds argv directly and passes
the DSN through the child's environment.

Verified pgbot contract facts (2026-08-26): exit 0/1/2 all carry valid JSON on
stdout (findings gate 1/2); 3 = connection/execution failure, one-line
`pgbot: <msg>` on stderr; 64 = malformed invocation. Connection resolution:
positional arg → `$DATABASE_URL` → `$PGBOT_DATABASE_URL`; env is the
sanctioned subprocess path. pgbot redacts DSNs in its own errors; pgterm
redacts again on top. Health score mirror: `100 − Σ(critical:10, warn:3,
info:1)` over non-suppressed findings, floor 0. There is no pgbot `history`
command → the History view is hidden in MVP. `pgbot ask` exists (AI; needs
OPENAI_API_KEY/GEMINI_API_KEY; `--yes` skips its confirm).

## Requirements

### Onboarding (one command per database)
- `pgterm add <name>` — detects `DATABASE_URL` (else `PGBOT_DATABASE_URL`),
  validates the connection with a real pgbot probe (schema profile, no store,
  `--fail-on=none`), persists `env = "DATABASE_URL"`. Helpful message when no
  source exists.
- `pgterm add <name> --env <ENV_NAME>` — explicit env-var reference.
- `--open` — validate, save, immediately open the TUI with that DB selected.
- Nothing is saved on any failure. No `--force`, no `--url`.
- `pgterm list` — names + env-var names + status, never values.
- `pgterm remove <name>` — local profile only; says "PostgreSQL was not
  modified."
- Unset env var: `Environment variable X is not set.` / `Nothing was saved.`

### Secrets (absolute)
- Config stores env-var *names* only. Secrets resolve in memory at spawn time
  and reach pgbot via its child environment (`DATABASE_URL=<value>`), never
  argv, never logs, never the screen. Every error passes a sanitizer that
  redacts URL/keyword-DSN passwords and the resolved secret itself.

### Config file
`$PGTERM_CONFIG` (override) → `$XDG_CONFIG_HOME/pgterm/config.toml` →
`~/.config/pgterm/config.toml`:

```toml
version = 1
[settings]
interval_seconds = 60
max_concurrent_checks = 3
[[databases]]
name = "production"
env = "PROD_DATABASE_URL"
```

Names unique; atomic 0600 writes.

### TUI
- Tabs: one per DB + `+ Add DB`. Status glyphs not color-only: `●` healthy,
  `!` warning/critical (colored), `○` unavailable, `◌` checking. Cross-tab
  monitoring updates other tabs' glyphs without stealing focus.
- Per-DB screen state persists across tab switches (view + cached results).
- Views: Inspect, Queries, Indexes, Tables, Why (History hidden — no backend).
- Shortcut row `1 Inspect 2 Queries 3 Indexes 4 Tables 5 Why` — keyboard and
  mouse. Keys: Tab/Shift+Tab switch DB, 1-5 views, `/` command bar, `r`
  refresh, `a` add DB, `?` help, `q`/Ctrl+C quit. When the command bar or a
  popup is focused, typed characters go to the input, never shortcuts.
- Command bar `production > _`: strict enum — inspect, queries, indexes,
  tables, why, refresh, ask <question…>. Never a shell; no bash/psql/SQL.
- Background monitoring: default 60s (`--interval 30s` / `--no-monitor`),
  bounded concurrency (semaphore, default 3), per-DB timeouts; a timeout
  affects only that tab; the UI never blocks (switch tabs, help, cached
  results, quit all remain available).
- Unavailable DB: `○` tab, screen shows last-successful-check age + `[r]
  Retry`; no credentials in errors.
- Add-DB popup (`a` / `+` tab / click): Name + Environment variable fields,
  Test/Add actions, Esc cancel; never displays or saves the env value; Add
  validates before saving, exactly like the CLI.
- First run (no profiles): welcome screen with add instructions, `[a]`/`[q]`.
- Terminal below 80×24: "Terminal too small" notice, no panic.
- Style: htop/lazygit restraint — minimal borders, restrained color, no
  charts/logos/gradients. Textual change arrows (`8ms → 26ms +225%`).
- Cleanup: raw mode / alternate screen / mouse capture restored on quit,
  error, and panic.

### Safety (absolute)
Diagnostic/read-only. No write SQL, no SQL console, no auto-fix, no shell,
no accounts/cloud. Whitelisted pgbot operations only. pgbot's grading renders
verbatim — "unused" never becomes "delete".

### pgbot discovery & versions
`pgterm` requires pgbot ≥ a version that provides `inspect --json` (0.4+).
When pgbot is missing: point at the installer
(`curl -fsSL https://pgbot.dev/install | sh`) or `PGBOT_BIN`. Version skew is
tolerated: unknown JSON fields are ignored; a missing/unparsable document
degrades to a sanitized error on that tab, never a crash.

### Packaging (this repo)
- `cargo build --release` → `pgterm`.
- CI: fmt + clippy + test on linux and macos.
- Release workflow: native builds for linux amd64/arm64 (musl, static) and
  darwin amd64/arm64, tarballs + checksums attached to a GitHub release.
  (Windows: not yet; crossterm supports it, revisit post-MVP.)

### Tests
- Unit: config parsing/saving/duplicates, connection-source resolution,
  command parser (rejects `rm -rf /`, `bash`, `psql`, `DROP DATABASE x`,
  `inspect; rm -rf /`, `$(whoami)`), sanitizer (a DSN never survives),
  health status/score/category mapping, pgbot arg mapping, duration parsing.
- Integration: a fake pgbot binary (deterministic JSON keyed off the DSN)
  drives the real runner and app state — DB A healthy, DB B warning, DB C
  unavailable, states independent; bounded concurrency observed; no real
  PostgreSQL anywhere.
- Acceptance: the real `pgterm` binary run as a subprocess for add/list/
  remove flows, asserting the config never contains a DSN.

### Definition of done
`export DATABASE_URL=…; pgterm add prod; pgterm` shows `[ prod ● ]` with a
live health view; 1-5 work; a second DB gives two independently monitored
tabs; no password persisted; no write SQL; no account; no web anything.
