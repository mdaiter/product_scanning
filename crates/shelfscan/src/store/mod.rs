//! Persistence behind a trait. In-memory by default; SQLite when the app wants scans, the SKU
//! registry, the fixture library and the price cache to survive a restart.
//!
//! Search is hybrid: keyword ranking and vector ranking are fused with reciprocal rank fusion.
//! Vectors are L2-normalised floats; similarity is cosine, computed in Rust (fine for thousands
//! of items, no native extension needed on phones).

use crate::error::Result;
use crate::types::CatalogItem;

pub mod memory;
#[cfg(feature = "sqlite")]
pub mod sqlite;

pub use memory::MemoryStore;
#[cfg(feature = "sqlite")]
pub use sqlite::SqliteStore;

pub trait Store: Send + Sync {
    fn save_item(&self, item: &CatalogItem, embedding: &[f32]) -> Result<()>;
    fn load_items(&self) -> Result<Vec<(CatalogItem, Vec<f32>)>>;
    fn delete_item(&self, id: &str) -> Result<()>;
    fn clear_items(&self) -> Result<()>;

    /// Next unused SKU for `prefix`, e.g. `SKU-0001`. Monotonic; never reuses a number.
    fn next_sku(&self, prefix: &str) -> Result<String>;

    /// Remember an appearance the user marked as background so it is hidden next time too.
    fn add_fixture(&self, label: &str, embedding: &[f32]) -> Result<()>;
    fn nearest_fixture(&self, embedding: &[f32], min_cosine: f32) -> Result<Option<(String, f32)>>;

    /// Nearest saved items by cosine similarity, best first.
    fn nearest_items(&self, embedding: &[f32], k: usize) -> Result<Vec<(String, f32)>>;
    /// Hybrid search over saved items. Either or both of `text` and `embedding` may be given.
    /// Returns `(item_id, fused_score)`, best first.
    fn search(&self, text: Option<&str>, embedding: Option<&[f32]>, k: usize) -> Result<Vec<(String, f32)>>;

    fn kv_get(&self, key: &str) -> Result<Option<String>>;
    fn kv_put(&self, key: &str, value: &str, ttl_secs: Option<u64>) -> Result<()>;
}

// ───────────────────────────── shared helpers ─────────────────────────────

pub(crate) fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub(crate) fn sanitize_prefix(prefix: &str) -> String {
    let p: String = prefix.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect();
    if p.is_empty() { "SKU".into() } else { p }
}

pub(crate) fn format_sku(prefix: &str, n: u64) -> String {
    format!("{}-{:04}", sanitize_prefix(prefix), n)
}

/// Lowercase alphanumeric words of length >= 2.
pub(crate) fn tokenize(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 2)
        .map(|w| w.to_lowercase())
        .collect()
}

/// Searchable text for an item.
pub(crate) fn item_text(item: &CatalogItem) -> String {
    let mut parts = vec![item.guess.title.clone(), item.label.clone()];
    parts.extend(item.guess.brand.clone());
    parts.extend(item.guess.category.clone());
    parts.extend(item.gtin.clone());
    parts.extend(item.sku.clone());
    for a in &item.guess.attributes {
        parts.push(a.value.clone());
    }
    parts.join(" ")
}

/// Reciprocal rank fusion over ranked id lists.
pub(crate) fn rrf(lists: &[Vec<String>], k: usize) -> Vec<(String, f32)> {
    let mut score: std::collections::HashMap<String, f32> = std::collections::HashMap::new();
    for list in lists {
        for (rank, id) in list.iter().enumerate() {
            *score.entry(id.clone()).or_default() += 1.0 / (60.0 + rank as f32 + 1.0);
        }
    }
    let mut v: Vec<_> = score.into_iter().collect();
    v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.0.cmp(&b.0)));
    v.truncate(k);
    v
}

pub(crate) fn rank_by_cosine<'a>(q: &[f32], rows: impl Iterator<Item = (&'a str, &'a [f32])>, k: usize) -> Vec<(String, f32)> {
    let mut v: Vec<(String, f32)> = rows.map(|(id, e)| (id.to_string(), crate::imaging::cosine(q, e))).collect();
    v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.0.cmp(&b.0)));
    v.truncate(k);
    v
}

pub(crate) fn embedding_to_bytes(e: &[f32]) -> Vec<u8> {
    e.iter().flat_map(|f| f.to_le_bytes()).collect()
}

pub(crate) fn embedding_from_bytes(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

/// Behaviour every Store must have. Run against each implementation.
#[cfg(test)]
pub(crate) mod contract {
    use super::*;
    use crate::types::*;

    fn item(id: &str, title: &str, gtin: Option<&str>) -> CatalogItem {
        CatalogItem {
            id: id.into(),
            quantity: 1,
            state: ItemState::Guessed,
            role: Role::Product,
            mode: ScanMode::BarcodeFirst,
            guess: ProductGuess { title: title.into(), brand: Some("Acme".into()), category: None, attributes: vec![], confidence: 0.5 },
            guess_ready: true,
            label: "product".into(),
            gtin: gtin.map(String::from),
            sku: None,
            needs_barcode: true,
            images: vec![vec![9, 9, 9]],
            first_seen_ms: 1,
        }
    }

    pub fn run(store: &dyn Store) {
        // items round trip (images included)
        store.save_item(&item("a", "Black wool jacket", None), &[1.0, 0.0, 0.0]).unwrap();
        store.save_item(&item("b", "Scream ghost mask", Some("4006381333931")), &[0.0, 1.0, 0.0]).unwrap();
        store.save_item(&item("c", "Denim jacket", None), &[0.9, 0.1, 0.0]).unwrap();
        let mut all = store.load_items().unwrap();
        all.sort_by(|x, y| x.0.id.cmp(&y.0.id));
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].0.images, vec![vec![9, 9, 9]]);
        assert_eq!(all[0].1, vec![1.0, 0.0, 0.0]);

        // upsert replaces, does not duplicate
        let mut a2 = item("a", "Black wool jacket v2", None);
        a2.quantity = 3;
        store.save_item(&a2, &[1.0, 0.0, 0.0]).unwrap();
        assert_eq!(store.load_items().unwrap().len(), 3);

        // vector search
        let near = store.nearest_items(&[1.0, 0.05, 0.0], 2).unwrap();
        assert_eq!(near[0].0, "a");
        assert_eq!(near[1].0, "c");

        // keyword search
        let kw = store.search(Some("ghost"), None, 5).unwrap();
        assert_eq!(kw.first().map(|x| x.0.as_str()), Some("b"));
        let by_gtin = store.search(Some("4006381333931"), None, 5).unwrap();
        assert_eq!(by_gtin.first().map(|x| x.0.as_str()), Some("b"));

        // hybrid: keyword says "jacket" (a, c), vector says closest to c
        let hy = store.search(Some("jacket"), Some(&[0.8, 0.2, 0.0]), 3).unwrap();
        let ids: Vec<_> = hy.iter().map(|x| x.0.as_str()).collect();
        assert!(ids.contains(&"a") && ids.contains(&"c"));
        assert!(!ids[..2].contains(&"b"), "{ids:?}");

        // delete
        store.delete_item("c").unwrap();
        assert_eq!(store.load_items().unwrap().len(), 2);
        assert!(store.nearest_items(&[0.9, 0.1, 0.0], 5).unwrap().iter().all(|x| x.0 != "c"));
        assert!(store.search(Some("denim"), None, 5).unwrap().is_empty());
        store.delete_item("does-not-exist").unwrap();

        // sku registry is monotonic and prefix-scoped
        assert_eq!(store.next_sku("shop").unwrap(), "SHOP-0001");
        assert_eq!(store.next_sku("shop").unwrap(), "SHOP-0002");
        assert_eq!(store.next_sku("other").unwrap(), "OTHER-0001");

        // fixtures
        assert!(store.nearest_fixture(&[0.0, 0.0, 1.0], 0.9).unwrap().is_none());
        store.add_fixture("gargoyle", &[0.0, 0.0, 1.0]).unwrap();
        let f = store.nearest_fixture(&[0.0, 0.05, 1.0], 0.9).unwrap().unwrap();
        assert_eq!(f.0, "gargoyle");
        assert!(store.nearest_fixture(&[1.0, 0.0, 0.0], 0.9).unwrap().is_none());

        // kv with ttl
        store.kv_put("k", "v", None).unwrap();
        assert_eq!(store.kv_get("k").unwrap().as_deref(), Some("v"));
        store.kv_put("gone", "x", Some(0)).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        assert_eq!(store.kv_get("gone").unwrap(), None);
        assert_eq!(store.kv_get("missing").unwrap(), None);

        store.clear_items().unwrap();
        assert!(store.load_items().unwrap().is_empty());
        // clearing items keeps the registry and fixtures
        assert_eq!(store.next_sku("shop").unwrap(), "SHOP-0003");
        assert!(store.nearest_fixture(&[0.0, 0.0, 1.0], 0.9).unwrap().is_some());
    }
}
