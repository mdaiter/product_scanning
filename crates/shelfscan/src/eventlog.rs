//! A listener that records events. Handy in tests and in the CLI.

use std::{
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

use crate::traits::EventListener;
use crate::types::Event;

#[derive(Default)]
pub struct EventLog {
    events: Mutex<Vec<Event>>,
    cv: Condvar,
}

impl EventLog {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn events(&self) -> Vec<Event> {
        self.events.lock().map(|g| g.clone()).unwrap_or_default()
    }

    pub fn count(&self, pred: impl Fn(&Event) -> bool) -> usize {
        self.events().iter().filter(|e| pred(e)).count()
    }

    pub fn find(&self, pred: impl Fn(&Event) -> bool) -> Option<Event> {
        self.events().into_iter().find(|e| pred(e))
    }

    /// Block until an event matching `pred` has been recorded (including ones already seen).
    pub fn wait_for(&self, pred: impl Fn(&Event) -> bool, timeout: Duration) -> Option<Event> {
        let deadline = Instant::now() + timeout;
        let mut g = self.events.lock().ok()?;
        loop {
            if let Some(e) = g.iter().find(|e| pred(e)) {
                return Some(e.clone());
            }
            let left = deadline.checked_duration_since(Instant::now())?;
            g = self.cv.wait_timeout(g, left).ok()?.0;
        }
    }
}

impl EventListener for EventLog {
    fn on_event(&self, event: Event) {
        if let Ok(mut g) = self.events.lock() {
            g.push(event);
            self.cv.notify_all();
        }
    }
}
