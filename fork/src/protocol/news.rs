//! NNTP / Usenet handler — `news://` and `nntp://` (TCP 119).
//!
//! URL forms:
//!   news://host/comp.lang.rust
//!   nntp://host/comp.lang.rust/12345
//! Empty path → LIST.

use super::net::{self, MAX_BODY};
use super::{ProtocolHandler, ProtocolPayload};
use anyhow::{bail, Result};
use tokio::net::TcpStream;
use url::Url;

pub struct NewsHandler {
    scheme: &'static str,
}

impl NewsHandler {
    pub fn news() -> Self {
        Self { scheme: "news" }
    }

    pub fn nntp() -> Self {
        Self { scheme: "nntp" }
    }
}

#[async_trait::async_trait]
impl ProtocolHandler for NewsHandler {
    fn scheme(&self) -> &'static str {
        self.scheme
    }

    async fn fetch(&self, url: &Url) -> Result<ProtocolPayload> {
        let host = net::url_host(url)?;
        let port = net::url_port(url, 119);
        let mut stream = net::tcp_connect(host, port).await?;
        let (code, banner) = net::read_coded_reply(&mut stream).await?;
        if !(200..300).contains(&code) {
            bail!("NNTP handshake failed: {banner}");
        }

        let path = url.path().trim_matches('/');
        let (group, article) = split_group_article(path);

        let (status, raw_bytes, title) = if group.is_empty() {
            write_cmd(&mut stream, "LIST").await?;
            let (code, text) = net::read_coded_reply(&mut stream).await?;
            if code != 215 && !(200..300).contains(&code) {
                bail!("NNTP LIST failed: {text}");
            }
            let body = net::read_dot_body(&mut stream, MAX_BODY).await?;
            (200, body, Some(format!("nntp list {host}")))
        } else {
            write_cmd(&mut stream, &format!("GROUP {group}")).await?;
            let (code, text) = net::read_coded_reply(&mut stream).await?;
            if !(200..300).contains(&code) {
                bail!("NNTP GROUP {group} failed: {text}");
            }
            if let Some(art) = article {
                write_cmd(&mut stream, &format!("ARTICLE {art}")).await?;
                let (code, text) = net::read_coded_reply(&mut stream).await?;
                if code != 220 && !(200..300).contains(&code) {
                    bail!("NNTP ARTICLE failed: {text}");
                }
                let body = net::read_dot_body(&mut stream, MAX_BODY).await?;
                (200, body, Some(format!("{group}/{art}")))
            } else {
                write_cmd(&mut stream, "HEAD").await?;
                let (code, text) = net::read_coded_reply(&mut stream).await?;
                if code != 221 && !(200..300).contains(&code) {
                    // empty group is still a valid snapshot of the GROUP reply
                    let _ = write_cmd(&mut stream, "QUIT").await;
                    return Ok(ProtocolPayload {
                        raw_bytes: text.as_bytes().to_vec(),
                        content_type: Some("text/plain".into()),
                        extracted_text: text.clone(),
                        status: 200,
                        title: Some(group.to_string()),
                        links: vec![],
                        final_url: Some(url.to_string()),
                    });
                }
                let body = net::read_dot_body(&mut stream, MAX_BODY).await?;
                (200, body, Some(group.to_string()))
            }
        };

        let _ = write_cmd(&mut stream, "QUIT").await;
        let extracted_text = String::from_utf8_lossy(&raw_bytes).into_owned();
        Ok(ProtocolPayload {
            raw_bytes,
            content_type: Some("message/rfc822".into()),
            extracted_text,
            status,
            title,
            links: vec![],
            final_url: Some(url.to_string()),
        })
    }
}

async fn write_cmd(stream: &mut TcpStream, cmd: &str) -> Result<()> {
    net::write_all(stream, format!("{cmd}\r\n").as_bytes()).await
}

pub(crate) fn split_group_article(path: &str) -> (&str, Option<&str>) {
    if path.is_empty() {
        return ("", None);
    }
    // article ids are numeric last segments; groups contain dots
    if let Some((group, art)) = path.rsplit_once('/') {
        if !art.is_empty() && art.chars().all(|c| c.is_ascii_digit()) {
            return (group, Some(art));
        }
    }
    (path, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn splits_group_and_article() {
        assert_eq!(split_group_article("comp.lang.rust"), ("comp.lang.rust", None));
        assert_eq!(
            split_group_article("comp.lang.rust/42"),
            ("comp.lang.rust", Some("42"))
        );
        assert_eq!(split_group_article(""), ("", None));
    }

    #[tokio::test]
    async fn fetches_article_from_mock_nntp() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            sock.write_all(b"200 mock nntp\r\n").await.unwrap();
            let mut buf = [0u8; 256];
            loop {
                let n = sock.read(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                let cmd = String::from_utf8_lossy(&buf[..n]).to_ascii_uppercase();
                if cmd.starts_with("GROUP") {
                    sock.write_all(b"211 1 1 1 comp.lang.rust\r\n").await.unwrap();
                } else if cmd.starts_with("ARTICLE") {
                    sock.write_all(b"220 1 <id@ex> article\r\nSubject: hi\r\n\r\nbody line\r\n.\r\n")
                        .await
                        .unwrap();
                } else if cmd.starts_with("QUIT") {
                    sock.write_all(b"205 bye\r\n").await.unwrap();
                    break;
                } else {
                    sock.write_all(b"500 unknown\r\n").await.unwrap();
                }
            }
        });
        let url = Url::parse(&format!("news://127.0.0.1:{}/comp.lang.rust/1", addr.port())).unwrap();
        let payload = NewsHandler::news().fetch(&url).await.unwrap();
        assert!(payload.extracted_text.contains("body line"));
        assert!(payload.extracted_text.contains("Subject: hi"));
        assert_eq!(payload.status, 200);
    }
}
