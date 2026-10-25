//! Guess and Lookup stages. Both fan work out to the blocking pool (bounded by a semaphore) so one
//! slow cloud call does not hold up the others; results come back to Catalog as messages.

use std::sync::Arc;

use super::ActorResult;
use ractor::{Actor, ActorRef};
use tokio::sync::Semaphore;

use super::{blocking, msg::*, Shared};
use crate::error::{Result, ShelfError};
use crate::imaging::rgb_to_frame;
use crate::types::*;

fn decode(jpeg: &[u8], ts: u64) -> Result<Frame> {
    let img = image::load_from_memory(jpeg).map_err(|e| ShelfError::internal(format!("stored view is not a valid image: {e}")))?;
    Ok(rgb_to_frame(&img.to_rgb8(), ts))
}

pub struct GuessActor;

pub struct GuessState {
    shared: Arc<Shared>,
    sem: Arc<Semaphore>,
}

impl Actor for GuessActor {
    type Msg = GuessMsg;
    type State = GuessState;
    type Arguments = Arc<Shared>;

    async fn pre_start(&self, _me: ActorRef<GuessMsg>, shared: Arc<Shared>) -> ActorResult<GuessState> {
        Ok(GuessState { shared, sem: Arc::new(Semaphore::new(3)) })
    }

    async fn handle(&self, _me: ActorRef<GuessMsg>, msg: GuessMsg, st: &mut GuessState) -> ActorResult<()> {
        let GuessMsg::Guess(job) = msg;
        let (shared, sem) = (st.shared.clone(), st.sem.clone());
        tokio::spawn(async move {
            let _permit = sem.acquire_owned().await;
            let (item_id, ticket, epoch) = (job.item_id.clone(), job.ticket, job.epoch);
            let sh = shared.clone();
            let result = blocking("guesser", move || {
                let mut frames = job.jpegs.iter().map(|j| decode(j, 0)).collect::<Result<Vec<_>>>()?;
                if frames.is_empty() {
                    return Err(ShelfError::internal("no image to guess from"));
                }
                let image = frames.remove(0);
                sh.plugins.guesser.guess(GuessRequest {
                    item_id: job.item_id,
                    image,
                    extra_views: frames,
                    detector_label: job.label,
                    dominant_colors: job.colors,
                    excluded_titles: job.excluded,
                    mode: job.mode,
                })
            })
            .await;
            shared.slots.catalog.cast(CatalogMsg::GuessDone { item_id, ticket, epoch, result });
        });
        Ok(())
    }
}

pub struct LookupActor;

impl Actor for LookupActor {
    type Msg = LookupMsg;
    type State = GuessState;
    type Arguments = Arc<Shared>;

    async fn pre_start(&self, _me: ActorRef<LookupMsg>, shared: Arc<Shared>) -> ActorResult<GuessState> {
        Ok(GuessState { shared, sem: Arc::new(Semaphore::new(2)) })
    }

    async fn handle(&self, _me: ActorRef<LookupMsg>, msg: LookupMsg, st: &mut GuessState) -> ActorResult<()> {
        let LookupMsg::Lookup { item_id, gtin, epoch } = msg;
        let (shared, sem) = (st.shared.clone(), st.sem.clone());
        tokio::spawn(async move {
            let _permit = sem.acquire_owned().await;
            let sh = shared.clone();
            let g = gtin.clone();
            let result = blocking("lookup", move || match &sh.plugins.lookup {
                Some(l) => l.lookup(g),
                None => Ok(None),
            })
            .await;
            shared.slots.catalog.cast(CatalogMsg::LookupDone { item_id, gtin, epoch, result });
        });
        Ok(())
    }
}
