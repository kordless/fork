//! Protocol abstraction module for Fork + Sigil integration.
//!
//! Provides the `ProtocolHandler` trait and dynamic `ProtocolRegistry` so
//! autonomous Sigil agents can scaffold and register new protocols (HTTP, DNS,
//! Finger, Gopher, Gemini, FTP, News) on demand.

mod dns;
mod finger;
mod ftp;
mod gemini;
mod gopher;
mod html;
mod http;
mod net;
mod news;

pub use html::{extract_links, extract_title, normalize_extraction};

use anyhow::{bail, Result};
use std::collections::HashMap;
use std::sync::Arc;
use url::Url;

/// Represents the raw and extracted payload returned by a protocol handler.
#[derive(Debug, Clone, Default)]
pub struct ProtocolPayload {
    pub raw_bytes: Vec<u8>,
    pub content_type: Option<String>,
    pub extracted_text: String,
    pub status: u16,
    pub title: Option<String>,
    pub links: Vec<String>,
    pub final_url: Option<String>,
}

/// Core trait implemented by self-forged protocol handlers.
#[async_trait::async_trait]
pub trait ProtocolHandler: Send + Sync {
    /// The URL scheme handled by this implementation (e.g. "http", "finger", "dns").
    fn scheme(&self) -> &'static str;

    /// Fetch raw network payload over the protocol.
    async fn fetch(&self, url: &Url) -> Result<ProtocolPayload>;
}

/// Registry managing active protocol handlers across a Fork node.
#[derive(Default, Clone)]
pub struct ProtocolRegistry {
    handlers: HashMap<String, Arc<dyn ProtocolHandler>>,
}

impl ProtocolRegistry {
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    /// Built-in handlers shipped with Fork (http(s), dns, finger, gopher, gemini, ftp, news/nntp).
    pub fn builtins() -> Self {
        let mut registry = Self::new();
        if let Ok(client) = http::HttpHandler::shared_client() {
            registry.register(http::HttpHandler::new("http", client.clone()));
            registry.register(http::HttpHandler::new("https", client));
        }
        registry.register(dns::DnsHandler);
        registry.register(finger::FingerHandler);
        registry.register(gopher::GopherHandler);
        registry.register(gemini::GeminiHandler::new());
        registry.register(ftp::FtpHandler);
        registry.register(news::NewsHandler::news());
        registry.register(news::NewsHandler::nntp());
        registry
    }

    /// Register a new protocol handler module (e.g. self-forged by a Sigil agent).
    pub fn register(&mut self, handler: impl ProtocolHandler + 'static) {
        let scheme = handler.scheme().to_lowercase();
        self.handlers.insert(scheme, Arc::new(handler));
    }

    /// Retrieve handler for a given URL scheme.
    pub fn get(&self, scheme: &str) -> Option<Arc<dyn ProtocolHandler>> {
        self.handlers.get(&scheme.to_lowercase()).cloned()
    }

    /// Sorted list of registered schemes.
    pub fn schemes(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.handlers.keys().cloned().collect();
        keys.sort();
        keys
    }

    /// Dispatch a fetch request to the appropriate protocol handler.
    pub async fn fetch(&self, url_str: &str) -> Result<ProtocolPayload> {
        let parsed = Url::parse(url_str)?;
        let scheme = parsed.scheme();
        if let Some(handler) = self.get(scheme) {
            handler.fetch(&parsed).await
        } else {
            bail!(
                "Unhandled scheme '{}'. Sigil agent trigger required for auto-forging module.",
                scheme
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockFingerHandler;

    #[async_trait::async_trait]
    impl ProtocolHandler for MockFingerHandler {
        fn scheme(&self) -> &'static str {
            "finger"
        }

        async fn fetch(&self, _url: &Url) -> Result<ProtocolPayload> {
            Ok(ProtocolPayload {
                raw_bytes: b"Finger plan text".to_vec(),
                content_type: Some("text/plain".into()),
                extracted_text: "Finger plan text".into(),
                status: 200,
                title: Some("finger mock".into()),
                links: vec![],
                final_url: None,
            })
        }
    }

    #[tokio::test]
    async fn test_protocol_registry_dispatch() {
        let mut registry = ProtocolRegistry::new();
        registry.register(MockFingerHandler);

        assert!(registry.get("finger").is_some());
        assert!(registry.get("http").is_none());

        let payload = registry.fetch("finger://user@example.com").await.unwrap();
        assert_eq!(payload.extracted_text, "Finger plan text");
    }

    #[test]
    fn builtins_register_documented_schemes() {
        let registry = ProtocolRegistry::builtins();
        for scheme in ["http", "https", "dns", "finger", "gopher", "gemini", "ftp", "news", "nntp"] {
            assert!(
                registry.get(scheme).is_some(),
                "missing builtin handler for {scheme}"
            );
        }
        assert!(registry.get("irc").is_none());
    }

    #[tokio::test]
    async fn unhandled_scheme_mentions_sigil() {
        let registry = ProtocolRegistry::new();
        let err = registry.fetch("irc://example.com/#chan").await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Unhandled scheme"));
        assert!(msg.contains("Sigil"));
    }
}
