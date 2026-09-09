//! The Data browser: schemas, then that schema's tables, then a page of rows.
//! Read-only whatever the profile allows — browsing is never a way to change
//! something.

use ratatui::layout::{Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::action::Hit;
use crate::app::{DataLevel, DbState};
use crate::screens::sql::result_lines;
use crate::ui::hover_style;

/// The trail at the top: where you are and how you got there.
pub fn breadcrumb(db: &DbState) -> String {
    let schema = db
        .data_schemas
        .as_ref()
        .and_then(|s| s.get(db.data_schema_cursor))
        .cloned();
    let table = db
        .data_tables
        .as_ref()
        .and_then(|t| t.get(db.data_table_cursor))
        .map(|(n, _, _)| n.clone());
    match (db.data_level, schema, table) {
        (DataLevel::Schemas, _, _) => "schemas".into(),
        (DataLevel::Tables, Some(s), _) => format!("schemas › {s}"),
        (DataLevel::Rows, Some(s), Some(t)) => format!("schemas › {s} › {t}"),
        _ => "schemas".into(),
    }
}

pub fn draw(f: &mut Frame, area: Rect, db: &DbState, hover: Option<&Hit>) -> Vec<(Rect, Hit)> {
    let dim = Style::default().fg(Color::DarkGray);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let inner = area.inner(Margin::new(1, 0));
    let mut hits = Vec::new();

    let mut lines = vec![
        Line::from(vec![
            Span::styled("DATA", bold),
            Span::raw(format!("   {}", breadcrumb(db))),
            Span::styled("   Enter opens · Esc goes back · r reloads", dim),
        ]),
        Line::from(""),
    ];

    if let Some(e) = &db.data_error {
        lines.push(Line::from(Span::styled(
            format!(" {e}"),
            Style::default().fg(Color::Red),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(" [r] retry", dim)));
        f.render_widget(Paragraph::new(lines), inner);
        return hits;
    }
    if db.data_loading {
        lines.push(Line::from(Span::styled(
            " Loading…",
            Style::default().fg(Color::Cyan),
        )));
        f.render_widget(Paragraph::new(lines), inner);
        return hits;
    }

    let row_style = |i: usize, cursor: usize, h: Option<&Hit>, hit: Hit| {
        let base = if i == cursor {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };
        hover_style(base, h == Some(&hit))
    };

    match db.data_level {
        DataLevel::Schemas => match db.data_schemas.as_deref() {
            None => lines.push(Line::from(Span::styled(" Press r to load.", dim))),
            Some([]) => lines.push(Line::from(Span::styled(" No schemas.", dim))),
            Some(schemas) => {
                lines.push(Line::from(Span::styled(format!("  {:<40}", "SCHEMA"), dim)));
                for (i, name) in schemas.iter().enumerate() {
                    let y = inner.y + lines.len() as u16;
                    lines.push(Line::from(Span::styled(
                        format!("  {name:<40}"),
                        row_style(i, db.data_schema_cursor, hover, Hit::SelectSchema(i)),
                    )));
                    if y < inner.bottom() {
                        hits.push((Rect::new(inner.x, y, inner.width, 1), Hit::SelectSchema(i)));
                    }
                }
            }
        },
        DataLevel::Tables => match db.data_tables.as_deref() {
            None => lines.push(Line::from(Span::styled(" Loading…", dim))),
            Some([]) => lines.push(Line::from(Span::styled(" No tables in this schema.", dim))),
            Some(tables) => {
                lines.push(Line::from(Span::styled(
                    format!("  {:<40} {:>14} {:>12}", "TABLE", "~ROWS", "SIZE"),
                    dim,
                )));
                for (i, (name, rows, size)) in tables.iter().enumerate() {
                    let y = inner.y + lines.len() as u16;
                    lines.push(Line::from(Span::styled(
                        format!("  {name:<40} {rows:>14} {size:>12}"),
                        row_style(i, db.data_table_cursor, hover, Hit::SelectTable(i)),
                    )));
                    if y < inner.bottom() {
                        hits.push((Rect::new(inner.x, y, inner.width, 1), Hit::SelectTable(i)));
                    }
                }
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "  Row counts are the planner's estimate, not a COUNT(*).",
                    dim,
                )));
            }
        },
        DataLevel::Rows => match &db.data_rows {
            None => lines.push(Line::from(Span::styled(" Loading…", dim))),
            Some(r) => lines.extend(result_lines(r, inner.width as usize)),
        },
    }
    f.render_widget(Paragraph::new(lines), inner);
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DatabaseProfile;

    fn db() -> DbState {
        DbState::new(DatabaseProfile {
            name: "prod".into(),
            env: "P_URL".into(),
            stage: None,
            pgrun_project: None,
            writes: false,
        })
    }

    #[test]
    fn the_breadcrumb_tracks_the_level() {
        let mut d = db();
        assert_eq!(breadcrumb(&d), "schemas");
        d.data_schemas = Some(vec!["public".into(), "billing".into()]);
        d.data_schema_cursor = 1;
        d.data_level = DataLevel::Tables;
        assert_eq!(breadcrumb(&d), "schemas › billing");
        d.data_tables = Some(vec![("orders".into(), "10".into(), "8 kB".into())]);
        d.data_level = DataLevel::Rows;
        assert_eq!(breadcrumb(&d), "schemas › billing › orders");
    }

    #[test]
    fn the_breadcrumb_never_claims_a_level_it_cannot_name() {
        let mut d = db();
        d.data_level = DataLevel::Tables;
        assert_eq!(breadcrumb(&d), "schemas", "no schema loaded yet");
        d.data_level = DataLevel::Rows;
        assert_eq!(breadcrumb(&d), "schemas");
    }
}
