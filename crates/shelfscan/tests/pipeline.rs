//! End-to-end behaviour of the engine with synthetic tabletop scenes. No camera, no network.

mod common;

use std::{
    sync::{atomic::Ordering, Arc, Mutex},
    time::Duration,
};

use common::*;
use shelfscan::synthetic::{Lcg, Scene, BLUE, RED};
use shelfscan::*;

#[tokio::test]
async fn discovers_guesses_and_labels_each_product() {
    let (e, log) = engine();
    scan(&e, &Scene::three_products(), 8);
    let items = wait_guessed(&e, 3).await;
    // keep the tracks open and show the user what we think
    pump(&e, Scene::three_products().frames(3, 6.0, 5000, 12));
    wait(&log, |ev| matches!(ev, Event::Overlay { boxes, .. } if boxes.iter().filter(|b| b.label.starts_with("Looks like")).count() == 3));

    let s = e.flush().await.unwrap();
    assert_eq!(s.items, 3);
    assert_eq!(items.len(), 3);
    for i in &items {
        assert_eq!(i.state, ItemState::Guessed);
        assert!(i.needs_barcode, "catalog mode asks for the barcode after the first guess");
        assert_eq!(i.quantity, 1);
        assert!(!i.images.is_empty());
        assert_eq!(i.role, Role::Product);
    }
    for c in ["red", "blue", "green"] {
        assert!(title_of(&items, c).is_some(), "no item for {c}: {:?}", items.iter().map(|i| &i.guess.title).collect::<Vec<_>>());
    }
    // events reach listeners asynchronously, in order: once ScanFlushed has arrived, everything before it has too
    wait(&log, |ev| matches!(ev, Event::ScanFlushed { .. }));
    assert_eq!(log.count(|ev| matches!(ev, Event::ItemDiscovered { .. })), 3);
    assert_eq!(log.count(|ev| matches!(ev, Event::GuessReady { .. })), 3);
    assert_eq!(log.count(|ev| matches!(ev, Event::BarcodeRequested { .. })), 3);
    assert_eq!(log.count(|ev| matches!(ev, Event::ScanFlushed { items: 3 })), 1);
}

#[tokio::test]
async fn quantity_is_the_most_units_seen_at_once() {
    let (e, _log) = engine();
    let scene = Scene::empty(480, 360)
        .with(shelfscan::BBox::new(0.05, 0.2, 0.18, 0.35), RED)
        .with(shelfscan::BBox::new(0.30, 0.2, 0.18, 0.35), RED)
        .with(shelfscan::BBox::new(0.65, 0.2, 0.2, 0.4), BLUE);
    scan(&e, &scene, 8);
    e.flush().await.unwrap();
    let items = e.catalog(false).await.unwrap();
    assert_eq!(items.len(), 2, "two red boxes are one SKU: {:?}", items.iter().map(|i| (&i.guess.title, i.quantity)).collect::<Vec<_>>());
    assert_eq!(title_of(&items, "red").unwrap().quantity, 2);
    assert_eq!(title_of(&items, "blue").unwrap().quantity, 1);
}

#[tokio::test]
async fn confirm_reject_and_typed_correction_follow_the_state_machine() {
    let (e, log) = engine();
    scan(&e, &Scene::three_products(), 8);
    let items = wait_guessed(&e, 3).await;
    let red = title_of(&items, "red").unwrap();
    assert_eq!(red.guess.title, "Unidentified red product");

    // reject -> automatic re-guess that never repeats the rejected title
    let r = e.reject_item(red.id.clone(), None).await.unwrap();
    assert_eq!(r.state, ItemState::Rejected);
    let again = wait_item(&e, &red.id, |i| i.state == ItemState::Guessed).await;
    assert_eq!(again.guess.title, "Unidentified red item");
    e.reject_item(red.id.clone(), None).await.unwrap();
    let third = wait_item(&e, &red.id, |i| i.state == ItemState::Guessed).await;
    assert_eq!(third.guess.title, "Unidentified red object");

    // guess_retries = 2: the next rejection stays rejected until the user types something
    let r = e.reject_item(red.id.clone(), None).await.unwrap();
    assert_eq!(r.state, ItemState::Rejected);
    wait(&log, |ev| matches!(ev, Event::Warning { message, .. } if message.contains("used up")));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(e.item(red.id.clone()).await.unwrap().unwrap().state, ItemState::Rejected);
    assert!(e.review_queue().await.unwrap().iter().any(|i| i.id == red.id));

    // a typed correction is the user's decision
    let fixed = e.reject_item(red.id.clone(), Some("  Red Stapler ".into())).await.unwrap();
    assert_eq!(fixed.state, ItemState::Confirmed { by: ConfirmedBy::User });
    assert_eq!(fixed.guess.title, "Red Stapler");
    assert!(!fixed.needs_barcode);

    // plain confirm
    let blue = title_of(&items, "blue").unwrap();
    let c = e.confirm_item(blue.id.clone()).await.unwrap();
    assert_eq!(c.state, ItemState::Confirmed { by: ConfirmedBy::User });
    // confirming twice is harmless, confirming something unknown is an error
    e.confirm_item(blue.id.clone()).await.unwrap();
    assert!(matches!(e.confirm_item("item-999".into()).await, Err(ShelfError::InvalidInput { .. })));
    assert_eq!(e.review_queue().await.unwrap().len(), 1, "only green is left to review");
}

#[tokio::test]
async fn barcode_reads_confirm_after_agreeing_votes_and_lookup_fills_the_title() {
    let scanner = Arc::new(FakeScanner(Mutex::new(vec![read("4006381333931")])));
    let (e, log) = engine_with(config(), |b| b.barcode_scanner(scanner.clone()).product_lookup(Arc::new(FakeLookup)));
    scan(&e, &Scene::three_products(), 6);
    let items = wait_guessed(&e, 3).await;
    let red = title_of(&items, "red").unwrap();

    // reads for an item that is not focused are ignored
    pump(&e, Scene::three_products().frames(3, 6.0, 4000, 5));
    assert!(e.item(red.id.clone()).await.unwrap().unwrap().gtin.is_none());

    e.focus_item(red.id.clone()).await.unwrap();
    assert_eq!(e.focused_item().as_deref(), Some(red.id.as_str()));
    pump(&e, Scene::three_products().frames(6, 6.0, 8000, 6));
    wait(&log, |ev| matches!(ev, Event::BarcodeRead { gtin, .. } if gtin == "4006381333931"));
    let done = wait_item(&e, &red.id, |i| i.guess.title == "Stabilo Boss Highlighter").await;
    assert_eq!(done.state, ItemState::Confirmed { by: ConfirmedBy::Barcode });
    assert_eq!(done.gtin.as_deref(), Some("4006381333931"));
    assert_eq!(done.guess.brand.as_deref(), Some("Stabilo"));
    assert!(!done.needs_barcode);
    assert!(done.guess.attributes.iter().any(|a| a.name == "Source" && a.value == "fixture"));
    // the other items are untouched
    for i in e.catalog(false).await.unwrap().into_iter().filter(|i| i.id != red.id) {
        assert!(i.gtin.is_none());
    }
}

#[tokio::test]
async fn invalid_barcodes_are_never_trusted() {
    // 1234567890123 has the wrong check digit
    let scanner = Arc::new(FakeScanner(Mutex::new(vec![read("1234567890123"), read("QR-HELLO")])));
    let (e, _log) = engine_with(config(), |b| b.barcode_scanner(scanner));
    scan(&e, &Scene::three_products(), 6);
    let items = wait_guessed(&e, 3).await;
    e.focus_item(items[0].id.clone()).await.unwrap();
    pump(&e, Scene::three_products().frames(8, 6.0, 8000, 6));
    tokio::time::sleep(Duration::from_millis(300)).await;
    let it = e.item(items[0].id.clone()).await.unwrap().unwrap();
    assert_eq!(it.state, ItemState::Guessed);
    assert!(it.gtin.is_none());

    // typed barcodes are validated too
    for bad in ["1234567890123", "abc", "", "12345"] {
        assert!(matches!(e.submit_barcode(it.id.clone(), bad.into()).await, Err(ShelfError::InvalidInput { .. })), "{bad}");
    }
    let ok = e.submit_barcode(it.id.clone(), "4006381333931".into()).await.unwrap();
    assert_eq!(ok.state, ItemState::Confirmed { by: ConfirmedBy::Barcode });
}

#[tokio::test]
async fn two_items_with_the_same_gtin_merge() {
    let (e, log) = engine();
    scan(&e, &Scene::three_products(), 8);
    let items = wait_guessed(&e, 3).await;
    let (a, b) = (items[0].id.clone(), items[1].id.clone());
    e.submit_barcode(a.clone(), "4006381333931".into()).await.unwrap();
    // the same GTIN on a differently-looking item: same product, so it merges into the older one
    let survivor = e.submit_barcode(b.clone(), "4006381333931".into()).await.unwrap();
    assert_eq!(survivor.id, a);
    wait(&log, |ev| matches!(ev, Event::ItemMerged { from_id, into_id } if *from_id == b && *into_id == a));
    assert_eq!(e.catalog(true).await.unwrap().len(), 2);
    assert!(e.item(b).await.unwrap().is_none());
}

#[tokio::test]
async fn hiding_is_reversible_and_teaches_the_fixture_library() {
    let (e, log) = engine();
    let scene = Scene::three_products();
    scan(&e, &scene, 8);
    let items = wait_guessed(&e, 3).await;
    let red = title_of(&items, "red").unwrap();

    let hidden = e.set_role(red.id.clone(), Role::Excluded, false).await.unwrap();
    assert_eq!(hidden.role, Role::Excluded);
    assert_eq!(e.catalog(false).await.unwrap().len(), 2);
    assert_eq!(e.catalog(true).await.unwrap().len(), 3, "hidden, not deleted");
    pump(&e, scene.frames(3, 6.0, 5000, 3));
    wait(&log, |ev| matches!(ev, Event::Overlay { hidden: 1, boxes, .. } if boxes.len() == 2));

    // bring it back
    let back = e.set_role(red.id.clone(), Role::Product, false).await.unwrap();
    assert_eq!(back.role, Role::Product);
    assert_eq!(e.catalog(false).await.unwrap().len(), 3);

    // hide again, start a fresh session: the same appearance is hidden on sight
    e.set_role(red.id.clone(), Role::Excluded, false).await.unwrap();
    e.new_session().await.unwrap();
    assert!(e.catalog(true).await.unwrap().is_empty());
    scan(&e, &scene, 8);
    e.flush().await.unwrap();
    let all = e.catalog(true).await.unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(e.catalog(false).await.unwrap().len(), 2);
    let auto = all.iter().find(|i| i.role == Role::Excluded).unwrap();
    assert!(!auto.guess_ready, "hidden items are not sent to the guesser");
}

#[tokio::test]
async fn large_objects_get_the_background_role_and_can_be_promoted() {
    let (e, _log) = engine();
    let scene = Scene::empty(480, 360).with(shelfscan::BBox::new(0.02, 0.05, 0.7, 0.85), BLUE).with(shelfscan::BBox::new(0.78, 0.3, 0.18, 0.35), RED);
    scan(&e, &scene, 8);
    e.flush().await.unwrap();
    let all = e.catalog(true).await.unwrap();
    assert_eq!(all.len(), 2);
    let big = all.iter().find(|i| i.role == Role::Background).expect("the big blue thing is background");
    assert!(!big.guess_ready);
    let promoted = e.set_role(big.id.clone(), Role::Product, false).await.unwrap();
    assert_eq!(promoted.role, Role::Product);
    let guessed = wait_item(&e, &big.id, |i| i.guess_ready).await;
    assert!(guessed.guess.title.contains("blue"));
}

#[tokio::test]
async fn switching_mode_changes_whether_a_barcode_is_requested() {
    let (e, _log) = engine();
    scan(&e, &Scene::three_products(), 8);
    let items = wait_guessed(&e, 3).await;
    assert!(items[0].needs_barcode);
    e.set_mode(Some(items[0].id.clone()), ScanMode::DescribeFirst).await.unwrap();
    let it = wait_item(&e, &items[0].id, |i| i.mode == ScanMode::DescribeFirst && i.guess_ready).await;
    assert!(!it.needs_barcode);
    // session default applies to new items only
    e.set_mode(None, ScanMode::DescribeFirst).await.unwrap();
    e.new_session().await.unwrap();
    scan(&e, &Scene::three_products(), 8);
    let fresh = wait_guessed(&e, 3).await;
    assert!(fresh.iter().all(|i| i.mode == ScanMode::DescribeFirst && !i.needs_barcode));
    e.skip_barcode(fresh[0].id.clone()).await.unwrap();
}

#[tokio::test]
async fn listings_are_drafts_with_stable_skus_and_visible_gaps() {
    let (e, _log) = engine();
    scan(&e, &Scene::three_products(), 8);
    let items = wait_guessed(&e, 3).await;

    // nothing is confirmed yet
    assert!(e.generate_listings(false).await.unwrap().is_empty());
    for i in &items[..2] {
        e.confirm_item(i.id.clone()).await.unwrap();
    }
    let ls = e.generate_listings(false).await.unwrap();
    assert_eq!(ls.len(), 2);
    for l in &ls {
        assert_eq!(l.status, ListingStatus::Draft);
        assert!(l.variants[0].sku.as_deref().unwrap().starts_with("SKU-"), "{:?}", l.variants[0].sku);
        assert_eq!(l.variants[0].price, "0.00");
        assert!(l.tags.contains(&"needs-price".to_string()));
        assert!(!l.images.is_empty());
        assert!(l.description_html.contains("Needs your input"));
    }
    let skus: Vec<_> = ls.iter().map(|l| l.variants[0].sku.clone().unwrap()).collect();
    assert_ne!(skus[0], skus[1]);

    // including unconfirmed adds the third; the first two keep their ids and SKUs
    let again = e.generate_listings(true).await.unwrap();
    assert_eq!(again.len(), 3);
    for old in &ls {
        let new = again.iter().find(|l| l.source_item_id == old.source_item_id).unwrap();
        assert_eq!(new.id, old.id);
        assert_eq!(new.variants[0].sku, old.variants[0].sku);
    }
    assert!(again.iter().any(|l| l.tags.contains(&"needs-review".to_string())));
    assert_eq!(e.listings().await.unwrap().len(), 3);

    // editing never activates
    let mut edited = again[0].clone();
    edited.title = "My better title".into();
    edited.status = ListingStatus::Active;
    let saved = e.update_listing(edited).await.unwrap();
    assert_eq!(saved.title, "My better title");
    assert_eq!(saved.status, ListingStatus::Draft);
}

#[tokio::test]
async fn gtin_becomes_the_sku_when_known() {
    let (e, _log) = engine();
    scan(&e, &Scene::three_products(), 8);
    let items = wait_guessed(&e, 3).await;
    e.submit_barcode(items[0].id.clone(), "4006381333931".into()).await.unwrap();
    let ls = e.generate_listings(false).await.unwrap();
    assert_eq!(ls.len(), 1);
    assert_eq!(ls[0].variants[0].sku.as_deref(), Some("4006381333931"));
    assert_eq!(ls[0].variants[0].barcode.as_deref(), Some("4006381333931"));
}

#[tokio::test]
async fn publishing_sends_only_drafts_and_reports_each_failure() {
    let (e, log) = engine_with(config(), |b| b.listing_writer(Arc::new(PushyWriter)));
    scan(&e, &Scene::three_products(), 8);
    let items = wait_guessed(&e, 3).await;
    for i in &items {
        e.confirm_item(i.id.clone()).await.unwrap();
    }
    let ls = e.generate_listings(false).await.unwrap();
    assert_eq!(ls.len(), 3);
    assert!(ls.iter().all(|l| l.status == ListingStatus::Draft && !l.published), "a pushy writer must not get listings activated");
    assert!(ls.iter().all(|l| l.source_item_id.starts_with("item-")), "engine owns ids");

    let store = Arc::new(RecordingStorefront::default());
    store.fail_ids.lock().unwrap().push(ls[1].id.clone());
    let ids: Vec<_> = ls.iter().map(|l| l.id.clone()).collect();
    let receipts = e.publish(ids, store.clone()).await.unwrap();
    assert_eq!(receipts.len(), 3);
    assert_eq!(receipts.iter().filter(|r| r.ok).count(), 2);
    assert!(!receipts.iter().find(|r| r.listing_id == ls[1].id).unwrap().ok);
    assert!(store.seen.lock().unwrap().iter().all(|l| l.status == ListingStatus::Draft));
    wait(&log, |ev| matches!(ev, Event::PublishProgress { done: 3, total: 3 }));
    let after = e.listings().await.unwrap();
    assert_eq!(after.iter().filter(|l| l.published).count(), 2);
}

#[tokio::test]
async fn publish_is_chunked() {
    let cfg = EngineConfig { publish_chunk_size: 2, ..config() };
    let (e, log) = engine_with(cfg, |b| b);
    scan(&e, &Scene::three_products(), 8);
    for i in wait_guessed(&e, 3).await {
        e.confirm_item(i.id).await.unwrap();
    }
    let ids: Vec<_> = e.generate_listings(false).await.unwrap().into_iter().map(|l| l.id).collect();
    e.publish(ids, Arc::new(RecordingStorefront::default())).await.unwrap();
    wait(&log, |ev| matches!(ev, Event::PublishProgress { done: 2, total: 3 }));
    wait(&log, |ev| matches!(ev, Event::PublishProgress { done: 3, total: 3 }));
}

#[tokio::test]
async fn price_suggestions_are_on_demand_and_never_invented() {
    let (e, _log) = engine();
    scan(&e, &Scene::three_products(), 8);
    let items = wait_guessed(&e, 3).await;
    let s = e.suggest_price(items[0].id.clone(), false).await.unwrap();
    assert!(s.comps.is_empty() && s.median.is_empty(), "the stub must not make up a price");
    assert!(s.note.contains("No price source"));
    assert!(matches!(e.suggest_price("item-999".into(), false).await, Err(ShelfError::InvalidInput { .. })));
}

#[tokio::test]
async fn a_full_pipeline_drops_frames_at_the_door_and_accounts_for_every_one() {
    let det: Arc<dyn Detector> = Arc::new(SlowDetector(Duration::from_millis(60), Arc::new(shelfscan::backends::BlobDetector::default())));
    let cfg = EngineConfig { frame_credits: 2, analysis_fps: 1000.0, ..config() };
    let (e, _log) = engine_with(cfg, |b| b.detector(det));
    let frames = Scene::three_products().frames(40, 500.0, 1000, 2);
    let accepted = frames.into_iter().filter(|f| e.push_frame(f.clone())).count();
    let s_now = e.stats();
    assert_eq!(s_now.frames_in, 40);
    assert!(s_now.frames_dropped > 0, "a 60ms detector cannot keep up with a 500fps camera");
    assert!(accepted <= 6, "at most the credits plus whatever drained while pushing, got {accepted}");
    e.flush().await.unwrap();
    let s = e.stats();
    assert_eq!(s.frames_in, s.frames_processed + s.frames_dropped + s.frames_skipped, "{s:?}");
    assert_eq!(e.shared().frame_credits.available(), 2, "every credit came back");
}

#[tokio::test]
async fn flush_waits_for_slow_guesses() {
    let g = Arc::new(SlowGuesser { delay: Duration::from_millis(300), calls: Default::default(), panic_first: false });
    let (e, _log) = engine_with(config(), |b| b.guesser(g.clone()));
    scan(&e, &Scene::three_products(), 8);
    let s = e.flush().await.unwrap();
    assert_eq!(s.items, 3);
    let items = e.catalog(false).await.unwrap();
    assert!(items.iter().all(|i| i.state == ItemState::Guessed), "flush returned before guesses finished: {:?}", items.iter().map(|i| i.state).collect::<Vec<_>>());
    assert_eq!(g.calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn a_panicking_plugin_becomes_a_warning_not_a_crash() {
    let g = Arc::new(SlowGuesser { delay: Duration::ZERO, calls: Default::default(), panic_first: true });
    let (e, log) = engine_with(config(), |b| b.guesser(g.clone()));
    scan(&e, &Scene::three_products(), 8);
    e.flush().await.unwrap();
    wait(&log, |ev| matches!(ev, Event::Warning { source, message } if source == "guess" && message.contains("panicked")));
    let items = e.catalog(false).await.unwrap();
    assert_eq!(items.iter().filter(|i| i.state == ItemState::Unknown).count(), 1);
    assert_eq!(items.iter().filter(|i| i.state == ItemState::Guessed).count(), 2);
    // asking again recovers the one that failed
    let failed = items.iter().find(|i| i.state == ItemState::Unknown).unwrap();
    e.reject_item(failed.id.clone(), None).await.unwrap();
    wait_item(&e, &failed.id, |i| i.state == ItemState::Guessed).await;
}

#[tokio::test]
async fn a_crashed_detect_stage_is_restarted_and_scanning_continues() {
    let (e, log) = engine();
    e.crash_stage_for_tests("detect");
    wait(&log, |ev| matches!(ev, Event::Warning { source, message } if source == "supervisor" && message.contains("Detect") && message.contains("restarted")));
    scan(&e, &Scene::three_products(), 8);
    assert_eq!(wait_guessed(&e, 3).await.len(), 3);
}

#[tokio::test]
async fn a_crashed_catalog_is_rebuilt_from_the_store() {
    let (e, log) = engine();
    scan(&e, &Scene::three_products(), 8);
    let before = wait_guessed(&e, 3).await;
    e.confirm_item(before[0].id.clone()).await.unwrap();
    e.flush().await.unwrap();
    e.crash_stage_for_tests("catalog");
    wait(&log, |ev| matches!(ev, Event::Warning { source, message } if source == "catalog" && message.contains("restored 3 items")));
    let after = e.catalog(true).await.unwrap();
    assert_eq!(after.len(), 3);
    for b in &before {
        let a = after.iter().find(|i| i.id == b.id).expect("same id");
        assert_eq!(a.guess.title, b.guess.title);
        assert!(!a.images.is_empty(), "images come back from the store");
    }
    assert_eq!(after.iter().find(|i| i.id == before[0].id).unwrap().state, ItemState::Confirmed { by: ConfirmedBy::User });
}

#[tokio::test]
async fn sqlite_keeps_items_skus_and_numbering_across_engines() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shelf.db").to_string_lossy().to_string();
    let build = |p: &str| {
        let log = EventLog::new();
        let e = EngineBuilder::new(config()).listener(log.clone()).sqlite_store(p.to_string()).unwrap().build().unwrap();
        (e, log)
    };
    let (first_ids, first_sku) = {
        let (e, _log) = build(&path);
        scan(&e, &Scene::three_products(), 8);
        let items = wait_guessed(&e, 3).await;
        for i in &items {
            e.confirm_item(i.id.clone()).await.unwrap();
        }
        let ls = e.generate_listings(false).await.unwrap();
        e.shutdown();
        (items.iter().map(|i| i.id.clone()).collect::<Vec<_>>(), ls[0].variants[0].sku.clone().unwrap())
    };

    let (e, log) = build(&path);
    assert!(e.catalog(true).await.unwrap().is_empty(), "a new session starts empty");
    assert_eq!(e.resume().await.unwrap(), 3);
    let resumed = e.catalog(false).await.unwrap();
    assert_eq!(resumed.iter().map(|i| i.id.clone()).collect::<std::collections::HashSet<_>>(), first_ids.iter().cloned().collect());
    assert!(resumed.iter().all(|i| matches!(i.state, ItemState::Confirmed { .. }) && !i.images.is_empty()));

    // numbering and SKUs continue where they left off
    let purple = Scene::empty(480, 360).with(shelfscan::BBox::new(0.4, 0.25, 0.2, 0.4), shelfscan::synthetic::PURPLE);
    scan(&e, &purple, 8);
    let all = wait_guessed(&e, 4).await;
    let new = all.iter().find(|i| !first_ids.contains(&i.id)).unwrap();
    assert_eq!(new.id, "item-004");
    e.confirm_item(new.id.clone()).await.unwrap();
    let ls = e.generate_listings(false).await.unwrap();
    let new_l = ls.iter().find(|l| l.source_item_id == "item-004").unwrap();
    assert_ne!(new_l.variants[0].sku.as_deref(), Some(first_sku.as_str()));
    assert!(new_l.variants[0].sku.as_deref().unwrap().starts_with("SKU-000"));
    let _ = log;
}

#[tokio::test]
async fn an_item_that_looks_like_a_saved_one_is_flagged_but_never_merged() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dup.db").to_string_lossy().to_string();
    // an exactly repeatable picture: no noise, no shake
    let still = |t0: u64| -> Vec<Frame> {
        let img = Scene::three_products().render((0.0, 0.0), 0, &mut Lcg::new(1));
        (0..6).map(|i| shelfscan::imaging::rgb_to_frame(&img, t0 + i * 100)).collect()
    };
    {
        let e = EngineBuilder::new(config()).sqlite_store(path.clone()).unwrap().build().unwrap();
        pump(&e, still(1000));
        e.flush().await.unwrap();
        assert_eq!(e.catalog(false).await.unwrap().len(), 3);
        e.shutdown();
    }
    let log = EventLog::new();
    let e = EngineBuilder::new(config()).listener(log.clone()).sqlite_store(path).unwrap().build().unwrap();
    pump(&e, still(1000));
    e.flush().await.unwrap();
    assert_eq!(e.catalog(false).await.unwrap().len(), 3, "new session, new items");
    wait(&log, |ev| matches!(ev, Event::ScanFlushed { .. }));
    assert_eq!(log.count(|ev| matches!(ev, Event::PossibleDuplicate { .. })), 3);
}

#[tokio::test]
async fn after_shutdown_every_command_says_closed() {
    let (e, _log) = engine();
    e.shutdown();
    assert!(matches!(e.flush().await, Err(ShelfError::Closed)));
    assert!(matches!(e.catalog(true).await, Err(ShelfError::Closed)));
    assert!(!e.push_frame(Scene::three_products().frames(1, 6.0, 1000, 1).remove(0)));
    e.shutdown(); // idempotent
}

#[tokio::test]
async fn default_gates_still_let_a_static_scene_through() {
    // default novelty/blur gates on: identical frames are skipped, but the keep-alive lets tracks mature
    let cfg = EngineConfig { analysis_fps: 20.0, ..EngineConfig::default() };
    let (e, _log) = engine_with(cfg, |b| b);
    let img = Scene::three_products().render((0.0, 0.0), 0, &mut Lcg::new(1));
    let frames: Vec<_> = (0..30).map(|i| shelfscan::imaging::rgb_to_frame(&img, 1000 + i * 100)).collect();
    pump(&e, frames);
    e.flush().await.unwrap();
    let s = e.stats();
    assert!(s.frames_skipped > 0, "{s:?}");
    assert_eq!(e.catalog(false).await.unwrap().len(), 3, "{s:?}");
}

#[cfg(feature = "barcode")]
#[tokio::test]
async fn showing_a_real_barcode_to_the_camera_confirms_the_focused_item_without_adding_items() {
    let log = EventLog::new();
    let e = EngineBuilder::new(config()).listener(log.clone()).product_lookup(Arc::new(FakeLookup)).build().unwrap();
    scan(&e, &Scene::three_products(), 8);
    let items = wait_guessed(&e, 3).await;
    let red = title_of(&items, "red").unwrap();
    let views_before = red.images.len();

    e.focus_item(red.id.clone()).await.unwrap();
    let frames: Vec<_> = (0..10).map(|i| shelfscan::synthetic::barcode_frame("4006381333931", 640, 480, 9000 + i * 150).unwrap()).collect();
    pump(&e, frames);
    wait(&log, |ev| matches!(ev, Event::BarcodeRead { gtin, .. } if gtin == "4006381333931"));
    let done = wait_item(&e, &red.id, |i| i.guess.title == "Stabilo Boss Highlighter").await;
    assert_eq!(done.state, ItemState::Confirmed { by: ConfirmedBy::Barcode });
    assert_eq!(done.gtin.as_deref(), Some("4006381333931"));

    e.clear_focus().await.unwrap();
    e.flush().await.unwrap();
    let all = e.catalog(true).await.unwrap();
    assert_eq!(all.len(), 3, "the barcode card was the focused item's extra view, not a new product: {:?}", all.iter().map(|i| (&i.id, &i.guess.title)).collect::<Vec<_>>());
    let after = all.iter().find(|i| i.id == red.id).unwrap();
    assert!(after.images.len() >= views_before, "views were kept");
}
