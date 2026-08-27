//! Why: pgbot's offline causal analysis, rendered exactly as the engine
//! reported it — symptom, mechanism hops, confidence. The UI never invents
//! causality.

use ratatui::layout::{Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::action::View;
use crate::app::DbState;
use crate::format;

pub fn change_line(before: Option<f64>, after: Option<f64>) -> Option<String> {
    let (b, a) = (before?, after?);
    if b <= 0.0 {
        return Some(format!("{} → {}", format::human_ms(b), format::human_ms(a)));
    }
    let pct = (a - b) / b * 100.0;
    Some(format!(
        "{} → {}   {}{:.0}%",
        format::human_ms(b),
        format::human_ms(a),
        if pct >= 0.0 { "+" } else { "" },
        pct
    ))
}

pub fn draw(f: &mut Frame, area: Rect, db: &DbState) {
    let Some(report) = &db.why else {
        return;
    };
    let dim = Style::default().fg(Color::DarkGray);
    let inner = area.inner(Margin::new(2, 1));

    let mut lines = vec![
        Line::from(vec![
            Span::styled("WHY", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::styled(
                format!(
                    "{} snapshots · {} queries analyzed · {} regressions",
                    report.snapshots, report.analyzed_queries, report.regressions_found
                ),
                dim,
            ),
        ]),
        Line::from(""),
    ];

    if report.snapshots == 0 {
        lines.push(Line::from("No history yet."));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "why compares snapshots over time — each refresh of Inspect (r on view 1)",
            dim,
        )));
        lines.push(Line::from(Span::styled(
            "stores one. Come back after a few runs.",
            dim,
        )));
    } else if report.chains.is_empty() {
        lines.push(Line::from("No regressions found in the observed window."));
    }

    for chain in &report.chains {
        lines.push(Line::from(Span::styled(
            chain.symptom.text.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        if let Some(delta) = change_line(chain.symptom.before, chain.symptom.after) {
            lines.push(Line::from(delta));
        }
        lines.push(Line::from(""));
        if !chain.hops.is_empty() {
            lines.push(Line::from(Span::styled("Evidence:", dim)));
            for hop in &chain.hops {
                lines.push(Line::from(format!("  {}", hop.text)));
            }
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled(
            format!("Confidence: {:.0}%", chain.confidence * 100.0),
            dim,
        )));
        lines.push(Line::from(""));
    }

    for note in &report.notes {
        lines.push(Line::from(Span::styled(format!("note: {note}"), dim)));
    }

    let scroll = db.scroll.get(&View::Why).copied().unwrap_or(0);
    f.render_widget(Paragraph::new(lines).scroll((scroll, 0)), inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn change_line_shows_before_after_and_percent() {
        assert_eq!(
            change_line(Some(8.0), Some(26.0)).unwrap(),
            "8 ms → 26 ms   +225%"
        );
        assert_eq!(
            change_line(Some(10.0), Some(5.0)).unwrap(),
            "10 ms → 5 ms   -50%"
        );
        assert!(change_line(None, Some(1.0)).is_none());
    }
}
