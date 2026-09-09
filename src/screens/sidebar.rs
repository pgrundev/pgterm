//! The database sidebar (wide layouts): one or two rows per database, a stage
//! badge, and the add row. Returns the regions it painted so the draw pass
//! stays the single authority on where things ended up.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::action::{Hit, Pane};
use crate::app::{App, DbState};
use crate::config::Stage;
use crate::format;
use crate::health::HealthStatus;
use crate::screens::overview::attention_findings;
use crate::ui::{hover_style, tab_glyph};

/// Sidebar width including the rule column the layout puts beside it.
pub const WIDTH: u16 = 26;

/// Badges are words first: the colour is a second channel, never the only one.
pub fn badge_style(stage: Stage) -> Style {
    let color = match stage {
        Stage::Prod => Color::Yellow,
        Stage::Staging => Color::Cyan,
        Stage::Dev => Color::Green,
        Stage::Local => Color::DarkGray,
    };
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}

/// The dim second line: enough to decide whether to look at this database
/// without switching to it.
pub fn detail_line(db: &DbState) -> String {
    match (&db.ctx, db.health) {
        (_, HealthStatus::Unavailable) => db
            .error
            .as_ref()
            .map(|e| e.message.clone())
            .unwrap_or_else(|| "unavailable".into()),
        (None, _) => "checking…".into(),
        (Some(ctx), HealthStatus::Warning | HealthStatus::Critical) => attention_findings(ctx)
            .first()
            .map(|f| f.title.clone())
            .unwrap_or_else(|| "needs attention".into()),
        // Healthy: the version is already in the tab row and the Overview, so
        // the sidebar spends its one line on freshness, which fits any width.
        (Some(_), _) => {
            let ago = db
                .last_checked
                .map(|t| format::ago(t.elapsed()))
                .unwrap_or_else(|| "—".into());
            format!("checked {ago}")
        }
    }
}

pub fn draw(f: &mut Frame, area: Rect, app: &App) -> Vec<(Rect, Hit)> {
    let dim = Style::default().fg(Color::DarkGray);
    let mut lines: Vec<Line> = vec![Line::from(Span::styled(" DATABASES", dim)), Line::from("")];
    let mut hits = Vec::new();
    let inner_w = area.width as usize;
    for (i, db) in app.dbs.iter().enumerate() {
        let y = area.y + lines.len() as u16;
        let (glyph, tone) = tab_glyph(db);
        let cursor = if app.pane == Pane::Sidebar && i == app.selected {
            "▸"
        } else {
            " "
        };
        let badge = db.profile.badge();
        let badge_w = badge.map(|s| s.label().len() + 1).unwrap_or(0);
        let name_w = inner_w.saturating_sub(4 + badge_w);
        let name: String = db.profile.name.chars().take(name_w).collect();
        let pad = name_w.saturating_sub(name.chars().count());
        let mut style = Style::default();
        if i == app.selected {
            style = style.add_modifier(Modifier::REVERSED);
        }
        if db.attention {
            style = style.add_modifier(Modifier::BOLD);
        }
        style = hover_style(style, app.hover == Some(Hit::SelectDb(i)));
        let mut spans = vec![
            Span::styled(format!(" {cursor}"), style),
            Span::styled(format!("{glyph} "), style.fg(tone)),
            Span::styled(format!("{name}{}", " ".repeat(pad)), style),
        ];
        if let Some(stage) = badge {
            spans.push(Span::styled(
                format!("{} ", stage.label()),
                badge_style(stage).patch(style),
            ));
        }
        lines.push(Line::from(spans));
        let rows = if app.ui.sidebar_detail { 2 } else { 1 };
        hits.push((Rect::new(area.x, y, area.width, rows), Hit::SelectDb(i)));
        if app.ui.sidebar_detail {
            let detail: String = detail_line(db)
                .chars()
                .take(inner_w.saturating_sub(6))
                .collect();
            lines.push(Line::from(Span::styled(format!("    {detail}"), dim)));
        }
    }
    let y = area.y + lines.len() as u16;
    lines.push(Line::from(Span::styled(
        " + Add database",
        hover_style(dim, app.hover == Some(Hit::OpenAdd)),
    )));
    hits.push((Rect::new(area.x, y, area.width, 1), Hit::OpenAdd));
    f.render_widget(Paragraph::new(lines), area);
    hits
}
