//! The command bar's grammar: a closed set of verbs, parsed into an enum.
//! Nothing here ever reaches a shell — the enum maps to argv built by the
//! runner. Anything outside the whitelist is an error, including shell
//! metacharacters, SQL, and pgbot subcommands the terminal does not expose.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserCommand {
    Inspect,
    Queries,
    Indexes,
    Tables,
    Why,
    Refresh,
    Ask(String),
}

const KNOWN: &str = "inspect, queries, indexes, tables, why, refresh, ask <question>";

pub fn parse(input: &str) -> Result<UserCommand, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(format!("try: {KNOWN}"));
    }
    let mut words = trimmed.split_whitespace();
    let verb = words.next().unwrap_or_default().to_ascii_lowercase();
    let rest: Vec<&str> = words.collect();

    let bare = |cmd: UserCommand| {
        if rest.is_empty() {
            Ok(cmd)
        } else {
            Err(format!("`{verb}` takes no arguments"))
        }
    };

    match verb.as_str() {
        "inspect" => bare(UserCommand::Inspect),
        "queries" => bare(UserCommand::Queries),
        "indexes" => bare(UserCommand::Indexes),
        "tables" => bare(UserCommand::Tables),
        "why" => bare(UserCommand::Why),
        "refresh" => bare(UserCommand::Refresh),
        "ask" => {
            if rest.is_empty() {
                Err("ask needs a question: ask why did checkout get slower?".into())
            } else {
                Ok(UserCommand::Ask(rest.join(" ")))
            }
        }
        _ => Err(format!("unknown command `{verb}` — try: {KNOWN}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_whitelist_parses() {
        assert_eq!(parse("inspect"), Ok(UserCommand::Inspect));
        assert_eq!(parse("QUERIES"), Ok(UserCommand::Queries));
        assert_eq!(parse("  indexes  "), Ok(UserCommand::Indexes));
        assert_eq!(parse("tables"), Ok(UserCommand::Tables));
        assert_eq!(parse("why"), Ok(UserCommand::Why));
        assert_eq!(parse("refresh"), Ok(UserCommand::Refresh));
    }

    #[test]
    fn ask_keeps_its_question_verbatim() {
        assert_eq!(
            parse("ask why did checkout get slower?"),
            Ok(UserCommand::Ask("why did checkout get slower?".into()))
        );
        // Hostile text is DATA inside Ask — it becomes one argv element in
        // the runner, never a shell string.
        assert_eq!(
            parse("ask $(whoami); rm -rf /"),
            Ok(UserCommand::Ask("$(whoami); rm -rf /".into()))
        );
        assert!(parse("ask").is_err(), "ask without a question");
    }

    #[test]
    fn shells_editors_and_sql_are_rejected() {
        for bad in [
            "",
            "   ",
            "rm -rf /",
            "bash",
            "zsh",
            "sh -c ls",
            "psql",
            "curl http://x",
            "DROP DATABASE x",
            "drop database x",
            "SELECT 1",
            "select * from users",
            "$(whoami)",
            "`id`",
            "vacuum full",
            "history",
        ] {
            assert!(parse(bad).is_err(), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn verbs_take_no_arguments() {
        for bad in [
            "inspect; rm -rf /",
            "inspect --raw-query-text",
            "queries now",
            "refresh please",
            "why 5",
        ] {
            assert!(parse(bad).is_err(), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn errors_name_the_whitelist() {
        let err = parse("frobnicate").unwrap_err();
        assert!(err.contains("inspect"), "{err}");
        assert!(err.contains("ask"), "{err}");
    }
}
