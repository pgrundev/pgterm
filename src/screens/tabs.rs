//! The main pane's tab row (numbered top-level tabs, plus server and
//! freshness on the right) and the PgBot sub-tab row.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::action::{Hit, Tab, View};
use crate::app::{App, DbState};
use crate::format;
use crate::ui::tab_glyph;

pub fn draw_tab_row(f: &mut Frame, area: Rect, app: &App) -> Vec<(Rect, Hit)> {
    let Some(db) = app.dbs.get(app.selected) else {
        return Vec::new();
    };
    let mut spans: Vec<Span> = vec![Span::raw(" ")];
    let mut hits = Vec::new();
    let mut x = area.x + 1;
    for (ch, tab, name) in Tab::NUMBERED {
        let label = format!(" {ch} {name} ");
        let w = label.chars().count() as u16;
        let mut style = Style::default();
        if db.tab == tab {
            style = style.add_modifier(Modifier::REVERSED);
        }
        // The findings changed since this tab was last looked at.
        if tab == Tab::PgBot && db.pgbot_changed() {
            style = style.add_modifier(Modifier::BOLD);
        }
        spans.push(Span::styled(label, style));
        spans.push(Span::raw(" "));
        hits.push((Rect::new(x, area.y, w, 1), Hit::SetTab(tab)));
        x += w + 1;
    }
    let (glyph, tone) = tab_glyph(db);
    let server = db
        .ctx
        .as_ref()
        .map(|c| c.server.short_version())
        .unwrap_or_else(|| "PostgreSQL".into());
    let ago = db
        .last_checked
        .map(|t| format::ago(t.elapsed()))
        .unwrap_or_else(|| "—".into());
    let right = format!(" {server} · {ago} ");
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let gap = (area.width as usize).saturating_sub(used + right.chars().count() + 1);
    spans.push(Span::raw(" ".repeat(gap)));
    spans.push(Span::styled(glyph, Style::default().fg(tone)));
    spans.push(Span::styled(right, Style::default().fg(Color::DarkGray)));
    f.render_widget(Paragraph::new(Line::from(spans)), area);
    hits
}

pub fn draw_subtabs(f: &mut Frame, area: Rect, db: &DbState) -> Vec<(Rect, Hit)> {
    let mut spans: Vec<Span> = vec![Span::raw(" ")];
    let mut hits = Vec::new();
    let mut x = area.x + 1;
    let mut entries: Vec<(View, &str)> = View::NUMBERED.iter().map(|(_, v, n)| (*v, *n)).collect();
    if db.ask_output.is_some() || db.view == View::Ask {
        entries.push((View::Ask, "Ask"));
    }
    for (view, name) in entries {
        let label = format!(" {name} ");
        let w = label.chars().count() as u16;
        let style = if db.view == view {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default().fg(Color::Gray)
        };
        spans.push(Span::styled(label, style));
        spans.push(Span::raw(" "));
        hits.push((Rect::new(x, area.y, w, 1), Hit::SetView(view)));
        x += w + 1;
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
    hits
}
