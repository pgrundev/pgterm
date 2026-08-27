//! The default per-database screen: pgbot's health dashboard — score,
//! category rows, summary — derived from the cached inspect Context.

use std::time::SystemTime;

use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Row, Table};
use ratatui::Frame;

use crate::app::DbState;
use crate::format;
use crate::health::{self, RowStatus};

pub fn status_style(s: RowStatus) -> Style {
    match s {
        RowStatus::Ok => Style::default().fg(Color::Green),
        RowStatus::Warn => Style::default().fg(Color::Yellow),
        RowStatus::Fail => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        RowStatus::Unknown => Style::default().fg(Color::DarkGray),
    }
}

pub fn draw(f: &mut Frame, area: Rect, db: &DbState) {
    let Some(ctx) = &db.ctx else {
        return;
    };
    let (rows, unmapped) = health::categories(ctx, SystemTime::now());
    let score = health::score(ctx);
    let dim = Style::default().fg(Color::DarkGray);

    let inner = area.inner(Margin::new(2, 1));
    let [header, _, score_row, _, table_area, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(rows.len() as u16),
        Constraint::Min(2),
    ])
    .areas(inner);

    // Header: name + server left, freshness right.
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                db.profile.name.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(ctx.server.short_version(), dim),
        ])),
        header,
    );
    let last = db
        .last_checked
        .map(|t| format::ago(t.elapsed()))
        .unwrap_or_else(|| "—".to_string());
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(format!("last check: {last}"), dim)))
            .alignment(Alignment::Right),
        header,
    );

    let score_style = if score < 70 {
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
    } else if score < 90 {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::Green)
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("DATABASE HEALTH", dim),
            Span::raw("   "),
            Span::styled(format!("{score} / 100"), score_style),
        ])),
        score_row,
    );

    let table_rows: Vec<Row> = rows
        .iter()
        .map(|r| {
            Row::new(vec![
                Span::raw(r.name),
                Span::styled(r.status.label(), status_style(r.status)),
                Span::raw(r.metric.clone()),
            ])
        })
        .collect();
    f.render_widget(
        Table::new(
            table_rows,
            [
                Constraint::Length(14),
                Constraint::Length(6),
                Constraint::Min(10),
            ],
        )
        .column_spacing(2),
        table_area,
    );

    let mut footer_lines = vec![Line::from(""), Line::from(health::summary_line(&rows))];
    if unmapped > 0 {
        footer_lines.push(Line::from(Span::styled(
            format!(
                "{unmapped} other finding{} — press 1 for the full report",
                if unmapped == 1 { "" } else { "s" }
            ),
            dim,
        )));
    }
    if let Some(err) = &db.error {
        footer_lines.push(Line::from(Span::styled(
            err.to_string(),
            Style::default().fg(Color::Red),
        )));
    }
    f.render_widget(Paragraph::new(footer_lines), footer);
}
