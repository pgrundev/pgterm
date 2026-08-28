//! The non-TUI entry points: `pgterm add|list|remove` and argument
//! parsing for the binary. `add` validates the connection through a real
//! pgbot probe BEFORE anything is persisted — a broken profile is never saved.

use crate::config::TerminalConfig;
use crate::model::Context;
use crate::runner::{self, ConnSource, PgbotCommand};
use crate::sanitize::ErrorKind;

pub const EXIT_OK: i32 = 0;
pub const EXIT_FAILED: i32 = 1;
pub const EXIT_NO_SOURCE: i32 = 2;
pub const EXIT_USAGE: i32 = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddOptions {
    pub name: String,
    pub env: Option<String>,
    pub open: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invocation {
    Tui {
        interval_seconds: Option<u64>,
        no_monitor: bool,
        select: Option<String>,
    },
    Add(AddOptions),
    List,
    Remove(String),
    Version,
    Usage(String),
}

const USAGE: &str = "usage: pgterm [--interval <dur>] [--no-monitor]
       pgterm add <name> [--env <ENV_NAME>] [--open]
       pgterm list
       pgterm remove <name>";

pub fn parse_args(args: &[String]) -> Invocation {
    let mut it = args.iter().peekable();
    match it.peek().map(|s| s.as_str()) {
        Some("--version") | Some("-v") => Invocation::Version,
        Some("add") => {
            it.next();
            let mut name = None;
            let mut env = None;
            let mut open = false;
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--env" => match it.next() {
                        Some(v) => env = Some(v.clone()),
                        None => return Invocation::Usage("--env needs a value".into()),
                    },
                    "--open" => open = true,
                    s if s.starts_with('-') => {
                        return Invocation::Usage(format!("unknown flag {s}"))
                    }
                    s if name.is_none() => name = Some(s.to_string()),
                    s => return Invocation::Usage(format!("unexpected argument {s}")),
                }
            }
            match name {
                Some(name) => Invocation::Add(AddOptions { name, env, open }),
                None => Invocation::Usage("add needs a database name".into()),
            }
        }
        Some("list") => {
            it.next();
            match it.next() {
                None => Invocation::List,
                Some(s) => Invocation::Usage(format!("unexpected argument {s}")),
            }
        }
        Some("remove") => {
            it.next();
            match (it.next(), it.next()) {
                (Some(name), None) => Invocation::Remove(name.clone()),
                (Some(_), Some(s)) => Invocation::Usage(format!("unexpected argument {s}")),
                (None, _) => Invocation::Usage("remove needs a database name".into()),
            }
        }
        _ => {
            let mut interval_seconds = None;
            let mut no_monitor = false;
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--interval" => match it.next().map(|v| parse_duration_secs(v)) {
                        Some(Some(s)) if s > 0 => interval_seconds = Some(s),
                        _ => {
                            return Invocation::Usage(
                                "--interval needs a duration like 30s or 2m".into(),
                            )
                        }
                    },
                    "--no-monitor" => no_monitor = true,
                    s => return Invocation::Usage(format!("unknown argument {s}")),
                }
            }
            Invocation::Tui {
                interval_seconds,
                no_monitor,
                select: None,
            }
        }
    }
}

/// "30s" / "2m" / "1h" / bare seconds ("45") → seconds.
pub fn parse_duration_secs(s: &str) -> Option<u64> {
    let s = s.trim();
    let (num, mult) = match s.chars().last()? {
        's' => (&s[..s.len() - 1], 1),
        'm' => (&s[..s.len() - 1], 60),
        'h' => (&s[..s.len() - 1], 3600),
        c if c.is_ascii_digit() => (s, 1),
        _ => return None,
    };
    num.parse::<u64>().ok().map(|n| n * mult)
}

pub fn print_usage_error(msg: &str) -> i32 {
    eprintln!("pgterm: {msg}\n{USAGE}");
    EXIT_USAGE
}

const NO_SOURCE_HELP: &str = "No PostgreSQL connection found.

Set DATABASE_URL:

  export DATABASE_URL='postgresql://...'
  pgterm add production

Or reference another environment variable:

  pgterm add production --env PROD_DATABASE_URL";

fn env_is_set(name: &str) -> bool {
    std::env::var(name)
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
}

/// `pgterm add`. Returns the process exit code; EXIT_OK means the
/// profile was validated and saved (the caller may then open the TUI).
pub async fn cmd_add(opts: &AddOptions) -> i32 {
    // Resolve the connection SOURCE (an env-var name, never a value).
    let (env_name, discovered) = match &opts.env {
        Some(e) => (e.clone(), false),
        None => {
            if env_is_set("DATABASE_URL") {
                ("DATABASE_URL".to_string(), true)
            } else if env_is_set("PGBOT_DATABASE_URL") {
                ("PGBOT_DATABASE_URL".to_string(), true)
            } else {
                eprintln!("{NO_SOURCE_HELP}");
                return EXIT_NO_SOURCE;
            }
        }
    };

    // Validate the profile shape before doing any work.
    let mut cfg = match TerminalConfig::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("pgterm: {e:#}");
            return EXIT_FAILED;
        }
    };
    if let Err(e) = cfg.clone().add(&opts.name, &env_name) {
        eprintln!("pgterm: {e}");
        eprintln!("Nothing was saved.");
        return EXIT_FAILED;
    }

    if !env_is_set(&env_name) {
        eprintln!("Environment variable {env_name} is not set.");
        eprintln!("Nothing was saved.");
        return EXIT_NO_SOURCE;
    }
    if discovered {
        println!("✓ Found {env_name}");
    }

    println!("Testing {}...\n", opts.name);
    let probe = runner::run_pgbot(
        &runner::pgbot_bin(),
        &ConnSource::Env(env_name.clone()),
        &PgbotCommand::Probe,
        runner::default_timeout(&PgbotCommand::Probe),
    )
    .await;

    let outcome = match probe {
        Ok(o) => o,
        Err(e) => {
            eprintln!("Unable to add {}.\n", opts.name);
            match e.kind {
                ErrorKind::ConnectionFailed | ErrorKind::Timeout => {
                    eprintln!("Connection failed:\n{}\n", e.message)
                }
                // PgbotMissing needs no special arm: Display itself carries
                // the install hint, for the TUI surfaces as much as here.
                _ => eprintln!("{e}\n"),
            }
            eprintln!("Nothing was saved.");
            return EXIT_FAILED;
        }
    };

    match Context::decode(&outcome.stdout) {
        Ok(ctx) => {
            if ctx.server.major() > 0 {
                println!("✓ PostgreSQL {}", ctx.server.major());
            }
            println!("✓ Connection successful");
            if ctx.server.has_pg_monitor {
                println!("✓ Read-only diagnostics available");
            } else {
                println!(
                    "· pg_monitor not granted — some checks will be limited (pgbot init --verify)"
                );
            }
        }
        Err(_) => {
            // The connection worked (exit 0/1/2) even if the document didn't
            // parse — an older pgbot. Still a valid profile.
            println!("✓ Connection successful");
        }
    }

    if let Err(e) = cfg.add(&opts.name, &env_name) {
        eprintln!("pgterm: {e}\nNothing was saved.");
        return EXIT_FAILED;
    }
    if let Err(e) = cfg.save() {
        eprintln!("pgterm: {e:#}\nNothing was saved.");
        return EXIT_FAILED;
    }

    println!("\nAdded {}", opts.name);
    if !opts.open {
        println!("\nOpen with:\n\n  pgterm");
    }
    EXIT_OK
}

pub fn cmd_list() -> i32 {
    let cfg = match TerminalConfig::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("pgterm: {e:#}");
            return EXIT_FAILED;
        }
    };
    if cfg.databases.is_empty() {
        println!("No databases added yet.\n");
        println!("Add your current database:\n\n  pgterm add production");
        println!("\nOr:\n\n  pgterm add production --env PROD_DATABASE_URL");
        return EXIT_OK;
    }
    let name_w = cfg
        .databases
        .iter()
        .map(|d| d.name.len())
        .chain(["NAME".len()])
        .max()
        .unwrap_or(4);
    let env_w = cfg
        .databases
        .iter()
        .map(|d| d.env.len())
        .chain(["CONNECTION".len()])
        .max()
        .unwrap_or(10);
    println!("{:name_w$}  {:env_w$}  STATUS", "NAME", "CONNECTION");
    for d in &cfg.databases {
        let status = if env_is_set(&d.env) {
            "configured"
        } else {
            "env not set"
        };
        println!("{:name_w$}  {:env_w$}  {status}", d.name, d.env);
    }
    EXIT_OK
}

pub fn cmd_remove(name: &str) -> i32 {
    let mut cfg = match TerminalConfig::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("pgterm: {e:#}");
            return EXIT_FAILED;
        }
    };
    if let Err(e) = cfg.remove(name) {
        eprintln!("pgterm: {e}");
        return EXIT_FAILED;
    }
    if let Err(e) = cfg.save() {
        eprintln!("pgterm: {e:#}");
        return EXIT_FAILED;
    }
    println!("Removed local profile \"{name}\".");
    println!("PostgreSQL was not modified.");
    EXIT_OK
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parse_add_variants() {
        assert_eq!(
            parse_args(&s(&["add", "prod"])),
            Invocation::Add(AddOptions {
                name: "prod".into(),
                env: None,
                open: false
            })
        );
        assert_eq!(
            parse_args(&s(&["add", "prod", "--env", "PROD_URL", "--open"])),
            Invocation::Add(AddOptions {
                name: "prod".into(),
                env: Some("PROD_URL".into()),
                open: true
            })
        );
        assert!(matches!(parse_args(&s(&["add"])), Invocation::Usage(_)));
        assert!(matches!(
            parse_args(&s(&["add", "p", "--env"])),
            Invocation::Usage(_)
        ));
        assert!(matches!(
            parse_args(&s(&["add", "p", "--url", "x"])),
            Invocation::Usage(_)
        ));
    }

    #[test]
    fn parse_tui_flags() {
        assert_eq!(
            parse_args(&[]),
            Invocation::Tui {
                interval_seconds: None,
                no_monitor: false,
                select: None
            }
        );
        assert_eq!(
            parse_args(&s(&["--interval", "30s", "--no-monitor"])),
            Invocation::Tui {
                interval_seconds: Some(30),
                no_monitor: true,
                select: None
            }
        );
        assert!(matches!(
            parse_args(&s(&["--interval"])),
            Invocation::Usage(_)
        ));
        assert!(matches!(
            parse_args(&s(&["--interval", "soon"])),
            Invocation::Usage(_)
        ));
        assert!(matches!(
            parse_args(&s(&["frobnicate"])),
            Invocation::Usage(_)
        ));
    }

    #[test]
    fn parse_list_remove_version() {
        assert_eq!(parse_args(&s(&["list"])), Invocation::List);
        assert_eq!(
            parse_args(&s(&["remove", "staging"])),
            Invocation::Remove("staging".into())
        );
        assert!(matches!(parse_args(&s(&["remove"])), Invocation::Usage(_)));
        assert_eq!(parse_args(&s(&["--version"])), Invocation::Version);
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration_secs("30s"), Some(30));
        assert_eq!(parse_duration_secs("2m"), Some(120));
        assert_eq!(parse_duration_secs("1h"), Some(3600));
        assert_eq!(parse_duration_secs("45"), Some(45));
        assert_eq!(parse_duration_secs("soon"), None);
        assert_eq!(parse_duration_secs(""), None);
    }
}
