use std::sync::Arc;

use super::ActorResult;
use ractor::{Actor, ActorRef};

use super::{blocking, msg::*, Shared};
use crate::error::Result;
use crate::types::*;

pub struct PublishActor;

async fn publish(shared: &Arc<Shared>, ids: Vec<String>, storefront: Arc<dyn crate::traits::Storefront>) -> Result<Vec<PublishReceipt>> {
    let mut listings = shared.slots.lists.ask(|reply| ListsMsg::Get { ids: Some(ids), reply }).await?;
    for l in &mut listings {
        l.status = ListingStatus::Draft; // belt and braces: nothing leaves as Active
    }
    let total = listings.len() as u32;
    let mut receipts: Vec<PublishReceipt> = Vec::with_capacity(listings.len());
    let chunk = shared.cfg.publish_chunk_size.max(1) as usize;
    for part in listings.chunks(chunk) {
        let (sf, batch) = (storefront.clone(), part.to_vec());
        let res = blocking("storefront", move || sf.publish(batch)).await;
        match res {
            Ok(rs) => {
                for l in part {
                    receipts.push(rs.iter().find(|r| r.listing_id == l.id).cloned().unwrap_or(PublishReceipt {
                        listing_id: l.id.clone(),
                        ok: false,
                        remote_id: None,
                        url: None,
                        message: "storefront returned no receipt for this listing".into(),
                    }));
                }
            }
            Err(e) => {
                for l in part {
                    receipts.push(PublishReceipt { listing_id: l.id.clone(), ok: false, remote_id: None, url: None, message: e.to_string() });
                }
            }
        }
        shared.emit(Event::PublishProgress { done: receipts.len() as u32, total });
    }
    shared.slots.lists.cast(ListsMsg::MarkPublished { receipts: receipts.clone() });
    Ok(receipts)
}

impl Actor for PublishActor {
    type Msg = PublishMsg;
    type State = Arc<Shared>;
    type Arguments = Arc<Shared>;

    async fn pre_start(&self, _me: ActorRef<PublishMsg>, shared: Arc<Shared>) -> ActorResult<Arc<Shared>> {
        Ok(shared)
    }

    async fn handle(&self, _me: ActorRef<PublishMsg>, msg: PublishMsg, shared: &mut Arc<Shared>) -> ActorResult<()> {
        let PublishMsg::Publish { ids, storefront, reply } = msg;
        let _ = reply.send(publish(shared, ids, storefront).await);
        Ok(())
    }
}
