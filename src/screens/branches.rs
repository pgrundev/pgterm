//! The Branches tab: this database's pgrun branches, and the sidebar section
//! that mirrors them. pgterm only reads and opens; creating and deleting
//! branches stays in the pgrun CLI, where the confirmations already live.

use ratatui::layout::{Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::action::Hit;
use crate::app::DbState;
use crate::pgrun::Branch;
use crate::ui::hover_style;

/// Glyph and tone per branch status: shape first, colour second.
pub fn branch_glyph(b: &Branch) -> (&'static str, Color) {
    if b.failed() {
        ("!", Color::Red)
    } else if b.in_progress() {
        ("◌", Color::Cyan)
    } else if b.is_base {
        ("●", Color::Blue)
    } else {
        ("●", Color::Green)
    }
}

/// "18m", "2h", "30d" from an RFC3339 timestamp — how long the branch has
/// existed, which is what you actually scan the list for.
pub fn age(created_at: Option<&str>) -> String {
    let Some(t) = created_at.and_then(crate::format::parse_rfc3339) else {
        return "—".into();
    };
    match std::time::SystemTime::now().duration_since(t) {
        Ok(d) => crate::format::duration_short(d.as_secs() as i64),
        Err(_) => "now".into(),
    }
}

/// What the tab (or sidebar) should say when there is nothing to list.
pub fn empty_reason(db: &DbState) -> Option<String> {
    if db.profile.pgrun_project.is_none() {
        return Some(format!(
            "No pgrun project set for {}. Add one to config.toml:\n\n  \
             [[databases]]\n  name = \"{}\"\n  pgrun_project = \"<slug>\"\n\n\
             Then press r. `pgrun project list` shows your projects.",
            db.profile.name, db.profile.name
        ));
    }
    if let Some(e) = &db.branch_error {
        return Some(format!("{e}\n\n[r] retry"));
    }
    if db.branches_loading && db.branches.is_none() {
        return Some("Asking pgrun for branches…".into());
    }
    match db.branches.as_deref() {
        Some([]) => Some("No branches yet.\n\n  pgrun branch create <project> --name <n>".into()),
        None => Some("Press r to load branches.".into()),
        _ => None,
    }
}

pub fn draw(f: &mut Frame, area: Rect, db: &DbState, hover: Option<&Hit>) -> Vec<(Rect, Hit)> {
    let dim = Style::default().fg(Color::DarkGray);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let inner = area.inner(Margin::new(1, 0));
    let mut hits = Vec::new();

    if let Some(msg) = empty_reason(db) {
        let mut lines = vec![Line::from(Span::styled("BRANCHES", bold)), Line::from("")];
        lines.extend(
            msg.lines()
                .map(|l| Line::from(Span::styled(l.to_string(), dim))),
        );
        f.render_widget(Paragraph::new(lines), inner);
        return hits;
    }
    let branches = db.branches.as_deref().unwrap_or_default();

    let mut lines = vec![
        Line::from(vec![
            Span::styled("BRANCHES", bold),
            Span::raw(format!("   {}", branches.len())),
            Span::styled("   Enter open · r refresh", dim),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            // Two leading spaces: the rows start with a glyph and a space.
            format!(
                "  {:<26} {:<10} {:<14} {:<8} {}",
                "NAME", "STATUS", "PARENT", "PG", "AGE"
            ),
            dim,
        )),
    ];
    let by_id = |id: &Option<String>| -> String {
        let Some(id) = id else { return "—".into() };
        branches
            .iter()
            .find(|b| &b.id == id)
            .map(|b| b.name.clone())
            .unwrap_or_else(|| id.clone())
    };
    for (i, b) in branches.iter().enumerate() {
        let y = inner.y + lines.len() as u16;
        let (glyph, tone) = branch_glyph(b);
        let mut style = Style::default();
        if i == db.branch_cursor {
            style = style.add_modifier(Modifier::REVERSED);
        }
        let style = hover_style(style, hover == Some(&Hit::SelectBranch(i)));
        let name = if b.is_base {
            format!("{} (base)", b.name)
        } else {
            b.name.clone()
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{glyph} "), Style::default().fg(tone)),
            Span::styled(
                format!(
                    "{:<26} {:<10} {:<14} {:<8} {}",
                    truncate(&name, 26),
                    truncate(&b.status, 10),
                    truncate(&by_id(&b.parent_branch_id), 14),
                    truncate(&b.postgres_version, 8),
                    age(b.created_at.as_deref())
                ),
                style,
            ),
        ]));
        if y < inner.bottom() {
            hits.push((Rect::new(inner.x, y, inner.width, 1), Hit::SelectBranch(i)));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " Opening a branch adds it as a session tab — its URL stays in memory,",
        dim,
    )));
    lines.push(Line::from(Span::styled(
        " never in config. Create and delete branches with the pgrun CLI.",
        dim,
    )));
    f.render_widget(Paragraph::new(lines), inner);
    hits
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n.saturating_sub(1)).collect::<String>() + "…"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DatabaseProfile;

    fn db_with(project: Option<&str>, branches: Option<Vec<Branch>>) -> DbState {
        let mut db = DbState::new(DatabaseProfile {
            name: "prod".into(),
            env: "P_URL".into(),
            stage: None,
            pgrun_project: project.map(String::from),
            writes: false,
            ssh: None,
        });
        db.branches = branches;
        db
    }

    fn branch(name: &str, status: &str) -> Branch {
        Branch {
            id: format!("branch_{name}"),
            name: name.into(),
            status: status.into(),
            ..Default::default()
        }
    }

    #[test]
    fn empty_reason_explains_every_way_the_list_can_be_missing() {
        let no_project = db_with(None, None);
        let msg = empty_reason(&no_project).unwrap();
        assert!(msg.contains("pgrun_project"), "{msg}");
        assert!(msg.contains("pgrun project list"), "{msg}");

        assert!(empty_reason(&db_with(Some("p"), None))
            .unwrap()
            .contains("Press r"));
        assert!(empty_reason(&db_with(Some("p"), Some(vec![])))
            .unwrap()
            .contains("No branches yet"));
        assert!(
            empty_reason(&db_with(Some("p"), Some(vec![branch("main", "ready")]))).is_none(),
            "a populated list has no placeholder"
        );

        let mut loading = db_with(Some("p"), None);
        loading.branches_loading = true;
        assert!(empty_reason(&loading).unwrap().contains("Asking pgrun"));

        let mut failed = db_with(Some("p"), None);
        failed.branch_error = Some(crate::sanitize::SafeError::new(
            crate::sanitize::ErrorKind::PgrunFailed,
            "not logged in",
            None,
        ));
        let msg = empty_reason(&failed).unwrap();
        assert!(
            msg.contains("not logged in") && msg.contains("[r] retry"),
            "{msg}"
        );
    }

    #[test]
    fn glyphs_differ_by_shape_per_status() {
        assert_eq!(branch_glyph(&branch("x", "failed")).0, "!");
        assert_eq!(branch_glyph(&branch("x", "creating")).0, "◌");
        let mut base = branch("main", "ready");
        base.is_base = true;
        assert_eq!(branch_glyph(&base).0, "●");
        assert_eq!(branch_glyph(&branch("x", "ready")).0, "●");
    }

    #[test]
    fn age_reads_the_timestamp_or_says_it_cannot() {
        assert_eq!(age(None), "—");
        assert_eq!(age(Some("nonsense")), "—");
        assert_eq!(age(Some("2020-01-01T00:00:00Z")).chars().last(), Some('d'));
    }
}
