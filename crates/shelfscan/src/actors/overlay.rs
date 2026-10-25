use std::{collections::HashMap, sync::Arc};

use super::ActorResult;
use ractor::{Actor, ActorRef};

use super::{msg::*, Shared};
use crate::types::*;

pub struct OverlayActor;

pub struct OverlayState {
    shared: Arc<Shared>,
    track_item: HashMap<u64, String>,
    items: HashMap<String, OverlayItem>,
    focus: Option<String>,
}

impl Actor for OverlayActor {
    type Msg = OverlayMsg;
    type State = OverlayState;
    type Arguments = Arc<Shared>;

    async fn pre_start(&self, _me: ActorRef<OverlayMsg>, shared: Arc<Shared>) -> ActorResult<OverlayState> {
        Ok(OverlayState { shared, track_item: HashMap::new(), items: HashMap::new(), focus: None })
    }

    async fn handle(&self, _me: ActorRef<OverlayMsg>, msg: OverlayMsg, st: &mut OverlayState) -> ActorResult<()> {
        match msg {
            OverlayMsg::Bind { track_id, item_id } => {
                st.track_item.insert(track_id, item_id);
            }
            OverlayMsg::Item(i) => {
                st.items.insert(i.item_id.clone(), i);
            }
            OverlayMsg::Rebind { from, into } => {
                for v in st.track_item.values_mut() {
                    if *v == from {
                        *v = into.clone();
                    }
                }
                st.items.remove(&from);
                if st.focus.as_deref() == Some(from.as_str()) {
                    st.focus = Some(into);
                }
            }
            OverlayMsg::Focus(f) => st.focus = f,
            OverlayMsg::Reset => {
                st.track_item.clear();
                st.items.clear();
                st.focus = None;
            }
            OverlayMsg::Boxes { id, ts_ms, boxes } => {
                let mut out = Vec::with_capacity(boxes.len());
                let mut hidden = 0u32;
                for b in boxes {
                    let item = st.track_item.get(&b.track_id).and_then(|i| st.items.get(i));
                    match item {
                        Some(i) if i.role != Role::Product => hidden += 1,
                        Some(i) => out.push(OverlayBox {
                            track_id: b.track_id,
                            bbox: b.bbox,
                            item_id: Some(i.item_id.clone()),
                            state: Some(i.state),
                            role: i.role,
                            label: i.label.clone(),
                            needs_barcode: i.needs_barcode,
                            focused: st.focus.as_deref() == Some(i.item_id.as_str()),
                        }),
                        None if b.role != Role::Product => hidden += 1,
                        None => out.push(OverlayBox {
                            track_id: b.track_id,
                            bbox: b.bbox,
                            item_id: None,
                            state: None,
                            role: b.role,
                            label: if b.promoted { "Identifying…".into() } else { String::new() },
                            needs_barcode: false,
                            focused: false,
                        }),
                    }
                }
                st.shared.emit(Event::Overlay { frame_id: id, timestamp_ms: ts_ms, boxes: out, hidden });
            }
        }
        Ok(())
    }
}
