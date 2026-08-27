//! Per-view screens. Each draw function renders one database's cached data
//! into the body area; fetching happens elsewhere (App::update effects).

pub mod ask;
pub mod health;
pub mod indexes;
pub mod inspect;
pub mod queries;
pub mod states;
pub mod tables;
pub mod why;

use ratatui::layout::Rect;
use ratatui::Frame;

use crate::action::View;
use crate::app::DbState;
use crate::health::HealthStatus;

/// Body dispatch for the selected database.
pub fn draw_body(f: &mut Frame, area: Rect, db: &DbState) {
    // No data at all yet: the connection-level states own the whole body.
    if !db.has_data(db.view) {
        if db.view == View::Ask {
            states::draw_ask_hint(f, area, db);
            return;
        }
        if db.health == HealthStatus::Unavailable {
            states::draw_unavailable(f, area, db);
            return;
        }
        if db.checking() || db.running_view_job() {
            states::draw_loading(f, area, db);
            return;
        }
        states::draw_empty_view(f, area, db);
        return;
    }
    match db.view {
        View::Inspect => {
            // The dashboard always lands first; when pgbot found something
            // and there is room, the full findings report follows below.
            let has_findings = db
                .ctx
                .as_ref()
                .map(|c| !c.findings.is_empty())
                .unwrap_or(false);
            if has_findings && area.height > 20 {
                let [top, bottom] = ratatui::layout::Layout::vertical([
                    ratatui::layout::Constraint::Length(14),
                    ratatui::layout::Constraint::Min(0),
                ])
                .areas(area);
                health::draw(f, top, db);
                inspect::draw(f, bottom, db);
            } else {
                health::draw(f, area, db);
            }
        }
        View::Queries => queries::draw(f, area, db),
        View::Indexes => indexes::draw(f, area, db),
        View::Tables => tables::draw(f, area, db),
        View::Why => why::draw(f, area, db),
        View::Ask => ask::draw(f, area, db),
    }
}
