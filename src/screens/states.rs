//! Whole-body states: first run, terminal too small, database unavailable,
//! check in flight, and a view with nothing fetched yet.

use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::DbState;
use crate::format;

pub const MIN_WIDTH: u16 = 80;
pub const MIN_HEIGHT: u16 = 24;

pub fn is_too_small(w: u16, h: u16) -> bool {
    w < MIN_WIDTH || h < MIN_HEIGHT
}

fn centered(f: &mut Frame, area: Rect, lines: Vec<Line>) {
    let height = lines.len() as u16;
    let [v] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), v);
}

pub fn draw_too_small(f: &mut Frame) {
    let area = f.area();
    centered(
        f,
        area,
        vec![
            Line::from("Terminal too small."),
            Line::from(""),
            Line::from(Span::styled(
                "Minimum recommended size:",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(format!("{MIN_WIDTH} × {MIN_HEIGHT}")),
        ],
    );
}

pub fn draw_first_run(f: &mut Frame, area: Rect) {
    let dim = Style::default().fg(Color::DarkGray);
    centered(
        f,
        area,
        vec![
            Line::from(Span::styled(
                "pgterm",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("No databases added yet."),
            Line::from(""),
            Line::from(Span::styled("Add your current database:", dim)),
            Line::from(""),
            Line::from("  pgterm add production"),
            Line::from(""),
            Line::from(Span::styled("Or:", dim)),
            Line::from(""),
            Line::from("  pgterm add production --env PROD_DATABASE_URL"),
            Line::from(""),
            Line::from(vec![
                Span::styled("[a]", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" Add database   "),
                Span::styled("[q]", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" Quit"),
            ]),
        ],
    );
}

pub fn draw_unavailable(f: &mut Frame, area: Rect, db: &DbState) {
    let last = db
        .last_ok
        .map(|t| format::ago(t.elapsed()))
        .unwrap_or_else(|| "never".to_string());
    let mut lines = vec![
        Line::from(Span::styled(
            db.profile.name.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "UNAVAILABLE",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("Could not connect to PostgreSQL."),
    ];
    if let Some(err) = &db.error {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            err.to_string(),
            Style::default().fg(Color::DarkGray),
        )));
    }
    lines.extend([
        Line::from(""),
        Line::from(Span::styled(
            "Last successful check:",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(last),
        Line::from(""),
        Line::from(vec![
            Span::styled("[r]", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" Retry"),
        ]),
    ]);
    centered(f, area, lines);
}

pub fn draw_loading(f: &mut Frame, area: Rect, db: &DbState) {
    let elapsed = db
        .last_checked
        .map(|t| format!("{}s", t.elapsed().as_secs()))
        .unwrap_or_default();
    centered(
        f,
        area,
        vec![
            Line::from(vec![
                Span::styled(
                    db.profile.name.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(" ◌ checking...", Style::default().fg(Color::Cyan)),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "Running health inspection",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(elapsed),
        ],
    );
}

pub fn draw_empty_view(f: &mut Frame, area: Rect, db: &DbState) {
    centered(
        f,
        area,
        vec![
            Line::from(Span::styled(
                db.profile.name.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("Nothing fetched yet."),
            Line::from(""),
            Line::from(vec![
                Span::styled("[r]", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(" Run it"),
            ]),
        ],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn too_small_threshold_is_80_by_24() {
        assert!(is_too_small(79, 24));
        assert!(is_too_small(80, 23));
        assert!(!is_too_small(80, 24));
        assert!(!is_too_small(200, 60));
    }
}
