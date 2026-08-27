// pgterm — htop for all your Postgres databases, powered by pgbot.
// Presentation/orchestration only: every diagnostic runs the pgbot CLI
// (PGBOT_BIN override, else `pgbot` on PATH) and renders its JSON. No SQL lives here.

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use pgterm::cli::{self, Invocation};

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
            let rt = runtime();
            let code = rt.block_on(cli::cmd_add(&opts));
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

fn run_tui(_interval: Option<u64>, _no_monitor: bool, _select: Option<String>) -> i32 {
    let mut terminal = ratatui::init();
    let result = run(&mut terminal);
    ratatui::restore();
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("pgterm: {e:#}");
            1
        }
    }
}

fn run(terminal: &mut ratatui::DefaultTerminal) -> anyhow::Result<()> {
    loop {
        terminal.draw(|frame| {
            frame.render_widget("pgterm — starting… (press q to quit)", frame.area());
        })?;
        if let Event::Key(key) = event::read()? {
            if !key.is_press() {
                continue;
            }
            let ctrl_c =
                key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL);
            if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) || ctrl_c {
                return Ok(());
            }
        }
    }
}
