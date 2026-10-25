//! Shopify product CSV export. Always `Status = draft` and `Published on online store = false`.
//!
//! Headers follow Shopify's current CSV documentation (checked against help.shopify.com; not
//! import-tested on a live store). Images are not included: the CSV needs public image URLs,
//! which a phone-only flow does not have yet (deferred).

use std::{
    collections::HashSet,
    fs::{File, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use crate::error::{Result, ShelfError};
use crate::traits::Storefront;
use crate::types::*;

pub const HEADERS: [&str; 20] = [
    "Title",
    "URL handle",
    "Description",
    "Vendor",
    "Product category",
    "Type",
    "Tags",
    "Published on online store",
    "Status",
    "SKU",
    "Barcodes",
    "Option1 name",
    "Option1 value",
    "Price",
    "Compare-at price",
    "Inventory tracker",
    "Inventory quantity",
    "Continue selling when out of stock",
    "SEO title",
    "SEO description",
];

#[derive(Default)]
struct State {
    started: bool,
    handles: HashSet<String>,
}

#[derive(uniffi::Object)]
pub struct CsvStorefront {
    path: PathBuf,
    push_inventory: bool,
    state: Mutex<State>,
}

#[uniffi::export]
impl CsvStorefront {
    /// Writes (and on first use truncates) `path`; later publishes through the same object append.
    /// `push_inventory` writes the scanned quantity estimate as stock; leave it off unless you trust the count.
    #[uniffi::constructor]
    pub fn new(path: String, push_inventory: bool) -> Arc<Self> {
        Arc::new(Self { path: PathBuf::from(path), push_inventory, state: Mutex::new(State::default()) })
    }
}

pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let t = out.trim_matches('-').to_string();
    if t.is_empty() { "product".into() } else { t.chars().take(80).collect() }
}

fn field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) || s.starts_with(' ') || s.ends_with(' ') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn row(cells: &[String]) -> String {
    let mut line = cells.iter().map(|c| field(c)).collect::<Vec<_>>().join(",");
    line.push('\n');
    line
}

impl CsvStorefront {
    fn rows_for(&self, l: &Listing, handles: &mut HashSet<String>) -> std::result::Result<String, String> {
        if l.options.len() > 1 {
            return Err("the CSV export supports one option per product; use the Admin API storefront for more".into());
        }
        let base = slug(&l.title);
        let mut handle = base.clone();
        let mut n = 2;
        while !handles.insert(handle.clone()) {
            handle = format!("{base}-{n}");
            n += 1;
        }
        let (opt_name, opt_values): (String, Vec<String>) = match l.options.first() {
            Some(o) => (o.name.clone(), o.values.clone()),
            None => ("Title".into(), vec!["Default Title".into()]),
        };
        let variants: Vec<&ListingVariant> = l.variants.iter().collect();
        if variants.is_empty() {
            return Err("listing has no variants".into());
        }
        let mut out = String::new();
        for (i, v) in variants.iter().enumerate() {
            let opt_value = if l.options.is_empty() { opt_values[0].clone() } else { v.option_values.first().cloned().ok_or("variant is missing its option value")? };
            let first = i == 0;
            let cell = |s: &str| if first { s.to_string() } else { String::new() };
            out.push_str(&row(&[
                cell(&l.title),
                handle.clone(),
                cell(&l.description_html),
                cell(&l.vendor),
                String::new(),
                cell(&l.product_type),
                cell(&l.tags.join(", ")),
                cell("false"),
                cell("draft"),
                v.sku.clone().unwrap_or_default(),
                v.barcode.clone().unwrap_or_default(),
                if first { opt_name.clone() } else { String::new() },
                opt_value,
                v.price.clone(),
                v.compare_at_price.clone().unwrap_or_default(),
                if self.push_inventory { "shopify".into() } else { String::new() },
                if self.push_inventory { v.quantity.to_string() } else { String::new() },
                if self.push_inventory { "deny".into() } else { String::new() },
                cell(l.seo_title.as_deref().unwrap_or("")),
                cell(l.seo_description.as_deref().unwrap_or("")),
            ]));
        }
        Ok(out)
    }
}

#[uniffi::export]
impl Storefront for CsvStorefront {
    fn name(&self) -> String {
        "Shopify CSV".into()
    }

    fn publish(&self, listings: Vec<Listing>) -> Result<Vec<PublishReceipt>> {
        let mut st = self.state.lock().map_err(|_| ShelfError::internal("csv storefront poisoned"))?;
        let mut body = String::new();
        let mut receipts = Vec::new();
        for l in &listings {
            match self.rows_for(l, &mut st.handles) {
                Ok(rows) => {
                    body.push_str(&rows);
                    receipts.push(PublishReceipt { listing_id: l.id.clone(), ok: true, remote_id: None, url: None, message: format!("written to {}", self.path.display()) });
                }
                Err(msg) => receipts.push(PublishReceipt { listing_id: l.id.clone(), ok: false, remote_id: None, url: None, message: msg }),
            }
        }
        let mut file: File = if st.started {
            OpenOptions::new().append(true).open(&self.path)?
        } else {
            let mut f = File::create(&self.path)?;
            f.write_all(row(&HEADERS.map(String::from)).as_bytes())?;
            f
        };
        file.write_all(body.as_bytes())?;
        file.flush()?;
        st.started = true;
        Ok(receipts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing(id: &str, title: &str) -> Listing {
        Listing {
            id: id.into(),
            source_item_id: "item-001".into(),
            title: title.into(),
            description_html: "<p>Warm, \"cosy\",\nand soft.</p>".into(),
            vendor: "Acme, Inc.".into(),
            product_type: "Jackets".into(),
            tags: vec!["wool".into(), "black".into()],
            status: ListingStatus::Active, // the storefront must still write draft
            options: vec![],
            variants: vec![ListingVariant { option_values: vec![], sku: Some("SKU-0001".into()), price: "0.00".into(), compare_at_price: None, quantity: 4, barcode: Some("4006381333931".into()) }],
            images: vec![],
            seo_title: Some("Black wool jacket".into()),
            seo_description: None,
            confidence: 0.5,
            published: false,
        }
    }

    fn read(path: &std::path::Path) -> (Vec<String>, Vec<std::collections::HashMap<String, String>>) {
        let mut rdr = csv::Reader::from_path(path).unwrap();
        let headers: Vec<String> = rdr.headers().unwrap().iter().map(String::from).collect();
        let rows = rdr.records().map(|r| headers.iter().cloned().zip(r.unwrap().iter().map(String::from)).collect()).collect();
        (headers, rows)
    }

    #[test]
    fn writes_draft_rows_with_tricky_text_and_no_inventory_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("products.csv");
        let sf = CsvStorefront::new(path.to_string_lossy().to_string(), false);
        let rs = sf.publish(vec![listing("listing-001", "Black Wool Jacket"), listing("listing-002", "Black Wool Jacket")]).unwrap();
        assert!(rs.iter().all(|r| r.ok));
        let (headers, rows) = read(&path);
        assert_eq!(headers, HEADERS);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["Status"], "draft");
        assert_eq!(rows[0]["Published on online store"], "false");
        assert_eq!(rows[0]["Description"], "<p>Warm, \"cosy\",\nand soft.</p>");
        assert_eq!(rows[0]["Vendor"], "Acme, Inc.");
        assert_eq!(rows[0]["Tags"], "wool, black");
        assert_eq!(rows[0]["SKU"], "SKU-0001");
        assert_eq!(rows[0]["Barcodes"], "4006381333931");
        assert_eq!(rows[0]["Option1 name"], "Title");
        assert_eq!(rows[0]["Option1 value"], "Default Title");
        assert_eq!(rows[0]["Inventory quantity"], "");
        assert_eq!(rows[0]["Inventory tracker"], "");
        assert_eq!(rows[0]["URL handle"], "black-wool-jacket");
        assert_eq!(rows[1]["URL handle"], "black-wool-jacket-2", "handles stay unique");
    }

    #[test]
    fn appends_across_calls_with_one_header_and_truncates_an_old_file_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.csv");
        std::fs::write(&path, "old,garbage\n1,2\n").unwrap();
        let sf = CsvStorefront::new(path.to_string_lossy().to_string(), true);
        sf.publish(vec![listing("a", "One")]).unwrap();
        sf.publish(vec![listing("b", "Two")]).unwrap();
        let (_, rows) = read(&path);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1]["Title"], "Two");
        assert_eq!(rows[0]["Inventory quantity"], "4", "inventory only when asked");
        assert_eq!(rows[0]["Inventory tracker"], "shopify");
        assert_eq!(std::fs::read_to_string(&path).unwrap().matches("URL handle").count(), 1);
    }

    #[test]
    fn extra_variants_continue_the_same_handle_and_unsupported_shapes_fail_per_listing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v.csv");
        let sf = CsvStorefront::new(path.to_string_lossy().to_string(), false);
        let mut multi = listing("m", "Tee");
        multi.options = vec![ListingOption { name: "Size".into(), values: vec!["S".into(), "M".into()] }];
        multi.variants = ["S", "M"].iter().map(|s| ListingVariant { option_values: vec![s.to_string()], sku: None, price: "10.00".into(), compare_at_price: None, quantity: 0, barcode: None }).collect();
        let mut two_opts = listing("x", "Hat");
        two_opts.options = vec![ListingOption { name: "Size".into(), values: vec![] }, ListingOption { name: "Color".into(), values: vec![] }];
        let rs = sf.publish(vec![multi, two_opts, listing("ok", "Scarf")]).unwrap();
        assert!(rs[0].ok && !rs[1].ok && rs[2].ok);
        assert!(rs[1].message.contains("one option"));
        let (_, rows) = read(&path);
        assert_eq!(rows.len(), 3);
        assert_eq!((rows[0]["Title"].as_str(), rows[0]["Option1 value"].as_str()), ("Tee", "S"));
        assert_eq!((rows[1]["Title"].as_str(), rows[1]["URL handle"].as_str(), rows[1]["Option1 value"].as_str()), ("", "tee", "M"));
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("  Hello, World! "), "hello-world");
        assert_eq!(slug("!!!"), "product");
        assert_eq!(slug("Crème brûlée"), "cr-me-br-l-e");
    }
}
