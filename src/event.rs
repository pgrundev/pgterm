//! Input pump: a dedicated thread blocks on crossterm and forwards events as
//! Actions. Key events are filtered to presses here so the state machine only
//! ever sees actionable input.

use crossterm::event::{self, Event};
use tokio::sync::mpsc::UnboundedSender;

use crate::action::Action;

pub fn spawn_input_thread(tx: UnboundedSender<Action>) {
    std::thread::spawn(move || loop {
        match event::read() {
            Ok(Event::Key(k)) if k.is_press() => {
                if tx.send(Action::Key(k)).is_err() {
                    break;
                }
            }
            Ok(Event::Mouse(m)) => {
                if tx.send(Action::Mouse(m)).is_err() {
                    break;
                }
            }
            Ok(Event::Resize(w, h)) => {
                if tx.send(Action::Resize(w, h)).is_err() {
                    break;
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    });
}
