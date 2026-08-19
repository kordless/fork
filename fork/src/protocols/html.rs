//! Deterministic HTML extraction shared by HTTP snapshots and handlers.

use scraper::{Html, Selector};
use url::Url;

pub fn normalize_extraction(html: &str) -> String {
    let document = Html::parse_document(html);
    let text = if let Ok(sel) = Selector::parse("body") {
        document
            .select(&sel)
            .next()
            .map(|el| {
                el.text()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default()
    } else {
        html.split_whitespace().collect::<Vec<_>>().join(" ")
    };
    text.chars().take(50_000).collect()
}

pub fn extract_title(html: &str) -> Option<String> {
    let document = Html::parse_document(html);
    Selector::parse("title").ok().and_then(|sel| {
        document
            .select(&sel)
            .next()
            .map(|el| el.text().collect::<String>().trim().to_string())
            .filter(|s| !s.is_empty())
    })
}

pub fn extract_links(base: &Url, html: &str) -> Vec<String> {
    let document = Html::parse_document(html);
    let mut links = Vec::new();
    if let Ok(sel) = Selector::parse("a[href]") {
        for el in document.select(&sel) {
            if let Some(href) = el.value().attr("href") {
                if let Ok(abs) = base.join(href) {
                    if matches!(abs.scheme(), "http" | "https") {
                        let mut clean = abs;
                        clean.set_fragment(None);
                        links.push(clean.to_string());
                    }
                }
            }
        }
    }
    links.sort();
    links.dedup();
    if links.len() > 200 {
        links.truncate(200);
    }
    links
}
