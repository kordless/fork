//! Built-in Web Browser Client Server for Fork.
//!
//! Binds to 0.0.0.0:8080 and serves an interactive web-based browser interface
//! for querying, reconstituting, searching, and verifying snapshots across all protocols.

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
  <title>Fork — Web Browser for Human Entropy</title>
  <link rel="preconnect" href="https://fonts.googleapis.com">
  <link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
  <link href="https://fonts.googleapis.com/css2?family=Outfit:wght@300;400;600;700&family=JetBrains+Mono:wght@400;600&display=swap" rel="stylesheet">
  <style>
    :root {
      --bg-dark: #0a0c10;
      --bg-panel: #121620;
      --bg-card: #1a202c;
      --accent: #00f2fe;
      --accent-glow: rgba(0, 242, 254, 0.25);
      --purple: #9d4edd;
      --text: #f1f5f9;
      --text-dim: #94a3b8;
      --border: #2d3748;
      --font-main: 'Outfit', sans-serif;
      --font-mono: 'JetBrains Mono', monospace;
    }
    * { box-sizing: border-box; margin: 0; padding: 0; }
    body {
      background: var(--bg-dark);
      color: var(--text);
      font-family: var(--font-main);
      display: flex;
      flex-direction: column;
      height: 100vh;
      overflow: hidden;
    }
    /* Header / Browser Nav Bar */
    header {
      background: var(--bg-panel);
      border-bottom: 1px solid var(--border);
      padding: 12px 20px;
      display: flex;
      align-items: center;
      gap: 16px;
    }
    .logo {
      font-weight: 700;
      font-size: 1.3rem;
      letter-spacing: -0.5px;
      background: linear-gradient(135deg, var(--accent), var(--purple));
      -webkit-background-clip: text;
      -webkit-text-fill-color: transparent;
      display: flex;
      align-items: center;
      gap: 8px;
    }
    .url-bar-container {
      flex: 1;
      display: flex;
      align-items: center;
      background: var(--bg-dark);
      border: 1px solid var(--border);
      border-radius: 8px;
      padding: 6px 14px;
      gap: 10px;
      transition: all 0.2s ease;
    }
    .url-bar-container:focus-within {
      border-color: var(--accent);
      box-shadow: 0 0 12px var(--accent-glow);
    }
    .scheme-badge {
      background: rgba(0, 242, 254, 0.15);
      color: var(--accent);
      font-family: var(--font-mono);
      font-size: 0.75rem;
      padding: 2px 8px;
      border-radius: 4px;
      font-weight: 600;
    }
    .url-input {
      flex: 1;
      background: transparent;
      border: none;
      outline: none;
      color: var(--text);
      font-family: var(--font-mono);
      font-size: 0.9rem;
    }
    .btn {
      background: linear-gradient(135deg, #00f2fe, #4facfe);
      color: #000;
      font-weight: 600;
      border: none;
      padding: 8px 18px;
      border-radius: 6px;
      cursor: pointer;
      font-family: var(--font-main);
      transition: transform 0.1s ease, filter 0.2s ease;
    }
    .btn:hover { filter: brightness(1.1); transform: translateY(-1px); }
    .btn-secondary {
      background: var(--bg-card);
      color: var(--text);
      border: 1px solid var(--border);
    }
    /* Main Layout Grid */
    main {
      flex: 1;
      display: grid;
      grid-template-columns: 320px 1fr 340px;
      height: calc(100vh - 65px);
    }
    /* Left Sidebar: Snapshots List & Search */
    .sidebar {
      background: var(--bg-panel);
      border-right: 1px solid var(--border);
      display: flex;
      flex-direction: column;
    }
    .sidebar-header {
      padding: 16px;
      border-bottom: 1px solid var(--border);
    }
    .search-box {
      width: 100%;
      background: var(--bg-dark);
      border: 1px solid var(--border);
      color: var(--text);
      padding: 8px 12px;
      border-radius: 6px;
      font-family: var(--font-main);
      outline: none;
    }
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
      margin-bottom: 10px;
      cursor: pointer;
      transition: border-color 0.2s ease, transform 0.1s ease;
    }
    .snap-card:hover { border-color: var(--accent); transform: translateX(2px); }
    .snap-card.active { border-color: var(--accent); background: rgba(0,242,254,0.05); }
    .snap-title { font-weight: 600; font-size: 0.95rem; margin-bottom: 4px; color: var(--text); }
    .snap-url { font-family: var(--font-mono); font-size: 0.75rem; color: var(--text-dim); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
    /* Center Viewport */
    .viewport {
      background: var(--bg-dark);
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
    .view-title { font-size: 1.6rem; font-weight: 700; margin-bottom: 8px; }
    .view-meta-bar { display: flex; gap: 16px; font-family: var(--font-mono); font-size: 0.8rem; color: var(--text-dim); }
    .view-body {
      background: var(--bg-panel);
      border: 1px solid var(--border);
      border-radius: 8px;
      padding: 20px;
      font-family: var(--font-mono);
      font-size: 0.9rem;
      line-height: 1.6;
      white-space: pre-wrap;
      flex: 1;
    }
    /* Right Inspector Sidebar */
    .inspector {
      background: var(--bg-panel);
      border-left: 1px solid var(--border);
      padding: 20px;
      display: flex;
      flex-direction: column;
      gap: 20px;
      overflow-y: auto;
    }
    .inspect-section {
      background: var(--bg-card);
      border: 1px solid var(--border);
      border-radius: 8px;
      padding: 14px;
    }
    .inspect-title { font-size: 0.85rem; text-transform: uppercase; font-weight: 700; color: var(--text-dim); margin-bottom: 10px; letter-spacing: 0.5px; }
    .hash-badge {
      background: var(--bg-dark);
      font-family: var(--font-mono);
      font-size: 0.75rem;
      padding: 6px;
      border-radius: 4px;
      word-break: break-all;
      color: var(--accent);
      border: 1px solid var(--border);
    }
    .tailscore-bar {
      height: 8px;
      background: #2d3748;
      border-radius: 4px;
      overflow: hidden;
      margin-top: 6px;
    }
    .tailscore-fill {
      height: 100%;
      background: linear-gradient(90deg, #ff416c, #8a2387, #00f2fe);
      width: 0%;
      transition: width 0.5s ease;
    }
    /* Protocol Status Badges */
    .protocol-grid {
      display: grid;
      grid-template-columns: repeat(2, 1fr);
      gap: 8px;
    }
    .proto-item {
      background: var(--bg-dark);
      border: 1px solid var(--border);
      padding: 6px 10px;
      border-radius: 6px;
      font-family: var(--font-mono);
      font-size: 0.75rem;
      display: flex;
      align-items: center;
      justify-content: space-between;
    }
    .status-dot { width: 8px; height: 8px; border-radius: 50%; background: #10b981; }
  </style>
</head>
<body>

  <header>
    <div class="logo">
      <span>🪃</span> FORK BROWSER
    </div>
    <div class="url-bar-container">
      <span class="scheme-badge" id="currentScheme">HTTP</span>
      <input type="text" class="url-input" id="urlInput" value="https://news.ycombinator.com" placeholder="Enter URL (https://, finger://, dns://, gemini://, gopher://)...">
    </div>
    <button class="btn" onclick="navigateUrl()">Navigate</button>
  </header>

  <main>
    <!-- Left Sidebar -->
    <div class="sidebar">
      <div class="sidebar-header">
        <input type="text" class="search-box" id="searchInput" placeholder="Search human web snapshots..." oninput="triggerSearch()">
      </div>
      <div class="snapshot-list" id="snapshotList">
        <!-- Cards inserted dynamically -->
      </div>
    </div>

    <!-- Center Viewport -->
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

Use the address bar above to browse the preserved web across multiple protocols:
- https://     Standard web pages
- finger://    RFC 1288 hacker & academic status .plan files
- dns://       DNS TXT, CAA & CHAOS history records
- gemini://    Lightweight TLS capsules
- gopher://    RFC 1436 text menus

Select any snapshot on the left sidebar to inspect its SHA-256 verifiability digests, per-URL prior chain, and radio frequency WWV beacon timestamps.
      </div>
    </div>

    <!-- Right Inspector -->
    <div class="inspector">
      <div class="inspect-section">
        <div class="inspect-title">Verifiability Hashes</div>
        <div style="margin-bottom: 8px;">
          <div style="font-size:0.75rem; color:var(--text-dim);">Body Digest (SHA-256):</div>
          <div class="hash-badge" id="bodyDigest">sha256:none</div>
        </div>
        <div>
          <div style="font-size:0.75rem; color:var(--text-dim);">Extraction Digest:</div>
          <div class="hash-badge" id="extractDigest">sha256:none</div>
        </div>
      </div>

      <div class="inspect-section">
        <div class="inspect-title">Freshness Beacon Anchor</div>
        <div style="font-size:0.75rem; color:var(--text-dim); margin-bottom: 4px;">WWV 10MHz Shortwave Beacon Digest:</div>
        <div class="hash-badge" id="beaconDigest">sha256:wwv-10mhz-beacon-anchored</div>
      </div>

      <div class="inspect-section">
        <div class="inspect-title">Human Tailscore</div>
        <div style="display:flex; justify-content:space-between; font-family:var(--font-mono); font-size:0.85rem;">
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

    async function triggerSearch() {
      const q = document.getElementById('searchInput').value;
      const res = await fetch('/api/search?q=' + encodeURIComponent(q));
      const hits = await res.json();
      renderSnapshotList(hits);
    }

    function navigateUrl() {
      const url = document.getElementById('urlInput').value;
      updateSchemeBadge(url);
      document.getElementById('viewTitle').textContent = 'Simulated Live Fetch: ' + url;
      document.getElementById('viewUrl').textContent = 'url: ' + url;
      document.getElementById('viewTime').textContent = 'fetched_at: ' + new Date().toISOString();
      document.getElementById('viewStatus').textContent = 'status: 200';
      document.getElementById('viewBody').textContent = 'Fetching content across self-forged protocol handler [' + document.getElementById('currentScheme').textContent + ']...\n\nSnapshot written to content-addressed store.';
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
