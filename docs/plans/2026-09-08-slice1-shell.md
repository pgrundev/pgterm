# pgterm v0.2.0 — Slice 1: the shell — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace pgterm's "one tab per database" chrome with the sidebar shell from the redesign: environment badges, per-database tabs (Overview, PgBot), stat tiles + pgbot's gauge strip + findings summary on Overview, a command palette, keymap-driven help, toasts, `--default-config`, and a Windows build — as release v0.2.0.

**Architecture:** Pure-state `App::update` stays the only mutation point; the draw pass (`ui.rs` + `screens/*`) renders from state and rebuilds the mouse hitmap. New modules: `keymap.rs` (the single key table that both dispatch and help read), `palette.rs` (items + matcher + state), `screens/{sidebar,tabs,overview}.rs`. Existing pgbot screens are untouched and become the body of the PgBot tab.

**Tech Stack:** Rust 2021, ratatui 0.30 (`crossterm_0_29`), crossterm 0.29, tokio 1, serde/serde_json, toml, anyhow, dirs. No new dependencies in this slice.

**Spec:** `docs/redesign-design.md` — "Architecture", "Config", "Slice 1 — the shell", "Packaging and docs".

## Global Constraints

- Config stores environment-variable **names**, never connection strings; nothing here changes that. A pre-0.2 `config.toml` (no `stage`, no `[ui]`) must load unchanged.
- No shell anywhere; pgbot argv stays the closed set in `runner.rs`.
- Every error shown passes `sanitize.rs`.
- Text carries meaning, colour is redundant: badges are words, gauges have status words, glyphs differ by shape.
- Minimum terminal 80×24; sidebar only at ≥ 100 columns.
- Keys after this slice: `Tab`/`Shift+Tab` toggle sidebar↔main; `[`/`]` previous/next database; `1..n` tabs; `←/→` `h/l` PgBot sub-tabs; `j/k ↑↓` move in sidebar or scroll main; `Enter` sidebar→main, Overview→PgBot; `Ctrl-K` or `:` palette; `/` command bar; `a` add; `r` refresh; `?` help; `q`/`Ctrl-C` quit. Digits are inert in the popup, bar and palette.
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --locked` green at every commit. Local build needs `CC=/usr/bin/cc CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER=/usr/bin/cc`.
- Commit per task, conventional messages, no co-author trailer.
- Gauge rules mirror pgbot PR #43 (`internal/render/gauges.go`) exactly: 20 cells, nearest-cell rounding with a one-cell minimum for non-zero, status from the grading finding, unmeasurable rows dim with `—`.

---

### Task 1: Config — `stage`, `[ui]`, `--default-config` text

**Files:**
- Modify: `src/config.rs` (DatabaseProfile, TerminalConfig, add), tests in the same file's `mod tests`

**Interfaces (produces):**
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Stage { Prod, Staging, Dev, Local }
impl Stage {
    pub const ALL: [Stage; 4];
    pub fn label(self) -> &'static str;         // "PROD" | "STAGING" | "DEV" | "LOCAL"
    pub fn parse(s: &str) -> Option<Stage>;     // "prod" | "staging" | "dev" | "local", case-insensitive
    pub fn infer(name: &str) -> Option<Stage>;  // prod → stag → local → dev, by substring, else None
}
pub struct DatabaseProfile { pub name: String, pub env: String, pub stage: Option<Stage> }
impl DatabaseProfile { pub fn badge(&self) -> Option<Stage> }  // explicit, else inferred
#[derive(Serialize, Deserialize)] #[serde(default)]
pub struct UiSettings { pub sidebar_detail: bool /* true */, pub bell: bool /* false */ }
pub struct TerminalConfig { ..., pub ui: UiSettings }
impl TerminalConfig { pub fn add_with_stage(&mut self, name: &str, env: &str, stage: Option<Stage>) -> anyhow::Result<()> }
pub const DEFAULT_CONFIG_TEXT: &str;   // annotated TOML of the defaults
```

- [ ] **Step 1: Write the failing tests** (append inside `mod tests` in `src/config.rs`)

```rust
    #[test]
    fn stage_parses_and_labels() {
        assert_eq!(Stage::parse("PROD"), Some(Stage::Prod));
        assert_eq!(Stage::parse("staging"), Some(Stage::Staging));
        assert_eq!(Stage::parse("production"), None);
        assert_eq!(Stage::Prod.label(), "PROD");
        assert_eq!(Stage::Local.label(), "LOCAL");
    }

    #[test]
    fn stage_is_inferred_from_the_name_explicit_wins() {
        assert_eq!(Stage::infer("production"), Some(Stage::Prod));
        assert_eq!(Stage::infer("eu-prod-2"), Some(Stage::Prod));
        assert_eq!(Stage::infer("staging"), Some(Stage::Staging));
        assert_eq!(Stage::infer("stage"), Some(Stage::Staging));
        assert_eq!(Stage::infer("localhost"), Some(Stage::Local));
        assert_eq!(Stage::infer("dev-box"), Some(Stage::Dev));
        assert_eq!(Stage::infer("analytics"), None);
        let p = DatabaseProfile { name: "production".into(), env: "X".into(), stage: Some(Stage::Dev) };
        assert_eq!(p.badge(), Some(Stage::Dev), "explicit stage beats the name");
        let p = DatabaseProfile { name: "production".into(), env: "X".into(), stage: None };
        assert_eq!(p.badge(), Some(Stage::Prod));
    }

    #[test]
    fn stage_and_ui_round_trip_and_old_files_still_load() {
        let mut cfg = TerminalConfig::default();
        cfg.add_with_stage("prod", "PROD_URL", Some(Stage::Prod)).unwrap();
        cfg.add("analytics", "AN_URL").unwrap();
        cfg.ui.bell = true;
        let text = toml::to_string_pretty(&cfg).unwrap();
        assert!(text.contains("stage = \"prod\""), "{text}");
        assert!(!text.contains("stage = \"\""), "absent stage must not serialize: {text}");
        let back: TerminalConfig = toml::from_str(&text).unwrap();
        assert_eq!(back, cfg);
        assert!(back.ui.bell && back.ui.sidebar_detail);

        let old = "version = 1\n[[databases]]\nname = \"p\"\nenv = \"P_URL\"\n";
        let cfg: TerminalConfig = toml::from_str(old).unwrap();
        assert_eq!(cfg.databases[0].stage, None);
        assert!(cfg.ui.sidebar_detail && !cfg.ui.bell, "ui defaults apply to old files");
    }

    #[test]
    fn unknown_stage_is_an_error_naming_the_four() {
        let bad = "version = 1\n[[databases]]\nname = \"p\"\nenv = \"P_URL\"\nstage = \"production\"\n";
        let err = toml::from_str::<TerminalConfig>(bad).unwrap_err().to_string();
        for s in ["prod", "staging", "dev", "local"] {
            assert!(err.contains(s), "error should name {s}: {err}");
        }
    }

    #[test]
    fn default_config_text_parses_to_the_defaults() {
        let cfg: TerminalConfig = toml::from_str(DEFAULT_CONFIG_TEXT).unwrap();
        assert_eq!(cfg.settings, Settings::default());
        assert_eq!(cfg.ui, UiSettings::default());
        assert!(cfg.databases.is_empty());
        assert!(DEFAULT_CONFIG_TEXT.contains("# stage"), "the text is annotated");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib config::tests`
Expected: compile errors — `Stage`, `stage`, `ui`, `add_with_stage`, `DEFAULT_CONFIG_TEXT` undefined.

- [ ] **Step 3: Implement** (in `src/config.rs`)

Add after the `Settings` impl:

```rust
/// Which environment a database belongs to. Drives the sidebar badge and,
/// in later slices, the extra confirmation before writes on PROD.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    Prod,
    Staging,
    Dev,
    Local,
}

impl Stage {
    pub const ALL: [Stage; 4] = [Stage::Prod, Stage::Staging, Stage::Dev, Stage::Local];

    pub fn label(self) -> &'static str {
        match self {
            Stage::Prod => "PROD",
            Stage::Staging => "STAGING",
            Stage::Dev => "DEV",
            Stage::Local => "LOCAL",
        }
    }

    pub fn parse(s: &str) -> Option<Stage> {
        match s.trim().to_ascii_lowercase().as_str() {
            "prod" => Some(Stage::Prod),
            "staging" => Some(Stage::Staging),
            "dev" => Some(Stage::Dev),
            "local" => Some(Stage::Local),
            _ => None,
        }
    }

    /// Best guess from a name, in a fixed order so "prod" beats "dev" in
    /// "dev-prod-mirror". Explicit config always wins over this.
    pub fn infer(name: &str) -> Option<Stage> {
        let n = name.to_ascii_lowercase();
        if n.contains("prod") {
            Some(Stage::Prod)
        } else if n.contains("stag") {
            Some(Stage::Staging)
        } else if n.contains("local") {
            Some(Stage::Local)
        } else if n.contains("dev") {
            Some(Stage::Dev)
        } else {
            None
        }
    }
}

/// Presentation switches. Nothing here affects what pgterm does to a database.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct UiSettings {
    /// Two rows per database in the sidebar: the second is a dim detail line.
    pub sidebar_detail: bool,
    /// Ring the terminal bell with a toast when an unselected database turns
    /// critical or unavailable.
    pub bell: bool,
}

impl Default for UiSettings {
    fn default() -> Self {
        UiSettings { sidebar_detail: true, bell: false }
    }
}

/// The annotated defaults, printed by `pgterm --default-config`.
pub const DEFAULT_CONFIG_TEXT: &str = "\
# pgterm configuration — ~/.config/pgterm/config.toml
# Stores environment-variable NAMES, never connection strings.
version = 1

[settings]
# Seconds between background health checks of every database.
interval_seconds = 60
# How many pgbot checks may run at once.
max_concurrent_checks = 3

[ui]
# Two rows per database in the sidebar (the second is a dim detail line).
sidebar_detail = true
# Terminal bell when a database you are not looking at turns critical.
bell = false

# One block per database:
# [[databases]]
# name = \"production\"
# env = \"PROD_DATABASE_URL\"   # the variable holding the connection string
# stage = \"prod\"              # prod | staging | dev | local — badge; inferred from the name when absent
";
```

Change `DatabaseProfile` and `TerminalConfig`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DatabaseProfile {
    pub name: String,
    pub env: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<Stage>,
}

impl DatabaseProfile {
    /// The badge to show: the configured stage, else one inferred from the name.
    pub fn badge(&self) -> Option<Stage> {
        self.stage.or_else(|| Stage::infer(&self.name))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TerminalConfig {
    pub version: u32,
    pub settings: Settings,
    pub ui: UiSettings,
    #[serde(rename = "databases")]
    pub databases: Vec<DatabaseProfile>,
}
```

Update `Default for TerminalConfig` to set `ui: UiSettings::default()`. Change `add` to delegate:

```rust
    pub fn add(&mut self, name: &str, env: &str) -> anyhow::Result<()> {
        self.add_with_stage(name, env, None)
    }

    pub fn add_with_stage(&mut self, name: &str, env: &str, stage: Option<Stage>) -> anyhow::Result<()> {
        // (the existing validation body, unchanged)
        self.databases.push(DatabaseProfile { name: name.to_string(), env: env.to_string(), stage });
        Ok(())
    }
```

Every other place that builds a `DatabaseProfile` literally (`src/app.rs` `DbState::session`, tests) gains `stage: None`.

- [ ] **Step 4: Run the tests**

Run: `cargo test --lib config::tests` then `cargo test --locked` (whole suite: the profile literal in `app.rs` must compile).
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/config.rs src/app.rs
git commit -m "feat(config): stage badges (explicit or inferred), [ui] settings, annotated default text"
```

---

### Task 2: CLI — `--stage`, `--default-config`, stage column in `list`

**Files:**
- Modify: `src/cli.rs` (AddOptions, Invocation, parse_args, cmd_add, cmd_list, USAGE), `src/main.rs`

**Interfaces:**
- Consumes: `Stage`, `TerminalConfig::add_with_stage`, `DEFAULT_CONFIG_TEXT` (Task 1).
- Produces: `AddOptions { name, env, open, stage: Option<Stage> }`, `Invocation::DefaultConfig`.

- [ ] **Step 1: Write the failing tests** (in `src/cli.rs` `mod tests`)

```rust
    #[test]
    fn add_accepts_a_stage_and_rejects_unknown_ones() {
        match parse_args(&s(&["add", "prod", "--env", "P", "--stage", "prod"])) {
            Invocation::Add(o) => assert_eq!(o.stage, Some(crate::config::Stage::Prod)),
            other => panic!("{other:?}"),
        }
        match parse_args(&s(&["add", "prod", "--stage", "production"])) {
            Invocation::Usage(msg) => assert!(msg.contains("prod, staging, dev, local"), "{msg}"),
            other => panic!("{other:?}"),
        }
        assert!(matches!(parse_args(&s(&["add", "p", "--stage"])), Invocation::Usage(_)));
    }

    #[test]
    fn default_config_flag_is_its_own_invocation() {
        assert_eq!(parse_args(&s(&["--default-config"])), Invocation::DefaultConfig);
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --lib cli::tests`
Expected: compile errors on `stage` / `DefaultConfig`.

- [ ] **Step 3: Implement**

`AddOptions` gains `pub stage: Option<Stage>`; `Invocation` gains `DefaultConfig`. In `parse_args`'s `add` loop add:

```rust
                    "--stage" => match it.next().and_then(|v| Stage::parse(v)) {
                        Some(st) => stage = Some(st),
                        None => {
                            return Invocation::Usage(
                                "--stage must be one of prod, staging, dev, local".into(),
                            )
                        }
                    },
```

(`let mut stage = None;` above the loop; `AddOptions { name, env, open, stage }` at the end.) In the TUI arm add `"--default-config" => return Invocation::DefaultConfig,`. `USAGE` becomes:

```
usage: pgterm [--interval <dur>] [--no-monitor]
       pgterm add <name> [--env <ENV_NAME>] [--stage prod|staging|dev|local] [--open]
       pgterm list
       pgterm remove <name>
       pgterm --default-config
```

In `cmd_add`, replace `cfg.add(&opts.name, &env_name)` with `cfg.add_with_stage(&opts.name, &env_name, opts.stage)`. In `cmd_list`, add a `STAGE` column between NAME and CONNECTION, value `d.badge().map(|s| s.label()).unwrap_or("—")`, width from the longest label. In `main.rs` add the arm:

```rust
        Invocation::DefaultConfig => {
            print!("{}", pgterm::config::DEFAULT_CONFIG_TEXT);
            0
        }
```

Also `tests/cli_add.rs`: add one acceptance test that `pgterm add prod --env P_URL --stage staging` writes `stage = "staging"` into the config file (copy the shape of the existing `add` tests there; assert the file text contains `stage = "staging"` and still contains no `://`).

- [ ] **Step 4: Run the tests**

Run: `cargo test --locked`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/cli.rs src/main.rs tests/cli_add.rs
git commit -m "feat(cli): --stage on add, STAGE column in list, --default-config"
```

---

### Task 3: Model fields and format helpers the Overview needs

**Files:**
- Modify: `src/model.rs`, `src/format.rs`, `tests/fixtures/context_healthy.json`, `tests/fixtures/context_warn.json`, `tests/fixtures/context_critical.json`

**Interfaces (produces):**
```rust
// model.rs
pub struct Server { ..., pub provider: String, pub uptime_seconds: i64 }
pub struct Activity { pub total: i64, pub active: i64 }
pub struct Health { ..., pub rollback_ratio: Option<f64> }
pub struct Window { pub window_age_seconds: Option<i64> }  impl Window { pub fn cold(&self) -> bool }  // < 900
pub struct Context { ..., pub window: Option<Window> }
// format.rs
pub fn duration_short(secs: i64) -> String;   // "12d" | "3h" | "41m" | "9s"
pub fn pct(v: f64) -> String;                 // 0.992 → "99.2%"
```

- [ ] **Step 1: Failing tests** (`src/model.rs` `mod tests` and `src/format.rs` `mod tests`)

```rust
    // model.rs
    #[test]
    fn overview_fields_decode_with_defaults() {
        let c = Context::decode(
            r#"{"server":{"provider":"rds","uptime_seconds":1036800},"activity":{"total":24,"active":6},
                "health":{"rollback_ratio":0.12},"window":{"window_age_seconds":120}}"#,
        )
        .unwrap();
        assert_eq!(c.server.provider, "rds");
        assert_eq!(c.server.uptime_seconds, 1_036_800);
        assert_eq!(c.activity.unwrap().active, 6);
        assert_eq!(c.health.unwrap().rollback_ratio, Some(0.12));
        assert!(c.window.unwrap().cold());
        let c = Context::decode(r#"{"window":{"window_age_seconds":86400}}"#).unwrap();
        assert!(!c.window.unwrap().cold());
        let c = Context::decode("{}").unwrap();
        assert!(c.window.is_none() && c.server.provider.is_empty());
    }

    // format.rs
    #[test]
    fn duration_short_picks_the_largest_unit() {
        assert_eq!(duration_short(9), "9s");
        assert_eq!(duration_short(2460), "41m");
        assert_eq!(duration_short(3 * 3600 + 5), "3h");
        assert_eq!(duration_short(12 * 86400 + 3600), "12d");
        assert_eq!(duration_short(-5), "0s");
    }

    #[test]
    fn pct_has_one_decimal() {
        assert_eq!(pct(0.992), "99.2%");
        assert_eq!(pct(0.0), "0.0%");
        assert_eq!(pct(1.0), "100.0%");
    }
```

- [ ] **Step 2: Run to verify failure**: `cargo test --lib model::tests::overview_fields format::tests` → compile errors.

- [ ] **Step 3: Implement**

`model.rs`: add the fields (all structs already carry `#[serde(default)]`), plus:

```rust
/// The stats window pgbot's counters cover. A window younger than 15 minutes
/// is "cold": counter-based signals (unused indexes) are not trustworthy yet.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Window {
    pub window_age_seconds: Option<i64>,
}

impl Window {
    pub const COLD_THRESHOLD_SECONDS: i64 = 900;
    pub fn cold(&self) -> bool {
        matches!(self.window_age_seconds, Some(s) if s < Self::COLD_THRESHOLD_SECONDS)
    }
}
```

`format.rs`:

```rust
/// One unit, no decimals: how long something has been the case.
pub fn duration_short(secs: i64) -> String {
    let s = secs.max(0);
    if s >= 86_400 {
        format!("{}d", s / 86_400)
    } else if s >= 3_600 {
        format!("{}h", s / 3_600)
    } else if s >= 60 {
        format!("{}m", s / 60)
    } else {
        format!("{s}s")
    }
}

pub fn pct(v: f64) -> String {
    format!("{:.1}%", v * 100.0)
}
```

Fixtures: add to each context fixture `"provider"` (`""` healthy, `"rds"` warn, `""` critical), `"uptime_seconds"` (1036800), `"active"` inside `activity`, `"rollback_ratio"` inside `health` (0.01 healthy, 0.12 warn, 0.3 critical) and a top-level `"window": {"window_age_seconds": 21600}`; add `"rollback_ratio": 0.12` and a `high_rollback_ratio` warning finding to `context_warn.json` (id `high_rollback_ratio`, severity `warning`, title `rollbacks 12% of transactions`, confidence 0.7) — the gauge tests in Task 8 rely on it.

- [ ] **Step 4: Run** `cargo test --locked` → PASS (existing fixture assertions still hold: warn has 3 findings now — update `context_fixtures_decode` if it asserts `w.findings.len() == 2` → 3).

- [ ] **Step 5: Commit**

```bash
git add src/model.rs src/format.rs tests/fixtures
git commit -m "feat(model): provider, uptime, active sessions, rollback ratio, stats window; duration_short/pct"
```

---

### Task 4: Keymap — one table for dispatch and help

**Files:**
- Create: `src/keymap.rs`
- Modify: `src/lib.rs` (add `pub mod keymap;`), `src/action.rs` (add `Tab`, `Pane`)

**Interfaces (produces):**
```rust
// action.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tab { Overview, PgBot }
impl Tab { pub const NUMBERED: [(char, Tab, &'static str); 2] = [('1', Tab::Overview, "Overview"), ('2', Tab::PgBot, "PgBot")]; }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane { Sidebar, Main }
// keymap.rs
pub enum KeyContext { Global, Main, Sidebar, PgBot, Overview }
pub enum KeyAction { Quit, Help, TogglePane, PrevDb, NextDb, Palette, CommandBar, AddDb, Refresh,
                     SetTab(Tab), PrevView, NextView, Up, Down, Enter }
pub struct Binding { pub keys: &'static [&'static str], pub context: KeyContext, pub action: KeyAction, pub help: &'static str }
pub static KEYMAP: &[Binding];
pub fn key_name(key: &KeyEvent) -> Option<String>;        // "q", "S-Tab", "C-k", "Left", "1" …
pub fn lookup(contexts: &[KeyContext], key: &KeyEvent) -> Option<KeyAction>;  // first context wins
pub fn help_text() -> String;                               // grouped, "key  padded  help"
```

- [ ] **Step 1: Failing tests** (`src/keymap.rs` `mod tests`)

```rust
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn k(code: KeyCode) -> KeyEvent { KeyEvent::new(code, KeyModifiers::NONE) }

    #[test]
    fn every_binding_has_help_and_no_context_binds_a_key_twice() {
        let mut seen = std::collections::HashSet::new();
        for b in KEYMAP {
            assert!(!b.help.is_empty(), "{:?} has no help", b.keys);
            for key in b.keys {
                assert!(seen.insert((b.context, *key)), "{key} bound twice in {:?}", b.context);
            }
        }
    }

    #[test]
    fn every_tab_has_a_number_binding() {
        for (ch, tab, _) in Tab::NUMBERED {
            let hit = lookup(&[KeyContext::Main], &k(KeyCode::Char(ch)));
            assert_eq!(hit, Some(KeyAction::SetTab(tab)));
        }
    }

    #[test]
    fn key_names_cover_modifiers_and_specials() {
        assert_eq!(key_name(&k(KeyCode::Char('q'))).as_deref(), Some("q"));
        assert_eq!(key_name(&k(KeyCode::BackTab)).as_deref(), Some("S-Tab"));
        assert_eq!(key_name(&KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL)).as_deref(), Some("C-k"));
        assert_eq!(key_name(&k(KeyCode::Left)).as_deref(), Some("Left"));
        assert_eq!(key_name(&k(KeyCode::Enter)).as_deref(), Some("Enter"));
    }

    #[test]
    fn context_order_decides_and_global_is_the_fallback() {
        // j in the sidebar moves; in the main pane it scrolls; both are Down.
        assert_eq!(lookup(&[KeyContext::Sidebar, KeyContext::Main], &k(KeyCode::Char('j'))), Some(KeyAction::Down));
        // Left only means "previous view" on the PgBot tab.
        assert_eq!(lookup(&[KeyContext::PgBot, KeyContext::Main], &k(KeyCode::Left)), Some(KeyAction::PrevView));
        assert_eq!(lookup(&[KeyContext::Overview, KeyContext::Main], &k(KeyCode::Left)), None);
        assert_eq!(lookup(&[KeyContext::Main], &k(KeyCode::Char('q'))), Some(KeyAction::Quit));
        assert_eq!(lookup(&[KeyContext::Main], &k(KeyCode::Char('['))), Some(KeyAction::PrevDb));
        assert_eq!(lookup(&[KeyContext::Main], &KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL)), Some(KeyAction::Palette));
    }

    #[test]
    fn help_text_lists_every_key() {
        let text = help_text();
        for b in KEYMAP {
            for key in b.keys {
                assert!(text.contains(key), "help is missing {key}:\n{text}");
            }
            assert!(text.contains(b.help), "help is missing {:?}", b.help);
        }
    }
```

- [ ] **Step 2: Run** `cargo test --lib keymap` → compile errors (module missing).

- [ ] **Step 3: Implement**

`action.rs` — add next to `View`:

```rust
/// Top-level tabs of the main pane, one current per database.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tab {
    Overview,
    PgBot,
}

impl Tab {
    pub const NUMBERED: [(char, Tab, &'static str); 2] =
        [('1', Tab::Overview, "Overview"), ('2', Tab::PgBot, "PgBot")];
}

/// Which pane keyboard focus lives in while `Focus::Main`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Sidebar,
    Main,
}
```

`keymap.rs`:

```rust
//! The key table. `App::update` dispatches through `lookup`, and the `?`
//! overlay is rendered from the same table by `help_text`, so the help can
//! never describe a binding that does not exist.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::action::Tab;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyContext {
    Global,
    Main,
    Sidebar,
    PgBot,
    Overview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    Quit,
    Help,
    TogglePane,
    PrevDb,
    NextDb,
    Palette,
    CommandBar,
    AddDb,
    Refresh,
    SetTab(Tab),
    PrevView,
    NextView,
    Up,
    Down,
    Enter,
}

pub struct Binding {
    pub keys: &'static [&'static str],
    pub context: KeyContext,
    pub action: KeyAction,
    pub help: &'static str,
}

const fn b(keys: &'static [&'static str], context: KeyContext, action: KeyAction, help: &'static str) -> Binding {
    Binding { keys, context, action, help }
}

pub static KEYMAP: &[Binding] = &[
    b(&["q"], KeyContext::Global, KeyAction::Quit, "quit"),
    b(&["?"], KeyContext::Global, KeyAction::Help, "help"),
    b(&["Tab", "S-Tab"], KeyContext::Main, KeyAction::TogglePane, "focus sidebar / main pane"),
    b(&["["], KeyContext::Main, KeyAction::PrevDb, "previous database"),
    b(&["]"], KeyContext::Main, KeyAction::NextDb, "next database"),
    b(&["1"], KeyContext::Main, KeyAction::SetTab(Tab::Overview), "overview tab"),
    b(&["2"], KeyContext::Main, KeyAction::SetTab(Tab::PgBot), "pgbot tab"),
    b(&["C-k", ":"], KeyContext::Main, KeyAction::Palette, "command palette"),
    b(&["/"], KeyContext::Main, KeyAction::CommandBar, "command bar (verbs, ask …)"),
    b(&["a"], KeyContext::Main, KeyAction::AddDb, "add database"),
    b(&["r"], KeyContext::Main, KeyAction::Refresh, "refresh"),
    b(&["j", "Down"], KeyContext::Main, KeyAction::Down, "scroll down / move down"),
    b(&["k", "Up"], KeyContext::Main, KeyAction::Up, "scroll up / move up"),
    b(&["Enter"], KeyContext::Sidebar, KeyAction::Enter, "open the selected database"),
    b(&["Left", "h"], KeyContext::PgBot, KeyAction::PrevView, "previous pgbot view"),
    b(&["Right", "l"], KeyContext::PgBot, KeyAction::NextView, "next pgbot view"),
    b(&["Enter"], KeyContext::Overview, KeyAction::Enter, "open pgbot findings"),
];

/// The canonical name of a key event, in the spelling KEYMAP uses.
pub fn key_name(key: &KeyEvent) -> Option<String> {
    let base = match key.code {
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Tab => "Tab".into(),
        KeyCode::BackTab => return Some("S-Tab".into()),
        KeyCode::Enter => "Enter".into(),
        KeyCode::Esc => "Esc".into(),
        KeyCode::Left => "Left".into(),
        KeyCode::Right => "Right".into(),
        KeyCode::Up => "Up".into(),
        KeyCode::Down => "Down".into(),
        KeyCode::PageUp => "PgUp".into(),
        KeyCode::PageDown => "PgDn".into(),
        KeyCode::Backspace => "Backspace".into(),
        _ => return None,
    };
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        Some(format!("C-{base}"))
    } else {
        Some(base)
    }
}

/// Resolve a key against the given contexts in order, then Global.
pub fn lookup(contexts: &[KeyContext], key: &KeyEvent) -> Option<KeyAction> {
    let name = key_name(key)?;
    let order = contexts.iter().copied().chain(std::iter::once(KeyContext::Global));
    for ctx in order {
        if let Some(b) = KEYMAP.iter().find(|b| b.context == ctx && b.keys.contains(&name.as_str())) {
            return Some(b.action);
        }
    }
    None
}

/// The `?` overlay body, grouped by context.
pub fn help_text() -> String {
    let groups = [
        (KeyContext::Main, "NAVIGATION"),
        (KeyContext::Sidebar, "SIDEBAR"),
        (KeyContext::Overview, "OVERVIEW"),
        (KeyContext::PgBot, "PGBOT TAB"),
        (KeyContext::Global, "GENERAL"),
    ];
    let mut out = String::from("pgterm\n");
    for (ctx, title) in groups {
        out.push('\n');
        out.push_str(title);
        out.push('\n');
        for b in KEYMAP.iter().filter(|b| b.context == ctx) {
            out.push_str(&format!("{:<18} {}\n", b.keys.join(" / "), b.help));
        }
    }
    out
}
```

Add `pub mod keymap;` to `lib.rs`.

- [ ] **Step 4: Run** `cargo test --lib keymap` → PASS; `cargo clippy --all-targets -- -D warnings` clean.

- [ ] **Step 5: Commit**

```bash
git add src/keymap.rs src/action.rs src/lib.rs
git commit -m "feat: keymap table — one source for key dispatch and the help overlay"
```

---

### Task 5: App state — panes, tabs, rollups, toasts, new key dispatch

**Files:**
- Modify: `src/app.rs` (Focus, DbState, App, handle_main_key, select_db, on_check_finished), `src/action.rs` (Hit variants)

**Interfaces (produces):**
```rust
// action.rs
pub enum Hit { SelectDb(usize), OpenAdd, SetView(View), SetTab(Tab), OpenPalette, PaletteItem(usize), PopupTest, PopupAdd, PopupCancel }
// app.rs
pub enum Focus { Main, CommandBar, Popup, Help, Palette }
pub struct Toast { pub text: String, pub until: Instant }
pub struct DbState { ..., pub tab: Tab, pub findings_now: Option<Vec<String>>, pub findings_seen: Option<Vec<String>> }
impl DbState { pub fn pgbot_changed(&self) -> bool; pub fn mark_pgbot_seen(&mut self) }
pub struct App { ..., pub pane: Pane, pub ui: UiSettings, pub toast: Option<Toast>, pub bell_pending: bool }
impl App {
    pub fn set_tab(&mut self, tab: Tab) -> Vec<Effect>;
    pub fn toggle_pane(&mut self);
    pub fn active_toast(&self) -> Option<&Toast>;   // None once expired
    pub fn take_bell(&mut self) -> bool;
    pub fn run_user_command(&mut self, cmd: UserCommand) -> Vec<Effect>;  // shared by bar + palette
}
```

- [ ] **Step 1: Failing tests** (`src/app.rs` `mod tests`; `key(code)` and `app(n)` helpers exist there)

```rust
    fn ctrl(c: char) -> Action {
        Action::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
    }

    #[test]
    fn tab_toggles_pane_and_brackets_switch_databases() {
        let mut a = app(3);
        assert_eq!(a.pane, Pane::Main);
        a.update(key(KeyCode::Tab));
        assert_eq!(a.pane, Pane::Sidebar);
        a.update(key(KeyCode::BackTab));
        assert_eq!(a.pane, Pane::Main);
        a.update(key(KeyCode::Char(']')));
        assert_eq!(a.selected, 1);
        a.update(key(KeyCode::Char('[')));
        a.update(key(KeyCode::Char('[')));
        assert_eq!(a.selected, 2, "wraps");
    }

    #[test]
    fn sidebar_jk_select_and_enter_returns_to_main() {
        let mut a = app(3);
        a.update(key(KeyCode::Tab));
        a.update(key(KeyCode::Char('j')));
        assert_eq!(a.selected, 1);
        a.update(key(KeyCode::Down));
        assert_eq!(a.selected, 2);
        a.update(key(KeyCode::Char('j')));
        assert_eq!(a.selected, 2, "no wrap in the sidebar list");
        a.update(key(KeyCode::Char('k')));
        assert_eq!(a.selected, 1);
        a.update(key(KeyCode::Enter));
        assert_eq!(a.pane, Pane::Main);
    }

    #[test]
    fn digits_switch_tabs_per_database_and_are_inert_in_inputs() {
        let mut a = app(2);
        assert_eq!(a.dbs[0].tab, Tab::Overview);
        a.update(key(KeyCode::Char('2')));
        assert_eq!(a.dbs[0].tab, Tab::PgBot);
        a.update(key(KeyCode::Char(']')));
        assert_eq!(a.dbs[1].tab, Tab::Overview, "tabs are per database");
        a.update(key(KeyCode::Char('[')));
        assert_eq!(a.dbs[0].tab, Tab::PgBot, "and survive switching");
        a.update(key(KeyCode::Char('/')));
        a.update(key(KeyCode::Char('1')));
        assert_eq!(a.cmdline, "1");
        assert_eq!(a.dbs[0].tab, Tab::PgBot);
    }

    #[test]
    fn arrows_cycle_pgbot_views_only_on_the_pgbot_tab() {
        let mut a = app(1);
        a.update(key(KeyCode::Right));
        assert_eq!(a.dbs[0].view, View::Inspect, "overview ignores arrows");
        a.update(key(KeyCode::Char('2')));
        a.update(key(KeyCode::Right));
        assert_eq!(a.dbs[0].view, View::Queries);
        a.update(key(KeyCode::Char('h')));
        assert_eq!(a.dbs[0].view, View::Inspect);
    }

    #[test]
    fn enter_on_overview_opens_pgbot_inspect() {
        let mut a = app(1);
        let effects = a.update(key(KeyCode::Enter));
        assert_eq!(a.dbs[0].tab, Tab::PgBot);
        assert_eq!(a.dbs[0].view, View::Inspect);
        assert_eq!(effects.len(), 1, "no cache yet → one inspect spawn");
    }

    #[test]
    fn pgbot_tab_is_marked_until_viewed_when_findings_change() {
        let mut a = app(2);
        a.update(Action::CheckFinished { db: 1, kind: CmdKind::Monitor, result: ok_ctx(HEALTHY) });
        assert!(!a.dbs[1].pgbot_changed(), "first result is not 'changed'");
        a.update(key(KeyCode::Char(']')));
        a.update(key(KeyCode::Char('2')));
        assert!(!a.dbs[1].pgbot_changed());
        a.update(key(KeyCode::Char('[')));
        a.update(Action::CheckFinished { db: 1, kind: CmdKind::Monitor, result: ok_ctx(WARN) });
        assert!(a.dbs[1].pgbot_changed(), "new findings while not viewing");
        a.update(key(KeyCode::Char(']')));
        assert!(a.dbs[1].pgbot_changed(), "selecting the database is not viewing the tab");
        a.update(key(KeyCode::Char('2')));
        assert!(!a.dbs[1].pgbot_changed());
    }

    #[test]
    fn toast_fires_for_an_unselected_database_turning_critical_not_for_recovery_or_self() {
        let mut a = app(2);
        a.ui.bell = true;
        a.update(Action::CheckFinished { db: 1, kind: CmdKind::Monitor, result: ok_ctx(HEALTHY) });
        assert!(a.active_toast().is_none());
        a.update(Action::CheckFinished { db: 1, kind: CmdKind::Monitor, result: ok_ctx(CRITICAL) });
        let t = a.active_toast().expect("toast");
        assert!(t.text.contains("critical") && t.text.contains("[ to open"), "{}", t.text);
        assert!(a.take_bell());
        assert!(!a.take_bell(), "bell is consumed");
        a.update(Action::CheckFinished { db: 1, kind: CmdKind::Monitor, result: ok_ctx(HEALTHY) });
        a.toast = None;
        a.update(Action::CheckFinished { db: 0, kind: CmdKind::Monitor, result: ok_ctx(CRITICAL) });
        assert!(a.active_toast().is_none(), "the selected database never toasts");
        a.update(Action::CheckFinished { db: 1, kind: CmdKind::Monitor, result: err() });
        assert!(a.active_toast().unwrap().text.contains("unavailable"));
        a.toast.as_mut().unwrap().until = Instant::now() - Duration::from_secs(1);
        assert!(a.active_toast().is_none(), "expired");
    }
```

(`CRITICAL` fixture constant: add `const CRITICAL: &str = include_str!("../tests/fixtures/context_critical.json");` next to `HEALTHY`/`WARN` in the test module.) Also update the existing tests that press `Tab` to switch databases (`tab_cycles_and_wraps_and_state_survives`, `check_finished_sets_health_and_flags_unselected_tabs`, `number_keys_map_to_views_and_fetch_only_when_empty`, `arrow_keys_cycle_the_numbered_views_and_wrap`): use `]`/`[` for databases, press `2` first before view arrows, and use `Enter`/`set_view` for views instead of digits.

- [ ] **Step 2: Run** `cargo test --lib app::tests` → compile errors.

- [ ] **Step 3: Implement** (in `src/app.rs`)

Imports: add `use crate::action::{Pane, Tab};`, `use crate::config::UiSettings;`, `use crate::keymap::{self, KeyAction, KeyContext};`, `use crate::parser::UserCommand;`.

`Focus` gains `Palette`. Add:

```rust
/// A one-line notice about a database you are not looking at.
#[derive(Debug, Clone)]
pub struct Toast {
    pub text: String,
    pub until: Instant,
}

pub const TOAST_SECONDS: u64 = 5;
```

`DbState` gains `pub tab: Tab`, `pub findings_now: Option<Vec<String>>`, `pub findings_seen: Option<Vec<String>>` (init `Tab::Overview`, `None`, `None`) and:

```rust
    /// The PgBot tab label is marked while the finding set differs from the
    /// one last viewed there. Never marked before the first view.
    pub fn pgbot_changed(&self) -> bool {
        matches!((&self.findings_seen, &self.findings_now), (Some(s), Some(n)) if s != n)
    }

    pub fn mark_pgbot_seen(&mut self) {
        self.findings_seen = self.findings_now.clone();
    }
```

`App` gains `pub pane: Pane` (Main), `pub ui: UiSettings` (`cfg.ui.clone()`), `pub toast: Option<Toast>` (None), `pub bell_pending: bool` (false), plus:

```rust
    pub fn active_toast(&self) -> Option<&Toast> {
        self.toast.as_ref().filter(|t| Instant::now() < t.until)
    }

    pub fn take_bell(&mut self) -> bool {
        std::mem::take(&mut self.bell_pending)
    }

    pub fn toggle_pane(&mut self) {
        self.pane = match self.pane {
            Pane::Sidebar => Pane::Main,
            Pane::Main => Pane::Sidebar,
        };
    }

    /// Switch the selected database's tab. Landing on PgBot counts as viewing
    /// its findings. Never spawns by itself; the PgBot body fetches on demand
    /// through `set_view` exactly as before.
    pub fn set_tab(&mut self, tab: Tab) -> Vec<Effect> {
        let selected = self.selected;
        let Some(db) = self.dbs.get_mut(selected) else {
            return Vec::new();
        };
        db.tab = tab;
        if tab == Tab::PgBot {
            db.mark_pgbot_seen();
            let view = db.view;
            return self.set_view(view);
        }
        Vec::new()
    }

    fn key_contexts(&self) -> Vec<KeyContext> {
        let mut v = Vec::with_capacity(3);
        if self.pane == Pane::Sidebar {
            v.push(KeyContext::Sidebar);
        } else if let Some(db) = self.dbs.get(self.selected) {
            v.push(match db.tab {
                Tab::Overview => KeyContext::Overview,
                Tab::PgBot => KeyContext::PgBot,
            });
        }
        v.push(KeyContext::Main);
        v
    }
```

Replace `handle_main_key` with dispatch through the keymap:

```rust
    fn handle_main_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let contexts = self.key_contexts();
        let Some(action) = keymap::lookup(&contexts, &key) else {
            return Vec::new();
        };
        match action {
            KeyAction::Quit => {
                self.should_quit = true;
                Vec::new()
            }
            KeyAction::Help => {
                self.focus = Focus::Help;
                Vec::new()
            }
            KeyAction::TogglePane => {
                self.toggle_pane();
                Vec::new()
            }
            KeyAction::PrevDb => {
                self.select_db(self.next_db(-1));
                Vec::new()
            }
            KeyAction::NextDb => {
                self.select_db(self.next_db(1));
                Vec::new()
            }
            KeyAction::Palette => self.open_palette(),
            KeyAction::CommandBar => {
                self.focus = Focus::CommandBar;
                self.cmd_error = None;
                Vec::new()
            }
            KeyAction::AddDb => {
                self.popup = Some(AddPopup::default());
                self.focus = Focus::Popup;
                Vec::new()
            }
            KeyAction::Refresh => self.refresh_selected(),
            KeyAction::SetTab(tab) => self.set_tab(tab),
            KeyAction::PrevView => self.cycle_view(-1),
            KeyAction::NextView => self.cycle_view(1),
            KeyAction::Up => {
                if self.pane == Pane::Sidebar {
                    if self.selected > 0 {
                        self.select_db(self.selected - 1);
                    }
                } else {
                    self.scroll_by(-1);
                }
                Vec::new()
            }
            KeyAction::Down => {
                if self.pane == Pane::Sidebar {
                    if self.selected + 1 < self.dbs.len() {
                        self.select_db(self.selected + 1);
                    }
                } else {
                    self.scroll_by(1);
                }
                Vec::new()
            }
            KeyAction::Enter => {
                if self.pane == Pane::Sidebar {
                    self.pane = Pane::Main;
                    Vec::new()
                } else {
                    // Overview: open the findings.
                    if let Some(db) = self.dbs.get_mut(self.selected) {
                        db.view = View::Inspect;
                    }
                    self.set_tab(Tab::PgBot)
                }
            }
        }
    }
```

`open_palette` is a stub for Task 6 (`fn open_palette(&mut self) -> Vec<Effect> { Vec::new() }`); Task 6 fills it. Factor the command-bar `match cmd { … }` in `submit_command` into `pub fn run_user_command(&mut self, cmd: UserCommand) -> Vec<Effect>` (same arms; `Inspect|Queries|Indexes|Tables|Why` now do `self.dbs[selected].tab = Tab::PgBot; mark seen;` then `set_view`) and call it from `submit_command`.

In `on_check_finished`, the `Ok(StoredResult::Ctx(ctx))` arm gains, before storing:

```rust
                let prev = state.health;
                let mut ids: Vec<String> = ctx
                    .findings
                    .iter()
                    .filter(|f| !f.suppressed)
                    .map(|f| f.id.clone())
                    .collect();
                ids.sort();
                state.findings_now = Some(ids);
                if state.findings_seen.is_none() || (db == selected && state.tab == Tab::PgBot) {
                    state.mark_pgbot_seen();
                }
```

and after `state.health = health::overall(&ctx);`:

```rust
                if db != selected && state.health == HealthStatus::Critical && prev != HealthStatus::Critical {
                    self.notify(&format!("{} is critical · [ to open", state.profile.name));
                }
```

(`notify` needs `&mut self` while `state` borrows `self.dbs` — take `let name = state.profile.name.clone(); let became_critical = …;` inside the borrow, drop it, then call `self.notify(...)`.) The `Err(e)` arm: when `kind` is Monitor/Inspect and `db != selected` and the previous health was not `Unavailable`, `self.notify(&format!("{name} is unavailable · [ to open"))`. Add:

```rust
    fn notify(&mut self, text: &str) {
        self.toast = Some(Toast { text: text.to_string(), until: Instant::now() + Duration::from_secs(TOAST_SECONDS) });
        if self.ui.bell {
            self.bell_pending = true;
        }
    }
```

`select_db` unchanged. `handle_mouse` gains `Some(Hit::SetTab(t)) if self.focus == Focus::Main => self.set_tab(t)` (palette hits come in Task 6). `action.rs` `Hit` gains `SetTab(Tab)`, `OpenPalette`, `PaletteItem(usize)`.

`main.rs` event loop, after `perform(...)`: `if app.take_bell() { let _ = std::io::Write::write_all(&mut std::io::stdout(), b"\x07"); }`.

- [ ] **Step 4: Run** `cargo test --locked` → PASS (after updating the older key tests as noted). `cargo clippy --all-targets -- -D warnings` clean.

- [ ] **Step 5: Commit**

```bash
git add src/app.rs src/action.rs src/main.rs
git commit -m "feat(app): sidebar/main panes, per-database tabs, keymap dispatch, PgBot rollup, toasts + bell"
```

---

### Task 6: Command palette

**Files:**
- Create: `src/palette.rs`
- Modify: `src/lib.rs`, `src/app.rs` (Focus::Palette handling, `open_palette`, mouse hits), `src/parser.rs` (verbs `overview`, `pgbot`)

**Interfaces (produces):**
```rust
// parser.rs
pub enum UserCommand { Inspect, Queries, Indexes, Tables, Why, Refresh, Overview, Pgbot, Ask(String) }
// palette.rs
#[derive(Debug, Clone, PartialEq)]
pub enum PaletteCmd { Verb(UserCommand), Tab(Tab), SwitchDb(usize), AddDb, Help, Quit }
pub struct PaletteItem { pub label: String, pub cmd: PaletteCmd }
pub fn items(db_names: &[&str]) -> Vec<PaletteItem>;
pub fn score(query: &str, label: &str) -> Option<i32>;
pub fn filter(items: &[PaletteItem], query: &str) -> Vec<usize>;   // indices, best first, stable
#[derive(Debug, Clone, Default)]
pub struct PaletteState { pub input: String, pub cursor: usize }
```

- [ ] **Step 1: Failing tests** (`src/palette.rs` `mod tests`, `src/parser.rs` tests, `src/app.rs` tests)

```rust
    // palette.rs
    #[test]
    fn scoring_prefers_prefix_then_word_starts_then_scattered() {
        let prefix = score("ref", "refresh").unwrap();
        let word = score("st", "switch to staging").unwrap();
        let scattered = score("sts", "switch to staging").unwrap();
        assert!(prefix > scattered);
        assert!(word > 0);
        assert!(score("sw st", "switch to staging").is_some());
        assert!(score("sw st", "switch to production").is_none());
        assert!(score("zzz", "refresh").is_none());
        assert_eq!(score("", "anything"), Some(0));
    }

    #[test]
    fn filter_orders_by_score_and_keeps_all_on_empty_query() {
        let items = items(&["production", "staging"]);
        assert_eq!(filter(&items, "").len(), items.len());
        let hits = filter(&items, "sw st");
        assert_eq!(items[hits[0]].label, "switch to staging");
        assert_eq!(hits.len(), 1);
        let hits = filter(&items, "tab");
        assert!(hits.iter().all(|i| items[*i].label.contains("tab")));
    }

    #[test]
    fn items_cover_every_verb_tab_and_database() {
        let items = items(&["a", "b"]);
        for want in ["refresh", "inspect", "queries", "indexes", "tables", "why", "overview tab", "pgbot tab", "add database", "help", "quit", "switch to a", "switch to b"] {
            assert!(items.iter().any(|i| i.label == want), "missing {want}");
        }
    }

    // parser.rs (add to the_whitelist_parses)
        assert_eq!(parse("overview"), Ok(UserCommand::Overview));
        assert_eq!(parse("PGBOT"), Ok(UserCommand::Pgbot));

    // app.rs
    #[test]
    fn palette_opens_filters_runs_and_closes() {
        let mut a = app(2);
        a.update(ctrl('k'));
        assert_eq!(a.focus, Focus::Palette);
        for c in "sw st".chars() {
            a.update(key(KeyCode::Char(c)));
        }
        a.update(key(KeyCode::Enter));
        assert_eq!(a.focus, Focus::Main);
        assert_eq!(a.selected, 1, "switch to staging ran");
        a.update(key(KeyCode::Char(':')));
        a.update(key(KeyCode::Esc));
        assert_eq!(a.focus, Focus::Main);
        // Free text that parses as a verb runs even with no item selected.
        a.update(key(KeyCode::Char(':')));
        for c in "ask why is it slow".chars() {
            a.update(key(KeyCode::Char(c)));
        }
        let effects = a.update(key(KeyCode::Enter));
        assert_eq!(effects.len(), 1);
        assert_eq!(a.dbs[1].view, View::Ask);
        a.update(key(KeyCode::Char(':')));
        for c in "nonsense".chars() {
            a.update(key(KeyCode::Char(c)));
        }
        a.update(key(KeyCode::Enter));
        assert_eq!(a.focus, Focus::Palette, "unknown input stays open with an error");
        assert!(a.cmd_error.is_some());
    }
```

(`app(2)` names databases `db0`, `db1`? Check the helper: it builds names via `format!("db{i}")` — adjust the "sw st" test to type `sw db1` and assert `selected == 1`, or add a helper `named_app(&["production","staging"])` in the test module.)

- [ ] **Step 2: Run** `cargo test --lib palette parser app::tests::palette` → failures/compile errors.

- [ ] **Step 3: Implement**

`parser.rs`: add `Overview`, `Pgbot` variants, arms `"overview" => bare(UserCommand::Overview)`, `"pgbot" => bare(UserCommand::Pgbot)`, and extend `KNOWN`.

`palette.rs`:

```rust
//! The command palette: every action pgterm can take, filtered by a small
//! subsequence matcher. Items are built from the same verbs the command bar
//! parses, so the palette can never do something the bar cannot.

use crate::action::Tab;
use crate::parser::UserCommand;

#[derive(Debug, Clone, PartialEq)]
pub enum PaletteCmd {
    Verb(UserCommand),
    Tab(Tab),
    SwitchDb(usize),
    AddDb,
    Help,
    Quit,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PaletteItem {
    pub label: String,
    pub cmd: PaletteCmd,
}

#[derive(Debug, Clone, Default)]
pub struct PaletteState {
    pub input: String,
    pub cursor: usize,
}

pub fn items(db_names: &[&str]) -> Vec<PaletteItem> {
    let mut v = vec![
        PaletteItem { label: "refresh".into(), cmd: PaletteCmd::Verb(UserCommand::Refresh) },
        PaletteItem { label: "overview tab".into(), cmd: PaletteCmd::Tab(Tab::Overview) },
        PaletteItem { label: "pgbot tab".into(), cmd: PaletteCmd::Tab(Tab::PgBot) },
        PaletteItem { label: "inspect".into(), cmd: PaletteCmd::Verb(UserCommand::Inspect) },
        PaletteItem { label: "queries".into(), cmd: PaletteCmd::Verb(UserCommand::Queries) },
        PaletteItem { label: "indexes".into(), cmd: PaletteCmd::Verb(UserCommand::Indexes) },
        PaletteItem { label: "tables".into(), cmd: PaletteCmd::Verb(UserCommand::Tables) },
        PaletteItem { label: "why".into(), cmd: PaletteCmd::Verb(UserCommand::Why) },
        PaletteItem { label: "add database".into(), cmd: PaletteCmd::AddDb },
        PaletteItem { label: "help".into(), cmd: PaletteCmd::Help },
        PaletteItem { label: "quit".into(), cmd: PaletteCmd::Quit },
    ];
    for (i, name) in db_names.iter().enumerate() {
        v.push(PaletteItem { label: format!("switch to {name}"), cmd: PaletteCmd::SwitchDb(i) });
    }
    v
}

/// Case-insensitive subsequence match. Higher is better: 3 per matched
/// character, +2 when it continues a run, +3 when it starts a word. `None`
/// when the query is not a subsequence; `Some(0)` for an empty query.
pub fn score(query: &str, label: &str) -> Option<i32> {
    let q: Vec<char> = query.to_lowercase().chars().collect();
    let l: Vec<char> = label.to_lowercase().chars().collect();
    if q.is_empty() {
        return Some(0);
    }
    let mut total = 0;
    let mut li = 0usize;
    let mut last: Option<usize> = None;
    for qc in q {
        let found = (li..l.len()).find(|&i| l[i] == qc)?;
        total += 3;
        if last == Some(found.wrapping_sub(1)) {
            total += 2;
        }
        if found == 0 || l[found - 1] == ' ' {
            total += 3;
        }
        last = Some(found);
        li = found + 1;
    }
    Some(total)
}

pub fn filter(items: &[PaletteItem], query: &str) -> Vec<usize> {
    let mut scored: Vec<(usize, i32)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, it)| score(query, &it.label).map(|s| (i, s)))
        .collect();
    scored.sort_by(|a, b| b.1.cmp(&a.1));
    scored.into_iter().map(|(i, _)| i).collect()
}
```

`app.rs`: `pub palette: Option<PaletteState>` on `App` (None). Fill in:

```rust
    fn open_palette(&mut self) -> Vec<Effect> {
        self.palette = Some(PaletteState::default());
        self.cmd_error = None;
        self.focus = Focus::Palette;
        Vec::new()
    }

    fn palette_items(&self) -> Vec<PaletteItem> {
        let names: Vec<&str> = self.dbs.iter().map(|d| d.profile.name.as_str()).collect();
        palette::items(&names)
    }

    fn handle_palette_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let Some(state) = self.palette.as_mut() else {
            self.focus = Focus::Main;
            return Vec::new();
        };
        match key.code {
            KeyCode::Esc => {
                self.palette = None;
                self.cmd_error = None;
                self.focus = Focus::Main;
                Vec::new()
            }
            KeyCode::Backspace => {
                state.input.pop();
                state.cursor = 0;
                Vec::new()
            }
            KeyCode::Up => {
                state.cursor = state.cursor.saturating_sub(1);
                Vec::new()
            }
            KeyCode::Down => {
                state.cursor += 1;
                Vec::new()
            }
            KeyCode::Enter => self.palette_submit(),
            KeyCode::Char(c) => {
                state.input.push(c);
                state.cursor = 0;
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn palette_submit(&mut self) -> Vec<Effect> {
        let Some(state) = self.palette.clone() else {
            return Vec::new();
        };
        let items = self.palette_items();
        let hits = palette::filter(&items, &state.input);
        let chosen = hits.get(state.cursor.min(hits.len().saturating_sub(1))).map(|i| items[*i].cmd.clone());
        let cmd = match chosen {
            Some(cmd) if !state.input.trim().is_empty() || state.cursor < hits.len() => Some(cmd),
            _ => None,
        };
        let cmd = match cmd {
            Some(c) => c,
            None => match crate::parser::parse(&state.input) {
                Ok(verb) => PaletteCmd::Verb(verb),
                Err(msg) => {
                    self.cmd_error = Some(msg);
                    return Vec::new();
                }
            },
        };
        self.palette = None;
        self.cmd_error = None;
        self.focus = Focus::Main;
        self.run_palette_cmd(cmd)
    }

    fn run_palette_cmd(&mut self, cmd: PaletteCmd) -> Vec<Effect> {
        match cmd {
            PaletteCmd::Verb(v) => self.run_user_command(v),
            PaletteCmd::Tab(t) => self.set_tab(t),
            PaletteCmd::SwitchDb(i) => {
                self.select_db(i);
                Vec::new()
            }
            PaletteCmd::AddDb => {
                self.popup = Some(AddPopup::default());
                self.focus = Focus::Popup;
                Vec::new()
            }
            PaletteCmd::Help => {
                self.focus = Focus::Help;
                Vec::new()
            }
            PaletteCmd::Quit => {
                self.should_quit = true;
                Vec::new()
            }
        }
    }
```

Rule for `ask …` in the palette: when the typed text starts with `ask ` no item matches (no item label contains it) and the parser path runs it — the `Some(cmd) if …` guard above must not pick a scattered match for "ask why is it slow"; make `palette_submit` prefer the parser whenever `state.input` starts with `"ask "`. `handle_key` routes `Focus::Palette => self.handle_palette_key(key)`. `run_user_command` handles `Overview => self.set_tab(Tab::Overview)` and `Pgbot => self.set_tab(Tab::PgBot)`. Mouse: `Hit::OpenPalette => self.open_palette()`, `Hit::PaletteItem(i)` → set cursor to `i` and submit. `lib.rs`: `pub mod palette;`.

- [ ] **Step 4: Run** `cargo test --locked` → PASS; clippy clean.

- [ ] **Step 5: Commit**

```bash
git add src/palette.rs src/parser.rs src/app.rs src/lib.rs
git commit -m "feat: command palette (Ctrl-K / :) over every verb, tab and database; overview/pgbot verbs"
```

---

### Task 7: Overview helpers and the gauge strip (pure, mirrors pgbot)

**Files:**
- Create: `src/screens/overview.rs` (helpers + tests in this task; drawing in Task 8)
- Modify: `src/screens/mod.rs` (`pub mod overview;`)

**Interfaces (produces):**
```rust
pub fn confidence_label(c: f64) -> &'static str;             // HIGH ≥0.8, MEDIUM ≥0.5, LOW
pub fn version_digits(text: &str) -> Option<String>;          // "PostgreSQL 17.4 on …" → "17.4"
pub fn provider_label(p: &str) -> Option<String>;             // "rds" → "RDS", "" → None
pub fn attention_findings(ctx: &Context) -> Vec<&Finding>;   // non-suppressed critical first, then warning
pub fn tiles(ctx: &Context) -> Vec<(&'static str, String)>;  // label, value; "—" when absent
#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum GaugeKind { Ok, Watch, Bad, Info }
#[derive(Debug, Clone, PartialEq)]
pub struct Gauge { pub label: &'static str, pub share: f64, pub value: String, pub status: String, pub kind: GaugeKind, pub measurable: bool }
pub const GAUGE_WIDTH: usize = 20;
pub fn gauge_cells(share: f64) -> usize;
pub fn gauge_bar(share: f64) -> String;
pub fn cache_hit_gauge(ctx: &Context) -> Gauge;
pub fn lock_wait_gauge(ctx: &Context) -> Gauge;
pub fn rollbacks_gauge(ctx: &Context) -> Gauge;
pub fn idle_index_gauge(ctx: &Context) -> Gauge;
pub fn gauges(ctx: &Context) -> [Gauge; 4];
```

- [ ] **Step 1: Failing tests** (`src/screens/overview.rs` `mod tests`; fixtures via `include_str!("../../tests/fixtures/…")`)

```rust
    #[test]
    fn confidence_buckets() {
        assert_eq!(confidence_label(0.95), "HIGH");
        assert_eq!(confidence_label(0.8), "HIGH");
        assert_eq!(confidence_label(0.5), "MEDIUM");
        assert_eq!(confidence_label(0.49), "LOW");
    }

    #[test]
    fn version_and_provider_labels() {
        assert_eq!(version_digits("PostgreSQL 17.4 on x86_64-pc-linux-gnu").as_deref(), Some("17.4"));
        assert_eq!(version_digits("PostgreSQL 16beta1").as_deref(), Some("16"));
        assert_eq!(version_digits("weird"), None);
        assert_eq!(provider_label("rds").as_deref(), Some("RDS"));
        assert_eq!(provider_label(""), None);
    }

    #[test]
    fn attention_findings_are_critical_first_and_skip_suppressed() {
        let ctx = Context::decode(CRITICAL).unwrap();
        let f = attention_findings(&ctx);
        assert!(!f.is_empty());
        assert!(f.windows(2).all(|w| severity_rank(&w[0].severity) <= severity_rank(&w[1].severity)));
        assert!(f.iter().all(|x| !x.suppressed && x.severity != "info"));
    }

    #[test]
    fn tiles_come_from_the_context_or_show_a_dash() {
        let ctx = Context::decode(HEALTHY).unwrap();
        let t = tiles(&ctx);
        let get = |k: &str| t.iter().find(|(l, _)| *l == k).map(|(_, v)| v.clone()).unwrap();
        assert_eq!(get("Connections"), "84 / 300");
        assert!(get("Size").ends_with("GB") || get("Size").ends_with("MB"), "{}", get("Size"));
        assert_eq!(get("Uptime"), "12d");
        let empty = Context::decode("{}").unwrap();
        assert!(tiles(&empty).iter().all(|(_, v)| v == "—"));
    }

    #[test]
    fn gauge_cells_round_to_nearest_with_a_one_cell_minimum() {
        for (share, cells) in [(0.0, 0), (0.001, 1), (0.024, 1), (0.026, 1), (0.074, 1), (0.076, 2), (0.5, 10), (0.974, 19), (0.976, 20), (1.0, 20), (1.4, 20)] {
            assert_eq!(gauge_cells(share), cells, "share {share}");
        }
        assert_eq!(gauge_bar(0.5).chars().filter(|c| *c == '█').count(), 10);
        assert_eq!(gauge_bar(0.3).chars().count(), GAUGE_WIDTH);
    }

    #[test]
    fn gauges_follow_pgbots_rules() {
        let healthy = Context::decode(HEALTHY).unwrap();
        let g = cache_hit_gauge(&healthy);
        assert!(g.measurable && g.status == "ok" && g.kind == GaugeKind::Ok && g.value.ends_with('%'));
        let warn = Context::decode(WARN).unwrap();
        let r = rollbacks_gauge(&warn);
        assert_eq!((r.value.as_str(), r.status.as_str(), r.kind), ("12.0%", "watch", GaugeKind::Watch));
        let ix = idle_index_gauge(&warn);
        assert_eq!(ix.status, "review");
        assert!(ix.share > 0.0 && ix.value.contains('B'));
        let crit = Context::decode(CRITICAL).unwrap();
        let lw = lock_wait_gauge(&crit);
        assert_eq!(lw.value, "—", "pgterm runs pgbot without wait sampling");
        assert_eq!(lw.status, "3 blocked");
        assert_eq!(lw.kind, GaugeKind::Bad);
        let empty = Context::decode("{}").unwrap();
        for g in gauges(&empty) {
            assert!(!g.measurable && g.value == "—", "{g:?}");
        }
        let cold = Context::decode(r#"{"indexes":{"unused":[]},"window":{"window_age_seconds":10}}"#).unwrap();
        assert_eq!(idle_index_gauge(&cold).status, "window < 15m");
        let thin = Context::decode(r#"{"health":{"cache_hit_ratio":0.5,"cache_blocks_sampled":10}}"#).unwrap();
        assert_eq!(cache_hit_gauge(&thin).status, "thin sample");
    }
```

(`severity_rank` is a private helper in the module: critical 0, warning 1, else 2.) Confirm fixture values before relying on them: healthy limits 84/300 and uptime 1036800 (Task 3), warn has `high_rollback_ratio` + `unused_indexes` findings and a non-empty `indexes.unused`, critical has `locks.blocked_count == 3`. Adjust fixture numbers in Task 3 if they differ.

- [ ] **Step 2: Run** `cargo test --lib screens::overview` → compile errors.

- [ ] **Step 3: Implement** (`src/screens/overview.rs`, helpers half)

```rust
//! The Overview tab: status line, stat tiles, pgbot's gauge strip, and the
//! findings summary. This half is pure — everything derives from the cached
//! Context and is unit-tested without a terminal. Gauge rules mirror pgbot's
//! own default view (pgbot internal/render/gauges.go) so both surfaces agree.

use crate::format;
use crate::model::{Context, Finding, CACHE_HIT_MIN_BLOCKS};

pub fn confidence_label(c: f64) -> &'static str {
    if c >= 0.8 {
        "HIGH"
    } else if c >= 0.5 {
        "MEDIUM"
    } else {
        "LOW"
    }
}

/// "17.4" out of "PostgreSQL 17.4 on x86_64…"; the leading digits/dots run.
pub fn version_digits(text: &str) -> Option<String> {
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let run: String = text[start..].chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    let run = run.trim_end_matches('.').to_string();
    if run.is_empty() { None } else { Some(run) }
}

pub fn provider_label(p: &str) -> Option<String> {
    let p = p.trim();
    if p.is_empty() || p == "unknown" { None } else { Some(p.to_ascii_uppercase()) }
}

fn severity_rank(s: &str) -> u8 {
    match s {
        "critical" => 0,
        "warning" => 1,
        _ => 2,
    }
}

pub fn attention_findings(ctx: &Context) -> Vec<&Finding> {
    let mut v: Vec<&Finding> = ctx
        .findings
        .iter()
        .filter(|f| !f.suppressed && matches!(f.severity.as_str(), "critical" | "warning"))
        .collect();
    v.sort_by_key(|f| severity_rank(&f.severity));
    v
}

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
    let active = ctx.activity.as_ref().map(|a| a.active.to_string()).unwrap_or_else(dash);
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
    Gauge { label, share: 0.0, value: "—".into(), status: why.into(), kind: GaugeKind::Info, measurable: false }
}

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

pub fn cache_hit_gauge(ctx: &Context) -> Gauge {
    let Some(h) = ctx.health.as_ref() else { return not_measurable("cache hit", "not measurable") };
    let Some(ratio) = h.cache_hit_ratio else { return not_measurable("cache hit", "not measurable") };
    if !matches!(h.cache_blocks, Some(b) if b >= CACHE_HIT_MIN_BLOCKS) {
        return not_measurable("cache hit", "thin sample");
    }
    let mut g = Gauge { label: "cache hit", share: ratio, value: format::pct(ratio), status: "ok".into(), kind: GaugeKind::Ok, measurable: true };
    if fired(ctx, "low_cache_hit") {
        g.status = "low".into();
        g.kind = GaugeKind::Bad;
    }
    g
}

/// pgterm runs pgbot with wait sampling off, so there is never a Lock share:
/// the value is "—" and the status comes from blocked sessions alone —
/// exactly what pgbot renders without a wait profile.
pub fn lock_wait_gauge(ctx: &Context) -> Gauge {
    let Some(locks) = ctx.locks.as_ref() else { return not_measurable("lock wait", "not measurable") };
    let mut g = Gauge { label: "lock wait", share: 0.0, value: "—".into(), status: "ok".into(), kind: GaugeKind::Ok, measurable: true };
    if locks.blocked_count > 0 {
        g.status = format!("{} blocked", locks.blocked_count);
        g.kind = GaugeKind::Bad;
    }
    g
}

pub fn rollbacks_gauge(ctx: &Context) -> Gauge {
    let ratio = match ctx.health.as_ref().and_then(|h| h.rollback_ratio) {
        Some(r) => r,
        None => return not_measurable("rollbacks", "not measurable"),
    };
    let mut g = Gauge { label: "rollbacks", share: ratio, value: format::pct(ratio), status: "ok".into(), kind: GaugeKind::Ok, measurable: true };
    if fired(ctx, "high_rollback_ratio") {
        g.status = "watch".into();
        g.kind = GaugeKind::Watch;
    }
    g
}

pub fn idle_index_gauge(ctx: &Context) -> Gauge {
    if ctx.window.as_ref().map(|w| w.cold()).unwrap_or(false) {
        return not_measurable("idle idx", "window < 15m");
    }
    let Some(ix) = ctx.indexes.as_ref() else { return not_measurable("idle idx", "not measurable") };
    let idle: i64 = ix.unused.iter().filter(|i| i.scans == 0).map(|i| i.bytes).sum();
    let share = match ctx.tables.as_ref().filter(|t| t.db_size_bytes > 0) {
        Some(t) => idle as f64 / t.db_size_bytes as f64,
        None => 0.0,
    };
    let mut g = Gauge { label: "idle idx", share, value: format::human_bytes(idle), status: "ok".into(), kind: GaugeKind::Ok, measurable: true };
    if fired(ctx, "unused_indexes") {
        g.status = "review".into();
        g.kind = GaugeKind::Watch;
    }
    g
}

pub fn gauges(ctx: &Context) -> [Gauge; 4] {
    [cache_hit_gauge(ctx), lock_wait_gauge(ctx), rollbacks_gauge(ctx), idle_index_gauge(ctx)]
}
```

(`format::human_bytes(0)` must return `"0 B"`; check and adjust if it returns something else. `CACHE_HIT_MIN_BLOCKS` is already `pub` in `model.rs`.)

- [ ] **Step 4: Run** `cargo test --lib screens::overview` → PASS.

- [ ] **Step 5: Commit**

```bash
git add src/screens/overview.rs src/screens/mod.rs
git commit -m "feat(overview): pure helpers — tiles, confidence buckets, and pgbot's four gauges"
```

---

### Task 8: Draw the shell — sidebar, tab row, Overview, PgBot tab, palette, toast, help

**Files:**
- Create: `src/screens/sidebar.rs`, `src/screens/tabs.rs`
- Modify: `src/screens/overview.rs` (add `draw`), `src/screens/mod.rs`, `src/ui.rs` (layout, help from keymap, palette, toast; delete `HELP` const and the old `draw_shortcuts`)

**Interfaces (produces):**
```rust
// sidebar.rs
pub const WIDTH: u16 = 26;
pub fn badge_style(stage: Stage) -> Style;
pub fn detail_line(db: &DbState) -> String;          // "PostgreSQL 17 · 12s ago" | top finding title | error | "checking…"
pub fn draw(f: &mut Frame, area: Rect, app: &App) -> Vec<(Rect, Hit)>;
// tabs.rs
pub fn draw_tab_row(f: &mut Frame, area: Rect, app: &App) -> Vec<(Rect, Hit)>;    // "1 Overview  2 PgBot" + right status
pub fn draw_subtabs(f: &mut Frame, area: Rect, db: &DbState) -> Vec<(Rect, Hit)>;  // Inspect … Why (+Ask)
// overview.rs
pub fn draw(f: &mut Frame, area: Rect, db: &DbState);
// ui.rs
pub fn is_wide(width: u16) -> bool;   // >= 100
pub fn draw(f: &mut Frame, app: &mut App);
```

- [ ] **Step 1: Failing render tests** (`src/ui.rs` `mod tests`; `render`, `app_with`, `feed` helpers exist)

```rust
    #[test]
    fn wide_layout_has_sidebar_badges_tabs_and_no_shortcut_row() {
        let mut app = app_with(&["production", "staging"]);
        feed(&mut app, 0, HEALTHY);
        let s = render(&mut app, 120, 36);
        assert!(s.contains("DATABASES"), "{s}");
        assert!(s.contains("PROD") && s.contains("STAGING"), "{s}");
        assert!(s.contains("1 Overview") && s.contains("2 PgBot"), "{s}");
        assert!(s.contains("+ Add database"), "{s}");
        assert!(!s.contains("1 Inspect"), "the old shortcut row is gone: {s}");
        assert!(s.contains("PostgreSQL 17 ·"), "sidebar detail line: {s}");
    }

    #[test]
    fn narrow_layout_uses_the_strip_instead_of_the_sidebar() {
        let mut app = app_with(&["production", "staging"]);
        feed(&mut app, 0, HEALTHY);
        let s = render(&mut app, 90, 30);
        assert!(!s.contains("DATABASES"), "{s}");
        assert!(s.contains("production") && s.contains("+ Add DB"), "{s}");
        assert!(s.contains("1 Overview"), "{s}");
    }

    #[test]
    fn overview_renders_status_tiles_gauges_and_findings() {
        let mut app = app_with(&["production"]);
        feed(&mut app, 0, WARN);
        let s = render(&mut app, 120, 40);
        assert!(s.contains("● Connected · PostgreSQL 17"), "{s}");
        assert!(s.contains("Connections") && s.contains("84 / 300"), "{s}");
        assert!(s.contains("cache hit  [") && s.contains("rollbacks  [") && s.contains("watch"), "{s}");
        assert!(s.contains("findings need attention"), "{s}");
        assert!(s.contains("confidence"), "{s}");
        assert!(s.contains("✓ "), "healthy categories listed: {s}");
    }

    #[test]
    fn unavailable_overview_offers_retry() {
        let mut app = app_with(&["production"]);
        app.update(Action::CheckFinished {
            db: 0,
            kind: CmdKind::Monitor,
            result: Err(crate::sanitize::SafeError::new(crate::sanitize::ErrorKind::ConnectionFailed, "connection refused", None)),
        });
        let s = render(&mut app, 120, 30);
        assert!(s.contains("○ Unavailable") && s.contains("r retry"), "{s}");
    }

    #[test]
    fn pgbot_tab_shows_subtabs_and_the_existing_screens() {
        let mut app = app_with(&["production"]);
        feed(&mut app, 0, HEALTHY);
        app.update(Action::Key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE)));
        let s = render(&mut app, 120, 36);
        assert!(s.contains("Inspect") && s.contains("Queries") && s.contains("Why"), "{s}");
        assert!(s.contains("DATABASE HEALTH"), "{s}");
    }

    #[test]
    fn palette_toast_and_help_render() {
        let mut app = app_with(&["production", "staging"]);
        app.update(Action::Key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE)));
        let s = render(&mut app, 120, 36);
        assert!(s.contains("switch to staging") && s.contains("refresh"), "{s}");
        app.update(Action::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        app.toast = Some(crate::app::Toast { text: "staging is critical · [ to open".into(), until: std::time::Instant::now() + std::time::Duration::from_secs(5) });
        let s = render(&mut app, 120, 36);
        assert!(s.contains("staging is critical"), "{s}");
        app.update(Action::Key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE)));
        let s = render(&mut app, 120, 40);
        assert!(s.contains("previous database") && s.contains("command palette"), "{s}");
    }
```

Update the existing `ui.rs` render tests that assert on the old chrome (`1 Inspect`, tab strip at 100 columns) to the new layout.

- [ ] **Step 2: Run** `cargo test --lib ui::tests` → failures.

- [ ] **Step 3: Implement**

`screens/sidebar.rs`:

```rust
//! The database sidebar (wide layouts): one or two rows per database, a
//! stage badge, the add row. Returns the hit regions it painted.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::action::{Hit, Pane};
use crate::app::{App, DbState};
use crate::config::Stage;
use crate::format;
use crate::health::HealthStatus;
use crate::screens::overview::attention_findings;
use crate::ui::tab_glyph;

pub const WIDTH: u16 = 26;

pub fn badge_style(stage: Stage) -> Style {
    let color = match stage {
        Stage::Prod => Color::Yellow,
        Stage::Staging => Color::Cyan,
        Stage::Dev => Color::Green,
        Stage::Local => Color::DarkGray,
    };
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}

/// The dim second line: enough to decide whether to look at this database.
pub fn detail_line(db: &DbState) -> String {
    match (&db.ctx, db.health) {
        (_, HealthStatus::Unavailable) => db.error.as_ref().map(|e| e.message.clone()).unwrap_or_else(|| "unavailable".into()),
        (None, _) => "checking…".into(),
        (Some(ctx), HealthStatus::Warning | HealthStatus::Critical) => attention_findings(ctx)
            .first()
            .map(|f| f.title.clone())
            .unwrap_or_else(|| "needs attention".into()),
        (Some(ctx), _) => {
            let ago = db.last_checked.map(|t| format::ago(t.elapsed())).unwrap_or_else(|| "—".into());
            format!("{} · {ago}", ctx.server.short_version())
        }
    }
}

pub fn draw(f: &mut Frame, area: Rect, app: &App) -> Vec<(Rect, Hit)> {
    let dim = Style::default().fg(Color::DarkGray);
    let mut lines: Vec<Line> = vec![Line::from(Span::styled(" DATABASES", dim)), Line::from("")];
    let mut hits = Vec::new();
    let inner_w = area.width.saturating_sub(1) as usize; // border column on the right
    for (i, db) in app.dbs.iter().enumerate() {
        let y = area.y + lines.len() as u16;
        let (glyph, tone) = tab_glyph(db);
        let cursor = if app.pane == Pane::Sidebar && i == app.selected { "▸" } else { " " };
        let badge = db.profile.badge().map(|s| s.label()).unwrap_or("");
        let name_w = inner_w.saturating_sub(4 + badge.len() + 1);
        let name: String = db.profile.name.chars().take(name_w).collect();
        let pad = name_w.saturating_sub(name.chars().count());
        let mut style = Style::default();
        if i == app.selected {
            style = style.add_modifier(Modifier::REVERSED);
        }
        if db.attention {
            style = style.add_modifier(Modifier::BOLD);
        }
        let mut spans = vec![
            Span::styled(format!(" {cursor}"), style),
            Span::styled(format!("{glyph} "), style.fg(tone)),
            Span::styled(format!("{name}{}", " ".repeat(pad)), style),
        ];
        if let Some(stage) = db.profile.badge() {
            spans.push(Span::styled(format!(" {}", stage.label()), badge_style(stage).patch(style)));
        }
        lines.push(Line::from(spans));
        let rows = if app.ui.sidebar_detail { 2 } else { 1 };
        hits.push((Rect::new(area.x, y, area.width, rows), Hit::SelectDb(i)));
        if app.ui.sidebar_detail {
            let detail: String = detail_line(db).chars().take(inner_w.saturating_sub(5)).collect();
            lines.push(Line::from(Span::styled(format!("     {detail}"), dim)));
        }
    }
    let y = area.y + lines.len() as u16;
    lines.push(Line::from(Span::styled(" + Add database", dim)));
    hits.push((Rect::new(area.x, y, area.width, 1), Hit::OpenAdd));
    f.render_widget(Paragraph::new(lines), area);
    hits
}
```

`screens/tabs.rs`:

```rust
//! The main pane's tab row (numbered top-level tabs + server/freshness on the
//! right) and the PgBot sub-tab row.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::action::{Hit, Tab, View};
use crate::app::{App, DbState};
use crate::format;
use crate::ui::tab_glyph;

pub fn draw_tab_row(f: &mut Frame, area: Rect, app: &App) -> Vec<(Rect, Hit)> {
    let Some(db) = app.dbs.get(app.selected) else {
        return Vec::new();
    };
    let mut spans: Vec<Span> = Vec::new();
    let mut hits = Vec::new();
    let mut x = area.x + 1;
    spans.push(Span::raw(" "));
    for (ch, tab, name) in Tab::NUMBERED {
        let label = format!(" {ch} {name} ");
        let w = label.chars().count() as u16;
        let mut style = Style::default();
        if db.tab == tab {
            style = style.add_modifier(Modifier::REVERSED);
        }
        if tab == Tab::PgBot && db.pgbot_changed() {
            style = style.add_modifier(Modifier::BOLD);
        }
        spans.push(Span::styled(label, style));
        spans.push(Span::raw(" "));
        hits.push((Rect::new(x, area.y, w, 1), Hit::SetTab(tab)));
        x += w + 1;
    }
    let (glyph, tone) = tab_glyph(db);
    let server = db.ctx.as_ref().map(|c| c.server.short_version()).unwrap_or_else(|| "PostgreSQL".into());
    let ago = db.last_checked.map(|t| format::ago(t.elapsed())).unwrap_or_else(|| "—".into());
    let right = format!("{server} · {ago} ");
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let gap = (area.width as usize).saturating_sub(used + right.chars().count() + 2);
    spans.push(Span::raw(" ".repeat(gap)));
    spans.push(Span::styled(glyph, Style::default().fg(tone)));
    spans.push(Span::styled(format!(" {right}"), Style::default().fg(Color::DarkGray)));
    f.render_widget(Paragraph::new(Line::from(spans)), area);
    hits
}

pub fn draw_subtabs(f: &mut Frame, area: Rect, db: &DbState) -> Vec<(Rect, Hit)> {
    let mut spans: Vec<Span> = vec![Span::raw(" ")];
    let mut hits = Vec::new();
    let mut x = area.x + 1;
    let mut entries: Vec<(View, &str)> = View::NUMBERED.iter().map(|(_, v, n)| (*v, *n)).collect();
    if db.ask_output.is_some() || db.view == View::Ask {
        entries.push((View::Ask, "Ask"));
    }
    for (view, name) in entries {
        let label = format!(" {name} ");
        let w = label.chars().count() as u16;
        let style = if db.view == view { Style::default().add_modifier(Modifier::REVERSED) } else { Style::default() };
        spans.push(Span::styled(label, style));
        spans.push(Span::raw(" "));
        hits.push((Rect::new(x, area.y, w, 1), Hit::SetView(view)));
        x += w + 1;
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
    hits
}
```

`screens/overview.rs` — add the draw half:

```rust
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::DbState;
use crate::health::{self, HealthStatus, RowStatus};
use crate::screens::sidebar::badge_style;
use std::time::SystemTime;

fn kind_style(k: GaugeKind, measurable: bool) -> Style {
    if !measurable {
        return Style::default().fg(Color::DarkGray);
    }
    match k {
        GaugeKind::Ok => Style::default().fg(Color::Green),
        GaugeKind::Watch => Style::default().fg(Color::Yellow),
        GaugeKind::Bad => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        GaugeKind::Info => Style::default().fg(Color::DarkGray),
    }
}

pub fn draw(f: &mut Frame, area: Rect, db: &DbState) {
    let dim = Style::default().fg(Color::DarkGray);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let inner = area.inner(Margin::new(1, 0));
    let mut lines: Vec<Line> = Vec::new();

    // Header: name + badge; keys on the right are part of the same line.
    let mut head = vec![Span::styled(db.profile.name.clone(), bold)];
    if let Some(stage) = db.profile.badge() {
        head.push(Span::raw("  "));
        head.push(Span::styled(stage.label(), badge_style(stage)));
    }
    head.push(Span::raw("   "));
    head.push(Span::styled("r refresh  2 pgbot", dim));
    lines.push(Line::from(head));

    // Status line.
    let ago = db.last_checked.map(|t| format::ago(t.elapsed())).unwrap_or_else(|| "—".into());
    let status = match (&db.ctx, db.health) {
        (_, HealthStatus::Unavailable) => {
            let msg = db.error.as_ref().map(|e| e.to_string()).unwrap_or_else(|| "unavailable".into());
            Line::from(vec![Span::styled("○ Unavailable", dim), Span::raw(format!(" · {msg} · ")), Span::styled("r retry", bold)])
        }
        (None, _) => Line::from(Span::styled("◌ Checking…", Style::default().fg(Color::Cyan))),
        (Some(ctx), _) => {
            let mut parts = vec![format!("PostgreSQL {}", version_digits(&ctx.server.version_text).unwrap_or_else(|| ctx.server.major().to_string()))];
            if let Some(p) = provider_label(&ctx.server.provider) {
                parts.push(p);
            }
            if ctx.server.uptime_seconds > 0 {
                parts.push(format!("up {}", format::duration_short(ctx.server.uptime_seconds)));
            }
            parts.push(format!("checked {ago}"));
            Line::from(vec![Span::styled("● Connected", Style::default().fg(Color::Green)), Span::styled(format!(" · {}", parts.join(" · ")), dim)])
        }
    };
    lines.push(status);
    lines.push(Line::from(""));
    let header_h = lines.len() as u16;
    let [header_area, tiles_area, rest] = Layout::vertical([
        Constraint::Length(header_h),
        Constraint::Length(if db.ctx.is_some() { tile_rows(inner.width) * 3 } else { 0 }),
        Constraint::Min(0),
    ])
    .areas(inner);
    f.render_widget(Paragraph::new(lines), header_area);

    let Some(ctx) = &db.ctx else {
        return;
    };
    draw_tiles(f, tiles_area, &tiles(ctx));

    // Gauge strip.
    let mut body: Vec<Line> = vec![Line::from("")];
    for g in gauges(ctx) {
        let style = kind_style(g.kind, g.measurable);
        body.push(Line::from(vec![
            Span::styled(format!("  {:<9}  [", g.label), dim),
            Span::styled(gauge_bar(g.share), style),
            Span::raw(format!("]  {:<8}  ", g.value)),
            Span::styled(g.status.clone(), style),
        ]));
    }
    body.push(Line::from(""));

    // Findings summary.
    let att = attention_findings(ctx);
    let headline = if att.is_empty() { "no findings".to_string() } else { format!("{} findings need attention", att.len()) };
    body.push(Line::from(vec![Span::styled("PGBOT", bold), Span::raw("   "), Span::raw(headline)]));
    let width = rest.width as usize;
    for f_ in att.iter().take(5) {
        let (glyph, style) = if f_.severity == "critical" {
            ("✗", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))
        } else {
            ("⚠", Style::default().fg(Color::Yellow))
        };
        let conf = format!("confidence {}", confidence_label(f_.confidence));
        let title_w = width.saturating_sub(conf.len() + 4);
        let title: String = f_.title.chars().take(title_w).collect();
        let gap = width.saturating_sub(2 + title.chars().count() + conf.len());
        body.push(Line::from(vec![Span::styled(format!("{glyph} "), style), Span::raw(title), Span::raw(" ".repeat(gap)), Span::styled(conf, dim)]));
    }
    if att.len() > 5 {
        body.push(Line::from(Span::styled(format!("… and {} more — 2 pgbot", att.len() - 5), dim)));
    }
    let (rows, _) = health::categories(ctx, SystemTime::now());
    for row in rows.iter().filter(|r| r.status == RowStatus::Ok) {
        body.push(Line::from(vec![
            Span::styled("✓ ", Style::default().fg(Color::Green)),
            Span::raw(format!("{:<12}", row.name)),
            Span::styled(row.metric.clone(), dim),
        ]));
    }
    f.render_widget(Paragraph::new(body).scroll((db.scroll.get(&View::Inspect).copied().unwrap_or(0), 0)), rest);
}

const TILE_W: u16 = 14;

fn tile_rows(width: u16) -> u16 {
    let per_row = (width / TILE_W).max(1);
    (5 + per_row - 1) / per_row
}

fn draw_tiles(f: &mut Frame, area: Rect, tiles: &[(&'static str, String)]) {
    let per_row = (area.width / TILE_W).max(1) as usize;
    for (i, (label, value)) in tiles.iter().enumerate() {
        let row = (i / per_row) as u16;
        let col = (i % per_row) as u16;
        let rect = Rect::new(area.x + col * TILE_W, area.y + row * 3, TILE_W, 3);
        if rect.right() > area.right() || rect.bottom() > area.bottom() {
            continue;
        }
        let block = Block::default().borders(Borders::ALL).title(Span::styled(format!(" {label} "), Style::default().fg(Color::DarkGray)));
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        f.render_widget(Paragraph::new(Span::styled(format!(" {value}"), Style::default().add_modifier(Modifier::BOLD))), inner);
    }
}
```

(`use crate::action::View;` for the scroll key. `RowStatus` must derive `PartialEq` — it does, `status_style` matches on it.)

`ui.rs` — replace `draw`, `draw_tabs`, `draw_shortcuts`, `draw_help`, `HELP`:

```rust
pub fn is_wide(width: u16) -> bool {
    width >= 100
}

pub fn draw(f: &mut Frame, app: &mut App) {
    app.hitmap.clear();
    let area = f.area();
    if states::is_too_small(area.width, area.height) {
        states::draw_too_small(f);
        return;
    }
    if app.dbs.is_empty() && app.popup.is_none() {
        states::draw_first_run(f, area);
        return;
    }
    let [top, body, cmd_row] = Layout::vertical([Constraint::Length(1), Constraint::Min(0), Constraint::Length(1)]).areas(area);
    draw_top_bar(f, top, app);

    let mut hits: Vec<(Rect, Hit)> = Vec::new();
    let main = if is_wide(area.width) {
        let [side, rule, main] = Layout::horizontal([Constraint::Length(sidebar::WIDTH - 1), Constraint::Length(1), Constraint::Min(0)]).areas(body);
        hits.extend(sidebar::draw(f, side, app));
        f.render_widget(Block::default().borders(Borders::LEFT).border_style(Style::default().fg(Color::DarkGray)), rule);
        main
    } else {
        let [strip, main] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(body);
        draw_tabs(f, strip, app); // the existing top strip, unchanged
        main
    };
    let [tab_row, rule, tab_body] = Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(0)]).areas(main);
    hits.extend(tabs::draw_tab_row(f, tab_row, app));
    f.render_widget(Block::default().borders(Borders::TOP).border_style(Style::default().fg(Color::DarkGray)), rule);
    if let Some(db) = app.dbs.get(app.selected) {
        match db.tab {
            Tab::Overview => overview::draw(f, tab_body, db),
            Tab::PgBot => {
                let [sub, rest] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(tab_body);
                hits.extend(tabs::draw_subtabs(f, sub, db));
                screens::draw_body(f, rest, db);
            }
        }
    }
    app.hitmap.extend(hits);
    draw_command_bar(f, cmd_row, app);
    draw_toast(f, cmd_row, app);

    if app.focus == Focus::Help {
        draw_help(f, area);
    }
    if app.focus == Focus::Palette {
        let hits = draw_palette(f, area, app);
        app.hitmap.extend(hits);
    }
    if let Some(popup) = app.popup.clone() {
        let hits = draw_popup(f, area, &popup, app.focus);
        app.hitmap.extend(hits);
    }
}

fn draw_top_bar(f: &mut Frame, area: Rect, app: &mut App) {
    let dim = Style::default().fg(Color::DarkGray);
    let mut spans = vec![Span::styled(" pgterm ", Style::default().add_modifier(Modifier::BOLD))];
    if let Some(db) = app.dbs.get(app.selected) {
        spans.push(Span::styled(format!("  ▸ {}", db.profile.name), Style::default()));
        if let Some(stage) = db.profile.badge() {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(stage.label(), sidebar::badge_style(stage)));
        }
    }
    let right = "^K commands  ? help ";
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let gap = (area.width as usize).saturating_sub(used + right.len());
    spans.push(Span::raw(" ".repeat(gap)));
    let x = area.x + (used + gap) as u16;
    spans.push(Span::styled(right, dim));
    app.hitmap.push((Rect::new(x, area.y, 11, 1), Hit::OpenPalette));
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_toast(f: &mut Frame, area: Rect, app: &App) {
    let Some(t) = app.active_toast() else { return };
    let w = (t.text.chars().count() as u16 + 2).min(area.width);
    let rect = Rect::new(area.right().saturating_sub(w), area.y, w, 1);
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(Span::styled(format!(" {} ", t.text), Style::default().fg(Color::Black).bg(Color::Yellow))), rect);
}

fn draw_help(f: &mut Frame, area: Rect) {
    let text = crate::keymap::help_text();
    let lines: Vec<Line> = text.lines().map(|l| Line::from(l.to_string())).collect();
    let h = (lines.len() as u16 + 2).min(area.height);
    let [v] = Layout::vertical([Constraint::Length(h)]).flex(Flex::Center).areas(area);
    let [rect] = Layout::horizontal([Constraint::Length(50)]).flex(Flex::Center).areas(v);
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" help — any key closes ")), rect);
}

fn draw_palette(f: &mut Frame, area: Rect, app: &App) -> Vec<(Rect, Hit)> {
    let Some(state) = &app.palette else { return Vec::new() };
    let names: Vec<&str> = app.dbs.iter().map(|d| d.profile.name.as_str()).collect();
    let items = crate::palette::items(&names);
    let hits = crate::palette::filter(&items, &state.input);
    let shown = hits.len().min(10);
    let [v] = Layout::vertical([Constraint::Length(shown as u16 + 4)]).flex(Flex::Start).areas(area.inner(Margin::new(0, 2)));
    let [rect] = Layout::horizontal([Constraint::Length(60)]).flex(Flex::Center).areas(v);
    f.render_widget(Clear, rect);
    let mut lines = vec![Line::from(vec![Span::styled("> ", Style::default().fg(Color::DarkGray)), Span::raw(state.input.clone()), Span::styled("█", Style::default().fg(Color::Gray))]), Line::from("")];
    let mut regions = Vec::new();
    let cursor = state.cursor.min(shown.saturating_sub(1));
    for (row, idx) in hits.iter().take(shown).enumerate() {
        let style = if row == cursor { Style::default().add_modifier(Modifier::REVERSED) } else { Style::default() };
        lines.push(Line::from(Span::styled(format!(" {} ", items[*idx].label), style)));
        regions.push((Rect::new(rect.x + 1, rect.y + 3 + row as u16, rect.width - 2, 1), Hit::PaletteItem(row)));
    }
    if let Some(err) = &app.cmd_error {
        lines.push(Line::from(Span::styled(err.clone(), Style::default().fg(Color::Red))));
    }
    f.render_widget(Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" commands ")), rect);
    regions
}
```

Delete the old `HELP` const and `draw_shortcuts`. Keep `draw_tabs` (narrow strip) and `draw_command_bar`. `screens/mod.rs`: `pub mod overview; pub mod sidebar; pub mod tabs;`. `ui.rs` imports: `use crate::action::{Hit, Tab}; use crate::screens::{self, overview, sidebar, states, tabs};` and `Margin`.

- [ ] **Step 4: Run** `cargo test --locked` → PASS; `cargo clippy --all-targets -- -D warnings` clean. Smoke by eye: `cargo build --release && ./demo/run.sh` — sidebar with three badged databases, Overview with tiles and the strip, `2` shows the old dashboard, `:` opens the palette, `?` shows the generated help, resize below 100 columns collapses the sidebar.

- [ ] **Step 5: Commit**

```bash
git add src/ui.rs src/screens
git commit -m "feat(ui): sidebar shell — badges, tab row, Overview (tiles, gauges, findings), PgBot sub-tabs, palette, toast, keymap help"
```

---

### Task 9: Add-database popup — Stage field

**Files:**
- Modify: `src/app.rs` (PopupField, AddPopup, handle_popup_key, popup_submit, on_probe_finished), `src/ui.rs` (`draw_popup`), `src/action.rs` (ProbeFinished/SpawnProbe carry `stage`)

**Interfaces:**
- `PopupField { Name, Stage, Env }`; `AddPopup.stage: Option<Stage>` (`None` = auto); `Action::ProbeFinished { …, stage: Option<Stage>, … }`; `Effect::SpawnProbe { …, stage, … }`; `app::run_probe(bin, name, source, save, persist_env, stage, sem)`.

- [ ] **Step 1: Failing tests** (`src/app.rs` `mod tests`)

```rust
    #[test]
    fn popup_stage_field_cycles_and_is_saved() {
        let _g = popup_env_guard();
        std::env::set_var("STAGE_TEST_URL", "postgres://x@mode-healthy.local/db");
        let mut a = app(0);
        a.update(key(KeyCode::Char('a')));
        for c in "warehouse".chars() {
            a.update(key(KeyCode::Char(c)));
        }
        a.update(key(KeyCode::Tab));
        assert_eq!(a.popup.as_ref().unwrap().field, PopupField::Stage);
        a.update(key(KeyCode::Right));
        assert_eq!(a.popup.as_ref().unwrap().stage, Some(Stage::Prod));
        a.update(key(KeyCode::Right));
        assert_eq!(a.popup.as_ref().unwrap().stage, Some(Stage::Staging));
        a.update(key(KeyCode::Left));
        a.update(key(KeyCode::Left));
        assert_eq!(a.popup.as_ref().unwrap().stage, None, "wraps back to auto");
        a.update(key(KeyCode::Char(' ')));
        assert_eq!(a.popup.as_ref().unwrap().stage, Some(Stage::Prod));
        a.update(key(KeyCode::Tab));
        assert_eq!(a.popup.as_ref().unwrap().field, PopupField::Env);
        for c in "STAGE_TEST_URL".chars() {
            a.update(key(KeyCode::Char(c)));
        }
        let effects = a.update(key(KeyCode::Enter));
        assert!(matches!(&effects[0], Effect::SpawnProbe { stage: Some(Stage::Prod), .. }), "{effects:?}");
        a.update(Action::ProbeFinished {
            name: "warehouse".into(),
            source: ConnSource::Env("STAGE_TEST_URL".into()),
            save: true,
            persist_env: None,
            stage: Some(Stage::Prod),
            result: ok_ctx(HEALTHY),
        });
        assert_eq!(a.dbs[0].profile.stage, Some(Stage::Prod));
        std::env::remove_var("STAGE_TEST_URL");
    }
```

(The existing popup tests already redirect `PGTERM_CONFIG` to a temp file through `popup_env_guard`; mirror their setup so nothing writes to the real config.)

- [ ] **Step 2: Run** → compile errors.

- [ ] **Step 3: Implement**

`PopupField` gains `Stage`; `AddPopup` gains `pub stage: Option<Stage>` (None). Field cycling: `Tab`/`Down`: Name → Stage → Env → Name; `Up` reverse. In `handle_popup_key`, before the generic `Char(c)` arm:

```rust
            KeyCode::Left | KeyCode::Right | KeyCode::Char(' ') if popup.field == PopupField::Stage => {
                let order: [Option<Stage>; 5] = [None, Some(Stage::Prod), Some(Stage::Staging), Some(Stage::Dev), Some(Stage::Local)];
                let i = order.iter().position(|s| *s == popup.stage).unwrap_or(0) as i64;
                let step = if key.code == KeyCode::Left { -1 } else { 1 };
                popup.stage = order[(i + step).rem_euclid(5) as usize];
                Vec::new()
            }
```

and `Char(c)` / `Backspace` ignore the Stage field. `popup_submit` passes `stage: popup.stage` into `Effect::SpawnProbe`; `run_probe` threads it into `Action::ProbeFinished`; `on_probe_finished` uses `cfg.add_with_stage(name, env, stage)` and sets `profile.stage = stage` on the new `DbState`. `draw_popup` inserts after the Name field:

```rust
        Line::from(Span::styled("Stage", dim)),
        Line::from(vec![
            Span::styled(
                popup.stage.map(|s| s.label().to_string()).unwrap_or_else(|| "auto".into()),
                field_style(PopupField::Stage),
            ),
            Span::styled("   ←/→ prod · staging · dev · local · auto", dim),
        ]),
        Line::from(""),
```

and the popup height grows from 14 to 17; the button-row hit offset moves from `rect.y + 9` to `rect.y + 12`. Update `mouse_clicks_dispatch_through_the_hitmap` if it asserts those offsets.

- [ ] **Step 4: Run** `cargo test --locked` → PASS; clippy clean.

- [ ] **Step 5: Commit**

```bash
git add src/app.rs src/ui.rs src/action.rs
git commit -m "feat(popup): Stage field — auto/prod/staging/dev/local, saved with the profile"
```

---

### Task 10: Windows release artifact and installer

**Files:**
- Modify: `.github/workflows/release.yml` (matrix, package, checksums, smoke), `README.md` (Install)
- Create: `install.ps1`

- [ ] **Step 1: Add the matrix entry and packaging**

In `build.strategy.matrix.include` add:

```yaml
          - runner: windows-latest
            target: x86_64-pc-windows-msvc
            os: windows
            arch: amd64
```

Guard the musl step with `if: matrix.os == 'linux'` (already). Replace the `package` step with a shell-agnostic version:

```yaml
      - name: package
        shell: bash
        run: |
          set -eu
          VERSION="${GITHUB_REF_NAME#v}"
          staging="pgterm_${VERSION}_${{ matrix.os }}_${{ matrix.arch }}"
          mkdir "$staging"
          if [ "${{ matrix.os }}" = "windows" ]; then
            cp "target/${{ matrix.target }}/release/pgterm.exe" README.md LICENSE "$staging/"
            7z a -tzip "$staging.zip" "$staging" > /dev/null
          else
            cp "target/${{ matrix.target }}/release/pgterm" README.md LICENSE "$staging/"
            tar -czf "$staging.tar.gz" "$staging"
          fi
```

Upload step `path: |` lists both `pgterm_*.tar.gz` and `pgterm_*.zip`. In `publish`, `sha256sum pgterm_*.tar.gz pgterm_*.zip > checksums.txt` and `gh release create … dist/pgterm_*.tar.gz dist/pgterm_*.zip dist/checksums.txt`. Add a `smoke-windows` job:

```yaml
  smoke-windows:
    needs: publish
    runs-on: windows-latest
    steps:
      - name: download, verify, run --version
        shell: pwsh
        env:
          GH_TOKEN: ${{ github.token }}
        run: |
          $ErrorActionPreference = "Stop"
          $version = $env:GITHUB_REF_NAME.TrimStart("v")
          $zip = "pgterm_${version}_windows_amd64.zip"
          gh release download $env:GITHUB_REF_NAME --repo $env:GITHUB_REPOSITORY --pattern $zip --pattern checksums.txt
          $want = (Select-String -Path checksums.txt -Pattern $zip).Line.Split(" ")[0]
          $got = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
          if ($want -ne $got) { throw "checksum mismatch" }
          Expand-Archive $zip -DestinationPath .
          $out = & ".\pgterm_${version}_windows_amd64\pgterm.exe" --version
          Write-Host $out
          if ($out -ne "pgterm $version") { throw "version mismatch: $out" }
```

The `homebrew` job is unaffected (the formula only reads the tar.gz lines; `formula.sh` greps the exact file names).

- [ ] **Step 2: Write `install.ps1`**

```powershell
# pgterm installer for Windows: downloads the latest release zip, verifies
# its SHA-256 against checksums.txt, and installs pgterm.exe into
# $env:LOCALAPPDATA\pgterm\bin (override with $env:PGTERM_INSTALL_DIR),
# adding that directory to the user PATH when it is not there yet.
#   irm https://pgterm.dev/install.ps1 | iex
$ErrorActionPreference = "Stop"
$repo = "pgrundev/pgterm"
$dir = if ($env:PGTERM_INSTALL_DIR) { $env:PGTERM_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "pgterm\bin" }
$release = Invoke-RestMethod "https://api.github.com/repos/$repo/releases/latest"
$version = $release.tag_name.TrimStart("v")
$zip = "pgterm_${version}_windows_amd64.zip"
$asset = $release.assets | Where-Object name -eq $zip
if (-not $asset) { throw "release $version has no Windows build ($zip)" }
$sums = ($release.assets | Where-Object name -eq "checksums.txt").browser_download_url
$tmp = Join-Path ([System.IO.Path]::GetTempPath()) "pgterm-install-$version"
New-Item -ItemType Directory -Force $tmp | Out-Null
Invoke-WebRequest $asset.browser_download_url -OutFile (Join-Path $tmp $zip)
Invoke-WebRequest $sums -OutFile (Join-Path $tmp "checksums.txt")
$want = (Select-String -Path (Join-Path $tmp "checksums.txt") -Pattern $zip).Line.Split(" ")[0]
$got = (Get-FileHash (Join-Path $tmp $zip) -Algorithm SHA256).Hash.ToLower()
if ($want -ne $got) { throw "checksum mismatch for $zip" }
Expand-Archive (Join-Path $tmp $zip) -DestinationPath $tmp -Force
New-Item -ItemType Directory -Force $dir | Out-Null
Copy-Item (Join-Path $tmp "pgterm_${version}_windows_amd64\pgterm.exe") (Join-Path $dir "pgterm.exe") -Force
$path = [Environment]::GetEnvironmentVariable("Path", "User")
if (($path -split ";") -notcontains $dir) {
  [Environment]::SetEnvironmentVariable("Path", "$path;$dir", "User")
  Write-Host "added $dir to your user PATH (open a new terminal to pick it up)"
}
Write-Host "installed pgterm $version to $dir"
Write-Host "pgterm drives pgbot; install it too: irm https://pgbot.dev/install.ps1 | iex"
```

README Install gains a Windows block with that one-liner and a note that pgbot is installed separately on Windows.

- [ ] **Step 3: Verify the YAML parses** — `ruby -ryaml -e 'YAML.load_file(".github/workflows/release.yml")'` — and that `cargo build --release --target x86_64-pc-windows-msvc` is not attempted locally (the release run proves it; Task 11's tag does).

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/release.yml install.ps1 README.md
git commit -m "release: Windows amd64 zip + install.ps1, verified by a Windows smoke job"
```

---

### Task 11: README, welcome screen, demo, version 0.2.0, release

**Files:**
- Modify: `README.md`, `src/screens/states.rs` (welcome mentions `:` and `?`), `docs/design.md` (pointer to the redesign spec), `Cargo.toml`, `Cargo.lock`, `docs/releasing.md` (Windows artifact line)

- [ ] **Step 1: README**

Replace the top figure with the wide layout from the spec (rendered from a real `demo/run.sh` session at 120 columns, copied verbatim). Rewrite the **Keys** section from `keymap::help_text()` output. Add a **What moved in 0.2** note:

```
- Tab / Shift+Tab now move between the sidebar and the main pane; `[` and `]` switch databases.
- Number keys pick tabs (1 Overview, 2 PgBot); inside PgBot, ←/→ or h/l step through Inspect · Queries · Indexes · Tables · Why.
- Ctrl-K (or `:`) opens the command palette; `/` is still the command bar.
```

Add **Stages and badges** (config `stage`, `--stage`, inference), **Overview** (tiles, the gauge strip with a sentence per gauge and the "same rules as pgbot" note), **Configuration** (`[ui]` keys, `pgterm --default-config`), and the Windows install. Keep the security posture section intact.

- [ ] **Step 2: Welcome screen** — in `states::draw_first_run` add a line `[:] commands   [?] help` next to `[a] Add database   [q] Quit`.

- [ ] **Step 3: Version bump** — `Cargo.toml` `version = "0.2.0"`, `cargo build` so `Cargo.lock` follows. `docs/releasing.md` step 3 lists the Windows zip. `docs/design.md` gets a one-line pointer at the top: "Superseded for the shell by `docs/redesign-design.md` (2026-09-08); the pgbot contracts and safety rules below still hold."

- [ ] **Step 4: Full verification**

```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test --locked
cargo build --release && ./demo/run.sh   # eyeball wide + narrow, palette, help, popup Stage, badges
```

- [ ] **Step 5: Commit, tag, release**

```bash
git add -A && git commit -m "chore: README for the shell, welcome hints, bump to 0.2.0"
git push origin main
git tag v0.2.0 && git push origin v0.2.0
gh run watch $(gh run list -w release -b v0.2.0 -L 1 --json databaseId -q '.[0].databaseId') --exit-status
```

Expected: build ×5, publish, smoke ×2, smoke-windows, homebrew, brew-smoke all green; `brew install pgrundev/tap/pgterm pgrundev/tap/pgbot` yields `pgterm 0.2.0`.

---

## Self-review

**Spec coverage.** Config `stage`/`[ui]`/inference/error → Task 1. CLI `--stage`, `list` column, `--default-config` → Task 2. Model additions and `duration_short` → Task 3. Keymap + generated help + README check → Tasks 4, 11. Panes, `[`/`]`, digits per database, arrows only on PgBot, Enter semantics, PgBot rollup, toasts + bell → Task 5. Palette (items, matcher, `ask` passthrough, `overview`/`pgbot` verbs) → Task 6. Overview status line, tiles, gauge strip mirroring pgbot, findings summary with confidence and ✓ rows → Tasks 7, 8. Sidebar with badges and detail rows, tab row with freshness, narrow fallback, PgBot sub-tabs, toast rendering, palette overlay → Task 8. Popup Stage field → Task 9. Windows zip + `install.ps1` → Task 10. README, welcome, version, release → Task 11. Homebrew caveats already list pgbot; pgrun/pgbook caveats arrive with their slices. Docs site: deferred past this slice (spec says "follows the shell").

**Placeholders.** None; every code step is concrete. Tests reference fixture values that Task 3 pins.

**Type consistency.** `Tab`, `Pane` (action.rs) used by keymap, app, tabs, ui; `Stage` (config.rs) by cli, app, sidebar, overview, ui; `Gauge`/`GaugeKind`/`gauges` (overview.rs) by overview draw; `PaletteCmd`/`PaletteState`/`items`/`filter` by app and ui; `Toast`, `active_toast`, `take_bell` by ui and main; `UserCommand::{Overview, Pgbot}` by parser, palette, app; `run_user_command` shared by bar and palette. `ProbeFinished`/`SpawnProbe`/`run_probe` all gain `stage` in Task 9 together.
