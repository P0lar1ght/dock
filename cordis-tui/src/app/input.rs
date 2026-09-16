//! Copied from grok-build/.../xai-grok-pager/src/app/event_loop.rs
//!
//! Read terminal events on a dedicated thread and forward them over mpsc.
//! Polling crossterm's `EventStream` in `select!` is NOT safe: dropping
//! `next()` mid-poll strands the waker (crossterm #936).

use std::time::Duration;

use crossterm::event::Event;
use tokio::sync::mpsc;

/// Take the event we just woke on, plus anything already queued.
/// Trackpad wheel bursts otherwise each trigger a full terminal draw.
pub(super) fn drain_events(first: Event, rx: &mut mpsc::UnboundedReceiver<Event>) -> Vec<Event> {
    let mut events = Vec::with_capacity(32);
    events.push(first);
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    events
}

pub(super) fn drain_notifies(rx: &mut mpsc::UnboundedReceiver<()>) {
    while rx.try_recv().is_ok() {}
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[tokio::test]
    async fn drain_events_collects_the_queued_burst() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        tx.send(Event::Resize(10, 10)).unwrap();
        tx.send(Event::Resize(20, 20)).unwrap();
        tx.send(Event::Key(KeyEvent::new(
            KeyCode::Char('j'),
            KeyModifiers::NONE,
        )))
        .unwrap();
        drop(tx);
        let first = rx.recv().await.unwrap();
        let batch = drain_events(first, &mut rx);
        assert_eq!(batch.len(), 3);
        assert!(matches!(batch[2], Event::Key(_)));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn drain_notifies_drops_coalesced_redraws() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        tx.send(()).unwrap();
        tx.send(()).unwrap();
        drain_notifies(&mut rx);
        assert!(rx.try_recv().is_err());
    }
}
