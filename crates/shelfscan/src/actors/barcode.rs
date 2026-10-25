use std::sync::Arc;

use super::ActorResult;
use ractor::{Actor, ActorRef};

use super::{blocking, msg::*, Shared};
use crate::imaging::{downscale_max_side, frame_to_rgb, rgb_to_frame};

/// Longest side handed to the scanner. Barcodes need resolution, but not 12 megapixels.
const MAX_SIDE: u32 = 1600;

pub struct BarcodeActor;

pub struct BarcodeState {
    shared: Arc<Shared>,
    last_warn_ms: u64,
}

impl Actor for BarcodeActor {
    type Msg = BarcodeMsg;
    type State = BarcodeState;
    type Arguments = Arc<Shared>;

    async fn pre_start(&self, _me: ActorRef<BarcodeMsg>, shared: Arc<Shared>) -> ActorResult<BarcodeState> {
        Ok(BarcodeState { shared, last_warn_ms: 0 })
    }

    async fn handle(&self, _me: ActorRef<BarcodeMsg>, msg: BarcodeMsg, st: &mut BarcodeState) -> ActorResult<()> {
        let BarcodeMsg::Scan { item_id, frame, credit } = msg;
        let sh = st.shared.clone();
        let res = blocking("barcode", move || {
            let Some(scanner) = sh.plugins.scanner.clone() else { return Ok(vec![]) };
            let ts = frame.timestamp_ms;
            let img = downscale_max_side(frame_to_rgb(&frame)?, MAX_SIDE);
            scanner.scan(rgb_to_frame(&img, ts))
        })
        .await;
        drop(credit);
        match res {
            Ok(reads) if !reads.is_empty() => {
                st.shared.slots.catalog.cast(CatalogMsg::BarcodeReads { item_id, reads });
            }
            Ok(_) => {}
            Err(e) => {
                let now = st.shared.now_ms();
                if now.saturating_sub(st.last_warn_ms) > 2000 {
                    st.last_warn_ms = now;
                    st.shared.warn("barcode", e);
                }
            }
        }
        Ok(())
    }
}
