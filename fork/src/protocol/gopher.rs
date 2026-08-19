//! Gopher protocol (RFC 1436 / RFC 4266) — TCP port 70.
//!
//! URL form: gopher://host[:70]/<type><selector>
//! Item type is the first character of the path (default `1` directory).

use super::net::{self, MAX_BODY};
use super::{ProtocolHandler, ProtocolPayload};
use anyhow::Result;
use url::Url;

pub struct GopherHandler;

#[async_trait::async_trait]
impl ProtocolHandler for GopherHandler {
    fn scheme(&self) -> &'static str {
        "gopher"
    }

    async fn fetch(&self, url: &Url) -> Result<ProtocolPayload> {
        let host = net::url_host(url)?;
        let port = net::url_port(url, 70);
        let (item_type, mut selector) = gopher_selector(url);
        if item_type == '7' {
            if let Some(q) = url.query() {
                if !selector.is_empty() {
                    selector.push('\t');
                }
                selector.push_str(q);
            }
        }
        let mut stream = net::tcp_connect(host, port).await?;
        net::write_all(&mut stream, format!("{selector}\r\n").as_bytes()).await?;
        let raw_bytes = net::read_all_eof(&mut stream, MAX_BODY).await?;
        let content_type = match item_type {
            '0' | '1' | '7' | 'i' => Some("text/plain".into()),
            'g' => Some("image/gif".into()),
            'I' => Some("image/*".into()),
            '9' | '5' => Some("application/octet-stream".into()),
            _ => Some("text/plain".into()),
        };
        let extracted_text = if matches!(item_type, '0' | '1' | '7' | 'h' | 'i') || content_type.as_deref() == Some("text/plain") {
            String::from_utf8_lossy(&raw_bytes).into_owned()
        } else {
            String::new()
        };
        let links = if item_type == '1' || item_type == '7' {
            parse_gopher_menu(host, port, &extracted_text)
        } else {
            vec![]
        };
        Ok(ProtocolPayload {
            raw_bytes,
            content_type,
            extracted_text,
            status: 200,
            title: Some(format!("gopher {host}{selector}")),
            links,
            final_url: Some(url.to_string()),
        })
    }
}

pub(crate) fn gopher_selector(url: &Url) -> (char, String) {
    let rest = url.path().trim_start_matches('/');
    if rest.is_empty() {
        return ('1', String::new());
    }
    let mut chars = rest.chars();
    let item_type = chars.next().unwrap_or('1');
    let selector = chars.as_str().to_string();
    (item_type, selector)
}

fn parse_gopher_menu(host: &str, port: u16, menu: &str) -> Vec<String> {
    let mut links = Vec::new();
    for line in menu.lines() {
        if line.is_empty() || line.starts_with('.') {
            continue;
        }
        let itype = line.chars().next().unwrap_or('i');
        if itype == 'i' {
            continue;
        }
        let rest = &line[itype.len_utf8()..];
        let mut parts = rest.split('\t');
        let _name = parts.next();
        let selector = parts.next().unwrap_or("");
        let h = parts.next().unwrap_or(host);
        let p = parts
            .next()
            .and_then(|s| s.trim().parse::<u16>().ok())
            .unwrap_or(port);
        let scheme = if itype == 'h' && selector.starts_with("URL:") {
            links.push(selector.trim_start_matches("URL:").to_string());
            continue;
        } else {
            "gopher"
        };
        links.push(format!("{scheme}://{h}:{p}/{itype}{selector}"));
        if links.len() >= 200 {
            break;
        }
    }
    links
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn splits_type_and_selector() {
        assert_eq!(gopher_selector(&Url::parse("gopher://ex.com/").unwrap()), ('1', String::new()));
        assert_eq!(
            gopher_selector(&Url::parse("gopher://ex.com/0/docs/readme").unwrap()),
            ('0', "/docs/readme".into())
        );
    }

    #[tokio::test]
    async fn fetches_directory_listing() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 64];
            let _ = sock.read(&mut buf).await;
            sock.write_all(b"0About\t/about\tlocalhost\t70\r\n.\r\n")
                .await
                .unwrap();
        });
        let url = Url::parse(&format!("gopher://{addr}/1")).unwrap();
        let payload = GopherHandler.fetch(&url).await.unwrap();
        assert!(payload.extracted_text.contains("About"));
        assert!(payload.links.iter().any(|l| l.contains("/0/about")));
    }
}
