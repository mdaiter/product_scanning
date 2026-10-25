use std::{collections::VecDeque, sync::Arc};

use super::ActorResult;
use ractor::{Actor, ActorRef};

use super::{blocking, msg::*, Shared};
use crate::imaging::{downscale_max_side, frame_to_rgb, rgb_to_frame, sharpness};

/// Even a static scene gets a frame through this often, so tracks can accumulate sightings.
const KEEPALIVE_MS: u64 = 400;

/// Frame gates. They live next to the detector so rejected frames cost almost nothing.
#[derive(Default)]
pub struct Gates {
    prev_thumb: Option<Vec<u8>>,
    last_pass_ms: u64,
    sharp: VecDeque<f32>,
}

pub enum Verdict {
    Pass,
    Repeat,
    Blurry,
}

impl Gates {
    pub fn check(&mut self, img: &image::RgbImage, ts_ms: u64, novelty_threshold: f32, blur_ratio: f32) -> Verdict {
        if blur_ratio > 0.0 {
            let s = sharpness(img);
            self.sharp.push_back(s);
            if self.sharp.len() > 15 {
                self.sharp.pop_front();
            }
            if self.sharp.len() >= 5 {
                let mut v: Vec<f32> = self.sharp.iter().copied().collect();
                v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                if s < blur_ratio * v[v.len() / 2] {
                    return Verdict::Blurry;
                }
            }
        }
        let thumb: Vec<u8> = image::imageops::resize(&image::imageops::grayscale(img), 32, 24, image::imageops::FilterType::Triangle).into_raw();
        if novelty_threshold > 0.0 {
            if let Some(prev) = &self.prev_thumb {
                let mad = thumb.iter().zip(prev).map(|(a, b)| (*a as i32 - *b as i32).unsigned_abs() as f32).sum::<f32>() / thumb.len() as f32;
                if mad < novelty_threshold && ts_ms.saturating_sub(self.last_pass_ms) < KEEPALIVE_MS {
                    return Verdict::Repeat;
                }
            }
        }
        self.prev_thumb = Some(thumb);
        self.last_pass_ms = ts_ms;
        Verdict::Pass
    }
}

pub struct DetectActor;

pub struct DetectState {
    shared: Arc<Shared>,
    gates: Gates,
    last_warn_ms: u64,
}

impl Actor for DetectActor {
    type Msg = DetectMsg;
    type State = DetectState;
    type Arguments = Arc<Shared>;

    async fn pre_start(&self, _me: ActorRef<DetectMsg>, shared: Arc<Shared>) -> ActorResult<DetectState> {
        Ok(DetectState { shared, gates: Gates::default(), last_warn_ms: 0 })
    }

    async fn handle(&self, _me: ActorRef<DetectMsg>, msg: DetectMsg, st: &mut DetectState) -> ActorResult<()> {
        let shared = st.shared.clone();
        match msg {
            DetectMsg::Frame { id, frame, credit } => {
                let ts = frame.timestamp_ms;
                let mut gates = std::mem::take(&mut st.gates);
                let sh = shared.clone();
                let out = blocking("detect", move || {
                    let mut run = || -> crate::error::Result<Option<(image::RgbImage, Vec<crate::types::Detection>)>> {
                        let img = downscale_max_side(frame_to_rgb(&frame)?, sh.cfg.analysis_max_side);
                        match gates.check(&img, ts, sh.cfg.novelty_threshold, sh.cfg.blur_gate_ratio) {
                            Verdict::Pass => {}
                            _ => return Ok(None),
                        }
                        let dets = sh.plugins.detector.detect(rgb_to_frame(&img, ts))?;
                        Ok(Some((img, dets)))
                    };
                    let r = run();
                    Ok((gates, r))
                })
                .await;
                match out {
                    Ok((gates, res)) => {
                        st.gates = gates;
                        match res {
                            Ok(Some((img, dets))) => {
                                shared.stats.frames_processed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                shared.stats.detections.fetch_add(dets.len() as u64, std::sync::atomic::Ordering::Relaxed);
                                shared.slots.track.cast(TrackMsg::Detections { id, ts_ms: ts, img, dets, credit });
                            }
                            Ok(None) => {
                                shared.stats.frames_skipped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            }
                            Err(e) => {
                                shared.stats.frames_dropped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                let now = shared.now_ms();
                                if now.saturating_sub(st.last_warn_ms) > 2000 {
                                    st.last_warn_ms = now;
                                    shared.warn("detect", e);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        // The plugin panicked; gates restart from scratch.
                        shared.stats.frames_dropped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        shared.warn("detect", e);
                    }
                }
            }
            DetectMsg::Barrier(b) => {
                st.gates = Gates::default();
                if !shared.slots.track.cast(TrackMsg::Barrier(b)) {
                    // Track is gone; dropping the barrier makes the caller's receiver error out.
                }
            }
            DetectMsg::Crash => panic!("detect actor crash requested"),
        }
        Ok(())
    }
}
