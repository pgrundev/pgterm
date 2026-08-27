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

use crate::action::{Action, CmdKind, Effect, Hit, StoredResult, View};
use crate::config::{DatabaseProfile, TerminalConfig};
use crate::health::{self, HealthStatus};
use crate::model::{Context, IndexesReport, WhyReport};
use crate::runner::{self, PgbotCommand, RunOutcome};
use crate::sanitize::SafeError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Main,
    CommandBar,
    Popup,
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopupField {
    Name,
    Env,
}

/// The Add Database popup. Field values are what the user typed — the env
/// var's VALUE is never resolved here, only its name is kept.
#[derive(Debug, Clone)]
pub struct AddPopup {
    pub name: String,
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
    pub health: HealthStatus,
    pub last_ok: Option<Instant>,
    pub last_checked: Option<Instant>,
    pub view: View,
    pub ctx: Option<Context>,
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
        DbState {
            profile,
            health: HealthStatus::Checking,
            last_ok: None,
            last_checked: None,
            view: View::Inspect,
            ctx: None,
            indexes: None,
            why: None,
            ask_output: None,
            running: HashSet::new(),
            error: None,
            attention: false,
            scroll: HashMap::new(),
        }
    }

    pub fn has_data(&self, view: View) -> bool {
        match view {
            View::Inspect | View::Queries | View::Tables => self.ctx.is_some(),
            View::Indexes => self.indexes.is_some(),
            View::Why => self.why.is_some(),
            View::Ask => self.ask_output.is_some(),
        }
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
            version_note: None,
        }
    }

    pub fn selected_db(&self) -> Option<&DbState> {
        self.dbs.get(self.selected)
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
            Action::ProbeFinished {
                name,
                env,
                save,
                result,
            } => self.on_probe_finished(&name, &env, save, result),
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
        match result {
            Ok(StoredResult::Ctx(ctx)) => {
                state.health = health::overall(&ctx);
                state.ctx = Some(*ctx);
                state.last_ok = Some(Instant::now());
                state.error = None;
                if db != selected
                    && matches!(state.health, HealthStatus::Warning | HealthStatus::Critical)
                {
                    state.attention = true;
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
                    state.health = HealthStatus::Unavailable;
                    if db != selected {
                        state.attention = true;
                    }
                }
                state.error = Some(e);
            }
        }
    }

    fn on_probe_finished(
        &mut self,
        name: &str,
        env: &str,
        save: bool,
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
                // Persist through the same validated path as the CLI.
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
                if let Err(e) = cfg.add(name, env).and_then(|()| cfg.save()) {
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
                    env: env.to_string(),
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
            Focus::Main => self.handle_main_key(key),
        }
    }

    fn handle_main_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        match key.code {
            KeyCode::Char('q') => {
                self.should_quit = true;
                Vec::new()
            }
            KeyCode::Tab => {
                self.select_db(self.next_db(1));
                Vec::new()
            }
            KeyCode::BackTab => {
                self.select_db(self.next_db(-1));
                Vec::new()
            }
            KeyCode::Char('/') => {
                self.focus = Focus::CommandBar;
                self.cmd_error = None;
                Vec::new()
            }
            KeyCode::Char('a') => {
                self.popup = Some(AddPopup::default());
                self.focus = Focus::Popup;
                Vec::new()
            }
            KeyCode::Char('?') => {
                self.focus = Focus::Help;
                Vec::new()
            }
            KeyCode::Char('r') => self.refresh_selected(),
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll_by(-1);
                Vec::new()
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll_by(1);
                Vec::new()
            }
            KeyCode::Char(c) => {
                if let Some((_, view, _)) = View::NUMBERED.iter().find(|(n, _, _)| *n == c) {
                    self.set_view(*view)
                } else {
                    Vec::new()
                }
            }
            _ => Vec::new(),
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
        use crate::parser::{parse, UserCommand};
        match parse(&self.cmdline) {
            Err(msg) => {
                self.cmd_error = Some(msg);
                Vec::new()
            }
            Ok(cmd) => {
                self.cmdline.clear();
                self.cmd_error = None;
                self.focus = Focus::Main;
                match cmd {
                    UserCommand::Inspect => self.set_view(View::Inspect),
                    UserCommand::Queries => self.set_view(View::Queries),
                    UserCommand::Indexes => self.set_view(View::Indexes),
                    UserCommand::Tables => self.set_view(View::Tables),
                    UserCommand::Why => self.set_view(View::Why),
                    UserCommand::Refresh => self.refresh_selected(),
                    UserCommand::Ask(q) => self.spawn_ask(q),
                }
            }
        }
    }

    /// `ask <question>`: switches to the Ask view and runs `pgbot ask --yes`
    /// with the question as one argv element. One ask per database at a time.
    fn spawn_ask(&mut self, question: String) -> Vec<Effect> {
        let selected = self.selected;
        let Some(db) = self.dbs.get_mut(selected) else {
            return Vec::new();
        };
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
            KeyCode::Tab | KeyCode::Up | KeyCode::Down => {
                popup.field = match popup.field {
                    PopupField::Name => PopupField::Env,
                    PopupField::Env => PopupField::Name,
                };
                Vec::new()
            }
            KeyCode::Backspace => {
                match popup.field {
                    PopupField::Name => popup.name.pop(),
                    PopupField::Env => popup.env.pop(),
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
        let env = popup.env.trim().to_string();
        // Same validation the CLI applies, before any subprocess runs.
        let mut probe_cfg = TerminalConfig::load().unwrap_or_default();
        for d in &self.dbs {
            // The in-memory tab list is the truth the user sees; a name that
            // collides with it must fail even if the file lags.
            if !probe_cfg.databases.iter().any(|p| p.name == d.profile.name) {
                let _ = probe_cfg.add(&d.profile.name, &d.profile.env);
            }
        }
        if let Err(e) = probe_cfg.add(&name, &env) {
            popup.message = Some(Err(SafeError::new(
                crate::sanitize::ErrorKind::Usage,
                &e.to_string(),
                None,
            )));
            return Vec::new();
        }
        if std::env::var(&env)
            .map(|v| v.trim().is_empty())
            .unwrap_or(true)
        {
            popup.message = Some(Err(SafeError::new(
                crate::sanitize::ErrorKind::EnvMissing,
                &format!("Environment variable {env} is not set."),
                None,
            )));
            return Vec::new();
        }
        popup.busy = true;
        popup.message = None;
        vec![Effect::SpawnProbe { name, env, save }]
    }

    fn handle_mouse(&mut self, m: MouseEvent) -> Vec<Effect> {
        if !matches!(m.kind, MouseEventKind::Down(_)) {
            return Vec::new();
        }
        let hit = self
            .hitmap
            .iter()
            .find(|(r, _)| {
                m.column >= r.x
                    && m.column < r.x + r.width
                    && m.row >= r.y
                    && m.row < r.y + r.height
            })
            .map(|(_, h)| h.clone());
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

    /// `r`: rerun the selected database's current view. Never duplicates an
    /// identical in-flight job.
    pub fn refresh_selected(&mut self) -> Vec<Effect> {
        let selected = self.selected;
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

/// Performs one Spawn effect: waits on the concurrency semaphore, runs pgbot,
/// decodes by command, and returns the Action to feed back into `update`.
pub async fn run_effect(
    pgbot_bin: PathBuf,
    env_var: String,
    db: usize,
    cmd: PgbotCommand,
    kind: CmdKind,
    sem: Arc<Semaphore>,
) -> Action {
    let _permit = sem.acquire_owned().await.ok();
    let timeout = runner::default_timeout(&cmd);
    let result = runner::run_pgbot(&pgbot_bin, &env_var, &cmd, timeout)
        .await
        .and_then(|out| decode_result(&cmd, &out));
    Action::CheckFinished { db, kind, result }
}

/// Performs one SpawnProbe effect for the add popup.
pub async fn run_probe(
    pgbot_bin: PathBuf,
    name: String,
    env: String,
    save: bool,
    sem: Arc<Semaphore>,
) -> Action {
    let _permit = sem.acquire_owned().await.ok();
    let cmd = PgbotCommand::Probe;
    let result = runner::run_pgbot(&pgbot_bin, &env, &cmd, runner::default_timeout(&cmd))
        .await
        .and_then(|out| decode_result(&cmd, &out));
    Action::ProbeFinished {
        name,
        env,
        save,
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

        // Selecting the flagged tab clears the flag.
        a.update(key(KeyCode::Tab));
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
    fn tab_cycles_and_wraps_and_state_survives() {
        let mut a = app(3);
        a.update(Action::CheckFinished {
            db: 0,
            kind: CmdKind::Monitor,
            result: ok_ctx(HEALTHY),
        });
        a.dbs[0].view = View::Indexes;
        a.update(key(KeyCode::Tab));
        a.update(key(KeyCode::Tab));
        assert_eq!(a.selected, 2);
        a.update(key(KeyCode::Tab));
        assert_eq!(a.selected, 0, "Tab wraps");
        assert_eq!(a.dbs[0].view, View::Indexes, "view survives tab switches");
        assert!(a.dbs[0].ctx.is_some(), "cache survives tab switches");
        a.update(key(KeyCode::BackTab));
        assert_eq!(a.selected, 2, "Shift+Tab wraps backwards");
    }

    #[test]
    fn number_keys_map_to_views_and_fetch_only_when_empty() {
        let mut a = app(1);
        let effects = a.update(key(KeyCode::Char('2')));
        assert_eq!(a.dbs[0].view, View::Queries);
        assert!(matches!(
            effects.as_slice(),
            [Effect::Spawn {
                cmd: PgbotCommand::InspectFull,
                kind: CmdKind::Inspect,
                ..
            }]
        ));
        // Same-kind job in flight → pressing 4 (also Context-backed) spawns nothing.
        let effects = a.update(key(KeyCode::Char('4')));
        assert_eq!(a.dbs[0].view, View::Tables);
        assert!(effects.is_empty(), "no duplicate identical jobs");

        a.update(Action::CheckFinished {
            db: 0,
            kind: CmdKind::Inspect,
            result: ok_ctx(HEALTHY),
        });
        let effects = a.update(key(KeyCode::Char('1')));
        assert!(effects.is_empty(), "cached Context serves Inspect too");

        let effects = a.update(key(KeyCode::Char('3')));
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
            env: "POPUP_ADD_URL".into(),
            save: true,
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
            env: "Y".into(),
            save: false,
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
            env: "Y".into(),
            save: true,
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
}
