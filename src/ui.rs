//! The draw pass. Renders the whole frame from App state and rebuilds the
//! mouse hitmap as it goes — the draw is the only authority on where things
//! ended up on screen.

use ratatui::layout::{Constraint, Flex, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::action::{Hit, Tab};
use crate::app::{App, DbState, Focus};
use crate::health::HealthStatus;
use crate::screens::{self, branches, data, overview, sidebar, sql, states, tabs};

/// What the pointer is over gets underlined: the standard "this is clickable"
/// affordance, and it survives a monochrome terminal.
pub fn hover_style(base: Style, hovered: bool) -> Style {
    if hovered {
        base.add_modifier(Modifier::UNDERLINED)
    } else {
        base
    }
}

/// Status glyph + tone for a database tab. Shape differs by state, never
/// color alone: ● healthy, ! warning/critical, ○ unavailable, ◌ checking.
pub fn tab_glyph(db: &DbState) -> (&'static str, Color) {
    if db.ctx.is_none() && db.checking() {
        return ("◌", Color::Cyan);
    }
    match db.health {
        HealthStatus::Healthy => ("●", Color::Green),
        HealthStatus::Warning => ("!", Color::Yellow),
        HealthStatus::Critical => ("!", Color::Red),
        HealthStatus::Unavailable => ("○", Color::DarkGray),
        HealthStatus::Checking => ("◌", Color::Cyan),
    }
}

/// Wide layouts get the database sidebar; narrow ones fall back to the
/// original top strip, so pgterm still works in a split pane.
pub fn is_wide(width: u16) -> bool {
    width >= 100
}

pub fn draw(f: &mut Frame, app: &mut App) {
    app.hitmap.clear();
    let area = f.area();
    if states::is_too_small(area.width, area.height) {
        states::draw_too_small(f);
        return;
    }
    if app.dbs.is_empty() && app.popup.is_none() {
        states::draw_first_run(f, area);
        return;
    }

    let [top, body, cmd_row] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);
    draw_top_bar(f, top, app);

    let mut hits: Vec<(Rect, Hit)> = Vec::new();
    let main = if is_wide(area.width) {
        let [side, rule, main] = Layout::horizontal([
            Constraint::Length(sidebar::WIDTH - 1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .areas(body);
        hits.extend(sidebar::draw(f, side, app));
        f.render_widget(
            Block::default()
                .borders(Borders::LEFT)
                .border_style(Style::default().fg(Color::DarkGray)),
            rule,
        );
        main
    } else {
        let [strip, main] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(body);
        draw_tabs(f, strip, app);
        main
    };

    let [tab_row, rule, tab_body] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(main);
    hits.extend(tabs::draw_tab_row(f, tab_row, app));
    f.render_widget(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(Color::DarkGray)),
        rule,
    );
    if let Some(db) = app.dbs.get(app.selected) {
        match db.tab {
            Tab::Overview => overview::draw(f, tab_body, db),
            Tab::PgBot => {
                let [sub, rest] =
                    Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(tab_body);
                hits.extend(tabs::draw_subtabs(f, sub, db, app.hover.as_ref()));
                screens::draw_body(f, rest, db);
            }
            Tab::Sql => sql::draw(f, tab_body, db),
            Tab::Data => {
                hits.extend(data::draw(f, tab_body, db, app.hover.as_ref()));
            }
            Tab::Branches => {
                hits.extend(branches::draw(f, tab_body, db, app.hover.as_ref()));
            }
        }
    }
    app.hitmap.extend(hits);

    draw_command_bar(f, cmd_row, app);
    draw_toast(f, cmd_row, app);

    if app.focus == Focus::Help {
        draw_help(f, area);
    }
    if app.focus == Focus::Palette {
        let hits = draw_palette(f, area, app);
        app.hitmap.extend(hits);
    }
    if let Some(popup) = app.popup.clone() {
        let hits = draw_popup(f, area, &popup, app.focus);
        app.hitmap.extend(hits);
    }
}

/// Product name, the database you are on, and where the two overlays live.
fn draw_top_bar(f: &mut Frame, area: Rect, app: &mut App) {
    let dim = Style::default().fg(Color::DarkGray);
    let mut spans = vec![Span::styled(
        " pgterm ",
        Style::default().add_modifier(Modifier::BOLD),
    )];
    if let Some(db) = app.dbs.get(app.selected) {
        spans.push(Span::raw(format!("  ▸ {}", db.profile.name)));
        if let Some(stage) = db.profile.badge() {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(stage.label(), sidebar::badge_style(stage)));
        }
    }
    let right = "^K commands  ? help ";
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let gap = (area.width as usize).saturating_sub(used + right.len());
    spans.push(Span::raw(" ".repeat(gap)));
    let x = area.x + (used + gap) as u16;
    spans.push(Span::styled(
        right,
        hover_style(dim, app.hover == Some(Hit::OpenPalette)),
    ));
    app.hitmap
        .push((Rect::new(x, area.y, 12, 1), Hit::OpenPalette));
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// A database you are not looking at needed saying something.
fn draw_toast(f: &mut Frame, area: Rect, app: &App) {
    let Some(t) = app.active_toast() else {
        return;
    };
    let w = (t.text.chars().count() as u16 + 2).min(area.width);
    let rect = Rect::new(area.right().saturating_sub(w), area.y, w, 1);
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(Span::styled(
            format!(" {} ", t.text),
            Style::default().fg(Color::Black).bg(Color::Yellow),
        )),
        rect,
    );
}

fn draw_tabs(f: &mut Frame, area: Rect, app: &mut App) {
    let mut spans: Vec<Span> = Vec::new();
    let mut x = area.x;
    let mut hits: Vec<(Rect, Hit)> = Vec::new();
    for (i, db) in app.dbs.iter().enumerate() {
        let (glyph, tone) = tab_glyph(db);
        let label = format!(" {} {} ", db.profile.name, glyph);
        let width = label.chars().count() as u16;
        let mut style = Style::default();
        if i == app.selected {
            style = style.add_modifier(Modifier::REVERSED);
        }
        if db.attention {
            style = style.add_modifier(Modifier::BOLD);
        }
        // Name in the tab style, glyph in its tone on the same background.
        let style = hover_style(style, app.hover == Some(Hit::SelectDb(i)));
        spans.push(Span::styled(format!(" {} ", db.profile.name), style));
        spans.push(Span::styled(format!("{glyph} "), style.fg(tone)));
        hits.push((Rect::new(x, area.y, width, 1), Hit::SelectDb(i)));
        x += width;
        spans.push(Span::raw(" "));
        x += 1;
    }
    let add_label = " + Add DB ";
    spans.push(Span::styled(
        add_label,
        hover_style(
            Style::default().fg(Color::DarkGray),
            app.hover == Some(Hit::OpenAdd),
        ),
    ));
    hits.push((
        Rect::new(x, area.y, add_label.chars().count() as u16, 1),
        Hit::OpenAdd,
    ));
    f.render_widget(Paragraph::new(Line::from(spans)), area);
    app.hitmap.extend(hits);
}

fn draw_command_bar(f: &mut Frame, area: Rect, app: &App) {
    let name = app
        .dbs
        .get(app.selected)
        .map(|d| d.profile.name.as_str())
        .unwrap_or("pgterm");
    let mut spans = vec![
        Span::styled(format!("{name} > "), Style::default().fg(Color::DarkGray)),
        Span::raw(app.cmdline.clone()),
    ];
    if app.focus == Focus::CommandBar {
        spans.push(Span::styled("█", Style::default().fg(Color::Gray)));
    }
    if let Some(err) = &app.cmd_error {
        spans.push(Span::raw("   "));
        spans.push(Span::styled(err.clone(), Style::default().fg(Color::Red)));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The help overlay is generated from the keymap, so it can never describe a
/// binding that does not exist.
fn draw_help(f: &mut Frame, area: Rect) {
    let text = crate::keymap::help_text();
    let lines: Vec<Line> = text.lines().map(|l| Line::from(l.to_string())).collect();
    let h = (lines.len() as u16 + 2).min(area.height);
    let [v] = Layout::vertical([Constraint::Length(h)])
        .flex(Flex::Center)
        .areas(area);
    let [rect] = Layout::horizontal([Constraint::Length(50)])
        .flex(Flex::Center)
        .areas(v);
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" help — any key closes "),
        ),
        rect,
    );
}

/// The command palette overlay: a query line and the filtered matches.
fn draw_palette(f: &mut Frame, area: Rect, app: &App) -> Vec<(Rect, Hit)> {
    let Some(state) = &app.palette else {
        return Vec::new();
    };
    let items = app.palette_items();
    let hits = crate::palette::filter(&items, &state.input);
    let shown = hits.len().min(10);
    let height = shown as u16 + 4;
    let [v] = Layout::vertical([Constraint::Length(height.min(area.height))])
        .flex(Flex::Start)
        .areas(area.inner(Margin::new(0, 2)));
    let [rect] = Layout::horizontal([Constraint::Length(60.min(area.width))])
        .flex(Flex::Center)
        .areas(v);
    f.render_widget(Clear, rect);
    let mut lines = vec![
        Line::from(vec![
            Span::styled("> ", Style::default().fg(Color::DarkGray)),
            Span::raw(state.input.clone()),
            Span::styled("█", Style::default().fg(Color::Gray)),
        ]),
        Line::from(""),
    ];
    let mut regions = Vec::new();
    let cursor = state.cursor.min(shown.saturating_sub(1));
    for (row, idx) in hits.iter().take(shown).enumerate() {
        let style = if row == cursor {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };
        let style = hover_style(style, app.hover == Some(Hit::PaletteItem(row)));
        lines.push(Line::from(Span::styled(
            format!(" {} ", items[*idx].label),
            style,
        )));
        regions.push((
            Rect::new(rect.x + 1, rect.y + 3 + row as u16, rect.width - 2, 1),
            Hit::PaletteItem(row),
        ));
    }
    if let Some(err) = &app.cmd_error {
        lines.push(Line::from(Span::styled(
            err.clone(),
            Style::default().fg(Color::Red),
        )));
    }
    f.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" commands ")),
        rect,
    );
    regions
}

fn draw_popup(
    f: &mut Frame,
    area: Rect,
    popup: &crate::app::AddPopup,
    focus: Focus,
) -> Vec<(Rect, Hit)> {
    let mut button_hits: Vec<(Rect, Hit)> = Vec::new();
    use crate::app::PopupField;
    let [v] = Layout::vertical([Constraint::Length(17)])
        .flex(Flex::Center)
        .areas(area);
    let [rect] = Layout::horizontal([Constraint::Length(56)])
        .flex(Flex::Center)
        .areas(v);
    f.render_widget(Clear, rect);

    let dim = Style::default().fg(Color::DarkGray);
    let field_style = |field: PopupField| {
        if popup.field == field && focus == Focus::Popup {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        }
    };
    let cursor = |field: PopupField| {
        if popup.field == field && focus == Focus::Popup {
            "█"
        } else {
            ""
        }
    };
    // Empty fields show a worked example instead of a blank line; it
    // disappears at the first typed character.
    let placeholder = Style::default()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::ITALIC);
    let field_line = |field: PopupField, value: String, example: &'static str| {
        if value.is_empty() {
            Line::from(vec![
                Span::raw(cursor(field)),
                Span::styled(example, placeholder),
            ])
        } else {
            Line::from(vec![
                Span::styled(value, field_style(field)),
                Span::raw(cursor(field)),
            ])
        }
    };
    let mut lines = vec![
        Line::from(Span::styled("Name", dim)),
        field_line(PopupField::Name, popup.name.clone(), "production"),
        Line::from(""),
        Line::from(Span::styled("Stage", dim)),
        Line::from(vec![
            Span::styled(
                popup
                    .stage
                    .map(|s| s.label().to_string())
                    .unwrap_or_else(|| "auto".into()),
                field_style(PopupField::Stage),
            ),
            Span::styled("   ←/→  auto · prod · staging · dev · local", dim),
        ]),
        Line::from(""),
        Line::from(Span::styled("Connection", dim)),
        field_line(
            PopupField::Env,
            // A pasted URL is a secret: mask it on screen immediately.
            crate::sanitize::redact(&popup.env, None),
            "STAGING_DATABASE_URL='postgresql://...'",
        ),
        Line::from(Span::styled(
            "paste NAME='URL' — connects now, saves only the NAME",
            dim,
        )),
        Line::from(Span::styled(
            "(bare URL: session-only · bare NAME: exported var)",
            dim,
        )),
        Line::from(""),
    ];
    if popup.busy {
        lines.push(Line::from(Span::styled(
            "◌ testing...",
            Style::default().fg(Color::Cyan),
        )));
    } else {
        match &popup.message {
            Some(Ok(msg)) => lines.push(Line::from(Span::styled(
                msg.clone(),
                Style::default().fg(Color::Green),
            ))),
            Some(Err(e)) => {
                // Validation and env problems are the user's input, not a
                // subprocess failure — show the guidance without the
                // error-kind prefix.
                use crate::sanitize::ErrorKind;
                let text = match e.kind {
                    ErrorKind::Usage | ErrorKind::EnvMissing => e.message.clone(),
                    _ => e.to_string(),
                };
                lines.push(Line::from(Span::styled(
                    text,
                    Style::default().fg(Color::Red),
                )));
            }
            None => lines.push(Line::from(vec![
                Span::styled("[ Test ]", Style::default().fg(Color::Cyan)),
                Span::raw("  Ctrl+T                 "),
                Span::styled("[ Add ]", Style::default().fg(Color::Green)),
                Span::raw("  Enter"),
            ])),
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Enter Add    Ctrl+T Test    Esc Cancel",
        dim,
    )));

    let show_buttons = !popup.busy && popup.message.is_none();
    f.render_widget(
        Paragraph::new(lines)
            .wrap(ratatui::widgets::Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Add Database "),
            ),
        rect,
    );
    if show_buttons {
        // The action row is the 12th content line inside the border; the
        // popup has a fixed layout so the offsets are stable.
        let y = rect.y + 12;
        button_hits.push((Rect::new(rect.x + 1, y, 8, 1), Hit::PopupTest));
        button_hits.push((Rect::new(rect.x + 34, y, 7, 1), Hit::PopupAdd));
    }
    button_hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::{Action, CmdKind, StoredResult, View};
    use crate::config::TerminalConfig;
    use crate::model::Context;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    const HEALTHY: &str = include_str!("../tests/fixtures/context_healthy.json");
    const WARN: &str = include_str!("../tests/fixtures/context_warn.json");

    fn app_with(names: &[&str]) -> App {
        let mut cfg = TerminalConfig::default();
        for n in names {
            cfg.add(n, &format!("{}_URL", n.to_uppercase())).unwrap();
        }
        App::new(&cfg, None, false, None)
    }

    fn feed(app: &mut App, db: usize, json: &str) {
        app.update(Action::CheckFinished {
            db,
            kind: CmdKind::Monitor,
            result: Ok(StoredResult::Ctx(Box::new(Context::decode(json).unwrap()))),
        });
    }

    fn render(app: &mut App, w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, app)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn healthy_dashboard_renders_score_and_rows() {
        let mut app = app_with(&["production"]);
        feed(&mut app, 0, HEALTHY);
        pgbot_tab(&mut app);
        let s = render(&mut app, 100, 30);
        assert!(s.contains("production"), "{s}");
        assert!(s.contains("●"), "healthy glyph: {s}");
        assert!(s.contains("DATABASE HEALTH"), "{s}");
        assert!(s.contains("100 / 100"), "{s}");
        assert!(s.contains("Connections"), "{s}");
        assert!(s.contains("84 / 300"), "{s}");
        assert!(s.contains("99.2%"), "{s}");
        assert!(s.contains("7 healthy"), "{s}");
        assert!(s.contains("2 PgBot"), "tab row: {s}");
        assert!(s.contains("Inspect"), "sub-tab row: {s}");
        assert!(s.contains("+ Add database"), "sidebar: {s}");
    }

    #[test]
    fn warn_dashboard_shows_warn_rows_and_glyph() {
        let mut app = app_with(&["prod", "staging"]);
        feed(&mut app, 0, HEALTHY);
        feed(&mut app, 1, WARN);
        pgbot_tab(&mut app);
        let s = render(&mut app, 100, 30);
        assert!(s.contains("!"), "warning glyph on the staging tab: {s}");
        // Selected tab (prod) still healthy.
        assert!(s.contains("100 / 100"), "{s}");
        // Switch to staging: its dashboard shows the warn rows.
        app.select_db(1);
        pgbot_tab(&mut app);
        let s = render(&mut app, 100, 30);
        assert!(s.contains("91 / 100"), "{s}");
        assert!(s.contains("2 unused · 20 GiB"), "{s}");
        assert!(s.contains("2 regressions"), "{s}");
        // The rollback finding maps to no category row, so the category
        // summary still counts two — the findings list below shows all three.
        assert!(s.contains("2 warnings"), "{s}");
    }

    #[test]
    fn first_run_and_too_small_screens() {
        let mut empty = App::new(&TerminalConfig::default(), None, false, None);
        let s = render(&mut empty, 100, 30);
        assert!(s.contains("No databases added yet."), "{s}");
        assert!(s.contains("pgterm add production"), "{s}");

        let mut app = app_with(&["prod"]);
        let s = render(&mut app, 60, 20);
        assert!(s.contains("Terminal too small."), "{s}");
        assert!(s.contains("80 × 24"), "{s}");
    }

    #[test]
    fn checking_state_shows_spinner_glyph() {
        let mut app = app_with(&["prod"]);
        app.update(Action::MonitorTick);
        let s = render(&mut app, 100, 30);
        assert!(s.contains("◌"), "{s}");
        assert!(s.contains("checking"), "{s}");
    }

    #[test]
    fn help_overlay_renders_on_question_mark() {
        use crossterm::event::{KeyEvent, KeyModifiers};
        let mut app = app_with(&["prod"]);
        feed(&mut app, 0, HEALTHY);
        app.update(Action::Key(KeyEvent::new(
            crossterm::event::KeyCode::Char('?'),
            KeyModifiers::NONE,
        )));
        let s = render(&mut app, 100, 40);
        assert!(s.contains("previous database"), "{s}");
        assert!(s.contains("command palette"), "{s}");
    }

    #[test]
    fn hitmap_covers_tabs_and_shortcuts() {
        let mut app = app_with(&["prod", "staging"]);
        feed(&mut app, 0, HEALTHY);
        pgbot_tab(&mut app);
        render(&mut app, 100, 30);
        assert!(app.hitmap.iter().any(|(_, h)| *h == Hit::SelectDb(1)));
        assert!(app.hitmap.iter().any(|(_, h)| *h == Hit::OpenAdd));
        assert!(app
            .hitmap
            .iter()
            .any(|(_, h)| *h == Hit::SetTab(crate::action::Tab::PgBot)));
        assert!(app
            .hitmap
            .iter()
            .any(|(_, h)| *h == Hit::SetView(View::Queries)));
    }

    #[test]
    fn glyphs_differ_by_shape_not_only_color() {
        let mut app = app_with(&["a"]);
        let db = &mut app.dbs[0];
        db.health = HealthStatus::Healthy;
        assert_eq!(tab_glyph(db).0, "●");
        db.health = HealthStatus::Warning;
        assert_eq!(tab_glyph(db).0, "!");
        db.health = HealthStatus::Critical;
        assert_eq!(tab_glyph(db).0, "!");
        db.health = HealthStatus::Unavailable;
        assert_eq!(tab_glyph(db).0, "○");
        db.health = HealthStatus::Checking;
        assert_eq!(tab_glyph(db).0, "◌");
    }

    const INDEXES_REPORT: &str = include_str!("../tests/fixtures/indexes_report.json");
    const WHY_REPORT: &str = include_str!("../tests/fixtures/why_report.json");

    /// The pgbot screens live behind the PgBot tab; these tests are about the
    /// screens, so put the app there first.
    fn pgbot_tab(app: &mut App) {
        app.set_tab(crate::action::Tab::PgBot);
    }

    fn press(app: &mut App, code: crossterm::event::KeyCode) -> Vec<crate::action::Effect> {
        use crossterm::event::{KeyEvent, KeyModifiers};
        app.update(Action::Key(KeyEvent::new(code, KeyModifiers::NONE)))
    }

    fn type_str(app: &mut App, s: &str) {
        for c in s.chars() {
            press(app, crossterm::event::KeyCode::Char(c));
        }
    }

    #[test]
    fn queries_and_tables_views_render_the_context() {
        let mut app = app_with(&["prod"]);
        feed(&mut app, 0, WARN);
        pgbot_tab(&mut app);
        app.set_view(crate::action::View::Queries);
        let s = render(&mut app, 110, 32);
        assert!(s.contains("QUERIES"), "{s}");
        assert!(s.contains("18.2k"), "calls column: {s}");
        assert!(s.contains("423 ms"), "mean column: {s}");
        assert!(
            s.contains("SELECT * FROM orders"),
            "scrubbed text passes through: {s}"
        );

        app.set_view(crate::action::View::Tables);
        let s = render(&mut app, 110, 32);
        assert!(s.contains("TABLES"), "{s}");
        assert!(s.contains("84 GiB"), "{s}");
        assert!(s.contains("public.events"), "{s}");
    }

    #[test]
    fn indexes_view_renders_pgbots_grading_verbatim() {
        let mut app = app_with(&["prod"]);
        app.update(Action::CheckFinished {
            db: 0,
            kind: CmdKind::Indexes,
            result: Ok(StoredResult::Indexes(Box::new(
                crate::model::IndexesReport::decode(INDEXES_REPORT).unwrap(),
            ))),
        });
        app.dbs[0].view = View::Indexes;
        pgbot_tab(&mut app);
        let s = render(&mut app, 130, 32);
        assert!(s.contains("CHECK CODE"), "{s}");
        assert!(s.contains("INCONCLUSIVE"), "{s}");
        assert!(s.contains("DO NOT DROP"), "{s}");
        assert!(s.contains("idx_events_type"), "{s}");
        assert!(!s.contains("drop it"), "never invents advice: {s}");
    }

    #[test]
    fn why_view_renders_chains_and_confidence() {
        let mut app = app_with(&["prod"]);
        app.update(Action::CheckFinished {
            db: 0,
            kind: CmdKind::Why,
            result: Ok(StoredResult::Why(Box::new(
                crate::model::WhyReport::decode(WHY_REPORT).unwrap(),
            ))),
        });
        app.dbs[0].view = View::Why;
        pgbot_tab(&mut app);
        let s = render(&mut app, 110, 32);
        assert!(s.contains("checkout query became 3.2x slower"), "{s}");
        assert!(s.contains("8 ms → 26 ms   +225%"), "{s}");
        assert!(s.contains("table public.orders grew +18%"), "{s}");
        assert!(s.contains("Confidence: 80%"), "{s}");
    }

    #[test]
    fn inspect_view_shows_dashboard_plus_findings_report() {
        let mut app = app_with(&["prod"]);
        feed(&mut app, 0, WARN);
        pgbot_tab(&mut app);
        let s = render(&mut app, 110, 36);
        assert!(s.contains("DATABASE HEALTH"), "{s}");
        assert!(s.contains("WARNING"), "{s}");
        assert!(s.contains("indexes with zero scans"), "{s}");
        assert!(
            s.contains("but: the stats window"),
            "caveats render inline: {s}"
        );
    }

    #[test]
    fn command_bar_runs_whitelisted_verbs_and_rejects_the_rest() {
        use crossterm::event::KeyCode;
        let mut app = app_with(&["prod"]);
        feed(&mut app, 0, HEALTHY);
        press(&mut app, KeyCode::Char('/'));
        type_str(&mut app, "indexes");
        let effects = press(&mut app, KeyCode::Enter);
        assert_eq!(app.dbs[0].view, View::Indexes);
        assert_eq!(effects.len(), 1, "indexes fetch spawned");
        assert_eq!(app.focus, Focus::Main);
        assert!(app.cmdline.is_empty());

        press(&mut app, KeyCode::Char('/'));
        type_str(&mut app, "rm -rf /");
        let effects = press(&mut app, KeyCode::Enter);
        assert!(effects.is_empty(), "rejected input must spawn nothing");
        assert!(app.cmd_error.is_some());
        assert_eq!(
            app.focus,
            Focus::CommandBar,
            "stay in the bar to fix the typo"
        );
        assert_eq!(app.cmdline, "rm -rf /", "input preserved for editing");
        let s = render(&mut app, 110, 32);
        assert!(s.contains("unknown command"), "{s}");
    }

    #[test]
    fn ask_command_switches_view_and_stores_output() {
        use crossterm::event::KeyCode;
        let mut app = app_with(&["prod"]);
        feed(&mut app, 0, HEALTHY);
        press(&mut app, KeyCode::Char('/'));
        type_str(&mut app, "ask why is checkout slow?");
        let effects = press(&mut app, KeyCode::Enter);
        assert_eq!(app.dbs[0].view, View::Ask);
        assert!(matches!(
            effects.as_slice(),
            [crate::action::Effect::Spawn {
                cmd: crate::runner::PgbotCommand::Ask(q),
                kind: CmdKind::Ask,
                ..
            }] if q == "why is checkout slow?"
        ));
        app.update(Action::CheckFinished {
            db: 0,
            kind: CmdKind::Ask,
            result: Ok(StoredResult::Text(
                "The checkout query lost its index.".into(),
            )),
        });
        let s = render(&mut app, 110, 32);
        assert!(s.contains("The checkout query lost its index."), "{s}");
    }

    #[test]
    fn pasted_url_is_masked_on_screen() {
        use crossterm::event::{KeyEvent, KeyModifiers};
        let mut app = app_with(&["prod"]);
        feed(&mut app, 0, HEALTHY);
        app.update(Action::Key(KeyEvent::new(
            crossterm::event::KeyCode::Char('a'),
            KeyModifiers::NONE,
        )));
        app.popup.as_mut().unwrap().env = "postgres://alex:hunter2@db/app".into();
        let s = render(&mut app, 100, 30);
        assert!(!s.contains("hunter2"), "password visible on screen: {s}");
        assert!(s.contains("REDACTED"), "{s}");
        assert!(
            s.contains("session-only"),
            "the hint explains the paste path: {s}"
        );
    }

    #[test]
    fn empty_popup_fields_show_worked_examples() {
        use crossterm::event::{KeyEvent, KeyModifiers};
        let mut app = app_with(&["prod"]);
        feed(&mut app, 0, HEALTHY);
        app.update(Action::Key(KeyEvent::new(
            crossterm::event::KeyCode::Char('a'),
            KeyModifiers::NONE,
        )));
        let s = render(&mut app, 100, 30);
        assert!(s.contains("production"), "name example: {s}");
        assert!(
            s.contains("STAGING_DATABASE_URL="),
            "connection example: {s}"
        );
        assert!(s.contains("saves only the NAME"), "{s}");

        // Typing replaces the example.
        for c in "billing".chars() {
            app.update(Action::Key(KeyEvent::new(
                crossterm::event::KeyCode::Char(c),
                KeyModifiers::NONE,
            )));
        }
        let s = render(&mut app, 100, 30);
        assert!(s.contains("billing"), "{s}");
    }

    #[test]
    fn wide_layout_has_sidebar_badges_tabs_and_no_shortcut_row() {
        let mut app = app_with(&["production", "staging"]);
        feed(&mut app, 0, HEALTHY);
        let s = render(&mut app, 120, 36);
        assert!(s.contains("DATABASES"), "{s}");
        assert!(s.contains("PROD") && s.contains("STAGING"), "{s}");
        assert!(s.contains("1 Overview") && s.contains("2 PgBot"), "{s}");
        assert!(s.contains("+ Add database"), "{s}");
        assert!(
            !s.contains("1 Inspect"),
            "the old shortcut row is gone: {s}"
        );
        assert!(s.contains("checked 0s ago"), "sidebar detail line: {s}");
    }

    #[test]
    fn narrow_layout_uses_the_strip_instead_of_the_sidebar() {
        let mut app = app_with(&["production", "staging"]);
        feed(&mut app, 0, HEALTHY);
        let s = render(&mut app, 90, 30);
        assert!(!s.contains("DATABASES"), "{s}");
        assert!(s.contains("production") && s.contains("+ Add DB"), "{s}");
        assert!(s.contains("1 Overview"), "{s}");
    }

    #[test]
    fn overview_renders_status_tiles_gauges_and_findings() {
        let mut app = app_with(&["production"]);
        feed(&mut app, 0, WARN);
        let s = render(&mut app, 120, 44);
        assert!(s.contains("● Connected · PostgreSQL 17"), "{s}");
        assert!(s.contains("RDS"), "provider in the status line: {s}");
        assert!(s.contains("Connections") && s.contains("84 / 300"), "{s}");
        assert!(s.contains("cache hit"), "{s}");
        assert!(s.contains("rollbacks") && s.contains("watch"), "{s}");
        assert!(s.contains("findings need attention"), "{s}");
        assert!(s.contains("confidence"), "{s}");
        assert!(s.contains("✓ "), "healthy categories listed: {s}");
    }

    #[test]
    fn unavailable_overview_offers_retry() {
        let mut app = app_with(&["production"]);
        app.update(Action::CheckFinished {
            db: 0,
            kind: CmdKind::Monitor,
            result: Err(crate::sanitize::SafeError::new(
                crate::sanitize::ErrorKind::ConnectionFailed,
                "connection refused",
                None,
            )),
        });
        let s = render(&mut app, 120, 30);
        assert!(s.contains("○ Unavailable") && s.contains("r retry"), "{s}");
    }

    #[test]
    fn pgbot_tab_shows_subtabs_and_the_existing_screens() {
        let mut app = app_with(&["production"]);
        feed(&mut app, 0, HEALTHY);
        press(&mut app, crossterm::event::KeyCode::Char('2'));
        let s = render(&mut app, 120, 36);
        assert!(
            s.contains("Inspect") && s.contains("Queries") && s.contains("Why"),
            "{s}"
        );
        assert!(s.contains("DATABASE HEALTH"), "{s}");
    }

    #[test]
    fn palette_toast_and_help_render() {
        let mut app = app_with(&["production", "staging"]);
        press(&mut app, crossterm::event::KeyCode::Char(':'));
        let s = render(&mut app, 120, 36);
        assert!(
            s.contains("switch to staging") && s.contains("refresh"),
            "{s}"
        );
        press(&mut app, crossterm::event::KeyCode::Esc);
        app.toast = Some(crate::app::Toast {
            text: "staging is critical · [ to open".into(),
            until: std::time::Instant::now() + std::time::Duration::from_secs(5),
        });
        let s = render(&mut app, 120, 36);
        assert!(s.contains("staging is critical"), "{s}");
        press(&mut app, crossterm::event::KeyCode::Char('?'));
        let s = render(&mut app, 120, 44);
        assert!(
            s.contains("previous database") && s.contains("command palette"),
            "{s}"
        );
    }

    #[test]
    fn sidebar_detail_lines_can_be_turned_off() {
        let mut app = app_with(&["production"]);
        feed(&mut app, 0, HEALTHY);
        let sidebar_only = |app: &mut App| -> String {
            render(app, 120, 30)
                .lines()
                .map(|l| l.chars().take(25).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n")
        };
        assert!(
            sidebar_only(&mut app).contains("checked"),
            "detail on by default"
        );
        app.ui.sidebar_detail = false;
        let off = sidebar_only(&mut app);
        assert!(!off.contains("checked"), "detail off: {off}");
        assert!(off.contains("production"), "the row itself stays: {off}");
    }

    /// Not an assertion — prints the shell so a human can look at it:
    /// `cargo test --lib show_shell -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn show_shell() {
        let mut app = app_with(&["production", "staging", "analytics"]);
        feed(&mut app, 0, WARN);
        feed(&mut app, 1, HEALTHY);
        println!("{}", render(&mut app, 120, 40));
    }

    /// Prints the Branches tab against whatever pgrun really returns:
    /// `PGTERM_LIVE_PROJECT=jobsgpt cargo test --lib show_branches -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn show_branches() {
        let Ok(project) = std::env::var("PGTERM_LIVE_PROJECT") else {
            println!("set PGTERM_LIVE_PROJECT to try this");
            return;
        };
        let mut app = app_with(&["production"]);
        app.dbs[0].profile.pgrun_project = Some(project.clone());
        let effects = app.set_tab(crate::action::Tab::Branches);
        println!("effects: {effects:?}");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let action = rt.block_on(crate::app::run_pgrun_effect(
            crate::pgrun::pgrun_bin(),
            0,
            crate::pgrun::PgrunCommand::List(project),
            false,
        ));
        app.update(action);
        println!("{}", render(&mut app, 120, 32));
    }

    /// The SQL and Data tabs against a real database:
    /// `PGTERM_TEST_DATABASE_URL=postgres://... cargo test --lib show_sql -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn show_sql_and_data() {
        let Ok(url) = std::env::var("PGTERM_TEST_DATABASE_URL") else {
            println!("set PGTERM_TEST_DATABASE_URL to try this");
            return;
        };
        let mut app = app_with(&["production"]);
        app.dbs[0].source = crate::runner::ConnSource::Session(url);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let conns =
            std::sync::Arc::new(tokio::sync::Mutex::new(crate::app::Connections::default()));

        // SQL tab.
        app.set_tab(crate::action::Tab::Sql);
        app.dbs[0].sql = crate::editor::Editor::from_text(
            "SELECT relname AS table, relkind AS kind, reltuples::bigint AS rows\n  FROM pg_class LIMIT 5",
        );
        let effects = app.run_sql();
        for e in effects {
            if let crate::action::Effect::SpawnSql {
                db,
                target,
                sql,
                policy,
            } = e
            {
                let source = app.dbs[db].source.clone();
                let action = rt.block_on(crate::app::run_sql_effect(
                    conns.clone(),
                    db,
                    source,
                    target,
                    sql,
                    policy,
                ));
                app.update(action);
            }
        }
        println!("{}", render(&mut app, 120, 30));

        // Data tab: schemas, then into one.
        let effects = app.set_tab(crate::action::Tab::Data);
        let mut queue = effects;
        for _ in 0..2 {
            for e in std::mem::take(&mut queue) {
                if let crate::action::Effect::SpawnSql {
                    db,
                    target,
                    sql,
                    policy,
                } = e
                {
                    let source = app.dbs[db].source.clone();
                    let action = rt.block_on(crate::app::run_sql_effect(
                        conns.clone(),
                        db,
                        source,
                        target,
                        sql,
                        policy,
                    ));
                    app.update(action);
                }
            }
            println!("{}", render(&mut app, 120, 24));
            queue = app.data_enter();
        }
    }
}
