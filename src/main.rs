// pgterm — htop for all your Postgres databases, powered by pgbot.
// Presentation/orchestration only: every diagnostic runs the pgbot CLI
// (PGBOT_BIN override, else `pgbot` on PATH) and renders its JSON. No SQL
// lives here.

use std::sync::Arc;

use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use tokio::sync::mpsc;
use tokio::sync::Semaphore;

use pgterm::action::{Action, Effect};
use pgterm::app::{self, App};
use pgterm::cli::{self, Invocation};
use pgterm::config::TerminalConfig;
use pgterm::{event, ui};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match cli::parse_args(&args) {
        Invocation::Version => {
            println!("pgterm {}", env!("CARGO_PKG_VERSION"));
            0
        }
        Invocation::Usage(msg) => cli::print_usage_error(&msg),
        Invocation::List => cli::cmd_list(),
        Invocation::Remove(name) => cli::cmd_remove(&name),
        Invocation::Add(opts) => {
            let code = runtime().block_on(cli::cmd_add(&opts));
            if code == cli::EXIT_OK && opts.open {
                run_tui(None, false, Some(opts.name.clone()))
            } else {
                code
            }
        }
        Invocation::Tui {
            interval_seconds,
            no_monitor,
            select,
        } => run_tui(interval_seconds, no_monitor, select),
    };
    std::process::exit(code);
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Runtime::new().expect("tokio runtime")
}

fn run_tui(interval: Option<u64>, no_monitor: bool, select: Option<String>) -> i32 {
    let cfg = match TerminalConfig::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("pgterm: {e:#}");
            return 1;
        }
    };
    let app = App::new(&cfg, interval, no_monitor, select.as_deref());

    let mut terminal = ratatui::init();
    // ratatui's panic hook restores raw mode + alt screen; chain mouse
    // capture teardown in front so a panic never leaves clicks captured.
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = crossterm::execute!(std::io::stdout(), DisableMouseCapture);
        prev(info);
    }));
    let _ = crossterm::execute!(std::io::stdout(), EnableMouseCapture);

    let result = runtime().block_on(event_loop(&mut terminal, app));

    let _ = crossterm::execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("pgterm: {e:#}");
            1
        }
    }
}

async fn event_loop(terminal: &mut ratatui::DefaultTerminal, mut app: App) -> anyhow::Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Action>();
    event::spawn_input_thread(tx.clone());

    // Background monitor cadence. The first sweep fires immediately.
    if app.monitor_enabled && !app.dbs.is_empty() {
        let mtx = tx.clone();
        let every = app.interval;
        tokio::spawn(async move {
            let mut iv = tokio::time::interval(every);
            iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                iv.tick().await;
                if mtx.send(Action::MonitorTick).is_err() {
                    break;
                }
            }
        });
    }

    let sem = Arc::new(Semaphore::new(app.max_concurrent));
    loop {
        terminal.draw(|f| ui::draw(f, &mut app))?;
        let Some(action) = rx.recv().await else {
            return Ok(());
        };
        let mut effects = app.update(action);
        // Coalesce whatever else is already queued before redrawing.
        while let Ok(a) = rx.try_recv() {
            effects.extend(app.update(a));
        }
        perform(&app, effects, &tx, &sem);
        if app.should_quit {
            return Ok(());
        }
    }
}

fn perform(
    app: &App,
    effects: Vec<Effect>,
    tx: &mpsc::UnboundedSender<Action>,
    sem: &Arc<Semaphore>,
) {
    for e in effects {
        match e {
            Effect::Spawn { db, cmd, kind } => {
                let Some(state) = app.dbs.get(db) else {
                    continue;
                };
                let env = state.profile.env.clone();
                let bin = app.pgbot_bin.clone();
                let tx = tx.clone();
                let sem = sem.clone();
                tokio::spawn(async move {
                    let _ = tx.send(app::run_effect(bin, env, db, cmd, kind, sem).await);
                });
            }
            Effect::SpawnProbe { name, env, save } => {
                let bin = app.pgbot_bin.clone();
                let tx = tx.clone();
                let sem = sem.clone();
                tokio::spawn(async move {
                    let _ = tx.send(app::run_probe(bin, name, env, save, sem).await);
                });
            }
        }
    }
}
