//! Finger protocol (RFC 1288) — TCP port 79.
//!
//! URL forms:
//!   finger://hostname/username
//!   finger://username@hostname
//!   finger://hostname/          (host listing)

use super::net::{self, MAX_BODY};
use super::{ProtocolHandler, ProtocolPayload};
use anyhow::Result;
use url::Url;

pub struct FingerHandler;

#[async_trait::async_trait]
impl ProtocolHandler for FingerHandler {
    fn scheme(&self) -> &'static str {
        "finger"
    }

    async fn fetch(&self, url: &Url) -> Result<ProtocolPayload> {
        let host = net::url_host(url)?;
        let port = net::url_port(url, 79);
        let query = finger_query(url);
        let mut stream = net::tcp_connect(host, port).await?;
        net::write_all(&mut stream, format!("{query}\r\n").as_bytes()).await?;
        let raw_bytes = net::read_all_eof(&mut stream, MAX_BODY).await?;
        let extracted_text = String::from_utf8_lossy(&raw_bytes).into_owned();
        Ok(ProtocolPayload {
            raw_bytes,
            content_type: Some("text/plain".into()),
            extracted_text: extracted_text.clone(),
            status: 200,
            title: Some(if query.is_empty() {
                format!("finger {host}")
            } else {
                format!("finger {query}@{host}")
            }),
            links: vec![],
            final_url: Some(url.to_string()),
        })
    }
}

pub(crate) fn finger_query(url: &Url) -> String {
    if !url.username().is_empty() {
        return url.username().to_string();
    }
    url.path().trim_start_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn parses_user_from_path_and_userinfo() {
        let u = Url::parse("finger://example.com/alice").unwrap();
        assert_eq!(finger_query(&u), "alice");
        let u = Url::parse("finger://bob@example.com/").unwrap();
        assert_eq!(finger_query(&u), "bob");
        let u = Url::parse("finger://example.com/").unwrap();
        assert_eq!(finger_query(&u), "");
    }

    #[tokio::test]
    async fn fetches_plan_from_local_server() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 128];
            let n = sock.read(&mut buf).await.unwrap();
            assert!(std::str::from_utf8(&buf[..n]).unwrap().starts_with("alice"));
            sock.write_all(b"Login: alice\r\nPlan:\r\nhello from finger\r\n")
                .await
                .unwrap();
        });
        let url = Url::parse(&format!("finger://{addr}/alice")).unwrap();
        let payload = FingerHandler.fetch(&url).await.unwrap();
        assert!(payload.extracted_text.contains("hello from finger"));
        assert_eq!(payload.status, 200);
    }
}
