//! The command palette: every action pgterm can take, filtered by a small
//! subsequence matcher. Items are built from the same verbs the command bar
//! parses, so the palette can never do something the bar cannot.

use crate::action::Tab;
use crate::parser::UserCommand;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteCmd {
    Verb(UserCommand),
    Tab(Tab),
    SwitchDb(usize),
    AddDb,
    Help,
    Quit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteItem {
    pub label: String,
    pub cmd: PaletteCmd,
}

#[derive(Debug, Clone, Default)]
pub struct PaletteState {
    pub input: String,
    /// Row within the filtered list, not an index into `items`.
    pub cursor: usize,
}

/// Switching database is the most common thing to reach for, so those items
/// lead: with an empty query every item scores 0 and declaration order wins.
pub fn items(db_names: &[&str]) -> Vec<PaletteItem> {
    let mut v: Vec<PaletteItem> = db_names
        .iter()
        .enumerate()
        .map(|(i, name)| PaletteItem {
            label: format!("switch to {name}"),
            cmd: PaletteCmd::SwitchDb(i),
        })
        .collect();
    let verbs = [
        ("refresh", PaletteCmd::Verb(UserCommand::Refresh)),
        ("overview tab", PaletteCmd::Tab(Tab::Overview)),
        ("pgbot tab", PaletteCmd::Tab(Tab::PgBot)),
        ("inspect", PaletteCmd::Verb(UserCommand::Inspect)),
        ("queries", PaletteCmd::Verb(UserCommand::Queries)),
        ("indexes", PaletteCmd::Verb(UserCommand::Indexes)),
        ("tables", PaletteCmd::Verb(UserCommand::Tables)),
        ("why", PaletteCmd::Verb(UserCommand::Why)),
        ("add database", PaletteCmd::AddDb),
        ("help", PaletteCmd::Help),
        ("quit", PaletteCmd::Quit),
    ];
    v.extend(verbs.into_iter().map(|(label, cmd)| PaletteItem {
        label: label.into(),
        cmd,
    }));
    v
}

/// Case-insensitive subsequence match. Higher is better: 3 per matched
/// character, +2 when it continues a run, +3 when it starts a word. `None`
/// when the query is not a subsequence at all; `Some(0)` for an empty query.
/// Spaces in the query are matched literally, so "sw st" reaches across words.
pub fn score(query: &str, label: &str) -> Option<i32> {
    let q: Vec<char> = query.to_lowercase().chars().collect();
    let l: Vec<char> = label.to_lowercase().chars().collect();
    if q.is_empty() {
        return Some(0);
    }
    let mut total = 0;
    let mut next = 0usize;
    let mut last: Option<usize> = None;
    for qc in q {
        let found = (next..l.len()).find(|&i| l[i] == qc)?;
        total += 3;
        if last.is_some_and(|p| p + 1 == found) {
            total += 2;
        }
        if found == 0 || l[found - 1] == ' ' {
            total += 3;
        }
        last = Some(found);
        next = found + 1;
    }
    Some(total)
}

/// Item indices that match, best first. Ties keep declaration order.
pub fn filter(items: &[PaletteItem], query: &str) -> Vec<usize> {
    let mut scored: Vec<(usize, i32)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, it)| score(query, &it.label).map(|s| (i, s)))
        .collect();
    // Reverse for best-first; sort_by_key is stable, so equal scores keep
    // declaration order (which is what puts the databases at the top).
    scored.sort_by_key(|&(_, score)| std::cmp::Reverse(score));
    scored.into_iter().map(|(i, _)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoring_prefers_prefix_then_word_starts_then_scattered() {
        let prefix = score("ref", "refresh").unwrap();
        let word = score("st", "switch to staging").unwrap();
        let scattered = score("sts", "switch to staging").unwrap();
        assert!(prefix > scattered, "{prefix} vs {scattered}");
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
        for want in [
            "refresh",
            "inspect",
            "queries",
            "indexes",
            "tables",
            "why",
            "overview tab",
            "pgbot tab",
            "add database",
            "help",
            "quit",
            "switch to a",
            "switch to b",
        ] {
            assert!(items.iter().any(|i| i.label == want), "missing {want}");
        }
    }
}
