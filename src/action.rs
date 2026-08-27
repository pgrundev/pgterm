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
    /// A popup-driven probe finished (the database does not exist yet).
    ProbeFinished {
        name: String,
        source: ConnSource,
        save: bool,
        /// For a pasted NAME='URL' assignment: the variable NAME to persist
        /// in config while the URL itself stays session-only in memory.
        persist_env: Option<String>,
        result: Result<StoredResult, SafeError>,
    },
    /// Bracketed paste from the terminal, routed to the focused input.
    Paste(String),
    Quit,
}

/// Side effects `update` asks the runtime to perform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Spawn {
        db: usize,
        cmd: PgbotCommand,
        kind: CmdKind,
    },
    SpawnProbe {
        name: String,
        source: ConnSource,
        save: bool,
        persist_env: Option<String>,
    },
}

/// Regions the draw pass registers for mouse hit-testing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hit {
    SelectDb(usize),
    OpenAdd,
    SetView(View),
    PopupTest,
    PopupAdd,
    PopupCancel,
}
