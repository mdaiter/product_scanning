use std::sync::Arc;

use crate::error::Result;
use crate::traits::{ListingWriter, ProductGuesser};
use crate::types::*;

const NOUNS: [&str; 4] = ["product", "item", "object", "thing"];

/// Offline guesser: "Unidentified red item". It never claims to know what something is
/// (confidence 0) and honours `excluded_titles` so the reject → re-guess loop is testable.
/// A specific detector class (e.g. "bottle" from a COCO model) is tried first as the noun: "Unidentified red bottle?"
/// is still a guess, which is why the confidence stays 0.
#[derive(uniffi::Object)]
pub struct PlaceholderGuesser;

#[uniffi::export]
impl PlaceholderGuesser {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self)
    }
}

#[uniffi::export]
impl ProductGuesser for PlaceholderGuesser {
    fn guess(&self, request: GuessRequest) -> Result<ProductGuess> {
        let req = request;
        let color = req.dominant_colors.first().cloned();
        let label = req.detector_label.trim().to_lowercase();
        let specific = (!label.is_empty() && !NOUNS.contains(&label.as_str())).then_some(label);
        let nouns: Vec<&str> = specific.iter().map(String::as_str).chain(NOUNS.iter().copied()).collect();
        let candidates = nouns.iter().map(|n| match &color {
            Some(c) => format!("Unidentified {c} {n}"),
            None => format!("Unidentified {n}"),
        });
        let title = candidates
            .clone()
            .find(|t| !req.excluded_titles.iter().any(|e| e.eq_ignore_ascii_case(t)))
            .unwrap_or_else(|| format!("Unidentified product #{}", req.excluded_titles.len() + 1));
        let attributes = req.dominant_colors.iter().take(2).map(|c| Attribute { name: "Color".into(), value: c.clone() }).collect();
        Ok(ProductGuess { title, brand: None, category: None, attributes, confidence: 0.0 })
    }
}

/// Offline writer: a plain draft from what is known, with every unknown clearly marked
/// "needs your input". Prices are left empty; the engine fills in the configured placeholder.
#[derive(uniffi::Object)]
pub struct TemplateWriter;

#[uniffi::export]
impl TemplateWriter {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self)
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn trim_to(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let cut: String = s.chars().take(n.saturating_sub(1)).collect();
        format!("{}…", cut.trim_end())
    }
}

#[uniffi::export]
impl ListingWriter for TemplateWriter {
    fn write(&self, item: CatalogItem, _style_prompt: String) -> Result<Listing> {
        let title = if item.guess.title.trim().is_empty() { "Untitled product".to_string() } else { item.guess.title.trim().to_string() };
        let mut html = format!("<p>{}</p>", esc(&title));
        if !item.guess.attributes.is_empty() {
            html.push_str("<ul>");
            for a in &item.guess.attributes {
                html.push_str(&format!("<li>{}: {}</li>", esc(&a.name), esc(&a.value)));
            }
            html.push_str("</ul>");
        }
        html.push_str("<p><em>[Needs your input: description, size, material, condition]</em></p>");
        let confirmed = matches!(item.state, ItemState::Confirmed { .. });
        let mut tags = vec!["shelfscan".to_string()];
        if !confirmed {
            tags.push("needs-review".into());
        }
        Ok(Listing {
            id: String::new(),
            source_item_id: item.id.clone(),
            title: title.clone(),
            description_html: html,
            vendor: item.guess.brand.clone().unwrap_or_default(),
            product_type: item.guess.category.clone().unwrap_or_default(),
            tags,
            status: ListingStatus::Draft,
            options: vec![],
            variants: vec![ListingVariant { option_values: vec![], sku: item.sku.clone(), price: String::new(), compare_at_price: None, quantity: 0, barcode: item.gtin.clone() }],
            images: vec![],
            seo_title: Some(trim_to(&title, 70)),
            seo_description: None,
            confidence: item.guess.confidence,
            published: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::imaging::rgb_to_frame;

    fn req(excluded: &[&str]) -> GuessRequest {
        req_labelled("product", excluded)
    }

    fn req_labelled(label: &str, excluded: &[&str]) -> GuessRequest {
        GuessRequest {
            item_id: "item-001".into(),
            image: rgb_to_frame(&image::RgbImage::new(4, 4), 0),
            extra_views: vec![],
            detector_label: label.into(),
            dominant_colors: vec!["red".into()],
            excluded_titles: excluded.iter().map(|s| s.to_string()).collect(),
            mode: ScanMode::BarcodeFirst,
        }
    }

    #[test]
    fn never_repeats_a_rejected_title_and_never_claims_confidence() {
        let g = PlaceholderGuesser;
        let first = g.guess(req(&[])).unwrap();
        assert_eq!(first.title, "Unidentified red product");
        assert_eq!(first.confidence, 0.0);
        let second = g.guess(req(&[&first.title])).unwrap();
        assert_ne!(second.title, first.title);
        let many = g.guess(req(&["Unidentified red product", "Unidentified red item", "Unidentified red object", "Unidentified red thing"])).unwrap();
        assert!(many.title.starts_with("Unidentified product #"));
    }

    #[test]
    fn uses_a_specific_detector_label_first() {
        let g = PlaceholderGuesser;
        assert_eq!(g.guess(req_labelled("bottle", &[])).unwrap().title, "Unidentified red bottle");
        assert_eq!(g.guess(req_labelled("bottle", &["Unidentified red bottle"])).unwrap().title, "Unidentified red product");
        assert_eq!(g.guess(req_labelled("Object", &[])).unwrap().title, "Unidentified red product");
        assert_eq!(g.guess(req_labelled("bottle", &[])).unwrap().confidence, 0.0);
    }

    #[test]
    fn writer_marks_unknowns_and_stays_draft() {
        let item = CatalogItem {
            id: "item-001".into(),
            quantity: 1,
            state: ItemState::Guessed,
            role: Role::Product,
            mode: ScanMode::DescribeFirst,
            guess: ProductGuess { title: "Red <b>box</b>".into(), brand: None, category: None, attributes: vec![], confidence: 0.0 },
            guess_ready: true,
            label: "product".into(),
            gtin: None,
            sku: Some("SKU-0001".into()),
            needs_barcode: false,
            images: vec![],
            first_seen_ms: 0,
        };
        let l = TemplateWriter.write(item, String::new()).unwrap();
        assert_eq!(l.status, ListingStatus::Draft);
        assert!(l.description_html.contains("Needs your input"));
        assert!(l.description_html.contains("&lt;b&gt;"), "html must be escaped");
        assert!(l.tags.contains(&"needs-review".to_string()));
        assert_eq!(l.variants[0].sku.as_deref(), Some("SKU-0001"));
        assert!(l.variants[0].price.is_empty(), "writer must not invent a price");
    }
}
