//! Indexes: pgbot's graded correlation report, rendered verbatim. The
//! confidence enum, do_not_drop guards and code-check instructions are
//! pgbot's evidence-based grading — this screen displays, it never
//! re-judges, and "unused" is never presented as "drop it".

use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Row, Table};
use ratatui::Frame;

use crate::app::DbState;
use crate::format;

pub fn grade(confidence: &str, do_not_drop: bool) -> (String, Style) {
    if do_not_drop {
        return (
            "DO NOT DROP".into(),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        );
    }
    match confidence {
        "catalog_proven" => ("PROVEN".into(), Style::default().fg(Color::Green)),
        "needs_code_check" => ("CHECK CODE".into(), Style::default().fg(Color::Yellow)),
        "inconclusive" => ("INCONCLUSIVE".into(), Style::default().fg(Color::DarkGray)),
        other => (other.to_string(), Style::default()),
    }
}

pub fn draw(f: &mut Frame, area: Rect, db: &DbState) {
    let Some(report) = &db.indexes else {
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

    let mut head_spans = vec![
        Span::styled("INDEXES", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("  "),
        Span::styled(
            format!(
                "{} graded · stats window {:.0} days",
                report.indexes.len(),
                report.stats_window_days
            ),
            dim,
        ),
    ];
    if report.cold_window {
        head_spans.push(Span::styled(
            "  COLD WINDOW — counters too young to trust",
            Style::default().fg(Color::Yellow),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(head_spans)), header);

    let rows: Vec<Row> = report
        .indexes
        .iter()
        .map(|ix| {
            let (label, style) = grade(&ix.confidence, ix.do_not_drop);
            Row::new(vec![
                Span::styled(label, style),
                Span::raw(format::human_bytes(ix.size_bytes)),
                Span::raw(format::human_count(ix.scans)),
                Span::raw(format!("{}.{}", ix.table, ix.name)),
                Span::styled(ix.reason.clone(), dim),
            ])
        })
        .collect();
    f.render_widget(
        Table::new(
            rows,
            [
                Constraint::Length(12),
                Constraint::Length(9),
                Constraint::Length(7),
                Constraint::Min(20),
                Constraint::Min(20),
            ],
        )
        .header(Row::new(vec!["GRADE", "SIZE", "SCANS", "INDEX", "WHY"]).style(dim))
        .column_spacing(2),
        body,
    );

    let note = report.note.clone().unwrap_or_else(|| {
        "grades are pgbot's evidence-based verdicts — verify before acting".into()
    });
    f.render_widget(Paragraph::new(Line::from(Span::styled(note, dim))), footer);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grades_render_pgbots_enum_verbatim() {
        assert_eq!(grade("catalog_proven", false).0, "PROVEN");
        assert_eq!(grade("needs_code_check", false).0, "CHECK CODE");
        assert_eq!(grade("inconclusive", false).0, "INCONCLUSIVE");
        // An unknown future grade passes through rather than being re-judged.
        assert_eq!(grade("new_grade", false).0, "new_grade");
        // do_not_drop wins over everything.
        assert_eq!(grade("catalog_proven", true).0, "DO NOT DROP");
    }
}
