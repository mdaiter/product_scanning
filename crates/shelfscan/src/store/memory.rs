use std::{collections::HashMap, sync::Mutex};

use super::*;
use crate::error::ShelfError;

#[derive(Default)]
struct Inner {
    items: HashMap<String, (CatalogItem, Vec<f32>)>,
    counters: HashMap<String, u64>,
    fixtures: Vec<(String, Vec<f32>)>,
    kv: HashMap<String, (String, Option<u64>)>,
}

/// Nothing survives the process. The default.
#[derive(Default)]
pub struct MemoryStore {
    inner: Mutex<Inner>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Inner>> {
        self.inner.lock().map_err(|_| ShelfError::internal("memory store poisoned"))
    }
}

impl Store for MemoryStore {
    fn save_item(&self, item: &CatalogItem, embedding: &[f32]) -> Result<()> {
        self.lock()?.items.insert(item.id.clone(), (item.clone(), embedding.to_vec()));
        Ok(())
    }

    fn load_items(&self) -> Result<Vec<(CatalogItem, Vec<f32>)>> {
        Ok(self.lock()?.items.values().cloned().collect())
    }

    fn delete_item(&self, id: &str) -> Result<()> {
        self.lock()?.items.remove(id);
        Ok(())
    }

    fn clear_items(&self) -> Result<()> {
        self.lock()?.items.clear();
        Ok(())
    }

    fn next_sku(&self, prefix: &str) -> Result<String> {
        let p = sanitize_prefix(prefix);
        let mut g = self.lock()?;
        let n = g.counters.entry(p.clone()).or_insert(0);
        *n += 1;
        Ok(format_sku(&p, *n))
    }

    fn add_fixture(&self, label: &str, embedding: &[f32]) -> Result<()> {
        self.lock()?.fixtures.push((label.to_string(), embedding.to_vec()));
        Ok(())
    }

    fn nearest_fixture(&self, embedding: &[f32], min_cosine: f32) -> Result<Option<(String, f32)>> {
        let g = self.lock()?;
        let best = g
            .fixtures
            .iter()
            .map(|(l, e)| (l.clone(), crate::imaging::cosine(embedding, e)))
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        Ok(best.filter(|(_, s)| *s >= min_cosine))
    }

    fn nearest_items(&self, embedding: &[f32], k: usize) -> Result<Vec<(String, f32)>> {
        let g = self.lock()?;
        Ok(rank_by_cosine(embedding, g.items.iter().map(|(id, (_, e))| (id.as_str(), e.as_slice())), k))
    }

    fn search(&self, text: Option<&str>, embedding: Option<&[f32]>, k: usize) -> Result<Vec<(String, f32)>> {
        let g = self.lock()?;
        let mut lists: Vec<Vec<String>> = Vec::new();
        if let Some(q) = text {
            let qt = tokenize(q);
            if !qt.is_empty() {
                let mut scored: Vec<(String, usize)> = g
                    .items
                    .iter()
                    .filter_map(|(id, (it, _))| {
                        let words = tokenize(&item_text(it));
                        let hits = qt.iter().filter(|t| words.contains(t)).count();
                        (hits > 0).then(|| (id.clone(), hits))
                    })
                    .collect();
                scored.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                lists.push(scored.into_iter().map(|x| x.0).collect());
            }
        }
        if let Some(e) = embedding {
            let ranked = rank_by_cosine(e, g.items.iter().map(|(id, (_, v))| (id.as_str(), v.as_slice())), usize::MAX);
            lists.push(ranked.into_iter().map(|x| x.0).collect());
        }
        Ok(rrf(&lists, k))
    }

    fn kv_get(&self, key: &str) -> Result<Option<String>> {
        let mut g = self.lock()?;
        match g.kv.get(key) {
            Some((v, Some(exp))) if *exp <= now_secs() => {
                let _ = v;
                g.kv.remove(key);
                Ok(None)
            }
            Some((v, _)) => Ok(Some(v.clone())),
            None => Ok(None),
        }
    }

    fn kv_put(&self, key: &str, value: &str, ttl_secs: Option<u64>) -> Result<()> {
        self.lock()?.kv.insert(key.to_string(), (value.to_string(), ttl_secs.map(|t| now_secs() + t)));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract() {
        contract::run(&MemoryStore::new());
    }
}
