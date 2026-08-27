//! Inspect: pgbot's findings, grouped by severity, with detail, evidence,
//! remediation and caveats inline — the full report behind the dashboard.

use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::action::View;
use crate::app::DbState;

fn severity_span(sev: &str) -> Span<'static> {
    match sev {
        "critical" => Span::styled(
            "CRITICAL",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
        "warn" => Span::styled("WARNING ", Style::default().fg(Color::Yellow)),
        _ => Span::styled("INFO    ", Style::default().fg(Color::DarkGray)),
    }
}

pub fn draw(f: &mut Frame, area: Rect, db: &DbState) {
    let Some(ctx) = &db.ctx else {
        return;
    };
    let dim = Style::default().fg(Color::DarkGray);
    let inner = area.inner(Margin::new(2, 1));
    let [header, body] = Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).areas(inner);

    f.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled("INSPECT", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw("  "),
                Span::styled(format!("{} findings", ctx.findings.len()), dim),
            ]),
            Line::from(""),
        ]),
        header,
    );

    let mut lines: Vec<Line> = Vec::new();
    if ctx.findings.is_empty() {
        lines.push(Line::from(
            "No findings — everything pgbot checked looks healthy.",
        ));
    }
    // Severity order: critical, warn, everything else.
    let order = |sev: &str| match sev {
        "critical" => 0u8,
        "warn" => 1,
        _ => 2,
    };
    let mut findings: Vec<_> = ctx.findings.iter().collect();
    findings.sort_by_key(|f| order(&f.severity));
    for finding in findings {
        let mut title_style = Style::default().add_modifier(Modifier::BOLD);
        if finding.suppressed {
            title_style = dim;
        }
        let mut head = vec![
            severity_span(&finding.severity),
            Span::raw("  "),
            Span::styled(finding.title.clone(), title_style),
        ];
        if let Some(obj) = &finding.object {
            head.push(Span::styled(format!("  {obj}"), dim));
        }
        if finding.suppressed {
            let reason = finding
                .suppression_reason
                .clone()
                .unwrap_or_else(|| "no reason given".into());
            head.push(Span::styled(format!("  (suppressed: {reason})"), dim));
        }
        lines.push(Line::from(head));
        if !finding.suppressed {
            lines.push(Line::from(Span::raw(format!(
                "         {}",
                finding.detail
            ))));
            for ev in &finding.evidence {
                lines.push(Line::from(Span::styled(format!("           · {ev}"), dim)));
            }
            if let Some(rem) = &finding.remediation {
                lines.push(Line::from(vec![
                    Span::styled("           → ", dim),
                    Span::raw(rem.clone()),
                ]));
            }
            for caveat in &finding.caveats {
                lines.push(Line::from(Span::styled(
                    format!("           but: {caveat}"),
                    Style::default().fg(Color::Yellow),
                )));
            }
        }
        lines.push(Line::from(""));
    }

    let scroll = db.scroll.get(&View::Inspect).copied().unwrap_or(0);
    f.render_widget(Paragraph::new(lines).scroll((scroll, 0)), body);
}
