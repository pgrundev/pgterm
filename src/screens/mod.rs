//! Per-view screens. Each draw function renders one database's cached data
//! into the body area; fetching happens elsewhere (App::update effects).

pub mod health;
pub mod states;

use ratatui::layout::Rect;
use ratatui::Frame;

use crate::action::View;
use crate::app::DbState;
use crate::health::HealthStatus;

/// Body dispatch for the selected database.
pub fn draw_body(f: &mut Frame, area: Rect, db: &DbState) {
    // No data at all yet: the connection-level states own the whole body.
    if !db.has_data(db.view) {
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
        View::Inspect => health::draw(f, area, db),
        // Tasks 10 renders the dedicated views; health data never lies in the
        // meantime.
        _ => health::draw(f, area, db),
    }
}
