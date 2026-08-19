//! Built-in Web Browser Client for Fork.
//!
//! Binds to 0.0.0.0 (default :8888) and serves a glassmorphic cyberpunk UI over
//! live snapshot APIs: `/api/snapshots`, `/api/search`, `/api/get`, `/api/snap`.

use anyhow::{Context, Result};
use crate::protocols::{extract_links, extract_title, normalize_extraction, ProtocolRegistry};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use url::Url;

pub async fn run_server(bind_addr: &str, snapshots_dir: PathBuf) -> Result<()> {
    std::fs::create_dir_all(&snapshots_dir).ok();
    let listener = TcpListener::bind(bind_addr)
        .await
        .with_context(|| format!("Failed to bind to {bind_addr}"))?;

    println!("============================================================");
    println!("Fork Web Browser Client running at http://{bind_addr}");
    println!("   Snapshots directory: {}", snapshots_dir.display());
    println!("============================================================");

    loop {
        let (mut socket, _) = listener.accept().await?;
        let dir_clone = snapshots_dir.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 16_384];
            if let Ok(n) = socket.read(&mut buf).await {
                if n == 0 {
                    return;
                }
                let req_str = String::from_utf8_lossy(&buf[..n]);
                let response = handle_request(&req_str, &dir_clone).await;
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
                let _ = socket.shutdown().await;
            }
        });
    }
}

async fn handle_request(req: &str, snapshots_dir: &Path) -> String {
    let first_line = req.lines().next().unwrap_or("");
    let mut parts = first_line.split_whitespace();
    let _method = parts.next().unwrap_or("GET");
    let path = parts.next().unwrap_or("/");
    let path_only = path.split('?').next().unwrap_or(path);

    match path_only {
        "/" | "/index.html" => http_response(200, "text/html; charset=utf-8", INDEX_HTML),
        "/api/snapshots" => {
            let list = list_local_snapshots(snapshots_dir);
            json_ok(&list)
        }
        "/api/protocols" => {
            let schemes = ProtocolRegistry::builtins().schemes();
            json_ok(&schemes)
        }
        "/api/search" => {
            let query = extract_query_param(path, "q").unwrap_or_default();
            json_ok(&search_snapshots(snapshots_dir, &query))
        }
        "/api/get" => {
            let target = extract_query_param(path, "target").unwrap_or_default();
            match get_snapshot_content(snapshots_dir, &target) {
                Ok(snap_json) => http_response(200, "application/json", &snap_json),
                Err(_) => json_err(404, "Snapshot not found"),
            }
        }
        "/api/snap" => {
            let url = extract_query_param(path, "url").unwrap_or_default();
            if url.is_empty() {
                return json_err(400, "missing url parameter");
            }
            match snap_url(snapshots_dir, &url).await {
                Ok(val) => json_ok(&val),
                Err(e) => json_err(502, &e.to_string()),
            }
        }
        _ => http_response(404, "text/plain; charset=utf-8", "404 Not Found"),
    }
}

fn json_ok<T: serde::Serialize>(val: &T) -> String {
    let body = serde_json::to_string(val).unwrap_or_else(|_| "null".into());
    http_response(200, "application/json; charset=utf-8", &body)
}

fn json_err(status: u16, msg: &str) -> String {
    let body = serde_json::json!({ "error": msg }).to_string();
    http_response(status, "application/json; charset=utf-8", &body)
}

fn http_response(status: u16, content_type: &str, body: &str) -> String {
    let status_text = match status {
        200 => "200 OK",
        400 => "400 Bad Request",
        404 => "404 Not Found",
        502 => "502 Bad Gateway",
        _ => "500 Internal Server Error",
    };
    format!(
        "HTTP/1.1 {status_text}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn extract_query_param(path: &str, key: &str) -> Option<String> {
    let query_str = path.split_once('?')?.1;
    url::form_urlencoded::parse(query_str.as_bytes())
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
}

fn sha256_hex(data: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(data)))
}

fn tailscore_breakdown(target: &str) -> (f32, Vec<serde_json::Value>) {
    let lower = target.to_lowercase();
    let mut score: f32 = 0.5;
    let mut factors = vec![serde_json::json!({
        "label": "baseline",
        "delta": 0.5,
        "note": "neutral prior"
    })];

    let mut bump = |cond: bool, delta: f32, label: &str, note: &str| {
        if cond {
            score += delta;
            factors.push(serde_json::json!({ "label": label, "delta": delta, "note": note }));
        }
    };
    bump(
        lower.contains("blog") || lower.contains("personal") || lower.contains("~") || lower.contains("forum"),
        0.2,
        "human-corner",
        "personal / blog / forum markers",
    );
    bump(
        lower.contains("wordpress") || lower.contains("blogspot") || lower.contains("tumblr"),
        0.1,
        "indie-cms",
        "self-hosted / classic CMS",
    );
    bump(
        lower.starts_with("finger:") || lower.starts_with("gopher:") || lower.starts_with("gemini:") || lower.starts_with("news:"),
        0.15,
        "long-tail-protocol",
        "non-web protocol (high entropy, low crawl coverage)",
    );
    bump(
        lower.contains("facebook") || lower.contains("twitter") || lower.contains("instagram") || lower.contains("tiktok"),
        -0.3,
        "platform-feed",
        "centralized social feed",
    );
    bump(
        lower.starts_with("https://github.com") || lower.contains("docs."),
        -0.1,
        "engineered-corpus",
        "highly optimized / generated-adjacent",
    );
    score = score.clamp(0.0, 1.0);
    (score, factors)
}

fn software_beacon() -> serde_json::Value {
    let captured_at = chrono::Utc::now().to_rfc3339();
    let digest = sha256_hex(format!("fork-clock|{captured_at}|WWV-10MHz").as_bytes());
    serde_json::json!({
        "digest": digest,
        "freq_hz": 10_000_000,
        "station": "WWV",
        "captured_at": captured_at,
        "duration_ms": 0,
        "receiver": "system-clock (RTL-SDR unattached)"
    })
}

fn find_prior(dir: &Path, url: &str) -> Option<String> {
    let mut best: Option<(String, String)> = None; // (fetched_at, body_digest)
    for entry in walkdir::WalkDir::new(dir).into_iter().filter_map(|e| e.ok()) {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(data) = std::fs::read_to_string(p) else { continue };
        let Ok(val) = serde_json::from_str::<serde_json::Value>(&data) else { continue };
        if val.get("url").and_then(|u| u.as_str()) != Some(url) {
            continue;
        }
        let fetched = val.get("fetched_at").and_then(|s| s.as_str()).unwrap_or("").to_string();
        let digest = val.get("body_digest").and_then(|s| s.as_str()).unwrap_or("").to_string();
        if digest.is_empty() {
            continue;
        }
        match &best {
            Some((prev, _)) if fetched <= *prev => {}
            _ => best = Some((fetched, digest)),
        }
    }
    best.map(|(_, d)| d)
}

async fn snap_url(dir: &Path, url_str: &str) -> Result<serde_json::Value> {
    let registry = ProtocolRegistry::builtins();
    let payload = registry.fetch(url_str).await?;
    let body = payload.raw_bytes;
    let body_digest = sha256_hex(&body);
    let extraction = if payload.extracted_text.is_empty() {
        None
    } else {
        Some(sha256_hex(payload.extracted_text.as_bytes()))
    };
    let final_url = payload
        .final_url
        .clone()
        .unwrap_or_else(|| url_str.to_string());
    let (title, preview, links) = if payload.title.is_some() || !payload.links.is_empty() {
        (
            payload.title.clone(),
            Some(payload.extracted_text.chars().take(4000).collect::<String>()),
            payload.links.clone(),
        )
    } else if payload
        .content_type
        .as_deref()
        .map(|c| c.contains("html"))
        .unwrap_or(false)
    {
        let parsed = Url::parse(&final_url).ok();
        let html = String::from_utf8_lossy(&body);
        (
            extract_title(&html),
            Some(normalize_extraction(&html).chars().take(4000).collect()),
            parsed.map(|u| extract_links(&u, &html)).unwrap_or_default(),
        )
    } else {
        (
            payload.title.clone(),
            Some(payload.extracted_text.chars().take(4000).collect()),
            vec![],
        )
    };

    let (score, factors) = tailscore_breakdown(url_str);
    let prior = find_prior(dir, url_str);
    let beacon = software_beacon();
    let fetched_at = chrono::Utc::now().to_rfc3339();

    let snap = serde_json::json!({
        "spec": "fork-snapshot/0.1",
        "url": url_str,
        "fetched_at": fetched_at,
        "warc_digest": serde_json::Value::Null,
        "body_digest": body_digest,
        "extraction_digest": extraction,
        "content_type": payload.content_type,
        "status": payload.status,
        "prior": prior,
        "tailscore": score,
        "beacon": beacon,
        "meta": {
            "title": title,
            "text_preview": preview,
            "links": links,
            "final_url": final_url,
            "tailscore_factors": factors,
            "scheme": Url::parse(url_str).map(|u| u.scheme().to_string()).unwrap_or_default()
        }
    });

    std::fs::create_dir_all(dir)?;
    let short = snap["body_digest"]
        .as_str()
        .unwrap_or("unknown")
        .trim_start_matches("sha256:");
    let short = &short[..std::cmp::min(16, short.len())];
    let json_path = dir.join(format!("{short}.json"));
    let body_path = dir.join(format!("{short}.body"));
    std::fs::write(&json_path, serde_json::to_string_pretty(&snap)?)?;
    std::fs::write(&body_path, body)?;
    Ok(snap)
}

fn list_local_snapshots(dir: &Path) -> Vec<serde_json::Value> {
    let mut list = Vec::new();
    if !dir.exists() {
        return list;
    }
    for entry in walkdir::WalkDir::new(dir).into_iter().filter_map(|e| e.ok()) {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) == Some("json") {
            if let Ok(data) = std::fs::read_to_string(p) {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&data) {
                    list.push(val);
                }
            }
        }
    }
    list.sort_by(|a, b| {
        let ta = a.get("fetched_at").and_then(|s| s.as_str()).unwrap_or("");
        let tb = b.get("fetched_at").and_then(|s| s.as_str()).unwrap_or("");
        tb.cmp(ta)
    });
    list
}

fn search_snapshots(dir: &Path, query: &str) -> Vec<serde_json::Value> {
    let q = query.to_lowercase();
    if q.is_empty() {
        return list_local_snapshots(dir);
    }
    let mut hits: Vec<(f32, serde_json::Value)> = Vec::new();
    for val in list_local_snapshots(dir) {
        let title = val["meta"]["title"].as_str().unwrap_or("").to_lowercase();
        let preview = val["meta"]["text_preview"].as_str().unwrap_or("").to_lowercase();
        let url = val["url"].as_str().unwrap_or("").to_lowercase();
        let mut score = 0.0f32;
        if title.contains(&q) {
            score += 3.0;
        }
        if preview.contains(&q) {
            score += 1.0;
        }
        if url.contains(&q) {
            score += 1.5;
        }
        if score > 0.0 {
            hits.push((score, val));
        }
    }
    hits.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    hits.into_iter().map(|(_, v)| v).collect()
}

fn get_snapshot_content(dir: &Path, target: &str) -> Result<String> {
    let needle = target.trim_start_matches("sha256:");
    if needle.is_empty() {
        anyhow::bail!("empty target");
    }
    for entry in walkdir::WalkDir::new(dir).into_iter().filter_map(|e| e.ok()) {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let data = std::fs::read_to_string(p)?;
        if data.contains(needle) || p.to_string_lossy().contains(needle) {
            return Ok(data);
        }
    }
    anyhow::bail!("Not found")
}

const INDEX_HTML: &str = r###"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Fork — Web Browser for Human Entropy</title>
<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
<link href="https://fonts.googleapis.com/css2?family=Outfit:wght@300;400;500;600;700;800&family=JetBrains+Mono:wght@400;600&display=swap" rel="stylesheet">
<style>
:root {
  --void: #06080d;
  --panel: rgba(12, 16, 28, 0.62);
  --card: rgba(18, 24, 42, 0.55);
  --cyan: #00f2fe;
  --blue: #4facfe;
  --purple: #9d4edd;
  --pink: #ff4d8d;
  --text: #f1f5f9;
  --muted: #8b9bb4;
  --line: rgba(0, 242, 254, 0.18);
  --glow: 0 0 24px rgba(0, 242, 254, 0.18);
  --font: "Outfit", sans-serif;
  --mono: "JetBrains Mono", monospace;
}
* { box-sizing: border-box; margin: 0; padding: 0; }
html, body { height: 100%; }
body {
  font-family: var(--font);
  color: var(--text);
  background:
    radial-gradient(1200px 600px at 10% -10%, rgba(157, 78, 221, 0.22), transparent 55%),
    radial-gradient(900px 500px at 110% 10%, rgba(0, 242, 254, 0.16), transparent 50%),
    linear-gradient(180deg, #070910 0%, var(--void) 100%);
  overflow: hidden;
}
body::before {
  content: "";
  pointer-events: none;
  position: fixed; inset: 0;
  background: repeating-linear-gradient(0deg, transparent, transparent 2px, rgba(0,0,0,0.08) 3px);
  opacity: 0.35;
}
.glass {
  background: var(--panel);
  backdrop-filter: blur(18px) saturate(140%);
  -webkit-backdrop-filter: blur(18px) saturate(140%);
  border: 1px solid var(--line);
  box-shadow: var(--glow), inset 0 1px 0 rgba(255,255,255,0.04);
}
.app { display: flex; flex-direction: column; height: 100vh; }
header.chrome {
  display: flex; align-items: center; gap: 10px;
  padding: 10px 14px;
  z-index: 20;
}
.wordmark {
  font-weight: 800; letter-spacing: 0.08em; font-size: 13px;
  background: linear-gradient(90deg, var(--cyan), var(--blue), var(--purple));
  -webkit-background-clip: text; -webkit-text-fill-color: transparent;
  white-space: nowrap;
}
.nav-btns { display: flex; gap: 6px; }
.icon-btn {
  width: 34px; height: 34px; border-radius: 10px;
  border: 1px solid var(--line); background: rgba(255,255,255,0.03);
  color: var(--text); cursor: pointer; font-size: 14px;
  transition: 0.18s ease; font-family: var(--mono);
}
.icon-btn:hover { border-color: var(--cyan); box-shadow: var(--glow); color: var(--cyan); }
.icon-btn:disabled { opacity: 0.35; cursor: default; box-shadow: none; }
.url-wrap {
  flex: 1; display: flex; align-items: center; gap: 8px;
  padding: 4px 10px; border-radius: 14px; min-width: 0;
  border: 1px solid var(--line); background: rgba(0,0,0,0.35);
  transition: 0.2s ease;
}
.url-wrap:focus-within { border-color: var(--cyan); box-shadow: var(--glow); }
.scheme {
  font-family: var(--mono); font-size: 10px; font-weight: 700; letter-spacing: 0.12em;
  padding: 4px 8px; border-radius: 999px;
  border: 1px solid rgba(0,242,254,0.35);
  background: linear-gradient(135deg, rgba(0,242,254,0.16), rgba(157,78,221,0.16));
  color: var(--cyan); display: flex; align-items: center; gap: 6px;
}
.scheme .pulse {
  width: 7px; height: 7px; border-radius: 50%; background: var(--cyan);
  box-shadow: 0 0 10px var(--cyan); animation: pulse 1.6s ease infinite;
}
@keyframes pulse { 50% { opacity: 0.35; transform: scale(0.8); } }
#urlInput {
  flex: 1; min-width: 0; background: transparent; border: 0; outline: 0;
  color: var(--text); font-family: var(--mono); font-size: 13px;
}
.go {
  border: 0; border-radius: 12px; padding: 8px 16px; cursor: pointer;
  font-family: var(--font); font-weight: 700;
  background: linear-gradient(135deg, var(--cyan), var(--blue) 55%, var(--purple));
  color: #041018; box-shadow: var(--glow);
}
.go:hover { filter: brightness(1.08); }
.go.busy { opacity: 0.7; }
.tabs {
  display: flex; gap: 6px; padding: 0 14px 8px; overflow-x: auto;
}
.tab {
  font-size: 12px; padding: 6px 10px; border-radius: 10px; cursor: pointer;
  border: 1px solid transparent; color: var(--muted); white-space: nowrap;
  background: rgba(255,255,255,0.03); max-width: 180px; overflow: hidden; text-overflow: ellipsis;
}
.tab.on { color: var(--text); border-color: var(--cyan); box-shadow: var(--glow); }
.presets {
  display: flex; flex-wrap: wrap; gap: 8px; align-items: center;
  padding: 8px 14px; border-top: 1px solid var(--line); border-bottom: 1px solid var(--line);
}
.presets label { font-size: 11px; letter-spacing: 0.14em; text-transform: uppercase; color: var(--muted); }
.pill {
  font-family: var(--mono); font-size: 11px; padding: 5px 10px; border-radius: 999px;
  border: 1px solid var(--line); background: rgba(255,255,255,0.03); color: var(--muted);
  cursor: pointer; transition: 0.18s ease;
}
.pill:hover { color: var(--cyan); border-color: var(--cyan); }
.stage {
  flex: 1; display: grid; grid-template-columns: minmax(220px, 280px) 1fr minmax(260px, 340px);
  min-height: 0;
}
.col { min-height: 0; display: flex; flex-direction: column; overflow: hidden; }
.sidebar { border-right: 1px solid var(--line); }
.inspector { border-left: 1px solid var(--line); }
.col-h { padding: 12px 12px 8px; font-size: 11px; letter-spacing: 0.16em; text-transform: uppercase; color: var(--muted); }
#searchInput {
  margin: 0 12px 10px; padding: 8px 10px; border-radius: 10px;
  border: 1px solid var(--line); background: rgba(0,0,0,0.35); color: var(--text);
  font-family: var(--font); outline: none;
}
#snapshotList { flex: 1; overflow: auto; padding: 0 10px 12px; }
.card {
  padding: 10px; border-radius: 12px; margin-bottom: 8px; cursor: pointer;
  border: 1px solid var(--line); background: var(--card);
  animation: rise 0.35s ease;
}
@keyframes rise { from { opacity: 0; transform: translateY(6px); } }
.card:hover, .card.on { border-color: var(--cyan); }
.card h4 { font-size: 13px; font-weight: 600; }
.card p { font-family: var(--mono); font-size: 10px; color: var(--muted); margin-top: 4px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.empty { padding: 16px; color: var(--muted); font-size: 13px; }
.viewport { padding: 18px; overflow: auto; }
.v-title { font-size: clamp(18px, 2.4vw, 28px); font-weight: 800; letter-spacing: -0.03em; }
.v-meta { display: flex; flex-wrap: wrap; gap: 10px; margin: 10px 0 16px; font-family: var(--mono); font-size: 11px; color: var(--muted); }
.v-body {
  white-space: pre-wrap; font-family: var(--mono); font-size: 13px; line-height: 1.65;
  padding: 16px; border-radius: 16px; min-height: 40%;
}
.inspect-pad { flex: 1; overflow: auto; padding: 12px; display: flex; flex-direction: column; gap: 12px; }
.block { padding: 12px; border-radius: 14px; }
.block h3 { font-size: 11px; letter-spacing: 0.14em; text-transform: uppercase; color: var(--cyan); margin-bottom: 8px; }
.hash {
  font-family: var(--mono); font-size: 11px; word-break: break-all;
  padding: 8px; border-radius: 8px; background: rgba(0,0,0,0.4); color: var(--cyan);
  border: 1px solid var(--line);
}
.kv { display: flex; justify-content: space-between; gap: 8px; font-size: 12px; margin: 4px 0; color: var(--muted); }
.kv b { color: var(--text); font-weight: 600; }
.bar { height: 8px; border-radius: 99px; background: rgba(255,255,255,0.08); overflow: hidden; }
.fill { height: 100%; background: linear-gradient(90deg, var(--pink), var(--purple), var(--cyan)); transition: width 0.5s ease; }
.factor { display: flex; justify-content: space-between; font-family: var(--mono); font-size: 11px; color: var(--muted); margin-top: 6px; }
.proto { display: grid; grid-template-columns: 1fr 1fr; gap: 6px; }
.proto i { font-style: normal; display: flex; justify-content: space-between; font-family: var(--mono); font-size: 10px;
  padding: 6px 8px; border-radius: 8px; border: 1px solid var(--line); }
.dot { width: 7px; height: 7px; border-radius: 50%; background: #22c55e; box-shadow: 0 0 8px #22c55e; }
.drawer-btn {
  display: none; position: fixed; right: 14px; bottom: 14px; z-index: 30;
  padding: 10px 14px; border-radius: 999px; border: 1px solid var(--cyan);
  background: rgba(6,8,13,0.9); color: var(--cyan); font-weight: 700; cursor: pointer;
}
.status-line { font-family: var(--mono); font-size: 11px; color: var(--muted); padding: 0 14px 8px; }
@media (max-width: 1080px) {
  .stage { grid-template-columns: minmax(200px, 240px) 1fr; }
  .inspector {
    position: fixed; top: 0; right: 0; bottom: 0; width: min(360px, 92vw); z-index: 25;
    transform: translateX(105%); transition: transform 0.25s ease;
  }
  .inspector.open { transform: none; }
  .drawer-btn { display: block; }
}
@media (max-width: 720px) {
  .stage { grid-template-columns: 1fr; }
  .sidebar { display: none; }
  .sidebar.open { display: flex; position: fixed; inset: 0 20% 0 0; z-index: 24; }
}
</style>
</head>
<body>
<div class="app">
  <header class="chrome glass">
    <div class="wordmark">FORK BROWSER</div>
    <div class="nav-btns">
      <button class="icon-btn" id="btnBack" title="Back" onclick="goBack()">←</button>
      <button class="icon-btn" id="btnFwd" title="Forward" onclick="goFwd()">→</button>
      <button class="icon-btn" id="btnReload" title="Refresh" onclick="reloadSnap()">↻</button>
      <button class="icon-btn" id="btnTab" title="New tab" onclick="newTab()">+</button>
    </div>
    <div class="url-wrap">
      <span class="scheme"><span class="pulse"></span><span id="currentScheme">HTTPS</span></span>
      <input id="urlInput" value="https://example.com" placeholder="https://  finger://  dns://  gemini://  gopher://  news://">
    </div>
    <button class="go" id="btnGo" onclick="navigateUrl()">Navigate</button>
  </header>
  <div class="tabs" id="tabStrip"></div>
  <div class="presets glass">
    <label>Protocol testers</label>
    <button class="pill" onclick="loadPreset('https://example.com')">https://example.com</button>
    <button class="pill" onclick="loadPreset('finger://telehack.com/cow')">finger://telehack.com/cow</button>
    <button class="pill" onclick="loadPreset('dns://example.com/TXT')">dns://example.com/TXT</button>
    <button class="pill" onclick="loadPreset('dns://version.bind?class=CH')">dns CHAOS</button>
    <button class="pill" onclick="loadPreset('gemini://geminiprotocol.net/')">gemini://geminiprotocol.net</button>
    <button class="pill" onclick="loadPreset('gopher://gopher.floodgap.com/1')">gopher://floodgap</button>
    <button class="pill" onclick="loadPreset('news://news.eternal-september.org/')">news://eternal-september</button>
  </div>
  <div class="status-line" id="statusLine">live APIs: /api/snapshots · /api/search · /api/get · /api/snap</div>
  <div class="stage">
    <aside class="col sidebar glass" id="sidebar">
      <div class="col-h">Archive</div>
      <input id="searchInput" placeholder="Search snapshots…" oninput="triggerSearch()">
      <div id="snapshotList" class="empty">Loading archive…</div>
    </aside>
    <section class="col viewport">
      <div class="v-title" id="viewTitle">Select a snapshot or navigate</div>
      <div class="v-meta">
        <span id="viewUrl">url: none</span>
        <span id="viewTime">fetched_at: —</span>
        <span id="viewStatus">status: —</span>
      </div>
      <div class="v-body glass" id="viewBody">Fork preserves human web entropy.\n\nUse Navigate or a protocol tester. The inspector shows SHA-256 digests, prior-chain linkage, WWV 10 MHz beacon fields, and tailscore breakdown from live API data.</div>
    </section>
    <aside class="col inspector glass" id="inspector">
      <div class="col-h">Verifiability inspector</div>
      <div class="inspect-pad">
        <div class="block glass">
          <h3>Content hashes</h3>
          <div class="kv"><span>body_digest</span></div>
          <div class="hash" id="bodyDigest">sha256:none</div>
          <div class="kv" style="margin-top:8px"><span>extraction_digest</span></div>
          <div class="hash" id="extractDigest">sha256:none</div>
        </div>
        <div class="block glass">
          <h3>Per-URL prior chain</h3>
          <div class="kv">linkage <b id="priorState">genesis</b></div>
          <div class="hash" id="priorDigest">null</div>
        </div>
        <div class="block glass">
          <h3>WWV 10 MHz beacon</h3>
          <div class="kv">station <b id="beaconStation">WWV</b></div>
          <div class="kv">freq <b id="beaconFreq">10000000 Hz</b></div>
          <div class="kv">captured_at <b id="beaconWhen">—</b></div>
          <div class="kv">receiver <b id="beaconRx">unattached</b></div>
          <div class="hash" id="beaconDigest">sha256:none</div>
        </div>
        <div class="block glass">
          <h3>Tailscore humanness</h3>
          <div class="kv">at-risk score <b id="tailscoreVal">0.00</b></div>
          <div class="bar"><div class="fill" id="tailscoreFill" style="width:0%"></div></div>
          <div id="tailFactors"></div>
        </div>
        <div class="block glass">
          <h3>Handlers</h3>
          <div class="proto" id="protoGrid"></div>
        </div>
      </div>
    </aside>
  </div>
</div>
<button class="drawer-btn" onclick="document.getElementById('inspector').classList.toggle('open')">Inspector</button>
<script>
const PRESETS = [];
let archive = [];
let tabs = [{ id: 1, title: "start", snap: null }];
let tabId = 1;
let activeTab = 1;
let hist = [];
let histIdx = -1;
let busy = false;

function schemeOf(u) {
  try { return new URL(u).protocol.replace(':',' ').trim().toUpperCase(); }
  catch { return (u.split(':')[0] || 'URL').toUpperCase(); }
}
function $(id) { return document.getElementById(id); }
function setStatus(s) { $('statusLine').textContent = s; }
function setBusy(v) {
  busy = v;
  $('btnGo').classList.toggle('busy', v);
  $('btnGo').textContent = v ? 'Fetching…' : 'Navigate';
}
function updateScheme(u) { $('currentScheme').textContent = schemeOf(u || $('urlInput').value); }
function esc(s) {
  return String(s == null ? '' : s).replace(/[&<>"']/g, c => ({
    '&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'
  })[c]);
}

function renderTabs() {
  $('tabStrip').innerHTML = tabs.map(t =>
    '<div class="tab' + (t.id === activeTab ? ' on' : '') + '" onclick="switchTab(' + t.id + ')">' + esc(t.title) + '</div>'
  ).join('');
}
function switchTab(id) {
  activeTab = id;
  const t = tabs.find(x => x.id === id);
  if (t && t.snap) showSnap(t.snap, false);
  renderTabs();
}
function newTab() {
  tabId += 1;
  tabs.push({ id: tabId, title: 'tab ' + tabId, snap: null });
  activeTab = tabId;
  renderTabs();
}
function pushHist(snap) {
  hist = hist.slice(0, histIdx + 1);
  hist.push(snap);
  histIdx = hist.length - 1;
  $('btnBack').disabled = histIdx <= 0;
  $('btnFwd').disabled = histIdx >= hist.length - 1;
}
function goBack() { if (histIdx > 0) { histIdx--; showSnap(hist[histIdx], false); } }
function goFwd() { if (histIdx < hist.length - 1) { histIdx++; showSnap(hist[histIdx], false); } }

function factorHtml(factors) {
  if (!Array.isArray(factors) || !factors.length) return '<div class="factor">no factors</div>';
  return factors.map(f => {
    const d = Number(f.delta || 0);
    const sign = d >= 0 ? '+' : '';
    return '<div class="factor"><span>' + esc(f.label) + '</span><span>' + sign + d.toFixed(2) + '</span></div>';
  }).join('');
}

function showSnap(snap, recordHist) {
  if (!snap) return;
  if (recordHist !== false) pushHist(snap);
  const title = (snap.meta && snap.meta.title) || snap.url || '(untitled)';
  $('viewTitle').textContent = title;
  $('viewUrl').textContent = 'url: ' + (snap.url || 'none');
  $('viewTime').textContent = 'fetched_at: ' + (snap.fetched_at || '—');
  $('viewStatus').textContent = 'status: ' + (snap.status != null ? snap.status : '—');
  $('viewBody').textContent = (snap.meta && snap.meta.text_preview) || '(no extraction)';
  $('urlInput').value = snap.url || '';
  updateScheme(snap.url);
  $('bodyDigest').textContent = snap.body_digest || 'sha256:none';
  $('extractDigest').textContent = snap.extraction_digest || 'sha256:none';
  const prior = snap.prior;
  $('priorState').textContent = prior ? 'linked' : 'genesis';
  $('priorDigest').textContent = prior || 'null';
  const b = snap.beacon || {};
  $('beaconStation').textContent = b.station || 'WWV';
  $('beaconFreq').textContent = (b.freq_hz || 10000000) + ' Hz';
  $('beaconWhen').textContent = b.captured_at || snap.fetched_at || '—';
  $('beaconRx').textContent = b.receiver || 'unattached';
  $('beaconDigest').textContent = b.digest || 'sha256:none';
  const score = snap.tailscore != null ? Number(snap.tailscore) : 0;
  $('tailscoreVal').textContent = score.toFixed(2);
  $('tailscoreFill').style.width = Math.round(score * 100) + '%';
  const factors = (snap.meta && snap.meta.tailscore_factors) || [];
  $('tailFactors').innerHTML = factorHtml(factors);
  const t = tabs.find(x => x.id === activeTab);
  if (t) { t.snap = snap; t.title = title.slice(0, 24); }
  renderTabs();
  document.querySelectorAll('.card').forEach(el => {
    el.classList.toggle('on', el.dataset.digest === (snap.body_digest || ''));
  });
  setStatus('loaded ' + (snap.url || '') + ' · ' + (snap.body_digest || '').slice(0, 22));
}

function renderList(list) {
  const box = $('snapshotList');
  if (!list || !list.length) {
    box.className = 'empty';
    box.textContent = 'No snapshots yet. Navigate or hit a protocol tester.';
    return;
  }
  box.className = '';
  box.innerHTML = list.map(s => {
    const title = (s.meta && s.meta.title) || s.url || '(untitled)';
    const digest = s.body_digest || '';
    return '<div class="card" data-digest="' + esc(digest) + '" onclick="openSnap(\'' + esc(digest) + '\')"><h4>' + esc(title) + '</h4><p>' + esc(s.url) + '</p></div>';
  }).join('');
}

async function loadSnapshots() {
  try {
    const res = await fetch('/api/snapshots');
    archive = await res.json();
    renderList(archive);
    const schemes = await (await fetch('/api/protocols')).json();
    $('protoGrid').innerHTML = (schemes || []).map(s => '<i>' + esc(s) + ' <span class="dot"></span></i>').join('');
  } catch (e) {
    $('snapshotList').textContent = 'API error: ' + e;
  }
}

async function triggerSearch() {
  const q = $('searchInput').value;
  try {
    const res = await fetch('/api/search?q=' + encodeURIComponent(q));
    renderList(await res.json());
    setStatus('search q=' + q);
  } catch (e) { setStatus('search failed: ' + e); }
}

async function openSnap(digest) {
  try {
    const res = await fetch('/api/get?target=' + encodeURIComponent(digest));
    if (!res.ok) { setStatus('get failed'); return; }
    showSnap(await res.json());
  } catch (e) { setStatus('get error: ' + e); }
}

async function navigateUrl() {
  const url = $('urlInput').value.trim();
  if (!url || busy) return;
  updateScheme(url);
  setBusy(true);
  setStatus('snapping ' + url + ' via protocol handler…');
  try {
    const res = await fetch('/api/snap?url=' + encodeURIComponent(url));
    const data = await res.json();
    if (!res.ok) throw new Error(data.error || res.statusText);
    showSnap(data);
    await loadSnapshots();
  } catch (e) {
    $('viewTitle').textContent = 'Fetch failed';
    $('viewBody').textContent = String(e);
    setStatus('snap error: ' + e);
  } finally { setBusy(false); }
}

function loadPreset(u) { $('urlInput').value = u; updateScheme(u); navigateUrl(); }
function reloadSnap() {
  const t = tabs.find(x => x.id === activeTab);
  if (t && t.snap && t.snap.url) { $('urlInput').value = t.snap.url; navigateUrl(); }
  else navigateUrl();
}
$('urlInput').addEventListener('keydown', ev => { if (ev.key === 'Enter') navigateUrl(); });
$('urlInput').addEventListener('input', () => updateScheme());
loadSnapshots();
renderTabs();
updateScheme();
</script>
</body>
</html>
"###;
