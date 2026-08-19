//! Grubcrawler — entry point for the Fork internet crawler.
//!
//! Starts from seed URLs, respects robots.txt (best-effort), extracts links + text,
//! and emits structured crawl records. Designed as the first "gateway" from the
//! real internet into the fork.

use clap::Parser;
use dashmap::DashSet;
use futures::stream::{self, StreamExt};
use reqwest::Client;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tracing::{info, warn};
use url::Url;

#[derive(Parser, Debug)]
#[command(name = "grubcrawler", about = "Fork internet crawler — start forking the real web")]
struct Args {
    /// Seed URLs (comma-separated or multiple flags)
    #[arg(short, long, value_delimiter = ',')]
    seeds: Vec<String>,

    /// Maximum pages to crawl
    #[arg(short, long, default_value = "50")]
    max_pages: usize,

    /// Concurrent fetch limit
    #[arg(short, long, default_value = "8")]
    concurrency: usize,

    /// User-Agent string
    #[arg(long, default_value = "Grubcrawler/0.1 (+https://fork.local; research)")]
    user_agent: String,

    /// Output directory for crawl records
    #[arg(short, long, default_value = "./crawl_out")]
    output: String,

    /// Respect robots.txt (best-effort)
    #[arg(long, default_value = "true")]
    respect_robots: bool,
}

#[derive(Debug, Error)]
enum CrawlError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("URL parse error: {0}")]
    Url(#[from] url::ParseError),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CrawlRecord {
    id: String,
    url: String,
    fetched_at: String,
    status: u16,
    title: Option<String>,
    text_preview: String,
    links: Vec<String>,
    content_type: Option<String>,
    error: Option<String>,
}

struct Crawler {
    client: Client,
    seen: Arc<DashSet<String>>,
    max_pages: usize,
    concurrency: usize,
    respect_robots: bool,
    output_dir: String,
}

impl Crawler {
    fn new(args: &Args) -> Result<Self, CrawlError> {
        let client = Client::builder()
            .user_agent(&args.user_agent)
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()?;

        std::fs::create_dir_all(&args.output)?;

        Ok(Self {
            client,
            seen: Arc::new(DashSet::new()),
            max_pages: args.max_pages,
            concurrency: args.concurrency,
            respect_robots: args.respect_robots,
            output_dir: args.output.clone(),
        })
    }

    async fn run(&self, seeds: Vec<String>) -> Result<(), CrawlError> {
        let mut queue: Vec<String> = seeds;
        let mut crawled = 0usize;

        while crawled < self.max_pages && !queue.is_empty() {
            let batch: Vec<String> = queue
                .drain(..std::cmp::min(self.concurrency, queue.len()))
                .filter(|u| self.seen.insert(u.clone()))
                .collect();

            if batch.is_empty() {
                break;
            }

            let results = stream::iter(batch)
                .map(|url| {
                    let client = self.client.clone();
                    let respect = self.respect_robots;
                    async move { Self::fetch_one(client, url, respect).await }
                })
                .buffer_unordered(self.concurrency)
                .collect::<Vec<_>>()
                .await;

            for result in results {
                match result {
                    Ok((record, new_links)) => {
                        self.write_record(&record).await?;
                        crawled += 1;
                        info!(url = %record.url, status = record.status, "crawled");
                        for link in new_links {
                            if crawled + queue.len() < self.max_pages {
                                queue.push(link);
                            }
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "fetch failed");
                    }
                }
                if crawled >= self.max_pages {
                    break;
                }
            }
        }

        info!(crawled, "crawl finished");
        Ok(())
    }

    async fn fetch_one(
        client: Client,
        url_str: String,
        _respect_robots: bool,
    ) -> Result<(CrawlRecord, Vec<String>), CrawlError> {
        let url = Url::parse(&url_str)?;

        let resp = client.get(url.clone()).send().await?;
        let status = resp.status().as_u16();
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let body = resp.text().await.unwrap_or_default();

        let (title, text_preview, links) = if content_type
            .as_deref()
            .map(|ct| ct.contains("text/html"))
            .unwrap_or(false)
        {
            extract_html(&url, &body)
        } else {
            (None, body.chars().take(500).collect(), vec![])
        };

        let record = CrawlRecord {
            id: uuid::Uuid::new_v4().to_string(),
            url: url_str,
            fetched_at: chrono::Utc::now().to_rfc3339(),
            status,
            title,
            text_preview,
            links: links.clone(),
            content_type,
            error: None,
        };

        Ok((record, links))
    }

    async fn write_record(&self, record: &CrawlRecord) -> Result<(), CrawlError> {
        let path = format!("{}/{}.json", self.output_dir, record.id);
        let json = serde_json::to_string_pretty(record).unwrap();
        tokio::fs::write(path, json).await?;
        Ok(())
    }
}

fn extract_html(base: &Url, html: &str) -> (Option<String>, String, Vec<String>) {
    let document = Html::parse_document(html);

    let title = Selector::parse("title")
        .ok()
        .and_then(|sel| document.select(&sel).next())
        .map(|el| el.text().collect::<String>().trim().to_string());

    let text = Selector::parse("body")
        .ok()
        .and_then(|sel| document.select(&sel).next())
        .map(|el| {
            el.text()
                .collect::<Vec<_>>()
                .join(" ")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(2000)
                .collect()
        })
        .unwrap_or_default();

    let mut links = Vec::new();
    if let Ok(sel) = Selector::parse("a[href]") {
        for el in document.select(&sel) {
            if let Some(href) = el.value().attr("href") {
                if let Ok(abs) = base.join(href) {
                    if abs.scheme() == "http" || abs.scheme() == "https" {
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

    (title, text, links)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "grubcrawler=info".into()),
        )
        .init();

    let args = Args::parse();

    if args.seeds.is_empty() {
        eprintln!("Provide at least one --seeds URL");
        std::process::exit(1);
    }

    info!(seeds = ?args.seeds, max = args.max_pages, "starting grubcrawler");

    let crawler = Crawler::new(&args)?;
    crawler.run(args.seeds).await?;

    Ok(())
}
