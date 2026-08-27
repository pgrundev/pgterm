//! Tables: largest tables with row counts and scan patterns, from the
//! cached Context.

use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Row, Table};
use ratatui::Frame;

use crate::app::DbState;
use crate::format;

pub fn draw(f: &mut Frame, area: Rect, db: &DbState) {
    let Some(ctx) = &db.ctx else {
        return;
    };
    let dim = Style::default().fg(Color::DarkGray);
    let inner = area.inner(Margin::new(2, 1));
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(0),
        Constraint::Length(2),
    ])
    .areas(inner);

    let Some(tables) = &ctx.tables else {
        f.render_widget(
            Paragraph::new("no user tables visible (need SELECT on pg_stat_user_tables)"),
            body,
        );
        return;
    };

    let dbsize = if tables.db_size_bytes > 0 {
        format!(" · {} database", format::human_bytes(tables.db_size_bytes))
    } else {
        String::new()
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("TABLES", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::styled(
                format!("top {} by total size{dbsize}", tables.top.len()),
                dim,
            ),
        ])),
        header,
    );

    let rows: Vec<Row> = tables
        .top
        .iter()
        .map(|t| {
            Row::new(vec![
                Span::raw(format::human_bytes(t.total_bytes)),
                Span::raw(format::human_count(t.live_tuples)),
                Span::raw(format!("{:.1}%", t.dead_ratio * 100.0)),
                Span::raw(format::human_count(t.seq_scans)),
                Span::raw(format::human_count(t.index_scans)),
                Span::raw(format!("{}.{}", t.schema, t.name)),
            ])
        })
        .collect();
    f.render_widget(
        Table::new(
            rows,
            [
                Constraint::Length(9),
                Constraint::Length(7),
                Constraint::Length(6),
                Constraint::Length(9),
                Constraint::Length(9),
                Constraint::Min(16),
            ],
        )
        .header(
            Row::new(vec![
                "SIZE",
                "ROWS",
                "DEAD%",
                "SEQ SCANS",
                "IDX SCANS",
                "TABLE",
            ])
            .style(dim),
        )
        .column_spacing(2),
        body,
    );

    f.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                "size = heap + indexes + TOAST. A large table with heavy seq scans and few",
                dim,
            )),
            Line::from(Span::styled(
                "index scans is a likely missing-index candidate.",
                dim,
            )),
        ]),
        footer,
    );
}
