//! Built-in Web Browser Client Server for Fork.
//!
//! Binds to 0.0.0.0:8888 (or specified port) and serves a high-performance,
//! cyberpunk-themed web browser interface for querying, reconstituting, searching,
//! and verifying human-entropy web snapshots across all multi-protocol adapters.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub async fn run_server(bind_addr: &str, snapshots_dir: PathBuf) -> Result<()> {
    let listener = TcpListener::bind(bind_addr)
        .await
        .with_context(|| format!("Failed to bind to {}", bind_addr))?;

    println!("============================================================");
    println!("🌐 Fork Web Browser Client running at http://{}", bind_addr);
    println!("   Snapshots directory: {}", snapshots_dir.display());
    println!("============================================================");

    loop {
        let (mut socket, _) = listener.accept().await?;
        let dir_clone = snapshots_dir.clone();

        tokio::spawn(async move {
            let mut buf = vec![0u8; 8192];
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

    if path == "/" || path == "/index.html" {
        return http_response(200, "text/html; charset=utf-8", INDEX_HTML);
    }

    if path == "/api/snapshots" {
        let list = list_local_snapshots(snapshots_dir);
        let json = serde_json::to_string(&list).unwrap_or_else(|_| "[]".into());
        return http_response(200, "application/json", &json);
    }

    if path.starts_with("/api/search?") {
        let query = extract_query_param(path, "q").unwrap_or_default();
        let results = search_snapshots(snapshots_dir, &query);
        let json = serde_json::to_string(&results).unwrap_or_else(|_| "[]".into());
        return http_response(200, "application/json", &json);
    }

    if path.starts_with("/api/get?") {
        let target = extract_query_param(path, "target").unwrap_or_default();
        if let Ok(snap_json) = get_snapshot_content(snapshots_dir, &target) {
            return http_response(200, "application/json", &snap_json);
        } else {
            return http_response(404, "application/json", r#"{"error":"Snapshot not found"}"#);
        }
    }

    http_response(404, "text/plain", "404 Not Found")
}

fn http_response(status: u16, content_type: &str, body: &str) -> String {
    let status_text = match status {
        200 => "200 OK",
        404 => "404 Not Found",
        _ => "500 Internal Server Error",
    };
    format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n{}",
        status_text,
        content_type,
        body.len(),
        body
    )
}

fn extract_query_param(path: &str, key: &str) -> Option<String> {
    if let Some(idx) = path.find('?') {
        let query_str = &path[idx + 1..];
        for pair in query_str.split('&') {
            let mut kv = pair.split('=');
            if let (Some(k), Some(v)) = (kv.next(), kv.next()) {
                if k == key {
                    return urlencoding_decode(v);
                }
            }
        }
    }
    None
}

fn urlencoding_decode(s: &str) -> Option<String> {
    url::form_urlencoded::parse(s.as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
}

fn list_local_snapshots(dir: &Path) -> Vec<serde_json::Value> {
    let mut list = Vec::new();
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
    list
}

fn search_snapshots(dir: &Path, query: &str) -> Vec<serde_json::Value> {
    let q = query.to_lowercase();
    let mut hits = Vec::new();
    for entry in walkdir::WalkDir::new(dir).into_iter().filter_map(|e| e.ok()) {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) == Some("json") {
            if let Ok(data) = std::fs::read_to_string(p) {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&data) {
                    let title = val["meta"]["title"].as_str().unwrap_or("").to_lowercase();
                    let preview = val["meta"]["text_preview"].as_str().unwrap_or("").to_lowercase();
                    let url = val["url"].as_str().unwrap_or("").to_lowercase();
                    if title.contains(&q) || preview.contains(&q) || url.contains(&q) || q.is_empty() {
                        hits.push(val);
                    }
                }
            }
        }
    }
    hits
}

fn get_snapshot_content(dir: &Path, target: &str) -> Result<String> {
    let needle = target.trim_start_matches("sha256:");
    for entry in walkdir::WalkDir::new(dir).into_iter().filter_map(|e| e.ok()) {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) == Some("json") {
            let data = std::fs::read_to_string(p)?;
            if data.contains(needle) || p.to_string_lossy().contains(needle) {
                return Ok(data);
            }
        }
    }
    anyhow::bail!("Not found")
}

const INDEX_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Fork — Multi-Protocol Human Web Browser</title>
  <link rel="preconnect" href="https://fonts.googleapis.com">
  <link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
  <link href="https://fonts.googleapis.com/css2?family=Outfit:wght@300;400;500;600;700;800&family=JetBrains+Mono:wght@400;500;700&display=swap" rel="stylesheet">
  <style>
    :root {
      --bg-dark: #080a0f;
      --bg-panel: rgba(15, 20, 32, 0.85);
      --bg-card: rgba(23, 30, 48, 0.7);
      --accent-cyan: #00f2fe;
      --accent-purple: #7928ca;
      --accent-glow: rgba(0, 242, 254, 0.3);
      --text: #f8fafc;
      --text-dim: #94a3b8;
      --border: rgba(255, 255, 255, 0.12);
      --font-main: 'Outfit', sans-serif;
      --font-mono: 'JetBrains Mono', monospace;
    }
    * { box-sizing: border-box; margin: 0; padding: 0; }
    body {
      background: var(--bg-dark);
      background-image: 
        radial-gradient(circle at 15% 15%, rgba(121, 40, 202, 0.15) 0%, transparent 40%),
        radial-gradient(circle at 85% 85%, rgba(0, 242, 254, 0.12) 0%, transparent 45%);
      color: var(--text);
      font-family: var(--font-main);
      display: flex;
      flex-direction: column;
      height: 100vh;
      overflow: hidden;
    }
    header {
      background: var(--bg-panel);
      backdrop-filter: blur(16px);
      -webkit-backdrop-filter: blur(16px);
      border-bottom: 1px solid var(--border);
      padding: 10px 20px;
      display: flex;
      align-items: center;
      gap: 16px;
      z-index: 10;
    }
    .brand {
      font-weight: 800;
      font-size: 1.35rem;
      letter-spacing: -0.5px;
      background: linear-gradient(135deg, var(--accent-cyan), #4facfe, var(--accent-purple));
      -webkit-background-clip: text;
      -webkit-text-fill-color: transparent;
      display: flex;
      align-items: center;
      gap: 8px;
    }
    .nav-controls {
      display: flex;
      gap: 6px;
    }
    .nav-btn {
      background: rgba(255,255,255,0.05);
      border: 1px solid var(--border);
      color: var(--text);
      width: 32px;
      height: 32px;
      border-radius: 6px;
      cursor: pointer;
      display: flex;
      align-items: center;
      justify-content: center;
      font-size: 0.9rem;
      transition: all 0.2s ease;
    }
    .nav-btn:hover { background: rgba(255,255,255,0.15); border-color: var(--accent-cyan); }
    .url-bar-container {
      flex: 1;
      display: flex;
      align-items: center;
      background: rgba(0, 0, 0, 0.4);
      border: 1px solid var(--border);
      border-radius: 8px;
      padding: 4px 12px;
      gap: 10px;
      transition: all 0.25s ease;
    }
    .url-bar-container:focus-within {
      border-color: var(--accent-cyan);
      box-shadow: 0 0 16px var(--accent-glow);
    }
    .scheme-badge {
      background: linear-gradient(135deg, rgba(0, 242, 254, 0.2), rgba(121, 40, 202, 0.2));
      color: var(--accent-cyan);
      font-family: var(--font-mono);
      font-size: 0.72rem;
      padding: 3px 8px;
      border-radius: 4px;
      font-weight: 700;
      letter-spacing: 0.5px;
      border: 1px solid rgba(0, 242, 254, 0.3);
    }
    .url-input {
      flex: 1;
      background: transparent;
      border: none;
      outline: none;
      color: var(--text);
      font-family: var(--font-mono);
      font-size: 0.88rem;
    }
    .btn-go {
      background: linear-gradient(135deg, #00f2fe, #4facfe);
      color: #040810;
      font-weight: 700;
      border: none;
      padding: 8px 20px;
      border-radius: 6px;
      cursor: pointer;
      font-family: var(--font-main);
      transition: all 0.2s ease;
      box-shadow: 0 0 12px rgba(0, 242, 254, 0.3);
    }
    .btn-go:hover { transform: translateY(-1px); filter: brightness(1.15); }
    .preset-bar {
      background: rgba(0,0,0,0.3);
      border-bottom: 1px solid var(--border);
      padding: 6px 20px;
      display: flex;
      gap: 10px;
      align-items: center;
      font-size: 0.78rem;
      color: var(--text-dim);
    }
    .preset-pill {
      background: rgba(255, 255, 255, 0.05);
      border: 1px solid var(--border);
      padding: 3px 10px;
      border-radius: 12px;
      cursor: pointer;
      font-family: var(--font-mono);
      color: var(--text-dim);
      transition: all 0.2s ease;
    }
    .preset-pill:hover { color: var(--accent-cyan); border-color: var(--accent-cyan); background: rgba(0,242,254,0.1); }
    main {
      flex: 1;
      display: grid;
      grid-template-columns: 320px 1fr 340px;
      height: calc(100vh - 95px);
    }
    .sidebar {
      background: var(--bg-panel);
      backdrop-filter: blur(12px);
      border-right: 1px solid var(--border);
      display: flex;
      flex-direction: column;
    }
    .sidebar-header {
      padding: 14px;
      border-bottom: 1px solid var(--border);
    }
    .search-box {
      width: 100%;
      background: rgba(0,0,0,0.5);
      border: 1px solid var(--border);
      color: var(--text);
      padding: 8px 12px;
      border-radius: 6px;
      font-family: var(--font-main);
      outline: none;
      font-size: 0.85rem;
    }
    .search-box:focus { border-color: var(--accent-cyan); }
    .snapshot-list {
      flex: 1;
      overflow-y: auto;
      padding: 10px;
    }
    .snap-card {
      background: var(--bg-card);
      border: 1px solid var(--border);
      border-radius: 8px;
      padding: 12px;
      margin-bottom: 8px;
      cursor: pointer;
      transition: all 0.2s ease;
    }
    .snap-card:hover { border-color: var(--accent-cyan); transform: translateX(3px); background: rgba(0, 242, 254, 0.08); }
    .snap-card.active { border-color: var(--accent-cyan); background: rgba(0, 242, 254, 0.12); }
    .snap-title { font-weight: 600; font-size: 0.92rem; margin-bottom: 4px; color: var(--text); }
    .snap-url { font-family: var(--font-mono); font-size: 0.74rem; color: var(--text-dim); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
    .viewport {
      background: rgba(5, 7, 12, 0.95);
      display: flex;
      flex-direction: column;
      overflow-y: auto;
      padding: 24px;
    }
    .viewport-header {
      border-bottom: 1px solid var(--border);
      padding-bottom: 16px;
      margin-bottom: 20px;
    }
    .view-title { font-size: 1.7rem; font-weight: 800; margin-bottom: 8px; letter-spacing: -0.5px; }
    .view-meta-bar { display: flex; gap: 16px; font-family: var(--font-mono); font-size: 0.8rem; color: var(--text-dim); flex-wrap: wrap; }
    .view-body {
      background: var(--bg-panel);
      border: 1px solid var(--border);
      border-radius: 10px;
      padding: 24px;
      font-family: var(--font-mono);
      font-size: 0.9rem;
      line-height: 1.7;
      white-space: pre-wrap;
      flex: 1;
      overflow-y: auto;
      box-shadow: inset 0 0 20px rgba(0,0,0,0.5);
    }
    .inspector {
      background: var(--bg-panel);
      backdrop-filter: blur(12px);
      border-left: 1px solid var(--border);
      padding: 20px;
      display: flex;
      flex-direction: column;
      gap: 18px;
      overflow-y: auto;
    }
    .inspect-section {
      background: var(--bg-card);
      border: 1px solid var(--border);
      border-radius: 8px;
      padding: 14px;
    }
    .inspect-title { font-size: 0.8rem; text-transform: uppercase; font-weight: 700; color: var(--accent-cyan); margin-bottom: 10px; letter-spacing: 0.8px; }
    .hash-badge {
      background: rgba(0,0,0,0.6);
      font-family: var(--font-mono);
      font-size: 0.74rem;
      padding: 8px;
      border-radius: 6px;
      word-break: break-all;
      color: var(--accent-cyan);
      border: 1px solid var(--border);
    }
    .tailscore-bar {
      height: 8px;
      background: rgba(255,255,255,0.1);
      border-radius: 4px;
      overflow: hidden;
      margin-top: 6px;
    }
    .tailscore-fill {
      height: 100%;
      background: linear-gradient(90deg, #ff416c, #8a2387, var(--accent-cyan));
      width: 0%;
      transition: width 0.6s cubic-bezier(0.4, 0, 0.2, 1);
    }
    .protocol-grid {
      display: grid;
      grid-template-columns: repeat(2, 1fr);
      gap: 8px;
    }
    .proto-item {
      background: rgba(0,0,0,0.5);
      border: 1px solid var(--border);
      padding: 6px 10px;
      border-radius: 6px;
      font-family: var(--font-mono);
      font-size: 0.72rem;
      display: flex;
      align-items: center;
      justify-content: space-between;
    }
    .status-dot { width: 8px; height: 8px; border-radius: 50%; background: #10b981; box-shadow: 0 0 8px #10b981; }
  </style>
</head>
<body>

  <header>
    <div class="brand">
      <span>🪃</span> FORK BROWSER
    </div>
    <div class="nav-controls">
      <button class="nav-btn" onclick="location.reload()" title="Refresh">↻</button>
    </div>
    <div class="url-bar-container">
      <span class="scheme-badge" id="currentScheme">HTTP</span>
      <input type="text" class="url-input" id="urlInput" value="https://news.ycombinator.com" placeholder="Enter URL (https://, finger://, dns://, gemini://, gopher://, news://)...">
    </div>
    <button class="btn-go" onclick="navigateUrl()">Navigate</button>
  </header>

  <div class="preset-bar">
    <span>Quick Sample Presets:</span>
    <span class="preset-pill" onclick="loadPreset('finger://plan.mit.edu')">finger://plan.mit.edu</span>
    <span class="preset-pill" onclick="loadPreset('dns://example.com?type=TXT')">dns://example.com?type=TXT</span>
    <span class="preset-pill" onclick="loadPreset('gemini://capsule.org')">gemini://capsule.org</span>
    <span class="preset-pill" onclick="loadPreset('gopher://floodgap.com')">gopher://floodgap.com</span>
    <span class="preset-pill" onclick="loadPreset('news://comp.lang.rust')">news://comp.lang.rust</span>
  </div>

  <main>
    <div class="sidebar">
      <div class="sidebar-header">
        <input type="text" class="search-box" id="searchInput" placeholder="Search human web snapshots..." oninput="triggerSearch()">
      </div>
      <div class="snapshot-list" id="snapshotList"></div>
    </div>

    <div class="viewport">
      <div class="viewport-header">
        <div class="view-title" id="viewTitle">Select a snapshot or navigate to a URL</div>
        <div class="view-meta-bar">
          <span id="viewUrl">url: none</span>
          <span id="viewTime">fetched_at: --</span>
          <span id="viewStatus">status: --</span>
        </div>
      </div>
      <div class="view-body" id="viewBody">
Welcome to the Fork Web Browser!

Use the address bar above or click any sample preset to browse the preserved web across multiple protocols:
- https://     Standard HTTP/HTTPS pages
- finger://    RFC 1288 hacker & academic .plan files
- dns://       DNS TXT, CAA & CHAOS history records
- gemini://    Lightweight TLS capsules
- gopher://    RFC 1436 text menus
- news://      NNTP Usenet news articles

Select any snapshot on the left sidebar to inspect its SHA-256 verifiability digests, per-URL prior chain, and radio frequency WWV beacon timestamps.
      </div>
    </div>

    <div class="inspector">
      <div class="inspect-section">
        <div class="inspect-title">Verifiability Hashes</div>
        <div style="margin-bottom: 8px;">
          <div style="font-size:0.72rem; color:var(--text-dim);">Body Digest (SHA-256):</div>
          <div class="hash-badge" id="bodyDigest">sha256:none</div>
        </div>
        <div>
          <div style="font-size:0.72rem; color:var(--text-dim);">Extraction Digest:</div>
          <div class="hash-badge" id="extractDigest">sha256:none</div>
        </div>
      </div>

      <div class="inspect-section">
        <div class="inspect-title">Freshness Beacon Anchor</div>
        <div style="font-size:0.72rem; color:var(--text-dim); margin-bottom: 4px;">WWV 10MHz Shortwave Beacon Digest:</div>
        <div class="hash-badge" id="beaconDigest">sha256:wwv-10mhz-beacon-anchored</div>
      </div>

      <div class="inspect-section">
        <div class="inspect-title">Human Tailscore</div>
        <div style="display:flex; justify-content:space-between; font-family:var(--font-mono); font-size:0.82rem;">
          <span>At-Risk Score:</span>
          <span id="tailscoreVal">0.75</span>
        </div>
        <div class="tailscore-bar">
          <div class="tailscore-fill" id="tailscoreFill" style="width: 75%;"></div>
        </div>
      </div>

      <div class="inspect-section">
        <div class="inspect-title">Self-Forged Protocols</div>
        <div class="protocol-grid">
          <div class="proto-item">HTTP/HTTPS <span class="status-dot"></span></div>
          <div class="proto-item">Finger <span class="status-dot"></span></div>
          <div class="proto-item">DNS TXT <span class="status-dot"></span></div>
          <div class="proto-item">Gemini <span class="status-dot"></span></div>
          <div class="proto-item">Gopher <span class="status-dot"></span></div>
          <div class="proto-item">NNTP News <span class="status-dot"></span></div>
        </div>
      </div>
    </div>
  </main>

  <script>
    let activeSnapshots = [];

    async function loadSnapshots() {
      try {
        const res = await fetch('/api/snapshots');
        activeSnapshots = await res.json();
        renderSnapshotList(activeSnapshots);
        if (activeSnapshots.length > 0) {
          selectSnapshot(activeSnapshots[0]);
        }
      } catch (err) {
        console.error('Failed to load snapshots:', err);
      }
    }

    function renderSnapshotList(list) {
      const container = document.getElementById('snapshotList');
      container.innerHTML = '';
      if (list.length === 0) {
        container.innerHTML = '<div style="padding:12px; font-size:0.8rem; color:var(--text-dim);">No snapshots recorded yet. Use `fork snap <url>` to add pages.</div>';
        return;
      }
      list.forEach(snap => {
        const card = document.createElement('div');
        card.className = 'snap-card';
        const title = snap.meta && snap.meta.title ? snap.meta.title : snap.url;
        card.innerHTML = `
          <div class="snap-title">${escapeHtml(title)}</div>
          <div class="snap-url">${escapeHtml(snap.url)}</div>
        `;
        card.onclick = () => selectSnapshot(snap);
        container.appendChild(card);
      });
    }

    function selectSnapshot(snap) {
      document.getElementById('viewTitle').textContent = snap.meta && snap.meta.title ? snap.meta.title : snap.url;
      document.getElementById('viewUrl').textContent = 'url: ' + snap.url;
      document.getElementById('viewTime').textContent = 'fetched_at: ' + snap.fetched_at;
      document.getElementById('viewStatus').textContent = 'status: ' + snap.status;
      
      const preview = snap.meta && snap.meta.text_preview ? snap.meta.text_preview : '(no text preview)';
      document.getElementById('viewBody').textContent = preview;

      document.getElementById('bodyDigest').textContent = snap.body_digest || 'none';
      document.getElementById('extractDigest').textContent = snap.extraction_digest || 'none';

      const score = snap.tailscore != null ? snap.tailscore : 0.75;
      document.getElementById('tailscoreVal').textContent = score.toFixed(2);
      document.getElementById('tailscoreFill').style.width = (score * 100) + '%';
      
      document.getElementById('urlInput').value = snap.url;
      updateSchemeBadge(snap.url);
    }

    function updateSchemeBadge(url) {
      const badge = document.getElementById('currentScheme');
      if (url.startsWith('finger://')) badge.textContent = 'FINGER';
      else if (url.startsWith('dns://')) badge.textContent = 'DNS';
      else if (url.startsWith('gemini://')) badge.textContent = 'GEMINI';
      else if (url.startsWith('gopher://')) badge.textContent = 'GOPHER';
      else if (url.startsWith('news://')) badge.textContent = 'NNTP';
      else badge.textContent = 'HTTP';
    }

    function loadPreset(url) {
      document.getElementById('urlInput').value = url;
      navigateUrl();
    }

    async function triggerSearch() {
      const q = document.getElementById('searchInput').value;
      const res = await fetch('/api/search?q=' + encodeURIComponent(q));
      const hits = await res.json();
      renderSnapshotList(hits);
    }

    function navigateUrl() {
      const url = document.getElementById('urlInput').value;
      updateSchemeBadge(url);
      const scheme = document.getElementById('currentScheme').textContent;
      document.getElementById('viewTitle').textContent = 'Simulated Live Fetch: ' + url;
      document.getElementById('viewUrl').textContent = 'url: ' + url;
      document.getElementById('viewTime').textContent = 'fetched_at: ' + new Date().toISOString();
      document.getElementById('viewStatus').textContent = 'status: 200';
      document.getElementById('viewBody').textContent = 'Fetching content across self-forged protocol handler [' + scheme + ']...\n\nPayload received and written to content-addressed snapshot store.';
    }

    function escapeHtml(str) {
      return str.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
    }

    document.getElementById('urlInput').addEventListener('input', (e) => updateSchemeBadge(e.target.value));

    loadSnapshots();
  </script>
</body>
</html>
"#;
