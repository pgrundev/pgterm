//! Queries: pg_stat_statements' top offenders from the cached Context.
//! Query text is already scrubbed by pgbot — it is truncated here, never
//! expanded.

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
        Constraint::Length(1),
    ])
    .areas(inner);

    let queries = match &ctx.queries {
        Some(q) if q.enabled => q,
        Some(q) => {
            let reason = q
                .reason
                .clone()
                .unwrap_or_else(|| "pg_stat_statements not enabled".into());
            f.render_widget(
                Paragraph::new(vec![
                    Line::from("pg_stat_statements is required for query stats."),
                    Line::from(Span::styled(reason, dim)),
                ]),
                body,
            );
            return;
        }
        None => return,
    };

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("QUERIES", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::styled(
                format!("top {} by total execution time", queries.top.len()),
                dim,
            ),
        ])),
        header,
    );

    let rows: Vec<Row> = queries
        .top
        .iter()
        .map(|q| {
            let share = if queries.total_exec_ms > 0.0 {
                format!("{:.1}%", q.total_ms / queries.total_exec_ms * 100.0)
            } else {
                "—".to_string()
            };
            Row::new(vec![
                Span::raw(total_time(q.total_ms)),
                Span::raw(share),
                Span::raw(format::human_count(q.calls)),
                Span::raw(format::human_ms(q.mean_ms)),
                Span::raw(one_line(&q.query)),
            ])
        })
        .collect();
    f.render_widget(
        Table::new(
            rows,
            [
                Constraint::Length(8),
                Constraint::Length(6),
                Constraint::Length(7),
                Constraint::Length(9),
                Constraint::Min(20),
            ],
        )
        .header(Row::new(vec!["TIME", "SHARE", "CALLS", "MEAN", "QUERY"]).style(dim))
        .column_spacing(2),
        body,
    );

    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "share = % of total execution time across all statements",
            dim,
        ))),
        footer,
    );
}

/// Multi-line SQL collapses to one row; the Table column clips the rest.
fn one_line(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Coarse human duration for cumulative execution time.
fn total_time(ms: f64) -> String {
    let secs = ms / 1000.0;
    if secs >= 86_400.0 {
        format!("{:.0}d{:.0}h", secs / 86_400.0, (secs % 86_400.0) / 3600.0)
    } else if secs >= 3600.0 {
        format!("{:.0}h {:.0}m", secs / 3600.0, (secs % 3600.0) / 60.0)
    } else if secs >= 60.0 {
        format!("{:.0}m {:.0}s", secs / 60.0, secs % 60.0)
    } else {
        format!("{secs:.1}s")
    }
}
