//! The event/action vocabulary. Crossterm events and background-task results
//! both become Actions; `App::update` consumes Actions and emits Effects; the
//! runtime performs Effects (spawning pgbot children) and feeds the results
//! back as Actions. State mutation happens in exactly one place.

use crossterm::event::{KeyEvent, MouseEvent};

use crate::model::{Context, IndexesReport, WhyReport};
use crate::runner::{ConnSource, PgbotCommand};
use crate::sanitize::SafeError;

/// The per-database screens. Ask is reachable only through the command bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum View {
    Inspect,
    Queries,
    Indexes,
    Tables,
    Why,
    Ask,
}

impl View {
    /// The shortcut row: number key ↔ view.
    pub const NUMBERED: [(char, View, &'static str); 5] = [
        ('1', View::Inspect, "Inspect"),
        ('2', View::Queries, "Queries"),
        ('3', View::Indexes, "Indexes"),
        ('4', View::Tables, "Tables"),
        ('5', View::Why, "Why"),
    ];
}

/// Top-level tabs of the main pane. One current tab per database, so
/// switching databases returns you where you were.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tab {
    Overview,
    PgBot,
    Sql,
    Data,
    Branches,
}

impl Tab {
    /// The tab row: number key ↔ tab.
    pub const NUMBERED: [(char, Tab, &'static str); 5] = [
        ('1', Tab::Overview, "Overview"),
        ('2', Tab::PgBot, "PgBot"),
        ('3', Tab::Sql, "SQL"),
        ('4', Tab::Data, "Data"),
        ('5', Tab::Branches, "Branches"),
    ];
}

/// Which pane holds keyboard focus while `Focus::Main`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Sidebar,
    Main,
}

/// What kind of background job is (or was) running for a database — the
/// dedupe key: one job of a kind per database at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CmdKind {
    Monitor,
    Inspect,
    Indexes,
    Why,
    Ask,
}

#[derive(Debug, Clone)]
pub enum StoredResult {
    Ctx(Box<Context>),
    Indexes(Box<IndexesReport>),
    Why(Box<WhyReport>),
    Text(String),
}

#[derive(Debug, Clone)]
pub enum Action {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Resize(u16, u16),
    /// The monitor cadence fired: sweep every database not already checking.
    MonitorTick,
    CheckFinished {
        db: usize,
        kind: CmdKind,
        result: Result<StoredResult, SafeError>,
    },
    /// A SQL run finished (the SQL tab, or one of the Data browser's queries).
    SqlFinished {
        db: usize,
        target: crate::action::SqlTarget,
        result: Result<Box<crate::db::QueryResult>, crate::sanitize::SafeError>,
    },
    /// A pgrun branch call finished for one database.
    BranchesFinished {
        db: usize,
        result: Result<Vec<crate::pgrun::Branch>, SafeError>,
    },
    /// `branch get` finished: the branch carries its connection URL.
    BranchOpened {
        db: usize,
        result: Result<Box<crate::pgrun::Branch>, SafeError>,
    },
    /// A popup-driven probe finished (the database does not exist yet).
    ProbeFinished {
        name: String,
        source: ConnSource,
        save: bool,
        /// The environment badge chosen in the popup; None = infer it.
        stage: Option<crate::config::Stage>,
        /// For a pasted NAME='URL' assignment: the variable NAME to persist
        /// in config while the URL itself stays session-only in memory.
        persist_env: Option<String>,
        result: Result<StoredResult, SafeError>,
    },
    /// Bracketed paste from the terminal, routed to the focused input.
    Paste(String),
    Quit,
}

/// Which surface asked for a SQL run, so its answer lands in the right place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SqlTarget {
    /// The SQL tab's editor.
    Editor,
    /// The Data browser's schema list.
    Schemas,
    /// The tables of one schema.
    Tables(String),
    /// One page of rows from a table.
    Rows { schema: String, table: String },
}

/// Side effects `update` asks the runtime to perform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Spawn {
        db: usize,
        cmd: PgbotCommand,
        kind: CmdKind,
    },
    /// Run SQL against a database's own connection.
    SpawnSql {
        db: usize,
        target: SqlTarget,
        sql: String,
        policy: crate::db::WritePolicy,
    },
    /// Ask pgrun for a database's branches, or for one branch's URL.
    SpawnPgrun {
        db: usize,
        cmd: crate::pgrun::PgrunCommand,
        /// Set when the answer should open the branch as a session tab.
        open: bool,
    },
    SpawnProbe {
        name: String,
        source: ConnSource,
        save: bool,
        stage: Option<crate::config::Stage>,
        persist_env: Option<String>,
    },
}

/// Regions the draw pass registers for mouse hit-testing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hit {
    SelectDb(usize),
    OpenAdd,
    SetView(View),
    SetTab(Tab),
    /// Row index within the selected database's branch list.
    SelectBranch(usize),
    /// A schema row in the Data browser.
    SelectSchema(usize),
    /// A table row in the Data browser.
    SelectTable(usize),
    OpenPalette,
    /// Row index within the palette's currently filtered list.
    PaletteItem(usize),
    PopupTest,
    PopupAdd,
    PopupCancel,
}
