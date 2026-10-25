//! Starts every stage and restarts a stage that panics (up to 3 times a minute). Plugin panics never
//! reach here: they are caught at the `blocking` boundary. A restarted Catalog rebuilds the current
//! session from the Store.

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use super::ActorResult;
use ractor::{Actor, ActorCell, ActorId, ActorRef, SpawnErr, SupervisionEvent};

use super::{barcode::*, catalog::*, detect::*, embed::*, events::*, guess::*, lists::*, overlay::*, pricing::*, publish::*, track::*, Shared};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Events,
    Overlay,
    Catalog,
    Lists,
    Publish,
    Pricing,
    Guess,
    Lookup,
    Barcode,
    Embed,
    Track,
    Detect,
}

/// Downstream first, so every stage finds its neighbours when it starts.
const START_ORDER: [Kind; 12] = [
    Kind::Events,
    Kind::Overlay,
    Kind::Catalog,
    Kind::Lists,
    Kind::Publish,
    Kind::Pricing,
    Kind::Guess,
    Kind::Lookup,
    Kind::Barcode,
    Kind::Embed,
    Kind::Track,
    Kind::Detect,
];

const MAX_RESTARTS: usize = 3;
const WINDOW: Duration = Duration::from_secs(60);

async fn spawn_kind(kind: Kind, shared: &Arc<Shared>, sup: ActorCell, restore: bool) -> Result<ActorId, SpawnErr> {
    macro_rules! sp {
        ($actor:expr, $slot:ident) => {{
            let (r, _h) = Actor::spawn_linked(None, $actor, shared.clone(), sup).await?;
            shared.slots.$slot.set(r.clone());
            r.get_id()
        }};
    }
    Ok(match kind {
        Kind::Events => sp!(EventsActor, events),
        Kind::Overlay => sp!(OverlayActor, overlay),
        Kind::Catalog => sp!(CatalogActor { restore }, catalog),
        Kind::Lists => sp!(ListsActor, lists),
        Kind::Publish => sp!(PublishActor, publish),
        Kind::Pricing => sp!(PricingActor, pricing),
        Kind::Guess => sp!(GuessActor, guess),
        Kind::Lookup => sp!(LookupActor, lookup),
        Kind::Barcode => sp!(BarcodeActor, barcode),
        Kind::Embed => sp!(EmbedActor, embed),
        Kind::Track => sp!(TrackActor, track),
        Kind::Detect => sp!(DetectActor, detect),
    })
}

pub struct SupervisorActor;

pub enum SupMsg {}

pub struct SupState {
    shared: Arc<Shared>,
    ids: HashMap<ActorId, Kind>,
    restarts: HashMap<Kind, Vec<Instant>>,
}

impl Actor for SupervisorActor {
    type Msg = SupMsg;
    type State = SupState;
    type Arguments = Arc<Shared>;

    async fn pre_start(&self, me: ActorRef<SupMsg>, shared: Arc<Shared>) -> ActorResult<SupState> {
        let mut ids = HashMap::new();
        for kind in START_ORDER {
            let id = spawn_kind(kind, &shared, me.get_cell(), false).await?;
            ids.insert(id, kind);
        }
        Ok(SupState { shared, ids, restarts: HashMap::new() })
    }

    async fn handle(&self, _me: ActorRef<SupMsg>, msg: SupMsg, _st: &mut SupState) -> ActorResult<()> {
        match msg {}
    }

    async fn handle_supervisor_evt(&self, me: ActorRef<SupMsg>, evt: SupervisionEvent, st: &mut SupState) -> ActorResult<()> {
        if let SupervisionEvent::ActorFailed(cell, err) = evt {
            let Some(kind) = st.ids.remove(&cell.get_id()) else { return Ok(()) };
            let log = st.restarts.entry(kind).or_default();
            log.retain(|t| t.elapsed() < WINDOW);
            if log.len() >= MAX_RESTARTS {
                st.shared.error("supervisor", format!("{kind:?} keeps crashing ({err}); not restarting it again"));
                return Ok(());
            }
            log.push(Instant::now());
            match spawn_kind(kind, &st.shared, me.get_cell(), true).await {
                Ok(id) => {
                    st.ids.insert(id, kind);
                    st.shared.warn("supervisor", format!("{kind:?} crashed ({err}) and was restarted"));
                }
                Err(e) => st.shared.error("supervisor", format!("could not restart {kind:?}: {e}")),
            }
        }
        Ok(())
    }
}
