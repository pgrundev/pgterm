//! The SQL tab: an editor above, the result grid below, and the write policy
//! stated on screen so it is never a surprise.

use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::DbState;
use crate::db::{QueryResult, WritePolicy};

/// The one-line policy banner. Text, not colour, carries the meaning.
pub fn policy_line(policy: WritePolicy) -> (&'static str, Color) {
    match policy {
        WritePolicy::ReadOnly => ("read only", Color::Green),
        WritePolicy::ConfirmWrites => ("writes on · PROD asks first", Color::Yellow),
        WritePolicy::Writes => ("writes on", Color::Yellow),
    }
}

/// Column widths that fit the pane: every column gets at least its header,
/// and the rest is shared out by the widest cell each actually needs.
pub fn column_widths(result: &QueryResult, available: usize) -> Vec<usize> {
    if result.columns.is_empty() {
        return Vec::new();
    }
    let mut want: Vec<usize> = result
        .columns
        .iter()
        .map(|c| c.chars().count().max(4))
        .collect();
    for row in &result.rows {
        for (i, cell) in row.iter().enumerate() {
            if let Some(w) = want.get_mut(i) {
                *w = (*w).max(cell.chars().count()).min(40);
            }
        }
    }
    let gaps = want.len().saturating_sub(1);
    let total: usize = want.iter().sum::<usize>() + gaps;
    if total <= available {
        return want;
    }
    // Too wide: shrink the widest first until it fits, never below 4.
    let mut widths = want;
    let mut over = total - available;
    while over > 0 {
        let Some((i, _)) = widths
            .iter()
            .enumerate()
            .max_by_key(|(_, w)| **w)
            .filter(|(_, w)| **w > 4)
        else {
            break;
        };
        widths[i] -= 1;
        over -= 1;
    }
    widths
}

fn fit(s: &str, w: usize) -> String {
    let n = s.chars().count();
    if n <= w {
        format!("{s}{}", " ".repeat(w - n))
    } else if w <= 1 {
        "…".repeat(w)
    } else {
        s.chars().take(w - 1).collect::<String>() + "…"
    }
}

/// The result grid, shared by the SQL tab and the Data browser's row view.
pub fn result_lines(result: &QueryResult, width: usize) -> Vec<Line<'static>> {
    let dim = Style::default().fg(Color::DarkGray);
    let mut lines = Vec::new();
    if result.columns.is_empty() {
        lines.push(Line::from(Span::styled(
            format!(
                " {} · {} ms",
                result.tag.clone().unwrap_or_else(|| "done".into()),
                result.elapsed_ms
            ),
            dim,
        )));
        return lines;
    }
    let widths = column_widths(result, width.saturating_sub(2));
    let header: String = result
        .columns
        .iter()
        .zip(&widths)
        .map(|(c, w)| fit(c, *w))
        .collect::<Vec<_>>()
        .join(" ");
    lines.push(Line::from(Span::styled(
        format!(" {header}"),
        Style::default().add_modifier(Modifier::BOLD),
    )));
    let types: String = result
        .types
        .iter()
        .zip(&widths)
        .map(|(t, w)| fit(t, *w))
        .collect::<Vec<_>>()
        .join(" ");
    lines.push(Line::from(Span::styled(format!(" {types}"), dim)));
    for row in &result.rows {
        let text: String = row
            .iter()
            .zip(&widths)
            .map(|(c, w)| fit(c, *w))
            .collect::<Vec<_>>()
            .join(" ");
        lines.push(Line::from(format!(" {text}")));
    }
    let mut footer = format!(
        " {} row{} · {} ms",
        result.rows.len(),
        if result.rows.len() == 1 { "" } else { "s" },
        result.elapsed_ms
    );
    if result.truncated {
        footer.push_str(" · truncated — add a LIMIT to see the rest");
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(footer, dim)));
    lines
}

pub fn draw(f: &mut Frame, area: Rect, db: &DbState) {
    let dim = Style::default().fg(Color::DarkGray);
    let inner = area.inner(Margin::new(1, 0));
    let editor_h =
        (db.sql.lines().len() as u16 + 2).clamp(5, inner.height.saturating_sub(4).max(5));
    let [head, editor_area, result_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(editor_h),
        Constraint::Min(0),
    ])
    .areas(inner);

    let (policy_text, policy_color) = policy_line(db.profile.write_policy());
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("SQL", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("   "),
            Span::styled(policy_text, Style::default().fg(policy_color)),
            Span::styled("   F5 or Ctrl-Enter runs · Esc leaves the editor", dim),
        ])),
        head,
    );

    // The editor, with a block cursor drawn into the text.
    let mut lines: Vec<Line> = Vec::new();
    for (i, line) in db.sql.lines().iter().enumerate() {
        if i == db.sql.row {
            let before: String = line.chars().take(db.sql.col).collect();
            let at: String = line.chars().skip(db.sql.col).take(1).collect();
            let after: String = line.chars().skip(db.sql.col + 1).collect();
            lines.push(Line::from(vec![
                Span::raw(format!(" {before}")),
                Span::styled(
                    if at.is_empty() { " ".into() } else { at },
                    Style::default().add_modifier(Modifier::REVERSED),
                ),
                Span::raw(after),
            ]));
        } else {
            lines.push(Line::from(format!(" {line}")));
        }
    }
    if db.sql.is_empty() && db.sql.lines().len() == 1 && db.sql.lines()[0].is_empty() {
        lines = vec![Line::from(vec![
            Span::styled(" ", Style::default().add_modifier(Modifier::REVERSED)),
            Span::styled(
                "SELECT * FROM … then F5",
                dim.add_modifier(Modifier::ITALIC),
            ),
        ])];
    }
    f.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" query ")),
        editor_area,
    );

    // Confirmation takes over the result area: nothing runs behind it.
    if let Some(typed) = &db.sql_confirm {
        f.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    format!(" This writes to {}, a PROD database.", db.profile.name),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    format!(
                        " Type its name to run it, or Esc to go back: {}",
                        db.profile.name
                    ),
                    dim,
                )),
                Line::from(vec![
                    Span::raw(" > "),
                    Span::raw(typed.clone()),
                    Span::styled("█", Style::default().fg(Color::Gray)),
                ]),
            ]),
            result_area,
        );
        return;
    }

    let body: Vec<Line> = if db.sql_running {
        vec![Line::from(Span::styled(
            " Running…",
            Style::default().fg(Color::Cyan),
        ))]
    } else if let Some(e) = &db.sql_error {
        vec![
            Line::from(Span::styled(
                format!(" {e}"),
                Style::default().fg(Color::Red),
            )),
            Line::from(""),
            Line::from(Span::styled(" Fix the query and run it again.", dim)),
        ]
    } else if let Some(r) = &db.sql_result {
        result_lines(r, result_area.width as usize)
    } else {
        vec![Line::from(Span::styled(
            " Results appear here. Every statement runs in a transaction with a 30s timeout.",
            dim,
        ))]
    };
    f.render_widget(Paragraph::new(body).scroll((db.sql_scroll, 0)), result_area);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(cols: &[&str], rows: &[&[&str]]) -> QueryResult {
        QueryResult {
            columns: cols.iter().map(|s| s.to_string()).collect(),
            types: cols.iter().map(|_| "text".to_string()).collect(),
            rows: rows
                .iter()
                .map(|r| r.iter().map(|s| s.to_string()).collect())
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn policy_is_stated_in_words() {
        assert_eq!(policy_line(WritePolicy::ReadOnly).0, "read only");
        assert!(policy_line(WritePolicy::ConfirmWrites).0.contains("asks"));
        assert_eq!(policy_line(WritePolicy::Writes).0, "writes on");
    }

    #[test]
    fn columns_fit_the_pane_and_never_collapse() {
        let r = result(&["id", "a_very_long_column_name"], &[&["1", "x"]]);
        let w = column_widths(&r, 200);
        assert_eq!(w[0], 4, "a short column still gets room for its header");
        assert_eq!(w[1], "a_very_long_column_name".chars().count());

        let narrow = column_widths(&r, 20);
        assert!(narrow.iter().sum::<usize>() < 20, "{narrow:?}");
        assert!(narrow.iter().all(|w| *w >= 4), "never below 4: {narrow:?}");

        assert!(column_widths(&QueryResult::default(), 80).is_empty());
    }

    #[test]
    fn the_grid_shows_headers_types_counts_and_truncation() {
        let mut r = result(&["id", "name"], &[&["1", "alice"], &["2", "bob"]]);
        r.elapsed_ms = 12;
        let text = result_lines(&r, 60)
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("id") && text.contains("name"), "{text}");
        assert!(text.contains("text"), "types row: {text}");
        assert!(text.contains("alice") && text.contains("bob"), "{text}");
        assert!(text.contains("2 rows · 12 ms"), "{text}");
        assert!(!text.contains("truncated"), "{text}");

        r.truncated = true;
        let text = result_lines(&r, 60)
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("truncated"), "{text}");
    }

    #[test]
    fn a_statement_with_no_columns_shows_its_tag() {
        let r = QueryResult {
            tag: Some("UPDATE 3".into()),
            elapsed_ms: 5,
            ..Default::default()
        };
        let text = result_lines(&r, 60)[0]
            .spans
            .iter()
            .map(|s| s.content.to_string())
            .collect::<String>();
        assert!(text.contains("UPDATE 3") && text.contains("5 ms"), "{text}");
    }

    #[test]
    fn one_row_is_singular() {
        let r = result(&["a"], &[&["1"]]);
        let text = result_lines(&r, 40)
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("1 row ·"), "{text}");
    }
}
