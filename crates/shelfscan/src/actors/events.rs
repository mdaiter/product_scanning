use std::sync::{atomic::Ordering, Arc};

use super::ActorResult;
use ractor::{Actor, ActorRef};

use super::{blocking, msg::*, Shared};
use crate::types::Event;

/// Overlay and Stats can be dropped under load; everything else is delivered in order.
pub fn is_lossy(e: &Event) -> bool {
    matches!(e, Event::Overlay { .. } | Event::Stats { .. })
}

pub struct EventsActor;

impl Actor for EventsActor {
    type Msg = EventMsg;
    type State = Arc<Shared>;
    type Arguments = Arc<Shared>;

    async fn pre_start(&self, _me: ActorRef<EventMsg>, shared: Arc<Shared>) -> ActorResult<Arc<Shared>> {
        Ok(shared)
    }

    async fn handle(&self, _me: ActorRef<EventMsg>, msg: EventMsg, shared: &mut Arc<Shared>) -> ActorResult<()> {
        let EventMsg::Emit(event) = msg;
        let lossy = is_lossy(&event);
        let listeners: Vec<_> = shared.listeners.read().map(|g| g.clone()).unwrap_or_default();
        if !listeners.is_empty() {
            // Foreign code runs on the blocking pool, in order, one event at a time.
            let _ = blocking("listener", move || {
                for l in &listeners {
                    let ev = event.clone();
                    // A listener that throws or panics must not break the others.
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| l.on_event(ev)));
                }
                Ok(())
            })
            .await;
        }
        if lossy {
            shared.lossy_inflight.fetch_sub(1, Ordering::AcqRel);
        }
        Ok(())
    }
}
