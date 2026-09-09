//! Application state and the one place it mutates: `App::update`. Crossterm
//! events and background results arrive as Actions; update returns Effects
//! (pgbot spawns) for the runtime to perform. No IO happens here, which is
//! what lets the integration tests drive the whole app without a terminal.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use tokio::sync::Semaphore;

use crate::action::SqlTarget;
use crate::action::{Action, CmdKind, Effect, Hit, Pane, StoredResult, Tab, View};
use crate::config::{DatabaseProfile, Stage, TerminalConfig, UiSettings};
use crate::db::{QueryResult, WritePolicy};
use crate::editor::Editor;
use crate::health::{self, HealthStatus};
use crate::keymap::{self, KeyAction, KeyContext};
use crate::model::{Context, IndexesReport, WhyReport};
use crate::palette::{self, PaletteCmd, PaletteItem, PaletteState};
use crate::parser::UserCommand;
use crate::pgrun::{self, Branch, PgrunCommand};
use crate::runner::{self, ConnSource, PgbotCommand, RunOutcome};
use crate::sanitize::SafeError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Main,
    CommandBar,
    Popup,
    Help,
    Palette,
}

/// A one-line notice about a database you are not looking at.
#[derive(Debug, Clone)]
pub struct Toast {
    pub text: String,
    pub until: Instant,
}

pub const TOAST_SECONDS: u64 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopupField {
    Name,
    Stage,
    Env,
}

/// The Add Database popup. Field values are what the user typed — the env
/// var's VALUE is never resolved here, only its name is kept.
#[derive(Debug, Clone)]
pub struct AddPopup {
    pub name: String,
    /// The chosen badge; None means "infer it from the name".
    pub stage: Option<Stage>,
    pub env: String,
    pub field: PopupField,
    pub busy: bool,
    /// Outcome of the last Test/Add probe, sanitized.
    pub message: Option<Result<String, SafeError>>,
}

impl Default for AddPopup {
    fn default() -> Self {
        AddPopup {
            name: String::new(),
            stage: None,
            env: String::new(),
            field: PopupField::Name,
            busy: false,
            message: None,
        }
    }
}

/// Everything one database tab owns. Nothing in here is shared: switching
/// tabs must never reset another database's view or cached results.
#[derive(Debug)]
pub struct DbState {
    pub profile: DatabaseProfile,
    /// Where the DSN comes from. Env profiles persist in config; Session
    /// sources exist only in this process.
    pub source: ConnSource,
    pub health: HealthStatus,
    pub last_ok: Option<Instant>,
    pub last_checked: Option<Instant>,
    pub tab: Tab,
    pub view: View,
    pub ctx: Option<Context>,
    /// The SQL tab's editor buffer, its last result, and what is in flight.
    pub sql: Editor,
    pub sql_result: Option<QueryResult>,
    pub sql_error: Option<SafeError>,
    pub sql_running: bool,
    pub sql_scroll: u16,
    /// Set while a write on a PROD database is waiting to be confirmed by
    /// typing the database name.
    pub sql_confirm: Option<String>,
    /// The Data browser: schemas, the tables of the chosen one, and a page of
    /// rows from the chosen table.
    pub data_schemas: Option<Vec<String>>,
    pub data_tables: Option<Vec<(String, String, String)>>,
    pub data_rows: Option<QueryResult>,
    pub data_error: Option<SafeError>,
    pub data_schema_cursor: usize,
    pub data_table_cursor: usize,
    pub data_level: DataLevel,
    pub data_loading: bool,
    /// This database's pgrun branches, once fetched.
    pub branches: Option<Vec<Branch>>,
    pub branch_error: Option<SafeError>,
    pub branch_cursor: usize,
    pub branches_loading: bool,
    /// Non-suppressed finding ids from the latest check, sorted.
    pub findings_now: Option<Vec<String>>,
    /// The set that was on screen the last time the PgBot tab was viewed.
    pub findings_seen: Option<Vec<String>>,
    pub indexes: Option<IndexesReport>,
    pub why: Option<WhyReport>,
    pub ask_output: Option<String>,
    pub running: HashSet<CmdKind>,
    pub error: Option<SafeError>,
    /// A non-selected tab turned Warning/Critical/Unavailable since the user
    /// last looked at it.
    pub attention: bool,
    pub scroll: HashMap<View, u16>,
}

impl DbState {
    pub fn new(profile: DatabaseProfile) -> Self {
        let source = ConnSource::Env(profile.env.clone());
        DbState {
            profile,
            source,
            health: HealthStatus::Checking,
            last_ok: None,
            last_checked: None,
            tab: Tab::Overview,
            view: View::Inspect,
            ctx: None,
            sql: Editor::new(),
            sql_result: None,
            sql_error: None,
            sql_running: false,
            sql_scroll: 0,
            sql_confirm: None,
            data_schemas: None,
            data_tables: None,
            data_rows: None,
            data_error: None,
            data_schema_cursor: 0,
            data_table_cursor: 0,
            data_level: DataLevel::Schemas,
            data_loading: false,
            branches: None,
            branch_error: None,
            branch_cursor: 0,
            branches_loading: false,
            findings_now: None,
            findings_seen: None,
            indexes: None,
            why: None,
            ask_output: None,
            running: HashSet::new(),
            error: None,
            attention: false,
            scroll: HashMap::new(),
        }
    }

    /// A tab backed by a pasted URL: never persisted, gone on exit.
    pub fn session(name: &str, url: String) -> Self {
        let mut db = DbState::new(DatabaseProfile {
            name: name.to_string(),
            env: String::new(),
            stage: None,
            pgrun_project: None,
            writes: false,
        });
        db.source = ConnSource::Session(url);
        db
    }

    pub fn has_data(&self, view: View) -> bool {
        match view {
            View::Inspect | View::Queries | View::Tables => self.ctx.is_some(),
            View::Indexes => self.indexes.is_some(),
            View::Why => self.why.is_some(),
            View::Ask => self.ask_output.is_some(),
        }
    }

    /// The PgBot tab is marked while the finding set differs from the one
    /// last viewed there. Never marked before the tab has been viewed once.
    pub fn pgbot_changed(&self) -> bool {
        matches!((&self.findings_seen, &self.findings_now), (Some(seen), Some(now)) if seen != now)
    }

    pub fn mark_pgbot_seen(&mut self) {
        self.findings_seen = self.findings_now.clone();
    }

    pub fn checking(&self) -> bool {
        self.running.contains(&CmdKind::Monitor) || self.running.contains(&CmdKind::Inspect)
    }

    /// Is the job that would fill the CURRENT view already in flight?
    pub fn running_view_job(&self) -> bool {
        view_command(self.view)
            .map(|(_, kind)| self.running.contains(&kind))
            .unwrap_or(false)
    }
}

/// Where the Data browser is: schemas, then that schema's tables, then a page
/// of one table's rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataLevel {
    Schemas,
    Tables,
    Rows,
}

/// The pgbot command (and dedupe kind) behind each view.
pub fn view_command(view: View) -> Option<(PgbotCommand, CmdKind)> {
    match view {
        View::Inspect | View::Queries | View::Tables => {
            Some((PgbotCommand::InspectFull, CmdKind::Inspect))
        }
        View::Indexes => Some((PgbotCommand::Indexes, CmdKind::Indexes)),
        View::Why => Some((PgbotCommand::Why, CmdKind::Why)),
        View::Ask => None,
    }
}

pub struct App {
    pub dbs: Vec<DbState>,
    pub selected: usize,
    pub focus: Focus,
    /// Which pane the keyboard drives while `Focus::Main`.
    pub pane: Pane,
    pub ui: UiSettings,
    pub palette: Option<crate::palette::PaletteState>,
    pub toast: Option<Toast>,
    /// Set when a toast should also ring the terminal bell; the runtime
    /// consumes it with `take_bell` after the draw.
    pub bell_pending: bool,
    pub cmdline: String,
    pub cmd_error: Option<String>,
    pub popup: Option<AddPopup>,
    pub should_quit: bool,
    pub monitor_enabled: bool,
    pub interval: Duration,
    pub max_concurrent: usize,
    pub pgbot_bin: PathBuf,
    pub size: (u16, u16),
    /// Interactive regions, rebuilt by every draw pass.
    pub hitmap: Vec<(Rect, Hit)>,
    /// What the pointer is currently over, so the draw pass can show that it
    /// is clickable. Set from mouse motion, cleared when it leaves.
    pub hover: Option<Hit>,
    pub version_note: Option<String>,
}

impl App {
    pub fn new(
        cfg: &TerminalConfig,
        interval_override: Option<u64>,
        no_monitor: bool,
        select: Option<&str>,
    ) -> App {
        let dbs: Vec<DbState> = cfg.databases.iter().cloned().map(DbState::new).collect();
        let selected = select
            .and_then(|name| dbs.iter().position(|d| d.profile.name == name))
            .unwrap_or(0);
        App {
            dbs,
            selected,
            focus: Focus::Main,
            pane: Pane::Main,
            ui: cfg.ui.clone(),
            palette: None,
            toast: None,
            bell_pending: false,
            cmdline: String::new(),
            cmd_error: None,
            popup: None,
            should_quit: false,
            monitor_enabled: !no_monitor,
            interval: Duration::from_secs(
                interval_override
                    .unwrap_or(cfg.settings.interval_seconds)
                    .max(5),
            ),
            max_concurrent: cfg.settings.max_concurrent_checks.max(1),
            pgbot_bin: runner::pgbot_bin(),
            size: (0, 0),
            hitmap: Vec::new(),
            hover: None,
            version_note: None,
        }
    }

    pub fn selected_db(&self) -> Option<&DbState> {
        self.dbs.get(self.selected)
    }

    /// The toast, while it is still fresh enough to show.
    pub fn active_toast(&self) -> Option<&Toast> {
        self.toast.as_ref().filter(|t| Instant::now() < t.until)
    }

    /// Consumed once by the runtime, which rings the bell.
    pub fn take_bell(&mut self) -> bool {
        std::mem::take(&mut self.bell_pending)
    }

    fn notify(&mut self, text: String) {
        self.toast = Some(Toast {
            text,
            until: Instant::now() + Duration::from_secs(TOAST_SECONDS),
        });
        if self.ui.bell {
            self.bell_pending = true;
        }
    }

    pub fn toggle_pane(&mut self) {
        self.pane = match self.pane {
            Pane::Sidebar => Pane::Main,
            Pane::Main => Pane::Sidebar,
        };
    }

    /// Switch the selected database's tab. Landing on PgBot counts as viewing
    /// its findings, and fetches the current view's data if there is no cache.
    pub fn set_tab(&mut self, tab: Tab) -> Vec<Effect> {
        let selected = self.selected;
        let Some(db) = self.dbs.get_mut(selected) else {
            return Vec::new();
        };
        db.tab = tab;
        match tab {
            Tab::PgBot => {
                db.mark_pgbot_seen();
                let view = db.view;
                self.set_view(view)
            }
            Tab::Branches if db.branches.is_none() => self.refresh_branches(),
            Tab::Data if db.data_schemas.is_none() => self.load_schemas(),
            _ => Vec::new(),
        }
    }

    /// Which key contexts apply right now, most specific first.
    /// Ask pgrun for the selected database's branches. Needs a configured
    /// project; nothing is spawned without one, and one call at a time.
    pub fn refresh_branches(&mut self) -> Vec<Effect> {
        let selected = self.selected;
        let Some(db) = self.dbs.get_mut(selected) else {
            return Vec::new();
        };
        let Some(project) = db.profile.pgrun_project.clone() else {
            return Vec::new();
        };
        if db.branches_loading {
            return Vec::new();
        }
        db.branches_loading = true;
        db.branch_error = None;
        vec![Effect::SpawnPgrun {
            db: selected,
            cmd: PgrunCommand::List(project),
            open: false,
        }]
    }

    /// Enter on a branch: fetch its connection URL, which only `branch get`
    /// returns. The URL never touches config — it becomes a session tab.
    pub fn open_selected_branch(&mut self) -> Vec<Effect> {
        let selected = self.selected;
        let Some(db) = self.dbs.get(selected) else {
            return Vec::new();
        };
        let Some(project) = db.profile.pgrun_project.clone() else {
            return Vec::new();
        };
        let Some(branch) = db
            .branches
            .as_ref()
            .and_then(|bs| bs.get(db.branch_cursor))
            .filter(|b| !b.in_progress() && !b.failed())
        else {
            return Vec::new();
        };
        vec![Effect::SpawnPgrun {
            db: selected,
            cmd: PgrunCommand::Get {
                project,
                branch: branch.name.clone(),
            },
            open: true,
        }]
    }

    fn on_branch_opened(
        &mut self,
        db: usize,
        result: Result<Box<Branch>, SafeError>,
    ) -> Vec<Effect> {
        let branch = match result {
            Ok(b) => b,
            Err(e) => {
                if let Some(state) = self.dbs.get_mut(db) {
                    state.branch_error = Some(e);
                }
                return Vec::new();
            }
        };
        let Some(url) = branch.connection_url.clone() else {
            if let Some(state) = self.dbs.get_mut(db) {
                state.branch_error = Some(SafeError::new(
                    crate::sanitize::ErrorKind::BadOutput,
                    "pgrun returned no connection URL for that branch",
                    None,
                ));
            }
            return Vec::new();
        };
        // Name the tab after the branch, disambiguated against what is open.
        let base = format!(
            "{}/{}",
            self.dbs
                .get(db)
                .map(|d| d.profile.name.as_str())
                .unwrap_or("branch"),
            branch.name
        );
        let mut name = base.clone();
        let mut n = 2;
        while self.dbs.iter().any(|d| d.profile.name == name) {
            name = format!("{base}-{n}");
            n += 1;
        }
        let mut state = DbState::session(&name, url);
        state.profile.stage = Some(crate::config::Stage::Dev);
        self.dbs.push(state);
        let idx = self.dbs.len() - 1;
        self.selected = idx;
        self.pane = Pane::Main;
        self.dbs[idx].running.insert(CmdKind::Monitor);
        vec![Effect::Spawn {
            db: idx,
            cmd: PgbotCommand::Monitor,
            kind: CmdKind::Monitor,
        }]
    }

    /// F5 / Ctrl-Enter in the SQL tab. A write on a PROD database asks for the
    /// database name first; the READ ONLY transaction is what actually stops a
    /// write everywhere else.
    pub fn run_sql(&mut self) -> Vec<Effect> {
        let selected = self.selected;
        let Some(db) = self.dbs.get_mut(selected) else {
            return Vec::new();
        };
        if db.sql_running || db.sql.is_empty() {
            return Vec::new();
        }
        let sql = db.sql.text();
        let policy = db.profile.write_policy();
        if policy == WritePolicy::ConfirmWrites
            && crate::db::looks_like_write(&sql)
            && db.sql_confirm.is_none()
        {
            db.sql_confirm = Some(String::new());
            return Vec::new();
        }
        db.sql_confirm = None;
        db.sql_running = true;
        db.sql_error = None;
        vec![Effect::SpawnSql {
            db: selected,
            target: SqlTarget::Editor,
            sql,
            policy,
        }]
    }

    /// Everything the Data browser asks for is read-only, whatever the profile
    /// allows: browsing is never a way to change something.
    fn data_query(&mut self, target: SqlTarget, sql: String) -> Vec<Effect> {
        let selected = self.selected;
        let Some(db) = self.dbs.get_mut(selected) else {
            return Vec::new();
        };
        db.data_loading = true;
        db.data_error = None;
        vec![Effect::SpawnSql {
            db: selected,
            target,
            sql,
            policy: WritePolicy::ReadOnly,
        }]
    }

    pub fn load_schemas(&mut self) -> Vec<Effect> {
        self.data_query(SqlTarget::Schemas, crate::db::SCHEMAS_SQL.to_string())
    }

    /// Enter in the Data browser: descend a level.
    pub fn data_enter(&mut self) -> Vec<Effect> {
        let selected = self.selected;
        let Some(db) = self.dbs.get(selected) else {
            return Vec::new();
        };
        match db.data_level {
            DataLevel::Schemas => {
                let Some(schema) = db
                    .data_schemas
                    .as_ref()
                    .and_then(|s| s.get(db.data_schema_cursor))
                    .cloned()
                else {
                    return Vec::new();
                };
                if let Some(db) = self.dbs.get_mut(selected) {
                    db.data_level = DataLevel::Tables;
                    db.data_tables = None;
                    db.data_table_cursor = 0;
                }
                self.data_query(
                    SqlTarget::Tables(schema.clone()),
                    crate::db::tables_sql(&schema),
                )
            }
            DataLevel::Tables => {
                let (Some(schema), Some(table)) = (
                    db.data_schemas
                        .as_ref()
                        .and_then(|s| s.get(db.data_schema_cursor))
                        .cloned(),
                    db.data_tables
                        .as_ref()
                        .and_then(|t| t.get(db.data_table_cursor))
                        .map(|(n, _, _)| n.clone()),
                ) else {
                    return Vec::new();
                };
                if let Some(db) = self.dbs.get_mut(selected) {
                    db.data_level = DataLevel::Rows;
                    db.data_rows = None;
                }
                self.data_query(
                    SqlTarget::Rows {
                        schema: schema.clone(),
                        table: table.clone(),
                    },
                    crate::db::rows_sql(&schema, &table, 0),
                )
            }
            DataLevel::Rows => Vec::new(),
        }
    }

    /// Esc in the Data browser: back up a level.
    pub fn data_back(&mut self) -> bool {
        let Some(db) = self.dbs.get_mut(self.selected) else {
            return false;
        };
        match db.data_level {
            DataLevel::Rows => {
                db.data_level = DataLevel::Tables;
                db.data_rows = None;
                true
            }
            DataLevel::Tables => {
                db.data_level = DataLevel::Schemas;
                db.data_tables = None;
                true
            }
            DataLevel::Schemas => false,
        }
    }

    fn on_sql_finished(
        &mut self,
        db: usize,
        target: SqlTarget,
        result: Result<QueryResult, SafeError>,
    ) -> Vec<Effect> {
        let Some(state) = self.dbs.get_mut(db) else {
            return Vec::new();
        };
        match target {
            SqlTarget::Editor => {
                state.sql_running = false;
                state.sql_scroll = 0;
                match result {
                    Ok(r) => {
                        state.sql_result = Some(r);
                        state.sql_error = None;
                    }
                    Err(e) => state.sql_error = Some(e),
                }
            }
            SqlTarget::Schemas => {
                state.data_loading = false;
                match result {
                    Ok(r) => {
                        let names: Vec<String> = r
                            .rows
                            .iter()
                            .filter_map(|row| row.first().cloned())
                            .collect();
                        state.data_schema_cursor =
                            state.data_schema_cursor.min(names.len().saturating_sub(1));
                        state.data_schemas = Some(names);
                        state.data_error = None;
                    }
                    Err(e) => state.data_error = Some(e),
                }
            }
            SqlTarget::Tables(_) => {
                state.data_loading = false;
                match result {
                    Ok(r) => {
                        let rows: Vec<(String, String, String)> = r
                            .rows
                            .iter()
                            .map(|row| {
                                (
                                    row.first().cloned().unwrap_or_default(),
                                    row.get(1).cloned().unwrap_or_default(),
                                    row.get(2).cloned().unwrap_or_default(),
                                )
                            })
                            .collect();
                        state.data_table_cursor =
                            state.data_table_cursor.min(rows.len().saturating_sub(1));
                        state.data_tables = Some(rows);
                        state.data_error = None;
                    }
                    Err(e) => state.data_error = Some(e),
                }
            }
            SqlTarget::Rows { .. } => {
                state.data_loading = false;
                match result {
                    Ok(r) => {
                        state.data_rows = Some(r);
                        state.data_error = None;
                    }
                    Err(e) => state.data_error = Some(e),
                }
            }
        }
        Vec::new()
    }

    /// Typed input while the SQL tab has focus, including the confirm prompt.
    fn handle_sql_key(&mut self, key: KeyEvent) -> Option<Vec<Effect>> {
        let selected = self.selected;
        if self.dbs.get(selected).map(|d| d.tab) != Some(Tab::Sql) || self.pane != Pane::Main {
            return None;
        }
        // Ctrl-Enter and F5 run; they are not text.
        if key.code == KeyCode::F(5)
            || (key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::CONTROL))
        {
            return Some(self.run_sql());
        }
        let name = self.dbs.get(selected)?.profile.name.clone();
        let db = self.dbs.get_mut(selected)?;
        if let Some(typed) = db.sql_confirm.as_mut() {
            match key.code {
                KeyCode::Esc => {
                    db.sql_confirm = None;
                }
                KeyCode::Backspace => {
                    typed.pop();
                }
                KeyCode::Char(c) => typed.push(c),
                // Only the exact name runs it; anything else falls through
                // and the prompt stays up.
                KeyCode::Enter if typed.trim() == name => {
                    db.sql_confirm = None;
                    db.sql_running = true;
                    db.sql_error = None;
                    let sql = db.sql.text();
                    let policy = db.profile.write_policy();
                    return Some(vec![Effect::SpawnSql {
                        db: selected,
                        target: SqlTarget::Editor,
                        sql,
                        policy,
                    }]);
                }
                _ => {}
            }
            return Some(Vec::new());
        }
        match key.code {
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => db.sql.insert(c),
            KeyCode::Enter => db.sql.newline(),
            KeyCode::Backspace => db.sql.backspace(),
            KeyCode::Delete => db.sql.delete(),
            KeyCode::Left => db.sql.left(),
            KeyCode::Right => db.sql.right(),
            KeyCode::Up => db.sql.up(),
            KeyCode::Down => db.sql.down(),
            KeyCode::Home => db.sql.home(),
            KeyCode::End => db.sql.end(),
            KeyCode::Esc => {
                self.pane = Pane::Sidebar;
            }
            _ => return None,
        }
        Some(Vec::new())
    }

    fn on_branches_tab(&self) -> bool {
        self.on_tab(Tab::Branches)
    }

    fn on_tab(&self, tab: Tab) -> bool {
        self.dbs.get(self.selected).map(|d| d.tab) == Some(tab)
    }

    /// Move the cursor at whichever level of the Data browser is showing.
    fn data_move(&mut self, delta: i64) {
        let Some(db) = self.dbs.get_mut(self.selected) else {
            return;
        };
        let (cursor, len) = match db.data_level {
            DataLevel::Schemas => (
                &mut db.data_schema_cursor,
                db.data_schemas.as_ref().map(|s| s.len()).unwrap_or(0),
            ),
            DataLevel::Tables => (
                &mut db.data_table_cursor,
                db.data_tables.as_ref().map(|t| t.len()).unwrap_or(0),
            ),
            DataLevel::Rows => return,
        };
        let next = (*cursor as i64 + delta).clamp(0, len.saturating_sub(1) as i64);
        *cursor = next as usize;
    }

    fn key_contexts(&self) -> Vec<KeyContext> {
        let mut v = Vec::with_capacity(2);
        if self.pane == Pane::Sidebar {
            v.push(KeyContext::Sidebar);
        } else if let Some(db) = self.dbs.get(self.selected) {
            v.push(match db.tab {
                Tab::Overview => KeyContext::Overview,
                Tab::PgBot => KeyContext::PgBot,
                Tab::Sql => KeyContext::Sql,
                Tab::Data => KeyContext::Data,
                Tab::Branches => KeyContext::Branches,
            });
        }
        v.push(KeyContext::Main);
        v
    }

    pub fn update(&mut self, action: Action) -> Vec<Effect> {
        match action {
            Action::Quit => {
                self.should_quit = true;
                Vec::new()
            }
            Action::Resize(w, h) => {
                self.size = (w, h);
                Vec::new()
            }
            Action::MonitorTick => self.monitor_sweep(),
            Action::CheckFinished { db, kind, result } => {
                self.on_check_finished(db, kind, result);
                Vec::new()
            }
            Action::SqlFinished { db, target, result } => {
                self.on_sql_finished(db, target, result.map(|b| *b))
            }
            Action::BranchesFinished { db, result } => {
                if let Some(state) = self.dbs.get_mut(db) {
                    state.branches_loading = false;
                    match result {
                        Ok(bs) => {
                            state.branch_cursor =
                                state.branch_cursor.min(bs.len().saturating_sub(1));
                            state.branches = Some(bs);
                            state.branch_error = None;
                        }
                        Err(e) => state.branch_error = Some(e),
                    }
                }
                Vec::new()
            }
            Action::BranchOpened { db, result } => self.on_branch_opened(db, result),
            Action::ProbeFinished {
                name,
                source,
                save,
                stage,
                persist_env,
                result,
            } => self.on_probe_finished(&name, source, save, stage, persist_env, result),
            Action::Paste(text) => {
                self.handle_paste(&text);
                Vec::new()
            }
            Action::Key(key) => self.handle_key(key),
            Action::Mouse(m) => self.handle_mouse(m),
        }
    }

    /// One background sweep: every database not already being checked.
    fn monitor_sweep(&mut self) -> Vec<Effect> {
        if !self.monitor_enabled {
            return Vec::new();
        }
        let mut effects = Vec::new();
        for (i, db) in self.dbs.iter_mut().enumerate() {
            if db.running.contains(&CmdKind::Monitor) {
                continue;
            }
            db.running.insert(CmdKind::Monitor);
            effects.push(Effect::Spawn {
                db: i,
                cmd: PgbotCommand::Monitor,
                kind: CmdKind::Monitor,
            });
        }
        effects
    }

    fn on_check_finished(
        &mut self,
        db: usize,
        kind: CmdKind,
        result: Result<StoredResult, SafeError>,
    ) {
        let selected = self.selected;
        let Some(state) = self.dbs.get_mut(db) else {
            return;
        };
        state.running.remove(&kind);
        state.last_checked = Some(Instant::now());
        // Collected inside the borrow, acted on after it: a toast mutates App.
        let mut announce: Option<String> = None;
        match result {
            Ok(StoredResult::Ctx(ctx)) => {
                let was = state.health;
                let mut ids: Vec<String> = ctx
                    .findings
                    .iter()
                    .filter(|f| !f.suppressed)
                    .map(|f| f.id.clone())
                    .collect();
                ids.sort();
                state.findings_now = Some(ids);
                // The first result is the baseline, and a tab you are looking
                // at is being viewed right now — neither counts as a change.
                if state.findings_seen.is_none() || (db == selected && state.tab == Tab::PgBot) {
                    state.mark_pgbot_seen();
                }
                state.health = health::overall(&ctx);
                state.ctx = Some(*ctx);
                state.last_ok = Some(Instant::now());
                state.error = None;
                if db != selected
                    && matches!(state.health, HealthStatus::Warning | HealthStatus::Critical)
                {
                    state.attention = true;
                }
                if db != selected
                    && state.health == HealthStatus::Critical
                    && was != HealthStatus::Critical
                {
                    announce = Some(format!("{} is critical · [ to open", state.profile.name));
                }
            }
            Ok(StoredResult::Indexes(r)) => {
                state.indexes = Some(*r);
                state.error = None;
            }
            Ok(StoredResult::Why(r)) => {
                state.why = Some(*r);
                state.error = None;
            }
            Ok(StoredResult::Text(t)) => {
                state.ask_output = Some(t);
                state.error = None;
            }
            Err(e) => {
                // Only a failed health/inspect run makes the DATABASE
                // unavailable; a failed view fetch is that view's problem.
                if matches!(kind, CmdKind::Monitor | CmdKind::Inspect) {
                    let was = state.health;
                    state.health = HealthStatus::Unavailable;
                    if db != selected {
                        state.attention = true;
                        if was != HealthStatus::Unavailable {
                            announce =
                                Some(format!("{} is unavailable · [ to open", state.profile.name));
                        }
                    }
                }
                state.error = Some(e);
            }
        }
        if let Some(text) = announce {
            self.notify(text);
        }
    }

    fn on_probe_finished(
        &mut self,
        name: &str,
        source: ConnSource,
        save: bool,
        stage: Option<Stage>,
        persist_env: Option<String>,
        result: Result<StoredResult, SafeError>,
    ) -> Vec<Effect> {
        let Some(popup) = self.popup.as_mut() else {
            return Vec::new();
        };
        popup.busy = false;
        match result {
            Ok(StoredResult::Ctx(ctx)) => {
                let version = if ctx.server.major() > 0 {
                    format!("PostgreSQL {}", ctx.server.major())
                } else {
                    "connected".to_string()
                };
                if !save {
                    popup.message = Some(Ok(format!("✓ {version} — connection successful")));
                    return Vec::new();
                }
                if let ConnSource::Session(url) = source {
                    // Pasted URL: memory only for THIS session. If it arrived
                    // as NAME='URL', the NAME (never the URL) is persisted so
                    // the tab returns next launch once the var is exported.
                    if let Some(env_name) = &persist_env {
                        let mut cfg = match TerminalConfig::load() {
                            Ok(c) => c,
                            Err(e) => {
                                popup.message = Some(Err(SafeError::new(
                                    crate::sanitize::ErrorKind::BadOutput,
                                    &format!("{e:#}"),
                                    None,
                                )));
                                return Vec::new();
                            }
                        };
                        if let Err(e) = cfg
                            .add_with_stage(name, env_name, stage)
                            .and_then(|()| cfg.save())
                        {
                            popup.message = Some(Err(SafeError::new(
                                crate::sanitize::ErrorKind::BadOutput,
                                &format!("{e:#}"),
                                None,
                            )));
                            return Vec::new();
                        }
                    }
                    self.popup = None;
                    self.focus = Focus::Main;
                    let mut db = DbState::session(name, url);
                    db.profile.stage = stage;
                    if let Some(env_name) = persist_env {
                        db.profile.env = env_name;
                    }
                    self.dbs.push(db);
                    let idx = self.dbs.len() - 1;
                    self.selected = idx;
                    self.dbs[idx].running.insert(CmdKind::Monitor);
                    return vec![Effect::Spawn {
                        db: idx,
                        cmd: PgbotCommand::Monitor,
                        kind: CmdKind::Monitor,
                    }];
                }
                // Env-var reference: persist through the same validated path
                // as the CLI.
                let mut cfg = match TerminalConfig::load() {
                    Ok(c) => c,
                    Err(e) => {
                        popup.message = Some(Err(SafeError::new(
                            crate::sanitize::ErrorKind::BadOutput,
                            &format!("{e:#}"),
                            None,
                        )));
                        return Vec::new();
                    }
                };
                let env_name = match &source {
                    ConnSource::Env(n) => n.clone(),
                    ConnSource::Session(_) => unreachable!("handled above"),
                };
                if let Err(e) = cfg
                    .add_with_stage(name, &env_name, stage)
                    .and_then(|()| cfg.save())
                {
                    popup.message = Some(Err(SafeError::new(
                        crate::sanitize::ErrorKind::BadOutput,
                        &format!("{e:#}"),
                        None,
                    )));
                    return Vec::new();
                }
                self.popup = None;
                self.focus = Focus::Main;
                self.dbs.push(DbState::new(DatabaseProfile {
                    name: name.to_string(),
                    env: env_name,
                    stage,
                    pgrun_project: None,
                    writes: false,
                }));
                let idx = self.dbs.len() - 1;
                self.selected = idx;
                self.dbs[idx].running.insert(CmdKind::Monitor);
                vec![Effect::Spawn {
                    db: idx,
                    cmd: PgbotCommand::Monitor,
                    kind: CmdKind::Monitor,
                }]
            }
            Ok(_) => Vec::new(),
            Err(e) => {
                popup.message = Some(Err(e));
                Vec::new()
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        // Ctrl+C quits from anywhere, matching the terminal reflex.
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.should_quit = true;
            return Vec::new();
        }
        match self.focus {
            Focus::Help => {
                self.focus = Focus::Main;
                Vec::new()
            }
            Focus::CommandBar => self.handle_command_bar_key(key),
            Focus::Popup => self.handle_popup_key(key),
            Focus::Palette => self.handle_palette_key(key),
            Focus::Main => self.handle_main_key(key),
        }
    }

    fn handle_main_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        // The SQL editor owns nearly every key while it has the main pane, so
        // it gets first refusal; it declines the ones the shell still needs.
        if let Some(effects) = self.handle_sql_key(key) {
            return effects;
        }
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
                } else if self.on_branches_tab() {
                    if let Some(db) = self.dbs.get_mut(self.selected) {
                        db.branch_cursor = db.branch_cursor.saturating_sub(1);
                    }
                } else if self.on_tab(Tab::Data) {
                    self.data_move(-1);
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
                } else if self.on_branches_tab() {
                    if let Some(db) = self.dbs.get_mut(self.selected) {
                        let last = db.branches.as_ref().map(|b| b.len()).unwrap_or(0);
                        if db.branch_cursor + 1 < last {
                            db.branch_cursor += 1;
                        }
                    }
                } else if self.on_tab(Tab::Data) {
                    self.data_move(1);
                } else {
                    self.scroll_by(1);
                }
                Vec::new()
            }
            KeyAction::RunSql => self.run_sql(),
            KeyAction::Back => {
                self.data_back();
                Vec::new()
            }
            KeyAction::Enter => {
                if self.pane == Pane::Sidebar {
                    // The sidebar picked a database; hand the keys to the body.
                    self.pane = Pane::Main;
                    Vec::new()
                } else if self.on_branches_tab() {
                    self.open_selected_branch()
                } else if self.on_tab(Tab::Data) {
                    self.data_enter()
                } else {
                    // Overview: open the findings behind the summary.
                    if let Some(db) = self.dbs.get_mut(self.selected) {
                        db.view = View::Inspect;
                    }
                    self.set_tab(Tab::PgBot)
                }
            }
        }
    }

    fn handle_command_bar_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        match key.code {
            KeyCode::Esc => {
                self.focus = Focus::Main;
                self.cmd_error = None;
                Vec::new()
            }
            KeyCode::Backspace => {
                self.cmdline.pop();
                Vec::new()
            }
            KeyCode::Enter => self.submit_command(),
            KeyCode::Char(c) => {
                self.cmdline.push(c);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// Enter in the command bar: parse against the closed verb set. A parse
    /// error keeps the input and focus so the user can fix it in place.
    fn submit_command(&mut self) -> Vec<Effect> {
        match crate::parser::parse(&self.cmdline) {
            Err(msg) => {
                self.cmd_error = Some(msg);
                Vec::new()
            }
            Ok(cmd) => {
                self.cmdline.clear();
                self.cmd_error = None;
                self.focus = Focus::Main;
                self.run_user_command(cmd)
            }
        }
    }

    /// Run one parsed verb. Shared by the command bar and the palette, so the
    /// two surfaces can never drift apart. A pgbot view implies the PgBot tab.
    pub fn run_user_command(&mut self, cmd: UserCommand) -> Vec<Effect> {
        let view = match cmd {
            UserCommand::Inspect => Some(View::Inspect),
            UserCommand::Queries => Some(View::Queries),
            UserCommand::Indexes => Some(View::Indexes),
            UserCommand::Tables => Some(View::Tables),
            UserCommand::Why => Some(View::Why),
            _ => None,
        };
        if let Some(view) = view {
            if let Some(db) = self.dbs.get_mut(self.selected) {
                db.tab = Tab::PgBot;
                db.mark_pgbot_seen();
            }
            return self.set_view(view);
        }
        match cmd {
            UserCommand::Overview => self.set_tab(Tab::Overview),
            UserCommand::Pgbot => self.set_tab(Tab::PgBot),
            UserCommand::Refresh => self.refresh_selected(),
            UserCommand::Ask(q) => self.spawn_ask(q),
            _ => unreachable!("view verbs handled above"),
        }
    }

    /// `ask <question>`: switches to the Ask view and runs `pgbot ask --yes`
    /// with the question as one argv element. One ask per database at a time.
    fn spawn_ask(&mut self, question: String) -> Vec<Effect> {
        let selected = self.selected;
        let Some(db) = self.dbs.get_mut(selected) else {
            return Vec::new();
        };
        db.tab = Tab::PgBot;
        db.view = View::Ask;
        if db.running.contains(&CmdKind::Ask) {
            return Vec::new();
        }
        db.ask_output = None;
        db.running.insert(CmdKind::Ask);
        vec![Effect::Spawn {
            db: selected,
            cmd: PgbotCommand::Ask(question),
            kind: CmdKind::Ask,
        }]
    }

    fn open_palette(&mut self) -> Vec<Effect> {
        self.palette = Some(PaletteState::default());
        self.cmd_error = None;
        self.focus = Focus::Palette;
        Vec::new()
    }

    pub fn palette_items(&self) -> Vec<PaletteItem> {
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

    /// Enter in the palette. A highlighted item wins; otherwise the typed text
    /// goes to the command bar's parser, so `ask …` works here too. Unknown
    /// input keeps the palette open with the parser's message.
    fn palette_submit(&mut self) -> Vec<Effect> {
        let Some(state) = self.palette.clone() else {
            return Vec::new();
        };
        let input = state.input.trim().to_string();
        let items = self.palette_items();
        let hits = palette::filter(&items, &state.input);
        // Free text that parses as a verb (`ask …`, `refresh`) is run as typed
        // rather than as whatever item happened to fuzzy-match it.
        let typed = crate::parser::parse(&input).ok().map(PaletteCmd::Verb);
        let chosen = match (
            input.is_empty(),
            hits.get(state.cursor.min(hits.len().saturating_sub(1))),
        ) {
            (_, Some(&i)) if typed.is_none() || state.cursor > 0 => Some(items[i].cmd.clone()),
            _ => typed.or_else(|| hits.first().map(|&i| items[i].cmd.clone())),
        };
        let Some(cmd) = chosen else {
            self.cmd_error = Some(
                crate::parser::parse(&input)
                    .err()
                    .unwrap_or_else(|| "no match".to_string()),
            );
            return Vec::new();
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

    fn handle_popup_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let Some(popup) = self.popup.as_mut() else {
            self.focus = Focus::Main;
            return Vec::new();
        };
        match key.code {
            KeyCode::Esc => {
                self.popup = None;
                self.focus = Focus::Main;
                Vec::new()
            }
            KeyCode::Tab | KeyCode::Down => {
                popup.field = match popup.field {
                    PopupField::Name => PopupField::Stage,
                    PopupField::Stage => PopupField::Env,
                    PopupField::Env => PopupField::Name,
                };
                Vec::new()
            }
            KeyCode::BackTab | KeyCode::Up => {
                popup.field = match popup.field {
                    PopupField::Name => PopupField::Env,
                    PopupField::Stage => PopupField::Name,
                    PopupField::Env => PopupField::Stage,
                };
                Vec::new()
            }
            // The Stage field is a cycle, not a text box.
            KeyCode::Left | KeyCode::Right | KeyCode::Char(' ')
                if popup.field == PopupField::Stage =>
            {
                const ORDER: [Option<Stage>; 5] = [
                    None,
                    Some(Stage::Prod),
                    Some(Stage::Staging),
                    Some(Stage::Dev),
                    Some(Stage::Local),
                ];
                let i = ORDER.iter().position(|s| *s == popup.stage).unwrap_or(0) as i64;
                let step = if key.code == KeyCode::Left { -1 } else { 1 };
                popup.stage = ORDER[(i + step).rem_euclid(ORDER.len() as i64) as usize];
                Vec::new()
            }
            KeyCode::Backspace => {
                match popup.field {
                    PopupField::Name => popup.name.pop(),
                    PopupField::Env => popup.env.pop(),
                    PopupField::Stage => None,
                };
                Vec::new()
            }
            KeyCode::Enter => self.popup_submit(true),
            KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.popup_submit(false)
            }
            KeyCode::Char(c) => {
                match popup.field {
                    PopupField::Name => popup.name.push(c),
                    PopupField::Env => popup.env.push(c),
                    PopupField::Stage => {}
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// Validate popup input and ask the runtime to probe. `save` distinguishes
    /// [ Add ] from [ Test ].
    pub fn popup_submit(&mut self, save: bool) -> Vec<Effect> {
        let Some(popup) = self.popup.as_mut() else {
            return Vec::new();
        };
        if popup.busy {
            return Vec::new();
        }
        let name = popup.name.trim().to_string();
        let conn = popup.env.trim().to_string();
        let stage = popup.stage;

        // A pasted URL becomes a session-only source: validated name, no
        // config involvement, secret stays in memory. The NAME='URL' shape
        // additionally persists the NAME for future launches.
        if conn.contains("://") || conn.contains('=') {
            if let Err(e) = validate_session_name(&name, &self.dbs) {
                popup.message = Some(Err(SafeError::new(
                    crate::sanitize::ErrorKind::Usage,
                    &e,
                    Some(&conn),
                )));
                return Vec::new();
            }
            let (url, persist_env) = match parse_export_assignment(&conn) {
                Some((var, url)) => (url, Some(var)),
                None => (conn, None),
            };
            popup.busy = true;
            popup.message = None;
            return vec![Effect::SpawnProbe {
                name,
                source: ConnSource::Session(url),
                save,
                stage,
                persist_env,
            }];
        }

        // Otherwise it is an env-var reference — same validation as the CLI.
        let mut probe_cfg = TerminalConfig::load().unwrap_or_default();
        for d in &self.dbs {
            // The in-memory tab list is the truth the user sees; a name that
            // collides with it must fail even if the file lags.
            if !probe_cfg.databases.iter().any(|p| p.name == d.profile.name) {
                let _ = probe_cfg.add(&d.profile.name, &d.profile.env);
            }
        }
        if let Err(e) = probe_cfg.add(&name, &conn) {
            popup.message = Some(Err(SafeError::new(
                crate::sanitize::ErrorKind::Usage,
                &e.to_string(),
                None,
            )));
            return Vec::new();
        }
        let source = ConnSource::Env(conn);
        if let Err(e) = source.resolve() {
            popup.message = Some(Err(e));
            return Vec::new();
        }
        popup.busy = true;
        popup.message = None;
        vec![Effect::SpawnProbe {
            name,
            source,
            save,
            stage,
            persist_env: None,
        }]
    }

    /// Pasted text goes to whichever input has focus; control characters are
    /// stripped so a trailing newline cannot fire Enter.
    fn handle_paste(&mut self, text: &str) {
        let clean: String = text.chars().filter(|c| !c.is_control()).collect();
        match self.focus {
            Focus::CommandBar => self.cmdline.push_str(&clean),
            Focus::Popup => {
                if let Some(popup) = self.popup.as_mut() {
                    match popup.field {
                        PopupField::Name => popup.name.push_str(clean.trim()),
                        PopupField::Env => popup.env.push_str(clean.trim()),
                        PopupField::Stage => {}
                    }
                }
            }
            _ => {}
        }
    }

    /// The interactive region under a point, if any.
    fn hit_at(&self, col: u16, row: u16) -> Option<Hit> {
        self.hitmap
            .iter()
            .find(|(r, _)| col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height)
            .map(|(_, h)| h.clone())
    }

    fn handle_mouse(&mut self, m: MouseEvent) -> Vec<Effect> {
        // Motion only updates what looks clickable; it never acts.
        if matches!(m.kind, MouseEventKind::Moved | MouseEventKind::Drag(_)) {
            self.hover = self.hit_at(m.column, m.row);
            return Vec::new();
        }
        if !matches!(m.kind, MouseEventKind::Down(_)) {
            return Vec::new();
        }
        let hit = self.hit_at(m.column, m.row);
        match hit {
            Some(Hit::SelectDb(i)) => {
                self.select_db(i);
                Vec::new()
            }
            Some(Hit::OpenAdd) => {
                self.popup = Some(AddPopup::default());
                self.focus = Focus::Popup;
                Vec::new()
            }
            Some(Hit::SetView(v)) if self.focus == Focus::Main => self.set_view(v),
            Some(Hit::SetTab(t)) if self.focus == Focus::Main => self.set_tab(t),
            Some(Hit::SelectSchema(i)) if self.focus == Focus::Main => {
                if let Some(db) = self.dbs.get_mut(self.selected) {
                    db.data_schema_cursor = i;
                }
                self.data_enter()
            }
            Some(Hit::SelectTable(i)) if self.focus == Focus::Main => {
                if let Some(db) = self.dbs.get_mut(self.selected) {
                    db.data_table_cursor = i;
                }
                self.data_enter()
            }
            Some(Hit::SelectBranch(i)) if self.focus == Focus::Main => {
                if let Some(db) = self.dbs.get_mut(self.selected) {
                    db.branch_cursor = i;
                }
                self.open_selected_branch()
            }
            Some(Hit::OpenPalette) if self.focus == Focus::Main => self.open_palette(),
            Some(Hit::PaletteItem(i)) if self.focus == Focus::Palette => {
                if let Some(p) = self.palette.as_mut() {
                    p.cursor = i;
                }
                self.palette_submit()
            }
            Some(Hit::PopupTest) => self.popup_submit(false),
            Some(Hit::PopupAdd) => self.popup_submit(true),
            Some(Hit::PopupCancel) => {
                self.popup = None;
                self.focus = Focus::Main;
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn next_db(&self, dir: i64) -> usize {
        let n = self.dbs.len();
        if n == 0 {
            return 0;
        }
        ((self.selected as i64 + dir).rem_euclid(n as i64)) as usize
    }

    pub fn select_db(&mut self, i: usize) {
        if i < self.dbs.len() {
            self.selected = i;
            self.dbs[i].attention = false;
        }
    }

    /// Switch the selected database to a view, fetching its data if there is
    /// no cache yet (and no identical job already in flight).
    pub fn set_view(&mut self, view: View) -> Vec<Effect> {
        let selected = self.selected;
        let Some(db) = self.dbs.get_mut(selected) else {
            return Vec::new();
        };
        db.view = view;
        let Some((cmd, kind)) = view_command(view) else {
            return Vec::new();
        };
        if db.has_data(view) || db.running.contains(&kind) {
            return Vec::new();
        }
        db.running.insert(kind);
        vec![Effect::Spawn {
            db: selected,
            cmd,
            kind,
        }]
    }

    /// ←/→: step through the numbered views in shortcut-row order, wrapping
    /// at the ends. From the (unnumbered) Ask view either arrow returns to
    /// Inspect.
    fn cycle_view(&mut self, dir: i64) -> Vec<Effect> {
        let Some(db) = self.dbs.get(self.selected) else {
            return Vec::new();
        };
        let views: [View; 5] = View::NUMBERED.map(|(_, v, _)| v);
        let next = match views.iter().position(|v| *v == db.view) {
            Some(i) => views[(i as i64 + dir).rem_euclid(views.len() as i64) as usize],
            None => views[0],
        };
        self.set_view(next)
    }

    /// `r`: rerun the selected database's current view. Never duplicates an
    /// identical in-flight job.
    pub fn refresh_selected(&mut self) -> Vec<Effect> {
        let selected = self.selected;
        if self.on_tab(Tab::Data) {
            if let Some(db) = self.dbs.get_mut(selected) {
                db.data_schemas = None;
                db.data_tables = None;
                db.data_rows = None;
                db.data_level = DataLevel::Schemas;
            }
            return self.load_schemas();
        }
        if self.dbs.get(selected).map(|d| d.tab) == Some(Tab::Branches) {
            if let Some(db) = self.dbs.get_mut(selected) {
                db.branches = None;
            }
            return self.refresh_branches();
        }
        let Some(db) = self.dbs.get_mut(selected) else {
            return Vec::new();
        };
        let Some((cmd, kind)) = view_command(db.view) else {
            return Vec::new();
        };
        if db.running.contains(&kind) {
            return Vec::new();
        }
        db.running.insert(kind);
        vec![Effect::Spawn {
            db: selected,
            cmd,
            kind,
        }]
    }

    fn scroll_by(&mut self, delta: i32) {
        if let Some(db) = self.dbs.get_mut(self.selected) {
            let view = db.view;
            let s = db.scroll.entry(view).or_insert(0);
            *s = s.saturating_add_signed(delta as i16);
        }
    }
}

/// Name rules for a session tab: same shape as config names, unique against
/// the live tab list (the config is irrelevant — nothing is written).
fn validate_session_name(name: &str, dbs: &[DbState]) -> Result<(), String> {
    if name.is_empty() {
        return Err("database name is empty".into());
    }
    if name.len() > 64
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err("database name may only contain letters, digits, '.', '_' and '-'".into());
    }
    if dbs.iter().any(|d| d.profile.name == name) {
        return Err(format!("database \"{name}\" already exists"));
    }
    Ok(())
}

/// Recognizes a pasted shell assignment: `STAGING_DATABASE_URL='postgresql://...'`
/// (optional `export ` prefix, single/double/no quotes). Returns the variable
/// name and the URL, or None when the input is not that shape.
fn parse_export_assignment(s: &str) -> Option<(String, String)> {
    let s = s.trim();
    let s = s.strip_prefix("export ").unwrap_or(s).trim();
    let (name, value) = s.split_once('=')?;
    let name = name.trim();
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    let value = value.trim();
    let value = value
        .strip_prefix('\'')
        .and_then(|v| v.strip_suffix('\''))
        .or_else(|| value.strip_prefix('"').and_then(|v| v.strip_suffix('"')))
        .unwrap_or(value)
        .trim();
    if !value.contains("://") {
        return None;
    }
    Some((name.to_string(), value.to_string()))
}

/// Performs one Spawn effect: waits on the concurrency semaphore, runs pgbot,
/// decodes by command, and returns the Action to feed back into `update`.
pub async fn run_effect(
    pgbot_bin: PathBuf,
    source: ConnSource,
    db: usize,
    cmd: PgbotCommand,
    kind: CmdKind,
    sem: Arc<Semaphore>,
) -> Action {
    let _permit = sem.acquire_owned().await.ok();
    let timeout = runner::default_timeout(&cmd);
    let result = runner::run_pgbot(&pgbot_bin, &source, &cmd, timeout)
        .await
        .and_then(|out| decode_result(&cmd, &out));
    Action::CheckFinished { db, kind, result }
}

/// Performs one SpawnProbe effect for the add popup.
#[allow(clippy::too_many_arguments)]
/// One connection per database, opened on first use and kept for the session.
/// pgbot-only databases never get one — this is opened by the SQL and Data
/// tabs, and by nothing else.
#[derive(Default)]
pub struct Connections(std::collections::HashMap<usize, tokio_postgres::Client>);

impl Connections {
    /// The live connection for a database, opening or replacing it as needed.
    pub async fn get(
        &mut self,
        db: usize,
        source: &ConnSource,
    ) -> Result<&mut tokio_postgres::Client, SafeError> {
        // A closed connection is indistinguishable from a working one until
        // it is used, so drop it and reconnect rather than fail the query.
        if self.0.get(&db).map(|c| c.is_closed()).unwrap_or(false) {
            self.0.remove(&db);
        }
        if let std::collections::hash_map::Entry::Vacant(slot) = self.0.entry(db) {
            slot.insert(crate::db::connect(source).await?);
        }
        Ok(self.0.get_mut(&db).expect("present or just inserted"))
    }

    pub fn drop_db(&mut self, db: usize) {
        self.0.remove(&db);
    }
}

/// Run one SQL effect and turn the answer back into an Action.
pub async fn run_sql_effect(
    conns: Arc<tokio::sync::Mutex<Connections>>,
    db: usize,
    source: ConnSource,
    target: SqlTarget,
    sql: String,
    policy: WritePolicy,
) -> Action {
    let mut guard = conns.lock().await;
    let result = match guard.get(db, &source).await {
        Ok(client) => crate::db::run_sql(client, &sql, policy).await.map(Box::new),
        Err(e) => Err(e),
    };
    // A broken connection should not poison the next attempt.
    if matches!(
        result.as_ref().err().map(|e| e.kind),
        Some(crate::sanitize::ErrorKind::ConnectionFailed | crate::sanitize::ErrorKind::Timeout)
    ) {
        guard.drop_db(db);
    }
    Action::SqlFinished { db, target, result }
}

/// Perform one pgrun call and turn it back into an Action.
pub async fn run_pgrun_effect(bin: PathBuf, db: usize, cmd: PgrunCommand, open: bool) -> Action {
    let timeout = pgrun::default_timeout(&cmd);
    let out = pgrun::run_pgrun(&bin, &cmd, timeout).await;
    if open {
        let result = out.and_then(|o| pgrun::decode_branch(&o.stdout).map(Box::new));
        Action::BranchOpened { db, result }
    } else {
        let result = out.and_then(|o| pgrun::decode_list(&o.stdout));
        Action::BranchesFinished { db, result }
    }
}

pub async fn run_probe(
    pgbot_bin: PathBuf,
    name: String,
    source: ConnSource,
    save: bool,
    stage: Option<Stage>,
    persist_env: Option<String>,
    sem: Arc<Semaphore>,
) -> Action {
    let _permit = sem.acquire_owned().await.ok();
    let cmd = PgbotCommand::Probe;
    let result = runner::run_pgbot(&pgbot_bin, &source, &cmd, runner::default_timeout(&cmd))
        .await
        .and_then(|out| decode_result(&cmd, &out));
    Action::ProbeFinished {
        name,
        source,
        save,
        stage,
        persist_env,
        result,
    }
}

fn decode_result(cmd: &PgbotCommand, out: &RunOutcome) -> Result<StoredResult, SafeError> {
    match cmd {
        PgbotCommand::Monitor | PgbotCommand::InspectFull | PgbotCommand::Probe => {
            Context::decode(&out.stdout).map(|c| StoredResult::Ctx(Box::new(c)))
        }
        PgbotCommand::Indexes => {
            IndexesReport::decode(&out.stdout).map(|r| StoredResult::Indexes(Box::new(r)))
        }
        PgbotCommand::Why => WhyReport::decode(&out.stdout).map(|r| StoredResult::Why(Box::new(r))),
        PgbotCommand::Ask(_) => Ok(StoredResult::Text(out.stdout.clone())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Context;

    const WARN: &str = include_str!("../tests/fixtures/context_warn.json");
    const HEALTHY: &str = include_str!("../tests/fixtures/context_healthy.json");
    const CRITICAL: &str = include_str!("../tests/fixtures/context_critical.json");

    fn key(code: KeyCode) -> Action {
        Action::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn app(n: usize) -> App {
        let mut cfg = TerminalConfig::default();
        for i in 0..n {
            cfg.add(&format!("db{i}"), &format!("APP_TEST_URL_{i}"))
                .unwrap();
        }
        App::new(&cfg, None, false, None)
    }

    fn ctrl(c: char) -> Action {
        Action::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
    }

    fn named_app(names: &[&str]) -> App {
        let mut cfg = TerminalConfig::default();
        for n in names {
            cfg.add(n, &format!("{}_URL", n.to_uppercase())).unwrap();
        }
        App::new(&cfg, None, false, None)
    }

    fn ok_ctx(json: &str) -> Result<StoredResult, SafeError> {
        Ok(StoredResult::Ctx(Box::new(Context::decode(json).unwrap())))
    }

    fn err() -> Result<StoredResult, SafeError> {
        Err(SafeError::new(
            crate::sanitize::ErrorKind::ConnectionFailed,
            "connection refused",
            None,
        ))
    }

    #[test]
    fn monitor_tick_spawns_once_per_db_then_dedupes() {
        let mut a = app(3);
        let effects = a.update(Action::MonitorTick);
        assert_eq!(effects.len(), 3);
        assert!(effects.iter().all(|e| matches!(
            e,
            Effect::Spawn {
                kind: CmdKind::Monitor,
                cmd: PgbotCommand::Monitor,
                ..
            }
        )));
        // All three are now in flight — a second tick spawns nothing.
        assert!(a.update(Action::MonitorTick).is_empty());
    }

    #[test]
    fn no_monitor_flag_disables_sweeps() {
        let cfg = {
            let mut c = TerminalConfig::default();
            c.add("db0", "X").unwrap();
            c
        };
        let mut a = App::new(&cfg, None, true, None);
        assert!(a.update(Action::MonitorTick).is_empty());
    }

    #[test]
    fn check_finished_sets_health_and_flags_unselected_tabs() {
        let mut a = app(2);
        a.update(Action::MonitorTick);
        a.update(Action::CheckFinished {
            db: 1,
            kind: CmdKind::Monitor,
            result: ok_ctx(WARN),
        });
        assert_eq!(a.dbs[1].health, HealthStatus::Warning);
        assert!(a.dbs[1].attention, "background warning must flag the tab");
        assert!(!a.dbs[1].running.contains(&CmdKind::Monitor));

        a.update(Action::CheckFinished {
            db: 0,
            kind: CmdKind::Monitor,
            result: ok_ctx(HEALTHY),
        });
        assert_eq!(a.dbs[0].health, HealthStatus::Healthy);
        assert!(!a.dbs[0].attention, "the selected tab is being watched");

        // Selecting the flagged database clears the flag.
        a.update(key(KeyCode::Char(']')));
        assert_eq!(a.selected, 1);
        assert!(!a.dbs[1].attention);
    }

    #[test]
    fn monitor_error_makes_db_unavailable_but_view_error_does_not() {
        let mut a = app(1);
        a.update(Action::MonitorTick);
        a.update(Action::CheckFinished {
            db: 0,
            kind: CmdKind::Monitor,
            result: err(),
        });
        assert_eq!(a.dbs[0].health, HealthStatus::Unavailable);

        a.update(Action::CheckFinished {
            db: 0,
            kind: CmdKind::Monitor,
            result: ok_ctx(HEALTHY),
        });
        assert_eq!(a.dbs[0].health, HealthStatus::Healthy);

        a.update(Action::CheckFinished {
            db: 0,
            kind: CmdKind::Why,
            result: err(),
        });
        assert_eq!(
            a.dbs[0].health,
            HealthStatus::Healthy,
            "a why failure is not an outage"
        );
        assert!(a.dbs[0].error.is_some());
    }

    #[test]
    fn brackets_cycle_databases_and_state_survives() {
        let mut a = app(3);
        a.update(Action::CheckFinished {
            db: 0,
            kind: CmdKind::Monitor,
            result: ok_ctx(HEALTHY),
        });
        a.dbs[0].view = View::Indexes;
        a.update(key(KeyCode::Char(']')));
        a.update(key(KeyCode::Char(']')));
        assert_eq!(a.selected, 2);
        a.update(key(KeyCode::Char(']')));
        assert_eq!(a.selected, 0, "] wraps");
        assert_eq!(a.dbs[0].view, View::Indexes, "view survives db switches");
        assert!(a.dbs[0].ctx.is_some(), "cache survives db switches");
        a.update(key(KeyCode::Char('[')));
        assert_eq!(a.selected, 2, "[ wraps backwards");
    }

    #[test]
    fn view_switches_fetch_only_when_the_cache_is_empty() {
        let mut a = app(1);
        let effects = a.set_view(View::Queries);
        assert_eq!(a.dbs[0].view, View::Queries);
        assert!(matches!(
            effects.as_slice(),
            [Effect::Spawn {
                cmd: PgbotCommand::InspectFull,
                kind: CmdKind::Inspect,
                ..
            }]
        ));
        // Same-kind job in flight → Tables (also Context-backed) spawns nothing.
        let effects = a.set_view(View::Tables);
        assert_eq!(a.dbs[0].view, View::Tables);
        assert!(effects.is_empty(), "no duplicate identical jobs");

        a.update(Action::CheckFinished {
            db: 0,
            kind: CmdKind::Inspect,
            result: ok_ctx(HEALTHY),
        });
        let effects = a.set_view(View::Inspect);
        assert!(effects.is_empty(), "cached Context serves Inspect too");

        let effects = a.set_view(View::Indexes);
        assert!(matches!(
            effects.as_slice(),
            [Effect::Spawn {
                cmd: PgbotCommand::Indexes,
                kind: CmdKind::Indexes,
                ..
            }]
        ));
    }

    #[test]
    fn refresh_dedupes_in_flight_jobs() {
        let mut a = app(1);
        let first = a.update(key(KeyCode::Char('r')));
        assert_eq!(first.len(), 1);
        let second = a.update(key(KeyCode::Char('r')));
        assert!(second.is_empty(), "refresh while running must not stack");
        a.update(Action::CheckFinished {
            db: 0,
            kind: CmdKind::Inspect,
            result: ok_ctx(HEALTHY),
        });
        let third = a.update(key(KeyCode::Char('r')));
        assert_eq!(third.len(), 1, "after completion refresh works again");
    }

    #[test]
    fn quit_keys() {
        let mut a = app(1);
        a.update(key(KeyCode::Char('q')));
        assert!(a.should_quit);

        let mut a = app(1);
        a.update(Action::Key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
        )));
        assert!(a.should_quit, "Ctrl+C quits from anywhere");
    }

    #[test]
    fn command_bar_captures_typed_characters() {
        let mut a = app(1);
        a.update(key(KeyCode::Char('/')));
        assert_eq!(a.focus, Focus::CommandBar);
        for c in ['q', '1', 'r', 'a'] {
            a.update(key(KeyCode::Char(c)));
        }
        assert!(!a.should_quit, "shortcuts must not fire while typing");
        assert_eq!(a.cmdline, "q1ra");
        assert_eq!(
            a.dbs[0].view,
            View::Inspect,
            "no view change from typed digits"
        );
        a.update(key(KeyCode::Esc));
        assert_eq!(a.focus, Focus::Main);
    }

    #[test]
    fn popup_captures_characters_and_esc_closes() {
        let mut a = app(1);
        a.update(key(KeyCode::Char('a')));
        assert_eq!(a.focus, Focus::Popup);
        for c in "prod".chars() {
            a.update(key(KeyCode::Char(c)));
        }
        // Name → Stage → Connection.
        a.update(key(KeyCode::Tab));
        a.update(key(KeyCode::Tab));
        for c in "P_URL".chars() {
            a.update(key(KeyCode::Char(c)));
        }
        let p = a.popup.as_ref().unwrap();
        assert_eq!((p.name.as_str(), p.env.as_str()), ("prod", "P_URL"));
        assert!(!a.should_quit);
        a.update(key(KeyCode::Esc));
        assert!(a.popup.is_none());
        assert_eq!(a.focus, Focus::Main);
    }

    #[test]
    fn help_opens_and_any_key_closes() {
        let mut a = app(1);
        a.update(key(KeyCode::Char('?')));
        assert_eq!(a.focus, Focus::Help);
        a.update(key(KeyCode::Char('x')));
        assert_eq!(a.focus, Focus::Main);
    }

    #[test]
    fn select_flag_starts_on_the_named_database() {
        let mut cfg = TerminalConfig::default();
        cfg.add("prod", "A").unwrap();
        cfg.add("staging", "B").unwrap();
        let a = App::new(&cfg, None, false, Some("staging"));
        assert_eq!(a.selected, 1);
        let a = App::new(&cfg, None, false, Some("nope"));
        assert_eq!(a.selected, 0, "unknown name falls back to the first tab");
    }

    fn popup_env_guard() -> std::sync::MutexGuard<'static, ()> {
        use std::sync::{Mutex, OnceLock};
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        match LOCK.get_or_init(|| Mutex::new(())).lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    #[test]
    fn popup_shortcut_keys_type_into_fields_not_the_app() {
        let mut a = app(1);
        a.update(key(KeyCode::Char('a')));
        for c in ['1', '5', 'q', 'r'] {
            a.update(key(KeyCode::Char(c)));
        }
        assert!(!a.should_quit);
        assert_eq!(
            a.dbs[0].view,
            View::Inspect,
            "digits typed, not view switches"
        );
        assert_eq!(a.popup.as_ref().unwrap().name, "15qr");
    }

    #[test]
    fn popup_rejects_unset_env_and_duplicate_names_before_probing() {
        let _g = popup_env_guard();
        let mut a = app(1); // db0 exists
        a.update(key(KeyCode::Char('a')));
        {
            let p = a.popup.as_mut().unwrap();
            p.name = "db0".into();
            p.env = "APP_TEST_URL_0".into();
        }
        let effects = a.popup_submit(true);
        assert!(effects.is_empty());
        let msg = a
            .popup
            .as_ref()
            .unwrap()
            .message
            .clone()
            .unwrap()
            .unwrap_err();
        assert!(msg.message.contains("already exists"), "{}", msg.message);

        {
            let p = a.popup.as_mut().unwrap();
            p.name = "fresh".into();
            p.env = "POPUP_UNSET_VAR".into();
            p.message = None;
        }
        std::env::remove_var("POPUP_UNSET_VAR");
        let effects = a.popup_submit(true);
        assert!(effects.is_empty(), "no probe without a resolvable env var");
        let msg = a
            .popup
            .as_ref()
            .unwrap()
            .message
            .clone()
            .unwrap()
            .unwrap_err();
        assert!(msg.message.contains("POPUP_UNSET_VAR"), "{}", msg.message);
    }

    #[test]
    fn popup_add_flow_appends_a_monitored_database() {
        let _g = popup_env_guard();
        let dir = std::env::temp_dir().join(format!("pgterm-popup-add-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("PGTERM_CONFIG", dir.join("config.toml"));
        std::env::set_var("POPUP_ADD_URL", "postgres://u:pw@mode-healthy/db");

        let mut a = app(1);
        a.update(key(KeyCode::Char('a')));
        {
            let p = a.popup.as_mut().unwrap();
            p.name = "staging".into();
            p.env = "POPUP_ADD_URL".into();
        }
        let effects = a.popup_submit(true);
        assert!(matches!(
            effects.as_slice(),
            [Effect::SpawnProbe { save: true, .. }]
        ));
        assert!(a.popup.as_ref().unwrap().busy);

        // Probe comes back OK → profile saved, tab appended and selected,
        // monitor spawned immediately.
        let effects = a.update(Action::ProbeFinished {
            name: "staging".into(),
            source: ConnSource::Env("POPUP_ADD_URL".into()),
            save: true,
            stage: None,
            persist_env: None,
            result: ok_ctx(HEALTHY),
        });
        assert!(a.popup.is_none());
        assert_eq!(a.dbs.len(), 2);
        assert_eq!(a.selected, 1);
        assert_eq!(a.dbs[1].profile.env, "POPUP_ADD_URL");
        assert!(matches!(
            effects.as_slice(),
            [Effect::Spawn {
                db: 1,
                kind: CmdKind::Monitor,
                ..
            }]
        ));
        let saved = std::fs::read_to_string(dir.join("config.toml")).unwrap();
        assert!(saved.contains("staging"), "{saved}");
        assert!(!saved.contains("postgres://"), "no DSN in config: {saved}");

        std::env::remove_var("PGTERM_CONFIG");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn popup_test_mode_reports_without_saving() {
        let _g = popup_env_guard();
        let mut a = app(1);
        a.update(key(KeyCode::Char('a')));
        {
            let p = a.popup.as_mut().unwrap();
            p.busy = true;
        }
        a.update(Action::ProbeFinished {
            name: "x".into(),
            source: ConnSource::Env("Y".into()),
            save: false,
            stage: None,
            persist_env: None,
            result: ok_ctx(HEALTHY),
        });
        let p = a.popup.as_ref().expect("popup stays open after Test");
        assert!(!p.busy);
        let msg = p.message.clone().unwrap().unwrap();
        assert!(msg.contains("PostgreSQL 17"), "{msg}");
        assert_eq!(a.dbs.len(), 1, "Test never saves");
    }

    #[test]
    fn probe_failure_shows_sanitized_error_in_popup() {
        let _g = popup_env_guard();
        let mut a = app(1);
        a.update(key(KeyCode::Char('a')));
        a.popup.as_mut().unwrap().busy = true;
        a.update(Action::ProbeFinished {
            name: "x".into(),
            source: ConnSource::Env("Y".into()),
            save: true,
            stage: None,
            persist_env: None,
            result: Err(SafeError::new(
                crate::sanitize::ErrorKind::ConnectionFailed,
                "connect postgres://u:sekret@h/db: refused",
                None,
            )),
        });
        let msg = a
            .popup
            .as_ref()
            .unwrap()
            .message
            .clone()
            .unwrap()
            .unwrap_err();
        assert!(!msg.message.contains("sekret"), "{}", msg.message);
        assert_eq!(a.dbs.len(), 1);
    }

    #[test]
    fn mouse_clicks_dispatch_through_the_hitmap() {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        use ratatui::layout::Rect;
        let mut a = app(3);
        a.hitmap = vec![
            (Rect::new(0, 0, 10, 1), Hit::SelectDb(2)),
            (Rect::new(20, 0, 10, 1), Hit::OpenAdd),
            (Rect::new(0, 28, 11, 1), Hit::SetView(View::Tables)),
        ];
        let click = |x: u16, y: u16| {
            Action::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: x,
                row: y,
                modifiers: KeyModifiers::NONE,
            })
        };
        a.update(click(5, 0));
        assert_eq!(a.selected, 2);
        a.update(click(3, 28));
        assert_eq!(a.dbs[2].view, View::Tables);
        a.update(click(25, 0));
        assert!(a.popup.is_some(), "clicking + Add DB opens the popup");
        a.update(click(70, 15));
        assert!(a.popup.is_some(), "a miss changes nothing");
    }

    #[test]
    fn arrow_keys_cycle_the_numbered_views_and_wrap() {
        let mut a = app(1);
        // Arrows only step through views on the PgBot tab.
        a.update(key(KeyCode::Char('2')));
        a.update(Action::CheckFinished {
            db: 0,
            kind: CmdKind::Monitor,
            result: ok_ctx(HEALTHY),
        });
        assert_eq!(a.dbs[0].view, View::Inspect);
        press_code(&mut a, KeyCode::Right);
        assert_eq!(a.dbs[0].view, View::Queries);
        press_code(&mut a, KeyCode::Right);
        assert_eq!(a.dbs[0].view, View::Indexes);
        press_code(&mut a, KeyCode::Left);
        press_code(&mut a, KeyCode::Left);
        assert_eq!(a.dbs[0].view, View::Inspect);
        press_code(&mut a, KeyCode::Left);
        assert_eq!(a.dbs[0].view, View::Why, "Left from Inspect wraps to Why");
        press_code(&mut a, KeyCode::Right);
        assert_eq!(a.dbs[0].view, View::Inspect, "Right from Why wraps back");

        // Stepping onto an unfetched view spawns its job, same as 1-5 keys.
        let effects = a.update(key(KeyCode::Right));
        assert_eq!(a.dbs[0].view, View::Queries);
        assert!(effects.is_empty(), "Context is cached — no refetch");
        press_code(&mut a, KeyCode::Right);
        let db = &a.dbs[0];
        assert!(
            db.running.contains(&CmdKind::Indexes),
            "Indexes fetch spawned"
        );

        // From Ask (unnumbered), either arrow lands on Inspect.
        a.dbs[0].view = View::Ask;
        press_code(&mut a, KeyCode::Left);
        assert_eq!(a.dbs[0].view, View::Inspect);
    }

    fn press_code(a: &mut App, code: KeyCode) {
        a.update(key(code));
    }

    #[test]
    fn pasted_url_becomes_a_session_only_tab_and_never_touches_config() {
        let _g = popup_env_guard();
        let dir = std::env::temp_dir().join(format!("pgterm-session-add-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("PGTERM_CONFIG", dir.join("config.toml"));

        let mut a = app(1);
        a.update(key(KeyCode::Char('a')));
        {
            let p = a.popup.as_mut().unwrap();
            p.name = "pasted".into();
            p.env = "postgres://alex:hunter2@db/app".into();
        }
        let effects = a.popup_submit(true);
        match effects.as_slice() {
            [Effect::SpawnProbe {
                source: ConnSource::Session(url),
                save: true,
                ..
            }] => assert!(url.contains("hunter2"), "the secret resolves in memory"),
            other => panic!("expected a session probe, got {other:?}"),
        }

        let effects = a.update(Action::ProbeFinished {
            name: "pasted".into(),
            source: ConnSource::Session("postgres://alex:hunter2@db/app".into()),
            save: true,
            stage: None,
            persist_env: None,
            result: ok_ctx(HEALTHY),
        });
        assert!(a.popup.is_none());
        assert_eq!(a.dbs.len(), 2);
        assert_eq!(a.selected, 1);
        assert!(matches!(a.dbs[1].source, ConnSource::Session(_)));
        assert!(matches!(
            effects.as_slice(),
            [Effect::Spawn {
                db: 1,
                kind: CmdKind::Monitor,
                ..
            }]
        ));
        assert!(
            !dir.join("config.toml").exists(),
            "a session tab must never be written to disk"
        );

        std::env::remove_var("PGTERM_CONFIG");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pasted_url_with_a_bad_name_is_refused_without_echo() {
        let _g = popup_env_guard();
        let mut a = app(1);
        a.update(key(KeyCode::Char('a')));
        {
            let p = a.popup.as_mut().unwrap();
            p.name = "has space".into();
            p.env = "postgres://alex:hunter2@db/app".into();
        }
        let effects = a.popup_submit(true);
        assert!(effects.is_empty());
        let msg = a
            .popup
            .as_ref()
            .unwrap()
            .message
            .clone()
            .unwrap()
            .unwrap_err();
        assert!(msg.message.contains("letters"), "{}", msg.message);
        assert!(!msg.message.contains("hunter2"), "{}", msg.message);
    }

    #[test]
    fn paste_routes_to_the_focused_input_and_strips_control_chars() {
        let mut a = app(1);
        // Command bar
        a.update(key(KeyCode::Char('/')));
        a.update(Action::Paste("inspect\n".into()));
        assert_eq!(a.cmdline, "inspect", "newline stripped — no auto-submit");
        a.update(key(KeyCode::Esc));
        // Popup env field
        a.update(key(KeyCode::Char('a')));
        a.update(key(KeyCode::Tab));
        a.update(key(KeyCode::Tab));
        a.update(Action::Paste("postgres://u:pw@h/db\n".into()));
        assert_eq!(a.popup.as_ref().unwrap().env, "postgres://u:pw@h/db");
        // Unfocused: ignored (Esc keeps the bar's text, so compare before/after)
        a.update(key(KeyCode::Esc));
        let before = a.cmdline.clone();
        a.update(Action::Paste("stray".into()));
        assert_eq!(
            a.cmdline, before,
            "paste with nothing focused must be inert"
        );
    }

    #[test]
    fn export_assignment_parses_name_and_url() {
        for input in [
            "STAGING_DATABASE_URL='postgresql://u:pw@h/db'",
            "STAGING_DATABASE_URL=\"postgresql://u:pw@h/db\"",
            "STAGING_DATABASE_URL=postgresql://u:pw@h/db",
            "export STAGING_DATABASE_URL='postgresql://u:pw@h/db'",
        ] {
            let (var, url) = parse_export_assignment(input).unwrap_or_else(|| panic!("{input}"));
            assert_eq!(var, "STAGING_DATABASE_URL");
            assert_eq!(url, "postgresql://u:pw@h/db");
        }
        // Not assignments: bare URL, bare name, keyword DSN value.
        assert!(parse_export_assignment("postgres://u:pw@h/db").is_none());
        assert!(parse_export_assignment("STAGING_DATABASE_URL").is_none());
        assert!(parse_export_assignment("X='host=h password=y'").is_none());
        assert!(parse_export_assignment("BAD NAME='postgres://h/db'").is_none());
    }

    #[test]
    fn pasted_assignment_connects_now_and_persists_only_the_name() {
        let _g = popup_env_guard();
        let dir = std::env::temp_dir().join(format!("pgterm-assign-add-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("PGTERM_CONFIG", dir.join("config.toml"));

        let mut a = app(1);
        a.update(key(KeyCode::Char('a')));
        {
            let p = a.popup.as_mut().unwrap();
            p.name = "staging".into();
            p.env = "STAGING_DATABASE_URL='postgres://alex:hunter2@db/app'".into();
        }
        let effects = a.popup_submit(true);
        match effects.as_slice() {
            [Effect::SpawnProbe {
                source: ConnSource::Session(url),
                persist_env: Some(var),
                save: true,
                ..
            }] => {
                assert_eq!(url, "postgres://alex:hunter2@db/app", "quotes stripped");
                assert_eq!(var, "STAGING_DATABASE_URL");
            }
            other => panic!("expected assignment probe, got {other:?}"),
        }

        a.update(Action::ProbeFinished {
            name: "staging".into(),
            source: ConnSource::Session("postgres://alex:hunter2@db/app".into()),
            save: true,
            stage: None,
            persist_env: Some("STAGING_DATABASE_URL".into()),
            result: ok_ctx(HEALTHY),
        });
        assert_eq!(a.dbs.len(), 2);
        assert!(
            matches!(a.dbs[1].source, ConnSource::Session(_)),
            "this session uses the URL"
        );
        assert_eq!(a.dbs[1].profile.env, "STAGING_DATABASE_URL");

        let saved = std::fs::read_to_string(dir.join("config.toml")).unwrap();
        assert!(saved.contains("env = \"STAGING_DATABASE_URL\""), "{saved}");
        assert!(
            !saved.contains("postgres://"),
            "URL leaked to disk: {saved}"
        );
        assert!(
            !saved.contains("hunter2"),
            "password leaked to disk: {saved}"
        );

        std::env::remove_var("PGTERM_CONFIG");
        let _ = std::fs::remove_dir_all(&dir);
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
        a.update(Action::CheckFinished {
            db: 1,
            kind: CmdKind::Monitor,
            result: ok_ctx(HEALTHY),
        });
        assert!(!a.dbs[1].pgbot_changed(), "first result is not 'changed'");
        a.update(key(KeyCode::Char(']')));
        a.update(key(KeyCode::Char('2')));
        assert!(!a.dbs[1].pgbot_changed());
        a.update(key(KeyCode::Char('[')));
        a.update(Action::CheckFinished {
            db: 1,
            kind: CmdKind::Monitor,
            result: ok_ctx(WARN),
        });
        assert!(a.dbs[1].pgbot_changed(), "new findings while not viewing");
        a.update(key(KeyCode::Char(']')));
        assert!(
            a.dbs[1].pgbot_changed(),
            "selecting the database is not viewing the tab"
        );
        a.update(key(KeyCode::Char('2')));
        assert!(!a.dbs[1].pgbot_changed());
    }

    #[test]
    fn toast_fires_for_an_unselected_database_turning_critical_not_for_recovery_or_self() {
        let mut a = app(2);
        a.ui.bell = true;
        a.update(Action::CheckFinished {
            db: 1,
            kind: CmdKind::Monitor,
            result: ok_ctx(HEALTHY),
        });
        assert!(a.active_toast().is_none());
        a.update(Action::CheckFinished {
            db: 1,
            kind: CmdKind::Monitor,
            result: ok_ctx(CRITICAL),
        });
        let t = a.active_toast().expect("toast");
        assert!(
            t.text.contains("critical") && t.text.contains("[ to open"),
            "{}",
            t.text
        );
        assert!(a.take_bell());
        assert!(!a.take_bell(), "bell is consumed");
        a.update(Action::CheckFinished {
            db: 1,
            kind: CmdKind::Monitor,
            result: ok_ctx(HEALTHY),
        });
        a.toast = None;
        a.update(Action::CheckFinished {
            db: 0,
            kind: CmdKind::Monitor,
            result: ok_ctx(CRITICAL),
        });
        assert!(
            a.active_toast().is_none(),
            "the selected database never toasts"
        );
        a.update(Action::CheckFinished {
            db: 1,
            kind: CmdKind::Monitor,
            result: err(),
        });
        assert!(a.active_toast().unwrap().text.contains("unavailable"));
        a.toast.as_mut().unwrap().until = Instant::now() - Duration::from_secs(1);
        assert!(a.active_toast().is_none(), "expired");
    }

    #[test]
    fn palette_opens_filters_runs_and_closes() {
        let mut a = named_app(&["production", "staging"]);
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
        // Free text that parses as a verb runs as typed.
        a.update(key(KeyCode::Char(':')));
        for c in "ask why is it slow".chars() {
            a.update(key(KeyCode::Char(c)));
        }
        let effects = a.update(key(KeyCode::Enter));
        assert_eq!(effects.len(), 1);
        assert_eq!(a.dbs[1].view, View::Ask);
        a.update(key(KeyCode::Char(':')));
        for c in "zzzz".chars() {
            a.update(key(KeyCode::Char(c)));
        }
        a.update(key(KeyCode::Enter));
        assert_eq!(
            a.focus,
            Focus::Palette,
            "unknown input stays open with an error"
        );
        assert!(a.cmd_error.is_some());
    }

    #[test]
    fn popup_stage_field_cycles_and_is_saved() {
        let _g = popup_env_guard();
        std::env::set_var("STAGE_TEST_URL", "postgres://x@mode-healthy.local/db");
        let dir = std::env::temp_dir().join(format!("pgterm-stage-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::env::set_var("PGTERM_CONFIG", dir.join("config.toml"));

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
        // Typing into a cycle field must not become text.
        a.update(key(KeyCode::Char('x')));
        assert_eq!(a.popup.as_ref().unwrap().stage, Some(Stage::Prod));

        a.update(key(KeyCode::Tab));
        assert_eq!(a.popup.as_ref().unwrap().field, PopupField::Env);
        for c in "STAGE_TEST_URL".chars() {
            a.update(key(KeyCode::Char(c)));
        }
        let effects = a.update(key(KeyCode::Enter));
        assert!(
            matches!(
                effects.as_slice(),
                [Effect::SpawnProbe {
                    stage: Some(Stage::Prod),
                    ..
                }]
            ),
            "{effects:?}"
        );
        a.update(Action::ProbeFinished {
            name: "warehouse".into(),
            source: ConnSource::Env("STAGE_TEST_URL".into()),
            save: true,
            stage: Some(Stage::Prod),
            persist_env: None,
            result: ok_ctx(HEALTHY),
        });
        assert_eq!(a.dbs[0].profile.stage, Some(Stage::Prod));
        assert_eq!(a.dbs[0].profile.badge(), Some(Stage::Prod));

        std::env::remove_var("STAGE_TEST_URL");
        std::env::remove_var("PGTERM_CONFIG");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn motion_marks_what_is_clickable_without_acting() {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        let mut a = app(2);
        // The draw pass owns the hitmap; fake one region for the test.
        a.hitmap = vec![
            (Rect::new(0, 0, 10, 1), Hit::SelectDb(1)),
            (Rect::new(0, 1, 10, 1), Hit::OpenAdd),
        ];
        let moved = |col, row| {
            Action::Mouse(MouseEvent {
                kind: MouseEventKind::Moved,
                column: col,
                row,
                modifiers: KeyModifiers::NONE,
            })
        };
        assert!(a.hover.is_none());
        let effects = a.update(moved(3, 0));
        assert_eq!(a.hover, Some(Hit::SelectDb(1)));
        assert!(effects.is_empty(), "hovering must never act");
        assert_eq!(a.selected, 0, "and must not select");
        a.update(moved(3, 1));
        assert_eq!(a.hover, Some(Hit::OpenAdd));
        assert!(a.popup.is_none(), "hovering the add row opens nothing");
        a.update(moved(50, 9));
        assert!(a.hover.is_none(), "off every region clears the hover");

        // A real click still acts.
        a.update(Action::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 3,
            row: 0,
            modifiers: KeyModifiers::NONE,
        }));
        assert_eq!(a.selected, 1);
    }
}
