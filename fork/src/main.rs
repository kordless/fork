//! Fork CLI — snapshot, verify, diff, get, and search the human web.
//!
//! Reference implementation of SPEC/snapshot.md + SPEC/entropy.md.
//! Makes the archive active: you can go back and use it.

pub mod protocols;
pub mod server;

use anyhow::{Context, Result};
use protocols::{extract_links, extract_title, normalize_extraction, ProtocolRegistry};
use clap::{Parser, Subcommand};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

use url::Url;

#[derive(Parser, Debug)]
#[command(name = "fork", about = "Preserve the web's human entropy — versioned, hash-anchored, many copies")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Fetch a URL and create a snapshot (WARC-lite + digests + chain pointer)
    Snap {
        url: String,
        #[arg(short, long, default_value = "./snapshots")]
        out: PathBuf,
        #[arg(long)]
        prior: Option<String>,
        #[arg(long)]
        tailscore: Option<f32>,
    },
    /// Retrieve / reconstitute a snapshot by digest or path (active use)
    Get {
        /// body_digest or path to snapshot JSON
        target: String,
        #[arg(short, long, default_value = "./snapshots")]
        dir: PathBuf,
    },
    /// Verify digests and basic integrity of snapshots in a directory/bundle
    Verify {
        #[arg(default_value = "./snapshots")]
        path: PathBuf,
    },
    /// Diff two snapshots (hash-level + optional text)
    Diff {
        a: String,
        b: String,
        #[arg(short, long, default_value = "./snapshots")]
        dir: PathBuf,
    },
    /// Rough at-risk / humanness heuristic
    Tailscore {
        target: String,
    },
    /// Simple keyword search across local snapshots
    Search {
        query: String,
        #[arg(short, long, default_value = "./snapshots")]
        dir: PathBuf,
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },
    /// List registered protocol handlers (http, finger, dns, gemini, gopher, ...)
    Protocols,
    /// Launch the interactive Web Browser Client server
    Serve {
        #[arg(short, long, default_value = "0.0.0.0:8080")]
        bind: String,
        #[arg(short, long, default_value = "./snapshots")]
        dir: PathBuf,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Snapshot {
    spec: String,
    url: String,
    fetched_at: String,
    warc_digest: Option<String>,
    body_digest: String,
    extraction_digest: Option<String>,
    content_type: Option<String>,
    status: u16,
    prior: Option<String>,
    tailscore: Option<f32>,
    beacon: Option<Beacon>,
    meta: Meta,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Beacon {
    digest: String,
    freq_hz: Option<u64>,
    station: Option<String>,
    captured_at: Option<String>,
    duration_ms: Option<u32>,
    receiver: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Meta {
    title: Option<String>,
    text_preview: Option<String>,
    links: Vec<String>,
    final_url: Option<String>,
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("sha256:{}", hex::encode(hasher.finalize()))
}



fn extract_meta(url: &Url, html: &str) -> Meta {
    Meta {
        title: extract_title(html),
        text_preview: Some(normalize_extraction(html).chars().take(1500).collect()),
        links: extract_links(url, html),
        final_url: Some(url.to_string()),
    }
}



fn write_snapshot(out: &Path, snap: &Snapshot, body: &[u8]) -> Result<PathBuf> {
    fs::create_dir_all(out)?;
    let short = snap.body_digest.trim_start_matches("sha256:");
    let short = &short[..std::cmp::min(16, short.len())];
    let json_path = out.join(format!("{}.json", short));
    let body_path = out.join(format!("{}.body", short));

    fs::write(&json_path, serde_json::to_string_pretty(snap)?)?;
    fs::write(&body_path, body)?;
    Ok(json_path)
}

fn load_snapshot(path: &Path) -> Result<Snapshot> {
    let data = fs::read_to_string(path).context("read snapshot")?;
    Ok(serde_json::from_str(&data)?)
}

fn find_snapshot_by_digest(dir: &Path, digest_or_short: &str) -> Result<PathBuf> {
    let needle = digest_or_short.trim_start_matches("sha256:");
    for entry in walkdir::WalkDir::new(dir).into_iter().filter_map(|e| e.ok()) {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) == Some("json") {
            if let Ok(snap) = load_snapshot(p) {
                if snap.body_digest.contains(needle) || p.file_stem().map(|s| s.to_string_lossy().contains(needle)).unwrap_or(false) {
                    return Ok(p.to_path_buf());
                }
            }
        }
    }
    anyhow::bail!("snapshot not found for {}", digest_or_short)
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Snap { url, out, prior, tailscore } => {
            println!("fork snap: {}", url);
            let registry = ProtocolRegistry::builtins();
            let payload = registry.fetch(&url).await?;
            let body = payload.raw_bytes;
            let status = payload.status;
            let content_type = payload.content_type;
            let final_url = payload.final_url.clone().unwrap_or_else(|| url.clone());
            let body_digest = sha256_hex(&body);

            let extraction = if !payload.extracted_text.is_empty() {
                Some(sha256_hex(payload.extracted_text.as_bytes()))
            } else {
                None
            };

            let meta = if payload.title.is_some() || !payload.links.is_empty() {
                Meta {
                    title: payload.title,
                    text_preview: Some(payload.extracted_text.chars().take(1500).collect()),
                    links: payload.links,
                    final_url: Some(final_url.clone()),
                }
            } else if content_type.as_deref().map(|c| c.contains("html")).unwrap_or(false) {
                let u = Url::parse(&final_url).unwrap_or_else(|_| Url::parse(&url).unwrap());
                extract_meta(&u, &String::from_utf8_lossy(&body))
            } else {
                Meta {
                    title: None,
                    text_preview: Some(payload.extracted_text.chars().take(1500).collect()),
                    links: vec![],
                    final_url: Some(final_url.clone()),
                }
            };

            let snap = Snapshot {
                spec: "fork-snapshot/0.1".into(),
                url: url.clone(),
                fetched_at: chrono::Utc::now().to_rfc3339(),
                warc_digest: None, // full WARC left for later / grubcrawler
                body_digest: body_digest.clone(),
                extraction_digest: extraction,
                content_type,
                status,
                prior,
                tailscore,
                beacon: None, // filled by entropy/beacon path later
                meta,
            };

            let path = write_snapshot(&out, &snap, &body)?;
            println!("  status: {}", status);
            println!("  body_digest: {}", body_digest);
            if let Some(t) = &snap.meta.title {
                println!("  title: {}", t);
            }
            println!("  wrote: {}", path.display());
        }

        Commands::Get { target, dir } => {
            let path = if Path::new(&target).exists() {
                PathBuf::from(&target)
            } else {
                find_snapshot_by_digest(&dir, &target)?
            };
            let snap = load_snapshot(&path)?;
            println!("url:          {}", snap.url);
            println!("fetched_at:   {}", snap.fetched_at);
            println!("body_digest:  {}", snap.body_digest);
            println!("status:       {}", snap.status);
            if let Some(t) = &snap.meta.title {
                println!("title:        {}", t);
            }
            if let Some(preview) = &snap.meta.text_preview {
                println!("\n--- text preview ---\n{}\n--------------------", preview.chars().take(1200).collect::<String>());
            }
            // Also surface the body file if present
            let body_path = path.with_extension("body");
            if body_path.exists() {
                println!("body file:    {}", body_path.display());
            }
        }

        Commands::Verify { path } => {
            let mut ok = 0;
            let mut fail = 0;
            for entry in walkdir::WalkDir::new(&path).into_iter().filter_map(|e| e.ok()) {
                let p = entry.path();
                if p.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                match load_snapshot(p) {
                    Ok(snap) => {
                        let body_path = p.with_extension("body");
                        if body_path.exists() {
                            let body = fs::read(&body_path)?;
                            let computed = sha256_hex(&body);
                            if computed == snap.body_digest {
                                ok += 1;
                            } else {
                                println!("FAIL digest mismatch: {}", p.display());
                                fail += 1;
                            }
                        } else {
                            println!("WARN missing body: {}", p.display());
                            ok += 1; // metadata-only still counts for now
                        }
                    }
                    Err(e) => {
                        println!("FAIL parse: {} — {}", p.display(), e);
                        fail += 1;
                    }
                }
            }
            println!("verify complete: {} ok, {} fail", ok, fail);
            if fail > 0 {
                std::process::exit(1);
            }
        }

        Commands::Diff { a, b, dir } => {
            let path_a = if Path::new(&a).exists() { PathBuf::from(&a) } else { find_snapshot_by_digest(&dir, &a)? };
            let path_b = if Path::new(&b).exists() { PathBuf::from(&b) } else { find_snapshot_by_digest(&dir, &b)? };
            let sa = load_snapshot(&path_a)?;
            let sb = load_snapshot(&path_b)?;
            println!("A: {} @ {}", sa.url, sa.fetched_at);
            println!("B: {} @ {}", sb.url, sb.fetched_at);
            println!("body_digest A: {}", sa.body_digest);
            println!("body_digest B: {}", sb.body_digest);
            if sa.body_digest == sb.body_digest {
                println!("result: identical body");
            } else {
                println!("result: bodies differ");
                if let (Some(ta), Some(tb)) = (&sa.meta.title, &sb.meta.title) {
                    if ta != tb {
                        println!("title changed: {:?} → {:?}", ta, tb);
                    }
                }
            }
            if sa.prior.as_ref() == Some(&sb.body_digest) || sb.prior.as_ref() == Some(&sa.body_digest) {
                println!("chain: linked via prior pointer");
            }
        }

        Commands::Tailscore { target } => {
            // Extremely rough heuristic for demo
            let mut score: f32 = 0.5;
            let lower = target.to_lowercase();
            if lower.contains("blog") || lower.contains("personal") || lower.contains("~") || lower.contains("forum") {
                score += 0.2;
            }
            if lower.contains("wordpress") || lower.contains("blogspot") || lower.contains("tumblr") {
                score += 0.1;
            }
            if lower.contains("facebook") || lower.contains("twitter") || lower.contains("instagram") || lower.contains("tiktok") {
                score -= 0.3;
            }
            if lower.starts_with("https://github.com") || lower.contains("docs.") {
                score -= 0.1;
            }
            score = score.clamp(0.0, 1.0);
            println!("target:    {}", target);
            println!("tailscore: {:.2}  (placeholder heuristic — domain patterns only)", score);
            println!("note: real tailscore will use link rarity, age, generator fingerprints, personal markers");
        }

        Commands::Search { query, dir, limit } => {
            let q = query.to_lowercase();
            let mut hits: Vec<(f32, Snapshot, PathBuf)> = Vec::new();

            for entry in walkdir::WalkDir::new(&dir).into_iter().filter_map(|e| e.ok()) {
                let p = entry.path();
                if p.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                if let Ok(snap) = load_snapshot(p) {
                    let mut score = 0.0f32;
                    let title = snap.meta.title.as_deref().unwrap_or("").to_lowercase();
                    let preview = snap.meta.text_preview.as_deref().unwrap_or("").to_lowercase();
                    if title.contains(&q) {
                        score += 3.0;
                    }
                    if preview.contains(&q) {
                        score += 1.0;
                    }
                    if snap.url.to_lowercase().contains(&q) {
                        score += 1.5;
                    }
                    if score > 0.0 {
                        hits.push((score, snap, p.to_path_buf()));
                    }
                }
            }
            hits.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            for (i, (score, snap, path)) in hits.into_iter().take(limit).enumerate() {
                println!("{}. [{:.1}] {}", i + 1, score, snap.meta.title.as_deref().unwrap_or("(no title)"));
                println!("   {}", snap.url);
                println!("   {}", path.display());
                println!();
            }
        }

        Commands::Protocols => {
            let registry = ProtocolRegistry::builtins();
            println!("registered protocol handlers:");
            for scheme in registry.schemes() {
                println!("  {scheme}");
            }
        }

        Commands::Serve { bind, dir } => {
            server::run_server(&bind, dir).await?;
        }
    }

    Ok(())
}
