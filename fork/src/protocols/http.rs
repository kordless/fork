//! HTTP and HTTPS handler.

use super::{ProtocolHandler, ProtocolPayload};
use anyhow::{Context, Result};
use std::time::Duration;
use url::Url;

#[derive(Clone)]
pub struct HttpHandler {
    scheme: &'static str,
    client: reqwest::Client,
}

impl HttpHandler {
    pub fn new(scheme: &'static str, client: reqwest::Client) -> Self {
        Self { scheme, client }
    }

    pub fn shared_client() -> Result<reqwest::Client> {
        reqwest::Client::builder()
            .user_agent("Fork/0.1 (+https://fork.local; human-entropy-preservation)")
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::limited(8))
            .build()
            .context("build HTTP client")
    }
}

#[async_trait::async_trait]
impl ProtocolHandler for HttpHandler {
    fn scheme(&self) -> &'static str {
        self.scheme
    }

    async fn fetch(&self, url: &Url) -> Result<ProtocolPayload> {
        let resp = self
            .client
            .get(url.as_str())
            .send()
            .await
            .with_context(|| format!("HTTP request failed for {url}"))?;

        let status = resp.status().as_u16();
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let final_url = resp.url().clone();
        let raw_bytes = resp.bytes().await?.to_vec();

        let is_html = content_type
            .as_deref()
            .map(|c| c.contains("html"))
            .unwrap_or(false);

        let (extracted_text, title, links) = if is_html {
            let html = String::from_utf8_lossy(&raw_bytes);
            (
                super::html::normalize_extraction(&html),
                super::html::extract_title(&html),
                super::html::extract_links(&final_url, &html),
            )
        } else {
            (
                String::from_utf8_lossy(&raw_bytes)
                    .chars()
                    .take(50_000)
                    .collect(),
                None,
                Vec::new(),
            )
        };

        Ok(ProtocolPayload {
            raw_bytes,
            content_type,
            extracted_text,
            status,
            title,
            links,
            final_url: Some(final_url.to_string()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn serve_http(status: u16, content_type: &str, body: &str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let body = body.to_string();
        let content_type = content_type.to_string();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 2048];
            let _ = sock.read(&mut buf).await;
            let resp = format!(
                "HTTP/1.1 {status} OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(resp.as_bytes()).await;
        });
        format!("http://{addr}/page")
    }

    #[tokio::test]
    async fn fetches_html_and_extracts_links() {
        let url = serve_http(
            200,
            "text/html; charset=utf-8",
            "<html><head><title>Hello Fork</title></head><body><p>preserved</p><a href=\"/next\">n</a></body></html>",
        )
        .await;
        let client = HttpHandler::shared_client().unwrap();
        let handler = HttpHandler::new("http", client);
        let parsed = Url::parse(&url).unwrap();
        let payload = handler.fetch(&parsed).await.unwrap();
        assert_eq!(payload.status, 200);
        assert_eq!(payload.title.as_deref(), Some("Hello Fork"));
        assert!(payload.extracted_text.contains("preserved"));
        assert!(payload.links.iter().any(|l| l.ends_with("/next")));
    }
}
