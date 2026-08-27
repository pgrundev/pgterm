//! The draw pass. Renders the whole frame from App state and rebuilds the
//! mouse hitmap as it goes — the draw is the only authority on where things
//! ended up on screen.

use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::action::{Hit, View};
use crate::app::{App, DbState, Focus};
use crate::health::HealthStatus;
use crate::screens::{self, states};

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

    let [tabs_row, body, shortcut_row, cmd_row] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_tabs(f, tabs_row, app);
    if let Some(db) = app.dbs.get(app.selected) {
        screens::draw_body(f, body, db);
    }
    draw_shortcuts(f, shortcut_row, app);
    draw_command_bar(f, cmd_row, app);

    if app.focus == Focus::Help {
        draw_help(f, area);
    }
    if let Some(popup) = app.popup.clone() {
        let hits = draw_popup(f, area, &popup, app.focus);
        app.hitmap.extend(hits);
    }
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
        Style::default().fg(Color::DarkGray),
    ));
    hits.push((
        Rect::new(x, area.y, add_label.chars().count() as u16, 1),
        Hit::OpenAdd,
    ));
    f.render_widget(Paragraph::new(Line::from(spans)), area);
    app.hitmap.extend(hits);
}

fn draw_shortcuts(f: &mut Frame, area: Rect, app: &mut App) {
    let current = app.dbs.get(app.selected).map(|d| d.view);
    let mut spans: Vec<Span> = Vec::new();
    let mut x = area.x;
    let mut hits: Vec<(Rect, Hit)> = Vec::new();
    for (key, view, name) in View::NUMBERED {
        let label = format!(" {key} {name} ");
        let width = label.chars().count() as u16;
        let style = if current == Some(view) {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };
        spans.push(Span::styled(label, style));
        spans.push(Span::raw("  "));
        hits.push((Rect::new(x, area.y, width, 1), Hit::SetView(view)));
        x += width + 2;
    }
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

const HELP: &str = "\
pgterm

DATABASES
Tab / Shift+Tab    switch database
a                  add database

VIEWS
1                  inspect
2                  queries
3                  indexes
4                  tables
5                  why
Left / Right       previous / next view

COMMANDS
/                  command input
r                  refresh

GENERAL
?                  help
q                  quit";

fn draw_help(f: &mut Frame, area: Rect) {
    let lines: Vec<Line> = HELP.lines().map(Line::from).collect();
    let h = lines.len() as u16 + 2;
    let [v] = Layout::vertical([Constraint::Length(h)])
        .flex(Flex::Center)
        .areas(area);
    let [rect] = Layout::horizontal([Constraint::Length(44)])
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

fn draw_popup(
    f: &mut Frame,
    area: Rect,
    popup: &crate::app::AddPopup,
    focus: Focus,
) -> Vec<(Rect, Hit)> {
    let mut button_hits: Vec<(Rect, Hit)> = Vec::new();
    use crate::app::PopupField;
    let [v] = Layout::vertical([Constraint::Length(14)])
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
    let mut lines = vec![
        Line::from(Span::styled("Name", dim)),
        Line::from(vec![
            Span::styled(popup.name.clone(), field_style(PopupField::Name)),
            Span::raw(cursor(PopupField::Name)),
        ]),
        Line::from(""),
        Line::from(Span::styled("Connection", dim)),
        Line::from(vec![
            Span::styled(
                // A pasted URL is a secret: mask it on screen immediately.
                crate::sanitize::redact(&popup.env, None),
                field_style(PopupField::Env),
            ),
            Span::raw(cursor(PopupField::Env)),
        ]),
        Line::from(Span::styled(
            "a variable name (saved to config), or paste a URL",
            dim,
        )),
        Line::from(Span::styled(
            "(a pasted URL is session-only and never saved)",
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
        // The action row is the 9th content line inside the border; the
        // popup has a fixed layout so the offsets are stable.
        let y = rect.y + 9;
        button_hits.push((Rect::new(rect.x + 1, y, 8, 1), Hit::PopupTest));
        button_hits.push((Rect::new(rect.x + 34, y, 7, 1), Hit::PopupAdd));
    }
    button_hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::{Action, CmdKind, StoredResult};
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
        let s = render(&mut app, 100, 30);
        assert!(s.contains("production"), "{s}");
        assert!(s.contains("●"), "healthy glyph: {s}");
        assert!(s.contains("DATABASE HEALTH"), "{s}");
        assert!(s.contains("100 / 100"), "{s}");
        assert!(s.contains("Connections"), "{s}");
        assert!(s.contains("84 / 300"), "{s}");
        assert!(s.contains("99.2%"), "{s}");
        assert!(s.contains("7 healthy"), "{s}");
        assert!(s.contains("1 Inspect"), "{s}");
        assert!(s.contains("+ Add DB"), "{s}");
    }

    #[test]
    fn warn_dashboard_shows_warn_rows_and_glyph() {
        let mut app = app_with(&["prod", "staging"]);
        feed(&mut app, 0, HEALTHY);
        feed(&mut app, 1, WARN);
        let s = render(&mut app, 100, 30);
        assert!(s.contains("!"), "warning glyph on the staging tab: {s}");
        // Selected tab (prod) still healthy.
        assert!(s.contains("100 / 100"), "{s}");
        // Switch to staging: its dashboard shows the warn rows.
        app.select_db(1);
        let s = render(&mut app, 100, 30);
        assert!(s.contains("94 / 100"), "{s}");
        assert!(s.contains("2 unused · 20 GiB"), "{s}");
        assert!(s.contains("2 regressions"), "{s}");
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
        let s = render(&mut app, 100, 30);
        assert!(s.contains("switch database"), "{s}");
        assert!(s.contains("command input"), "{s}");
    }

    #[test]
    fn hitmap_covers_tabs_and_shortcuts() {
        let mut app = app_with(&["prod", "staging"]);
        feed(&mut app, 0, HEALTHY);
        render(&mut app, 100, 30);
        assert!(app.hitmap.iter().any(|(_, h)| *h == Hit::SelectDb(1)));
        assert!(app.hitmap.iter().any(|(_, h)| *h == Hit::OpenAdd));
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
        use crossterm::event::KeyCode;
        let mut app = app_with(&["prod"]);
        feed(&mut app, 0, WARN);
        press(&mut app, KeyCode::Char('2'));
        let s = render(&mut app, 110, 32);
        assert!(s.contains("QUERIES"), "{s}");
        assert!(s.contains("18.2k"), "calls column: {s}");
        assert!(s.contains("423 ms"), "mean column: {s}");
        assert!(
            s.contains("SELECT * FROM orders"),
            "scrubbed text passes through: {s}"
        );

        press(&mut app, KeyCode::Char('4'));
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
}
