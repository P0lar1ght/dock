//! Copied from grok-build/.../xai-grok-pager/src/app/event_loop.rs
//!
//! Read terminal events on a dedicated thread and forward them over mpsc.
//! Polling crossterm's `EventStream` in `select!` is NOT safe: dropping
//! `next()` mid-poll strands the waker (crossterm #936).

use std::time::Duration;

use crossterm::event::Event;
use tokio::sync::mpsc;

pub(super) fn spawn_reader() -> mpsc::UnboundedReceiver<Event> {
    let (tx, rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        const POLL_TIMEOUT: Duration = Duration::from_millis(20);
        let mut consecutive_event_errors: u32 = 0;
        loop {
            if tx.is_closed() {
                break;
            }
            let event = match crossterm::event::poll(POLL_TIMEOUT) {
                Ok(true) => crossterm::event::read(),
                Ok(false) => continue,
                Err(e) => Err(e),
            };
            match event {
                Ok(ev) => {
                    consecutive_event_errors = 0;
                    if tx.send(ev).is_err() {
                        break;
                    }
                }
                Err(_) => {
                    // VTE / SSH PTYs can emit garbage crossterm rejects
                    // (ratatui#1275). Skip transients; bail if they never stop.
                    consecutive_event_errors += 1;
                    if consecutive_event_errors >= 50 {
                        break;
                    }
                }
            }
        }
    });
    rx
}
