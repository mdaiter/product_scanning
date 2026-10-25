use std::{path::Path, sync::Mutex};

use rusqlite::{params, Connection, OptionalExtension};

use super::*;
use crate::error::ShelfError;

fn db(e: rusqlite::Error) -> ShelfError {
    ShelfError::Io { msg: format!("sqlite: {e}") }
}

/// SQLite-backed store. Keyword search uses FTS5 (bundled SQLite); vectors are stored as blobs and
/// ranked by cosine in Rust; the two rankings are fused with reciprocal rank fusion.
pub struct SqliteStore {
    conn: Mutex<Connection>,
}

const SCHEMA: &str = "
create table if not exists items(id text primary key, json text not null, embedding blob not null);
create table if not exists item_images(item_id text not null, idx integer not null, jpeg blob not null, primary key(item_id, idx));
create virtual table if not exists items_fts using fts5(item_id unindexed, text);
create table if not exists fixtures(id integer primary key autoincrement, label text not null, embedding blob not null);
create table if not exists sku_counters(prefix text primary key, n integer not null);
create table if not exists kv(key text primary key, value text not null, expires_at integer);
";

impl SqliteStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::from_conn(Connection::open(path).map_err(db)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::from_conn(Connection::open_in_memory().map_err(db)?)
    }

    fn from_conn(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL").ok();
        conn.pragma_update(None, "synchronous", "NORMAL").ok();
        conn.execute_batch(SCHEMA).map_err(db)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.conn.lock().map_err(|_| ShelfError::internal("sqlite store poisoned"))
    }
}

/// FTS5 MATCH expression: every token quoted (so user text can never be syntax), OR-ed together.
fn fts_query(text: &str) -> Option<String> {
    let toks = tokenize(text);
    if toks.is_empty() {
        return None;
    }
    Some(toks.iter().map(|t| format!("\"{}\"", t.replace('"', ""))).collect::<Vec<_>>().join(" OR "))
}

impl Store for SqliteStore {
    fn save_item(&self, item: &CatalogItem, embedding: &[f32]) -> Result<()> {
        let json = serde_json::to_string(item).map_err(ShelfError::internal)?;
        let mut g = self.lock()?;
        let tx = g.transaction().map_err(db)?;
        tx.execute(
            "insert into items(id, json, embedding) values (?1, ?2, ?3)
             on conflict(id) do update set json = excluded.json, embedding = excluded.embedding",
            params![item.id, json, embedding_to_bytes(embedding)],
        )
        .map_err(db)?;
        tx.execute("delete from item_images where item_id = ?1", params![item.id]).map_err(db)?;
        for (i, jpeg) in item.images.iter().enumerate() {
            tx.execute("insert into item_images(item_id, idx, jpeg) values (?1, ?2, ?3)", params![item.id, i as i64, jpeg]).map_err(db)?;
        }
        tx.execute("delete from items_fts where item_id = ?1", params![item.id]).map_err(db)?;
        tx.execute("insert into items_fts(item_id, text) values (?1, ?2)", params![item.id, item_text(item)]).map_err(db)?;
        tx.commit().map_err(db)
    }

    fn load_items(&self) -> Result<Vec<(CatalogItem, Vec<f32>)>> {
        let g = self.lock()?;
        let mut st = g.prepare("select id, json, embedding from items order by id").map_err(db)?;
        let rows: Vec<(String, String, Vec<u8>)> = st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(db)?
            .collect::<std::result::Result<_, _>>()
            .map_err(db)?;
        let mut img = g.prepare("select jpeg from item_images where item_id = ?1 order by idx").map_err(db)?;
        let mut out = Vec::with_capacity(rows.len());
        for (id, json, emb) in rows {
            let mut item: CatalogItem = serde_json::from_str(&json).map_err(ShelfError::internal)?;
            item.images = img.query_map(params![id], |r| r.get::<_, Vec<u8>>(0)).map_err(db)?.collect::<std::result::Result<_, _>>().map_err(db)?;
            out.push((item, embedding_from_bytes(&emb)));
        }
        Ok(out)
    }

    fn delete_item(&self, id: &str) -> Result<()> {
        let mut g = self.lock()?;
        let tx = g.transaction().map_err(db)?;
        tx.execute("delete from items where id = ?1", params![id]).map_err(db)?;
        tx.execute("delete from item_images where item_id = ?1", params![id]).map_err(db)?;
        tx.execute("delete from items_fts where item_id = ?1", params![id]).map_err(db)?;
        tx.commit().map_err(db)
    }

    fn clear_items(&self) -> Result<()> {
        self.lock()?.execute_batch("delete from items; delete from item_images; delete from items_fts;").map_err(db)
    }

    fn next_sku(&self, prefix: &str) -> Result<String> {
        let p = sanitize_prefix(prefix);
        let mut g = self.lock()?;
        let tx = g.transaction().map_err(db)?;
        tx.execute("insert into sku_counters(prefix, n) values (?1, 1) on conflict(prefix) do update set n = n + 1", params![p]).map_err(db)?;
        let n: i64 = tx.query_row("select n from sku_counters where prefix = ?1", params![p], |r| r.get(0)).map_err(db)?;
        tx.commit().map_err(db)?;
        Ok(format_sku(&p, n as u64))
    }

    fn add_fixture(&self, label: &str, embedding: &[f32]) -> Result<()> {
        self.lock()?.execute("insert into fixtures(label, embedding) values (?1, ?2)", params![label, embedding_to_bytes(embedding)]).map_err(db)?;
        Ok(())
    }

    fn nearest_fixture(&self, embedding: &[f32], min_cosine: f32) -> Result<Option<(String, f32)>> {
        let g = self.lock()?;
        let mut st = g.prepare("select label, embedding from fixtures").map_err(db)?;
        let rows: Vec<(String, Vec<u8>)> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(db)?.collect::<std::result::Result<_, _>>().map_err(db)?;
        let best = rows
            .into_iter()
            .map(|(l, e)| (l, crate::imaging::cosine(embedding, &embedding_from_bytes(&e))))
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        Ok(best.filter(|(_, s)| *s >= min_cosine))
    }

    fn nearest_items(&self, embedding: &[f32], k: usize) -> Result<Vec<(String, f32)>> {
        let g = self.lock()?;
        let mut st = g.prepare("select id, embedding from items").map_err(db)?;
        let rows: Vec<(String, Vec<f32>)> = st
            .query_map([], |r| Ok((r.get::<_, String>(0)?, embedding_from_bytes(&r.get::<_, Vec<u8>>(1)?))))
            .map_err(db)?
            .collect::<std::result::Result<_, _>>()
            .map_err(db)?;
        Ok(rank_by_cosine(embedding, rows.iter().map(|(id, e)| (id.as_str(), e.as_slice())), k))
    }

    fn search(&self, text: Option<&str>, embedding: Option<&[f32]>, k: usize) -> Result<Vec<(String, f32)>> {
        let mut lists: Vec<Vec<String>> = Vec::new();
        if let Some(q) = text.and_then(fts_query) {
            let g = self.lock()?;
            let mut st = g.prepare("select item_id from items_fts where items_fts match ?1 order by bm25(items_fts) limit 200").map_err(db)?;
            let ids: Vec<String> = st.query_map(params![q], |r| r.get(0)).map_err(db)?.collect::<std::result::Result<_, _>>().map_err(db)?;
            lists.push(ids);
        }
        if let Some(e) = embedding {
            lists.push(self.nearest_items(e, usize::MAX)?.into_iter().map(|x| x.0).collect());
        }
        Ok(rrf(&lists, k))
    }

    fn kv_get(&self, key: &str) -> Result<Option<String>> {
        let g = self.lock()?;
        let row: Option<(String, Option<i64>)> = g
            .query_row("select value, expires_at from kv where key = ?1", params![key], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()
            .map_err(db)?;
        match row {
            Some((_, Some(exp))) if exp as u64 <= now_secs() => {
                g.execute("delete from kv where key = ?1", params![key]).map_err(db)?;
                Ok(None)
            }
            Some((v, _)) => Ok(Some(v)),
            None => Ok(None),
        }
    }

    fn kv_put(&self, key: &str, value: &str, ttl_secs: Option<u64>) -> Result<()> {
        let exp = ttl_secs.map(|t| (now_secs() + t) as i64);
        self.lock()?
            .execute("insert into kv(key, value, expires_at) values (?1, ?2, ?3) on conflict(key) do update set value = excluded.value, expires_at = excluded.expires_at", params![key, value, exp])
            .map_err(db)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_in_memory() {
        contract::run(&SqliteStore::open_in_memory().unwrap());
    }

    #[test]
    fn survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shelf.db");
        {
            let s = SqliteStore::open(&path).unwrap();
            assert_eq!(s.next_sku("acme").unwrap(), "ACME-0001");
            s.add_fixture("rack", &[1.0, 0.0]).unwrap();
            s.kv_put("k", "v", None).unwrap();
        }
        let s = SqliteStore::open(&path).unwrap();
        assert_eq!(s.next_sku("acme").unwrap(), "ACME-0002");
        assert!(s.nearest_fixture(&[1.0, 0.0], 0.9).unwrap().is_some());
        assert_eq!(s.kv_get("k").unwrap().as_deref(), Some("v"));
    }

    #[test]
    fn user_text_cannot_break_fts_syntax() {
        let s = SqliteStore::open_in_memory().unwrap();
        for q in ["\"unbalanced", "a AND OR NOT (", "col:zz*", "'; drop table items;--"] {
            s.search(Some(q), None, 3).unwrap();
        }
    }
}
