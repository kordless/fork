//! Gemini protocol (gemini://) — TLS on port 1965.
//!
//! Capsules commonly use self-signed certificates; this handler accepts the
//! peer cert for snapshot capture (TOFU storage is a later enhancement).

use super::net::{self, IO_TIMEOUT, MAX_BODY};
use super::{ProtocolHandler, ProtocolPayload};
use anyhow::{bail, Context, Result};
use std::sync::Arc;
use std::sync::OnceLock;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::timeout;
use tokio_rustls::rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use tokio_rustls::rustls::{ClientConfig, DigitallySignedStruct, Error as TlsError, SignatureScheme};
use tokio_rustls::TlsConnector;
use url::Url;

#[derive(Clone)]
pub struct GeminiHandler {
    connector: TlsConnector,
}

impl Default for GeminiHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl GeminiHandler {
    pub fn new() -> Self {
        Self {
            connector: gemini_connector().clone(),
        }
    }
}

fn gemini_connector() -> &'static TlsConnector {
    static CONNECTOR: OnceLock<TlsConnector> = OnceLock::new();
    CONNECTOR.get_or_init(|| {
        let provider = tokio_rustls::rustls::crypto::ring::default_provider();
        let config = ClientConfig::builder_with_provider(provider.into())
            .with_safe_default_protocol_versions()
            .expect("gemini TLS versions")
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAny))
            .with_no_client_auth();
        TlsConnector::from(Arc::new(config))
    })
}

#[derive(Debug)]
struct AcceptAny;

impl ServerCertVerifier for AcceptAny {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        tokio_rustls::rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[async_trait::async_trait]
impl ProtocolHandler for GeminiHandler {
    fn scheme(&self) -> &'static str {
        "gemini"
    }

    async fn fetch(&self, url: &Url) -> Result<ProtocolPayload> {
        let host = net::url_host(url)?;
        let port = net::url_port(url, 1965);
        let mut request_url = url.clone();
        request_url.set_fragment(None);
        let request = format!("{request_url}\r\n");
        if request.len() > 1026 {
            bail!("gemini request exceeds 1024 bytes");
        }

        let stream = net::tcp_connect(host, port).await?;
        let server_name = ServerName::try_from(host.to_string())
            .map_err(|_| anyhow::anyhow!("invalid SNI host {host}"))?;
        let mut tls = timeout(IO_TIMEOUT, self.connector.connect(server_name, stream))
            .await
            .context("gemini TLS timeout")?
            .context("gemini TLS handshake")?;

        timeout(IO_TIMEOUT, tls.write_all(request.as_bytes()))
            .await
            .context("gemini write timeout")?
            .context("gemini write")?;

        let mut raw = Vec::new();
        let mut tmp = [0u8; 4096];
        loop {
            let n = timeout(IO_TIMEOUT, tls.read(&mut tmp))
                .await
                .context("gemini read timeout")?
                .context("gemini read")?;
            if n == 0 {
                break;
            }
            raw.extend_from_slice(&tmp[..n]);
            if raw.len() > MAX_BODY {
                bail!("gemini response exceeds {MAX_BODY} bytes");
            }
        }

        let parsed = parse_gemini_response(&raw)?;
        Ok(ProtocolPayload {
            raw_bytes: raw,
            content_type: parsed.meta.filter(|_| parsed.status / 10 == 2),
            extracted_text: parsed.body_text,
            status: map_gemini_status(parsed.status),
            title: parsed.title,
            links: parsed.links,
            final_url: Some(url.to_string()),
        })
    }
}

pub(crate) struct GeminiParsed {
    status: u8,
    meta: Option<String>,
    body_text: String,
    title: Option<String>,
    links: Vec<String>,
}

pub(crate) fn parse_gemini_response(raw: &[u8]) -> Result<GeminiParsed> {
    let header_end = raw
        .windows(2)
        .position(|w| w == b"\r\n")
        .context("gemini response missing CRLF header terminator")?;
    let header = std::str::from_utf8(&raw[..header_end]).context("gemini header is not UTF-8")?;
    let mut parts = header.splitn(2, ' ');
    let status: u8 = parts
        .next()
        .and_then(|s| s.parse().ok())
        .context("gemini status code")?;
    let meta = parts.next().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let body = &raw[header_end + 2..];
    let body_text = String::from_utf8_lossy(body).into_owned();
    let mut links = Vec::new();
    let mut title = None;
    for line in body_text.lines() {
        if let Some(rest) = line.strip_prefix("=>") {
            let rest = rest.trim();
            let href = rest.split_whitespace().next().unwrap_or("");
            if !href.is_empty() {
                links.push(href.to_string());
            }
        } else if title.is_none() {
            if let Some(t) = line.strip_prefix("# ") {
                title = Some(t.trim().to_string());
            }
        }
    }
    Ok(GeminiParsed {
        status,
        meta,
        body_text,
        title,
        links,
    })
}

fn map_gemini_status(status: u8) -> u16 {
    match status / 10 {
        1 => 102,
        2 => 200,
        3 => 302,
        4 => 404,
        5 => 502,
        6 => 401,
        _ => status as u16,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_success_with_links() {
        let raw = b"20 text/gemini\r\n# Capsule\r\n=> /posts Posts\r\nhello\r\n";
        let p = parse_gemini_response(raw).unwrap();
        assert_eq!(p.status, 20);
        assert_eq!(p.meta.as_deref(), Some("text/gemini"));
        assert_eq!(p.title.as_deref(), Some("Capsule"));
        assert_eq!(p.links, vec!["/posts"]);
        assert!(p.body_text.contains("hello"));
        assert_eq!(map_gemini_status(20), 200);
        assert_eq!(map_gemini_status(51), 502);
    }

    #[test]
    fn rejects_headerless_body() {
        assert!(parse_gemini_response(b"no header").is_err());
    }
}
