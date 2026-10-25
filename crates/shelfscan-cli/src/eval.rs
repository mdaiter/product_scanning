//! `shelfscan score --run DIR --truth truth.json`: compare a scan's SKUs with the ground truth (Phase 1.6).
//!
//! A run folder is what a scan writes: `catalog.json` (all items, hidden ones included) and `crops/<item>.jpg`.
//! Found items are tied to truth items by **labels** (`labels.json`: item id -> truth names). An item labelled with two
//! names is a false merge, a truth item with two items is a false split, an item labelled `[]` is not a product.
//! Items whose GTIN matches a truth GTIN are labelled automatically; the rest are asked on the terminal (with the crop
//! path) when stdin is interactive, and the answers are saved back to the labels file so a run is labelled once.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::IsTerminal,
    path::Path,
};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use shelfscan::Role;

#[derive(Debug, Clone, Deserialize)]
pub struct TruthItem {
    pub name: String,
    #[serde(default)]
    pub gtin: Option<String>,
    pub quantity: u32,
    #[serde(default)]
    pub same_colour_as: Vec<String>,
}

/// The fields of `CatalogItem` the scorer needs (other writers, e.g. a Swift replay tool, only have to emit these).
#[derive(Debug, Clone, Deserialize)]
pub struct RunItem {
    pub id: String,
    pub quantity: u32,
    pub role: Role,
    #[serde(default)]
    pub gtin: Option<String>,
    #[serde(default)]
    pub label: String,
}

pub type Labels = BTreeMap<String, Vec<String>>;

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct TruthResult {
    pub name: String,
    pub items: Vec<String>,
    pub quantity_truth: u32,
    pub quantity_found: u32,
    pub quantity_exact: bool,
}

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct Score {
    pub truth_items: usize,
    /// Product items (role Product) found in the scan.
    pub skus_found: usize,
    pub truth_found: usize,
    pub false_merges: usize,
    /// Of the false merges, how many joined two items listed as same colour in the truth sheet.
    pub same_colour_merges: usize,
    pub false_splits: usize,
    /// Product items that are not one of the truth items (decor, table, a hand, ...).
    pub not_products: usize,
    /// Truth items that only appear as hidden items (Background / Excluded).
    pub hidden_truth: usize,
    pub quantity_exact: usize,
    pub per_truth: Vec<TruthResult>,
}

pub fn score(truth: &[TruthItem], items: &[RunItem], labels: &Labels) -> Score {
    let names: BTreeSet<&str> = truth.iter().map(|t| t.name.as_str()).collect();
    let label_of = |id: &str| -> Vec<&str> { labels.get(id).map(|v| v.iter().map(String::as_str).filter(|n| names.contains(n)).collect()).unwrap_or_default() };
    let products: Vec<&RunItem> = items.iter().filter(|i| i.role == Role::Product).collect();
    let mut s = Score { truth_items: truth.len(), skus_found: products.len(), ..Score::default() };

    for i in &products {
        let l = label_of(&i.id);
        match l.len() {
            0 => s.not_products += 1,
            1 => {}
            n => {
                s.false_merges += n - 1;
                let same = l.iter().enumerate().any(|(a, x)| {
                    l.iter().skip(a + 1).any(|y| truth.iter().any(|t| (t.name == *x && t.same_colour_as.iter().any(|c| c == y)) || (t.name == *y && t.same_colour_as.iter().any(|c| c == x))))
                });
                if same {
                    s.same_colour_merges += 1;
                }
            }
        }
    }
    for t in truth {
        let mine: Vec<&RunItem> = products.iter().copied().filter(|i| label_of(&i.id).contains(&t.name.as_str())).collect();
        let hidden = items.iter().any(|i| i.role != Role::Product && label_of(&i.id).contains(&t.name.as_str()));
        let found: u32 = mine.iter().map(|i| i.quantity).sum();
        // quantity counts only when the product is one clean item: not split, not merged with another
        let exact = mine.len() == 1 && label_of(&mine[0].id).len() == 1 && found == t.quantity;
        if !mine.is_empty() {
            s.truth_found += 1;
            s.false_splits += mine.len() - 1;
        } else if hidden {
            s.hidden_truth += 1;
        }
        if exact {
            s.quantity_exact += 1;
        }
        s.per_truth.push(TruthResult { name: t.name.clone(), items: mine.iter().map(|i| i.id.clone()).collect(), quantity_truth: t.quantity, quantity_found: found, quantity_exact: exact });
    }
    s
}

/// GTIN matches label themselves; existing labels are kept.
pub fn auto_label(truth: &[TruthItem], items: &[RunItem], labels: &mut Labels) {
    for i in items {
        if labels.contains_key(&i.id) {
            continue;
        }
        if let Some(g) = &i.gtin {
            let hits: Vec<String> = truth.iter().filter(|t| t.gtin.as_deref() == Some(g.as_str())).map(|t| t.name.clone()).collect();
            if !hits.is_empty() {
                labels.insert(i.id.clone(), hits);
            }
        }
    }
}

fn ask_labels(truth: &[TruthItem], items: &[RunItem], labels: &mut Labels, run: &Path) -> Result<()> {
    let missing: Vec<&RunItem> = items.iter().filter(|i| !labels.contains_key(&i.id)).collect();
    if missing.is_empty() {
        return Ok(());
    }
    if !std::io::stdin().is_terminal() {
        bail!("{} item(s) have no label ({}); add them to the labels file or run interactively", missing.len(), missing.iter().map(|i| i.id.as_str()).collect::<Vec<_>>().join(", "));
    }
    eprintln!("\nLabel each item: numbers of the truth items it shows (comma-separated), or - if it is not one of them.");
    for (n, t) in truth.iter().enumerate() {
        eprintln!("  {:>2}. {}", n + 1, t.name);
    }
    for i in missing {
        let crop = run.join("crops").join(format!("{}.jpg", i.id));
        loop {
            eprint!("{} x{} {:?} \"{}\"  crop: {}\n> ", i.id, i.quantity, i.role, i.label, crop.display());
            let mut line = String::new();
            if std::io::stdin().read_line(&mut line)? == 0 {
                bail!("labelling aborted");
            }
            let line = line.trim();
            if line == "-" {
                labels.insert(i.id.clone(), vec![]);
                break;
            }
            let picked: Option<Vec<String>> = line.split(',').map(|x| x.trim().parse::<usize>().ok().and_then(|k| truth.get(k.wrapping_sub(1))).map(|t| t.name.clone())).collect();
            match picked {
                Some(v) if !v.is_empty() => {
                    labels.insert(i.id.clone(), v);
                    break;
                }
                _ => eprintln!("  numbers 1..{} separated by commas, or -", truth.len()),
            }
        }
    }
    Ok(())
}

pub fn print(s: &Score) {
    eprintln!("\nSKUs found {} (truth {}), truth items found {}", s.skus_found, s.truth_items, s.truth_found);
    eprintln!("false merges {} (same colour {}), false splits {}, not products {}, truth only hidden {}", s.false_merges, s.same_colour_merges, s.false_splits, s.not_products, s.hidden_truth);
    eprintln!("quantity exact {}/{}", s.quantity_exact, s.truth_items);
    for t in &s.per_truth {
        let ok = if t.quantity_exact { "" } else { "  <-" };
        eprintln!("  {:<28} items [{}]  qty {}/{}{}", t.name, t.items.join(", "), t.quantity_found, t.quantity_truth, ok);
    }
}

/// Load, label (auto, then interactive), score, print, and write `eval.json` into the run folder.
pub fn run(run_dir: &Path, truth_path: &Path, labels_path: Option<&Path>) -> Result<Score> {
    let truth: Vec<TruthItem> = serde_json::from_slice(&std::fs::read(truth_path).with_context(|| format!("reading {}", truth_path.display()))?).context("truth.json")?;
    let items: Vec<RunItem> = serde_json::from_slice(&std::fs::read(run_dir.join("catalog.json")).with_context(|| format!("reading {}/catalog.json", run_dir.display()))?).context("catalog.json")?;
    let default_labels = run_dir.join("labels.json");
    let labels_path = labels_path.unwrap_or(&default_labels);
    let mut labels: Labels = match std::fs::read(labels_path) {
        Ok(b) => serde_json::from_slice(&b).context("labels.json")?,
        Err(_) => Labels::new(),
    };
    auto_label(&truth, &items, &mut labels);
    let asked = ask_labels(&truth, &items, &mut labels, run_dir);
    std::fs::write(labels_path, serde_json::to_string_pretty(&labels)?)?;
    asked?;
    let s = score(&truth, &items, &labels);
    print(&s);
    std::fs::write(run_dir.join("eval.json"), serde_json::to_string_pretty(&s)?)?;
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(name: &str, q: u32, same: &[&str]) -> TruthItem {
        TruthItem { name: name.into(), gtin: None, quantity: q, same_colour_as: same.iter().map(|s| s.to_string()).collect() }
    }
    fn i(id: &str, q: u32, role: Role) -> RunItem {
        RunItem { id: id.into(), quantity: q, role, gtin: None, label: String::new() }
    }
    fn labels(pairs: &[(&str, &[&str])]) -> Labels {
        pairs.iter().map(|(k, v)| (k.to_string(), v.iter().map(|s| s.to_string()).collect())).collect()
    }

    #[test]
    fn perfect_run() {
        let truth = [t("a", 1, &[]), t("b", 2, &[])];
        let items = [i("1", 1, Role::Product), i("2", 2, Role::Product)];
        let s = score(&truth, &items, &labels(&[("1", &["a"]), ("2", &["b"])]));
        assert_eq!((s.truth_found, s.false_merges, s.false_splits, s.not_products, s.quantity_exact), (2, 0, 0, 0, 2));
    }

    #[test]
    fn merge_split_decor_and_hidden() {
        let truth = [t("red mug", 1, &["red box"]), t("red box", 1, &["red mug"]), t("tea", 3, &[]), t("pen", 1, &[])];
        let items = [
            i("1", 1, Role::Product),    // red mug + red box merged
            i("2", 2, Role::Product),    // tea, part one
            i("3", 1, Role::Product),    // tea, part two (split)
            i("4", 1, Role::Product),    // the plant
            i("5", 1, Role::Background), // pen, hidden as background
        ];
        let l = labels(&[("1", &["red mug", "red box"]), ("2", &["tea"]), ("3", &["tea"]), ("4", &[]), ("5", &["pen"])]);
        let s = score(&truth, &items, &l);
        assert_eq!(s.skus_found, 4);
        assert_eq!(s.false_merges, 1);
        assert_eq!(s.same_colour_merges, 1);
        assert_eq!(s.false_splits, 1);
        assert_eq!(s.not_products, 1);
        assert_eq!(s.hidden_truth, 1);
        assert_eq!(s.truth_found, 3);
        // merged and split items never count as exact quantity, even when the numbers add up (tea: 2 + 1 = 3)
        assert_eq!(s.quantity_exact, 0);
        let tea = s.per_truth.iter().find(|r| r.name == "tea").unwrap();
        assert_eq!((tea.quantity_found, tea.items.len()), (3, 2));
    }

    #[test]
    fn gtin_labels_itself_and_keeps_existing_labels() {
        let mut truth = vec![t("cereal", 1, &[])];
        truth[0].gtin = Some("4006381333931".into());
        let mut a = i("1", 1, Role::Product);
        a.gtin = Some("4006381333931".into());
        let mut b = i("2", 1, Role::Product);
        b.gtin = Some("4006381333931".into());
        let mut l = labels(&[("2", &[])]);
        auto_label(&truth, &[a, b], &mut l);
        assert_eq!(l["1"], vec!["cereal".to_string()]);
        assert!(l["2"].is_empty());
    }

    #[test]
    fn reads_catalog_json_written_by_the_engine() {
        // the scorer must accept the CLI's own catalog.json (full CatalogItem) as well as a minimal writer's
        let full = r#"[{"id":"item-001","quantity":2,"state":"Guessed","role":"Product","mode":"BarcodeFirst",
            "guess":{"title":"x","brand":null,"category":null,"attributes":[],"confidence":0.0},"guess_ready":true,
            "label":"x","gtin":null,"sku":null,"needs_barcode":true,"first_seen_ms":1000}]"#;
        let items: Vec<RunItem> = serde_json::from_str(full).unwrap();
        assert_eq!((items[0].quantity, items[0].role), (2, Role::Product));
        let minimal: Vec<RunItem> = serde_json::from_str(r#"[{"id":"a","quantity":1,"role":"Background"}]"#).unwrap();
        assert_eq!(minimal[0].role, Role::Background);
    }
}
