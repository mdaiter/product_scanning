use std::sync::Arc;

use image::RgbImage;
use super::ActorResult;
use ractor::{Actor, ActorRef};

use super::{msg::*, Shared};
use crate::imaging::{color_signature, cosine, crop, crop_quality, cutout_on_white};
use crate::types::*;

struct Track {
    id: u64,
    bbox: BBox,
    sig: Vec<f32>,
    hits: u32,
    misses: u32,
    label: String,
    best: Option<(f32, RgbImage)>,
    best_truncated: bool,
    sent_quality: f32,
    sent_views: u32,
    promoted: bool,
    central_hits: u32,
}

struct Det {
    bbox: BBox,
    label: String,
    sig: Vec<f32>,
    crop: RgbImage,
    quality: f32,
    truncated: bool,
}

pub struct TrackActor;

pub struct TrackState {
    shared: Arc<Shared>,
    tracks: Vec<Track>,
    next_id: u64,
}

fn nms(mut dets: Vec<Detection>, iou: f32, max: usize) -> Vec<Detection> {
    dets.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    let mut kept: Vec<Detection> = Vec::new();
    for d in dets {
        if kept.iter().all(|k| k.bbox.iou(&d.bbox) <= iou) {
            kept.push(d);
            if kept.len() >= max {
                break;
            }
        }
    }
    kept
}

impl TrackState {
    fn role_for(&self, bbox: &BBox) -> Role {
        if bbox.area() > self.shared.cfg.background_area {
            Role::Background
        } else {
            Role::Product
        }
    }

    /// Match detections to tracks, update them, and return what to tell the other stages.
    fn process(&mut self, img: &RgbImage, ts_ms: u64, dets: Vec<Detection>) -> (Vec<Candidate>, Vec<u64>, Vec<TrackBox>) {
        let cfg = self.shared.cfg.clone();
        let focus_on = self.shared.focus().is_some();

        let dets = nms(
            dets.into_iter().filter(|d| d.score >= cfg.min_score && d.bbox.clamp01().area() >= cfg.min_box_area).collect(),
            cfg.nms_iou,
            cfg.max_detections as usize,
        );
        let prepared: Vec<Det> = dets
            .into_iter()
            .map(|d| {
                let bbox = d.bbox.clamp01();
                let mut c = crop(img, &bbox, 0.0);
                if cfg.cutout_on_white {
                    if let Some(m) = &d.mask {
                        c = cutout_on_white(&c, m);
                    }
                }
                let truncated = bbox.touches_border(0.01);
                Det { bbox, label: d.label, sig: color_signature(&c, 2), quality: crop_quality(&c, truncated), crop: c, truncated }
            })
            .collect();

        // Greedy association: IoU plus colour agreement, so a track cannot jump to a different product.
        let mut pairs: Vec<(f32, usize, usize)> = Vec::new();
        for (ti, t) in self.tracks.iter().enumerate() {
            for (di, d) in prepared.iter().enumerate() {
                let iou = t.bbox.iou(&d.bbox);
                let cos = cosine(&t.sig, &d.sig);
                let ((tx, ty), (dx, dy)) = (t.bbox.center(), d.bbox.center());
                let dist = ((tx - dx).powi(2) + (ty - dy).powi(2)).sqrt();
                if (iou >= 0.15 || (dist < 0.08 && cos > 0.8)) && cos > 0.5 {
                    pairs.push((iou * 0.7 + cos * 0.3, ti, di));
                }
            }
        }
        pairs.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut t_used = vec![false; self.tracks.len()];
        let mut d_used = vec![false; prepared.len()];
        let mut matched: Vec<(usize, usize)> = Vec::new();
        for (_, ti, di) in pairs {
            if !t_used[ti] && !d_used[di] {
                t_used[ti] = true;
                d_used[di] = true;
                matched.push((ti, di));
            }
        }

        let mut prepared: Vec<Option<Det>> = prepared.into_iter().map(Some).collect();
        for &(ti, di) in &matched {
            let d = prepared[di].take().expect("each detection matches once");
            self.update(ti, d, focus_on);
        }
        for (di, d) in prepared.into_iter().enumerate() {
            if let (Some(d), false) = (d, d_used[di]) {
                let id = self.next_id;
                self.next_id += 1;
                self.tracks.push(Track {
                    id,
                    bbox: d.bbox,
                    sig: d.sig.clone(),
                    hits: 0,
                    misses: 0,
                    label: d.label.clone(),
                    best: None,
                    best_truncated: false,
                    sent_quality: 0.0,
                    sent_views: 0,
                    promoted: false,
                    central_hits: 0,
                });
                let ti = self.tracks.len() - 1;
                self.update(ti, d, focus_on);
                t_used.push(true);
            }
        }
        for (ti, used) in t_used.iter().enumerate() {
            if !used {
                self.tracks[ti].misses += 1;
            }
        }

        // Closed tracks.
        let mut closed = Vec::new();
        let max_age = cfg.max_track_age;
        self.tracks.retain(|t| {
            if t.misses > max_age {
                if t.promoted {
                    closed.push(t.id);
                }
                false
            } else {
                true
            }
        });

        // Candidates: promotion at N sightings, and better views later.
        let mut candidates = Vec::new();
        for t in self.tracks.iter_mut() {
            if t.hits < cfg.min_track_hits || t.misses > 0 {
                continue;
            }
            let Some((q, crop)) = &t.best else { continue };
            let improved = t.promoted && *q >= t.sent_quality * 1.4 && t.sent_views < cfg.max_images_per_item * 2;
            if t.promoted && !improved {
                continue;
            }
            t.promoted = true;
            t.sent_quality = *q;
            t.sent_views += 1;
            let role = if t.bbox.area() > cfg.background_area { Role::Background } else { Role::Product };
            candidates.push(Candidate {
                track_id: t.id,
                label: t.label.clone(),
                bbox: t.bbox,
                role,
                crop: crop.clone(),
                quality: *q,
                focus_bind: focus_on && t.central_hits * 2 >= t.hits,
                ts_ms,
            });
        }

        let boxes = self
            .tracks
            .iter()
            .filter(|t| t.promoted || t.hits >= 2)
            .map(|t| TrackBox { track_id: t.id, bbox: t.bbox, label: t.label.clone(), role: self.role_for(&t.bbox), promoted: t.promoted })
            .collect();
        (candidates, closed, boxes)
    }

    fn update(&mut self, ti: usize, d: Det, focus_on: bool) {
        let t = &mut self.tracks[ti];
        t.bbox = if t.hits == 0 {
            d.bbox
        } else {
            BBox::new(0.4 * t.bbox.x + 0.6 * d.bbox.x, 0.4 * t.bbox.y + 0.6 * d.bbox.y, 0.4 * t.bbox.w + 0.6 * d.bbox.w, 0.4 * t.bbox.h + 0.6 * d.bbox.h)
        };
        for (a, b) in t.sig.iter_mut().zip(&d.sig) {
            *a = 0.7 * *a + 0.3 * *b;
        }
        t.hits += 1;
        t.misses = 0;
        let (cx, cy) = d.bbox.center();
        if focus_on && ((cx - 0.5).powi(2) + (cy - 0.5).powi(2)).sqrt() < 0.28 {
            t.central_hits += 1;
        }
        if t.best.as_ref().map(|(q, _)| d.quality > *q).unwrap_or(true) {
            t.best = Some((d.quality, d.crop));
            t.best_truncated = d.truncated;
        }
    }

    /// Close every track (end of a scan segment). Promoted tracks tell the catalog they are gone.
    fn close_all(&mut self) -> Vec<u64> {
        let ids = self.tracks.iter().filter(|t| t.promoted).map(|t| t.id).collect();
        self.tracks.clear();
        ids
    }
}

impl Actor for TrackActor {
    type Msg = TrackMsg;
    type State = TrackState;
    type Arguments = Arc<Shared>;

    async fn pre_start(&self, _me: ActorRef<TrackMsg>, shared: Arc<Shared>) -> ActorResult<TrackState> {
        Ok(TrackState { shared, tracks: vec![], next_id: 1 })
    }

    async fn handle(&self, _me: ActorRef<TrackMsg>, msg: TrackMsg, st: &mut TrackState) -> ActorResult<()> {
        let shared = st.shared.clone();
        match msg {
            TrackMsg::Detections { id, ts_ms, img, dets, credit } => {
                let before = st.tracks.len();
                let (cands, closed, boxes) = st.process(&img, ts_ms, dets);
                drop(img);
                let promoted_now = cands.len();
                let _ = before;
                for c in cands {
                    shared.slots.embed.cast(EmbedMsg::Candidate(c));
                }
                for id in closed {
                    shared.slots.embed.cast(EmbedMsg::TrackClosed(id));
                }
                shared.stats.tracks_promoted.fetch_add(promoted_now as u64, std::sync::atomic::Ordering::Relaxed);
                shared.slots.overlay.cast(OverlayMsg::Boxes { id, ts_ms, boxes });
                let n = shared.stats.frames_processed.load(std::sync::atomic::Ordering::Relaxed);
                if n % 6 == 0 {
                    shared.emit(Event::Stats { summary: shared.stats.snapshot(), tracks_open: st.tracks.len() as u32 });
                }
                drop(credit); // the frame has left the frame path
            }
            TrackMsg::Barrier(b) => {
                for id in st.close_all() {
                    shared.slots.embed.cast(EmbedMsg::TrackClosed(id));
                }
                shared.slots.embed.cast(EmbedMsg::Barrier(b));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::imaging::rgb_to_frame;
    use crate::synthetic::{Lcg, Scene};

    // Tracking is exercised end-to-end in engine tests; this checks NMS in isolation.
    #[test]
    fn nms_keeps_best_of_overlapping() {
        let mk = |x: f32, s: f32| Detection { bbox: BBox::new(x, 0.1, 0.3, 0.3), score: s, label: "p".into(), mask: None };
        let kept = nms(vec![mk(0.10, 0.6), mk(0.11, 0.9), mk(0.6, 0.5)], 0.5, 10);
        assert_eq!(kept.len(), 2);
        assert!((kept[0].score - 0.9).abs() < 1e-6);
        let _ = (rgb_to_frame, Scene::empty(1, 1), Lcg::new(1));
    }
}
