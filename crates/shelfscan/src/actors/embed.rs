use std::sync::Arc;

use super::ActorResult;
use ractor::{Actor, ActorRef};

use super::{blocking, msg::*, Shared};
use crate::imaging::{dominant_colors, encode_jpeg, l2_normalize, rgb_to_frame};

pub struct EmbedActor;

impl Actor for EmbedActor {
    type Msg = EmbedMsg;
    type State = Arc<Shared>;
    type Arguments = Arc<Shared>;

    async fn pre_start(&self, _me: ActorRef<EmbedMsg>, shared: Arc<Shared>) -> ActorResult<Arc<Shared>> {
        Ok(shared)
    }

    async fn handle(&self, _me: ActorRef<EmbedMsg>, msg: EmbedMsg, shared: &mut Arc<Shared>) -> ActorResult<()> {
        let shared = shared.clone();
        match msg {
            EmbedMsg::Candidate(c) => {
                let sh = shared.clone();
                let res = blocking("embed", move || {
                    let mut v = sh.plugins.embedder.embed(rgb_to_frame(&c.crop, c.ts_ms))?;
                    l2_normalize(&mut v);
                    let jpeg = encode_jpeg(&c.crop, sh.cfg.jpeg_quality)?;
                    Ok(Observation {
                        track_id: c.track_id,
                        embedding: v,
                        jpeg,
                        colors: dominant_colors(&c.crop, 3),
                        quality: c.quality,
                        label: c.label,
                        bbox: c.bbox,
                        role: c.role,
                        focus_bind: c.focus_bind,
                        ts_ms: c.ts_ms,
                    })
                })
                .await;
                match res {
                    Ok(o) => {
                        shared.slots.catalog.cast(CatalogMsg::Observe(o));
                    }
                    Err(e) => shared.warn("embed", e),
                }
            }
            EmbedMsg::TrackClosed(id) => {
                shared.slots.catalog.cast(CatalogMsg::TrackClosed(id));
            }
            EmbedMsg::Barrier(b) => {
                shared.slots.catalog.cast(CatalogMsg::Barrier(b));
            }
        }
        Ok(())
    }
}
