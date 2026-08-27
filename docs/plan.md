# pgterm Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** Ship pgterm — a standalone multi-database Ratatui TUI that orchestrates the pgbot CLI for diagnostics, with one-command database onboarding and background health monitoring.

**Architecture:** Rust binary `pgterm` owns the profile config (`~/.config/pgterm/config.toml`, env-var references only), the CLI flows (`add`/`list`/`remove`), and the TUI; every diagnostic runs pgbot (`$PGBOT_BIN` or PATH) as a subprocess with the DSN passed via child env, consuming pgbot's JSON contracts (`inspect --json` Context 1.2.0, `indexes --json`, `why --json`).

**Tech Stack:** Rust 2021, ratatui 0.30 (`crossterm_0_29` feature), crossterm 0.29, tokio 1, serde/serde_json, toml, anyhow, dirs.

**Spec:** `docs/design.md`.

**Status (2026-08-26):** ALL TASKS DONE. Built as `pgbot-terminal` inside the pgbot repo (Tasks 1–7), extracted here as standalone `pgterm`, then Tasks 8–12 completed in this repo: app core + bounded monitoring, TUI shell, all five views + strict command bar, popup + mouse, CI/release workflows. 106 tests green, clippy clean.

## Global Constraints

- Never persist, log, or display a DSN, password, or env-var *value* — config stores env-var **names**; secrets go to the pgbot child via env, never argv.
- Read-only product: no write SQL, no SQL console, no shell execution anywhere; command bar parses a closed enum.
- pgbot's JSON is the single source of truth; its grading renders verbatim.
- History view hidden (no pgbot backend). No `--force`/`--url` on add.
- Terminal state always restored (raw mode, alt screen, mouse capture) on quit/error/panic.
- pgbot exit contract: 0/1/2 ⇒ stdout JSON valid; 3 ⇒ failure on stderr; 64 ⇒ pgterm bug.
- `cargo test` green, `cargo fmt --check` clean at every commit. Commit per task, conventional messages.
- Local build quirk (this machine): `cc` on PATH is not a linker — export `CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER=/usr/bin/cc` for cargo commands.

---

### Task 8: app core — state, actions, bounded monitor

Files: `src/action.rs` (already written), `src/app.rs`, `tests/monitor.rs`

**Interfaces (produces):**
```rust
// action.rs (already written): View, CmdKind, StoredResult, Action, Effect, Hit
pub struct DbState { profile, health: HealthStatus, last_ok/last_checked: Option<Instant>,
    view: View, ctx: Option<Context>, indexes: Option<IndexesReport>, why: Option<WhyReport>,
    ask_output: Option<String>, running: HashSet<CmdKind>, error: Option<SafeError>,
    attention: bool, scroll: HashMap<View, u16> }
pub struct App { dbs: Vec<DbState>, selected: usize, focus: Focus, cmdline: String,
    cmd_error: Option<String>, popup: Option<AddPopup>, should_quit: bool,
    monitor_enabled: bool, interval: Duration, pgbot_bin: PathBuf, size: (u16,u16),
    hitmap: Vec<(Rect, Hit)> }
impl App {
    pub fn new(cfg: &TerminalConfig, opts…) -> App;
    pub fn update(&mut self, a: Action) -> Vec<Effect>;   // pure state, no IO
}
pub async fn run_effect(pgbot_bin, profile_env/name, effect, sem: Arc<Semaphore>) -> Action;
```
Rules encoded in `update`: MonitorTick spawns Monitor for every db not already running it; CheckFinished(Ok(Ctx)) sets health via `health::overall`, stores ctx, clears error, stamps last_ok/last_checked, sets `attention` on non-selected dbs that turned Warning/Critical/Unavailable (cleared on select); Err ⇒ health Unavailable only for Monitor/Inspect kinds (view kinds keep health, set error); per-db state survives tab switches; Refresh/SetView dedupe against `running`; view→command mapping Inspect/Queries/Tables→InspectFull, Indexes→Indexes, Why→Why.

- [x] Unit tests: quit, tab cycling wraps, attention set/cleared, refresh-while-running no-ops, view mapping.
- [x] Integration `tests/monitor.rs`: 3 dbs (fake modes healthy/warn/refuse via distinct env vars); one sweep through real `run_pgbot` under `Semaphore(2)`; assert A Healthy / B Warning / C Unavailable (sanitized error), state independence, dedupe, peak concurrency ≤ 2 (fake writes a running-marker count with FAKE_PGBOT_DELAY).
- [x] Commit `feat: event/action core with bounded concurrent monitoring`.

### Task 9: TUI shell — tabs, health screen, states, help, resize

Files: `src/ui.rs`, `src/event.rs`, `src/screens/{mod,health,states}.rs`; rewrite `main.rs` TUI entry (async event loop: mpsc<Action>, crossterm read thread, monitor interval task, draw ≤30fps, ratatui init/restore).
Layout: tabs row (glyphs ● ! ○ ◌, selected reversed, `+ Add DB`), body, shortcut row, command bar. Health screen: `last check: 12s ago`, `DATABASE HEALTH  94 / 100`, seven category rows (status textual OK/WARN/FAIL), summary line, unmapped-findings note. States: first-run welcome, <80×24 too-small, checking-with-no-cache, UNAVAILABLE with retry. Help overlay (`?`). Hitmap recorded during draw.
- [x] Unit tests for pure pieces (glyph per status, age formatting, too-small predicate). Manual smoke with fake pgbot. Commit.

### Task 10: views + command bar + parser

Files: `src/parser.rs`, `src/screens/{inspect,queries,indexes,tables,why,ask}.rs`.
Parser (tests FIRST — security property): closed verb set inspect/queries/indexes/tables/why/refresh + `ask <text>`; rejects ``, `rm -rf /`, `bash`, `psql`, `DROP DATABASE x`, `SELECT 1`, `inspect; rm -rf /`, `$(whoami)`, `history`.
Screens render cached StoredResults: Inspect (findings grouped by severity, j/k selection, detail pane with evidence/remediation/caveats, suppressed dimmed); Queries (TOTAL/SHARE/CALLS/MEAN/QUERY from ctx.queries.top, pgss-off state); Tables (SIZE/ROWS/DEAD%/SEQ/IDX/TABLE); Indexes (from IndexesReport verbatim: confidence column, DO NOT DROP flag, note footer); Why (chains: symptom, before→after ×factor, hops, Confidence %, zero-snapshot state); Ask (scrolled text).
Keys: 1-5 SetView (+spawn when no cache), `/` focus bar (Esc/Enter/Backspace; typed chars to input), parse errors inline.
- [x] Commit `feat: diagnostic views and strict command bar`.

### Task 11: mouse + add-database popup

Files: `src/screens/addpopup.rs`; mouse capture on/off with init/restore; hitmap dispatch (tabs, + Add DB, shortcut row, popup buttons).
Popup: Name + Env fields (Tab switches, Esc cancels, typed chars to focused field), [ Test ] probes, [ Add ] probes then saves via config and appends a DbState + immediate monitor spawn; errors sanitized; duplicate names surface the config error; env values never rendered.
- [x] Unit tests: field routing, shortcuts inert while popup open, successful add appends without disturbing others. Commit.

### Task 12: repo polish — CI, release, README

- `.github/workflows/ci.yml`: fmt --check, clippy -D warnings, cargo test on ubuntu-latest + macos-latest.
- `.github/workflows/release.yml` on `v*` tags: native matrix — ubuntu-latest (x86_64-unknown-linux-musl), ubuntu-24.04-arm (aarch64-unknown-linux-musl), macos-latest (aarch64-apple-darwin + x86_64-apple-darwin via rustup target add); `cargo build --release --locked`; tarballs `pgterm_<ver>_<os>_<arch>.tar.gz` + sha256 checksums; attach to GitHub release.
- README: what/why, install (release tarball, `cargo install --path .`), quickstart (`pgterm add prod` → `pgterm`), key table, config format, pgbot requirement, security posture (env-name-only config, no write SQL), screenshot placeholder.
- [x] Commit `chore: CI, release workflow, README`.
