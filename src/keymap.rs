//! The key table. `App::update` dispatches through `lookup`, and the `?`
//! overlay is rendered from the same table by `help_text`, so the help can
//! never describe a binding that does not exist.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::action::Tab;

/// Where a key applies. `lookup` tries the caller's contexts in order, then
/// falls back to Global — so `j` can move in the sidebar and scroll in the
/// main pane without either binding knowing about the other.
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

const fn b(
    keys: &'static [&'static str],
    context: KeyContext,
    action: KeyAction,
    help: &'static str,
) -> Binding {
    Binding {
        keys,
        context,
        action,
        help,
    }
}

pub static KEYMAP: &[Binding] = &[
    b(&["q"], KeyContext::Global, KeyAction::Quit, "quit"),
    b(&["?"], KeyContext::Global, KeyAction::Help, "help"),
    b(
        &["Tab", "S-Tab"],
        KeyContext::Main,
        KeyAction::TogglePane,
        "focus sidebar / main pane",
    ),
    b(
        &["["],
        KeyContext::Main,
        KeyAction::PrevDb,
        "previous database",
    ),
    b(&["]"], KeyContext::Main, KeyAction::NextDb, "next database"),
    b(
        &["1"],
        KeyContext::Main,
        KeyAction::SetTab(Tab::Overview),
        "overview tab",
    ),
    b(
        &["2"],
        KeyContext::Main,
        KeyAction::SetTab(Tab::PgBot),
        "pgbot tab",
    ),
    b(
        &["C-k", ":"],
        KeyContext::Main,
        KeyAction::Palette,
        "command palette",
    ),
    b(
        &["/"],
        KeyContext::Main,
        KeyAction::CommandBar,
        "command bar (verbs, ask …)",
    ),
    b(&["a"], KeyContext::Main, KeyAction::AddDb, "add database"),
    b(&["r"], KeyContext::Main, KeyAction::Refresh, "refresh"),
    b(
        &["j", "Down"],
        KeyContext::Main,
        KeyAction::Down,
        "scroll down / move down",
    ),
    b(
        &["k", "Up"],
        KeyContext::Main,
        KeyAction::Up,
        "scroll up / move up",
    ),
    b(
        &["Enter"],
        KeyContext::Sidebar,
        KeyAction::Enter,
        "open the selected database",
    ),
    b(
        &["Left", "h"],
        KeyContext::PgBot,
        KeyAction::PrevView,
        "previous pgbot view",
    ),
    b(
        &["Right", "l"],
        KeyContext::PgBot,
        KeyAction::NextView,
        "next pgbot view",
    ),
    b(
        &["Enter"],
        KeyContext::Overview,
        KeyAction::Enter,
        "open pgbot findings",
    ),
];

/// The canonical name of a key event, in the spelling KEYMAP uses.
pub fn key_name(key: &KeyEvent) -> Option<String> {
    let base = match key.code {
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Tab => "Tab".to_string(),
        // Shift+Tab arrives as its own code, with the modifier already folded in.
        KeyCode::BackTab => return Some("S-Tab".to_string()),
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Esc => "Esc".to_string(),
        KeyCode::Left => "Left".to_string(),
        KeyCode::Right => "Right".to_string(),
        KeyCode::Up => "Up".to_string(),
        KeyCode::Down => "Down".to_string(),
        KeyCode::PageUp => "PgUp".to_string(),
        KeyCode::PageDown => "PgDn".to_string(),
        KeyCode::Backspace => "Backspace".to_string(),
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
    let order = contexts
        .iter()
        .copied()
        .chain(std::iter::once(KeyContext::Global));
    for ctx in order {
        if let Some(binding) = KEYMAP
            .iter()
            .find(|b| b.context == ctx && b.keys.contains(&name.as_str()))
        {
            return Some(binding.action);
        }
    }
    None
}

/// The `?` overlay body, grouped by context. Generated, never hand-written.
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
        for binding in KEYMAP.iter().filter(|b| b.context == ctx) {
            out.push_str(&format!(
                "{:<18} {}\n",
                binding.keys.join(" / "),
                binding.help
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn every_binding_has_help_and_no_context_binds_a_key_twice() {
        let mut seen = std::collections::HashSet::new();
        for binding in KEYMAP {
            assert!(!binding.help.is_empty(), "{:?} has no help", binding.keys);
            for key in binding.keys {
                assert!(
                    seen.insert((binding.context, *key)),
                    "{key} bound twice in {:?}",
                    binding.context
                );
            }
        }
    }

    #[test]
    fn every_tab_has_a_number_binding() {
        for (ch, tab, _) in Tab::NUMBERED {
            assert_eq!(
                lookup(&[KeyContext::Main], &k(KeyCode::Char(ch))),
                Some(KeyAction::SetTab(tab))
            );
        }
    }

    #[test]
    fn key_names_cover_modifiers_and_specials() {
        assert_eq!(key_name(&k(KeyCode::Char('q'))).as_deref(), Some("q"));
        assert_eq!(key_name(&k(KeyCode::BackTab)).as_deref(), Some("S-Tab"));
        assert_eq!(
            key_name(&KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL)).as_deref(),
            Some("C-k")
        );
        assert_eq!(key_name(&k(KeyCode::Left)).as_deref(), Some("Left"));
        assert_eq!(key_name(&k(KeyCode::Enter)).as_deref(), Some("Enter"));
    }

    #[test]
    fn context_order_decides_and_global_is_the_fallback() {
        // j in the sidebar moves; in the main pane it scrolls; both are Down.
        assert_eq!(
            lookup(
                &[KeyContext::Sidebar, KeyContext::Main],
                &k(KeyCode::Char('j'))
            ),
            Some(KeyAction::Down)
        );
        // Left only means "previous view" on the PgBot tab.
        assert_eq!(
            lookup(&[KeyContext::PgBot, KeyContext::Main], &k(KeyCode::Left)),
            Some(KeyAction::PrevView)
        );
        assert_eq!(
            lookup(&[KeyContext::Overview, KeyContext::Main], &k(KeyCode::Left)),
            None
        );
        assert_eq!(
            lookup(&[KeyContext::Main], &k(KeyCode::Char('q'))),
            Some(KeyAction::Quit)
        );
        assert_eq!(
            lookup(&[KeyContext::Main], &k(KeyCode::Char('['))),
            Some(KeyAction::PrevDb)
        );
        assert_eq!(
            lookup(
                &[KeyContext::Main],
                &KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL)
            ),
            Some(KeyAction::Palette)
        );
    }

    #[test]
    fn help_text_lists_every_key() {
        let text = help_text();
        for binding in KEYMAP {
            for key in binding.keys {
                assert!(text.contains(key), "help is missing {key}:\n{text}");
            }
            assert!(
                text.contains(binding.help),
                "help is missing {:?}",
                binding.help
            );
        }
    }

    /// The README documents these keys; a table that drifts from the code is
    /// worse than no table.
    #[test]
    fn readme_key_table_matches_the_keymap() {
        let readme = include_str!("../README.md");
        let table = readme
            .split("## Keys")
            .nth(1)
            .expect("README has a Keys section");
        for binding in KEYMAP {
            let keys = binding.keys.join(" / ");
            assert!(
                table.contains(&keys),
                "README key table is missing {keys:?}"
            );
            assert!(
                table.contains(binding.help),
                "README key table is missing {:?}",
                binding.help
            );
        }
    }
}
