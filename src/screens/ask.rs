//! Ask: the raw text answer from `pgbot ask`, scrollable. Reached only via
//! the command bar (`ask why did checkout get slower?`).

use ratatui::layout::{Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::action::View;
use crate::app::DbState;

pub fn draw(f: &mut Frame, area: Rect, db: &DbState) {
    let Some(output) = &db.ask_output else {
        return;
    };
    let dim = Style::default().fg(Color::DarkGray);
    let inner = area.inner(Margin::new(2, 1));
    let mut lines = vec![
        Line::from(vec![
            Span::styled("ASK", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::styled(
                "grounded on pgbot's findings, answered by your configured AI",
                dim,
            ),
        ]),
        Line::from(""),
    ];
    lines.extend(output.lines().map(|l| Line::from(l.to_string())));
    let scroll = db.scroll.get(&View::Ask).copied().unwrap_or(0);
    f.render_widget(Paragraph::new(lines).scroll((scroll, 0)), inner);
}
