//! Minimal FTP handler (RFC 959) — LIST or RETR over PASV/EPSV.
//!
//! URL forms:
//!   ftp://host/path/file.txt
//!   ftp://user:pass@host/dir/
//! Trailing slash or empty path → LIST; otherwise RETR.

use super::net::{self, MAX_BODY};
use super::{ProtocolHandler, ProtocolPayload};
use anyhow::{bail, Context, Result};
use tokio::net::TcpStream;
use url::Url;

pub struct FtpHandler;

#[async_trait::async_trait]
impl ProtocolHandler for FtpHandler {
    fn scheme(&self) -> &'static str {
        "ftp"
    }

    async fn fetch(&self, url: &Url) -> Result<ProtocolPayload> {
        let host = net::url_host(url)?;
        let port = net::url_port(url, 21);
        let user = if url.username().is_empty() {
            "anonymous"
        } else {
            url.username()
        };
        let pass = url
            .password()
            .map(|p| percent_decode(p))
            .unwrap_or_else(|| "fork@preservation.local".into());
        let path = url.path();
        let list = path.is_empty() || path.ends_with('/');

        let mut ctrl = net::tcp_connect(host, port).await?;
        let (code, banner) = net::read_coded_reply(&mut ctrl).await?;
        if !(200..300).contains(&code) {
            bail!("FTP handshake failed: {banner}");
        }

        write_cmd(&mut ctrl, &format!("USER {user}")).await?;
        let (code, _) = net::read_coded_reply(&mut ctrl).await?;
        if code == 331 {
            write_cmd(&mut ctrl, &format!("PASS {pass}")).await?;
            let (code, text) = net::read_coded_reply(&mut ctrl).await?;
            if !(200..300).contains(&code) {
                bail!("FTP login failed: {text}");
            }
        } else if !(200..300).contains(&code) {
            bail!("FTP USER rejected ({code})");
        }

        write_cmd(&mut ctrl, "TYPE I").await?;
        let _ = net::read_coded_reply(&mut ctrl).await?;

        let mut data = open_data_socket(&mut ctrl, host).await?;
        let cmd = if list {
            if path.is_empty() || path == "/" {
                "LIST".to_string()
            } else {
                format!("LIST {path}")
            }
        } else {
            format!("RETR {path}")
        };
        write_cmd(&mut ctrl, &cmd).await?;
        let (code, text) = net::read_coded_reply(&mut ctrl).await?;
        if !(100..200).contains(&code) && !(200..300).contains(&code) {
            bail!("FTP {cmd} failed: {text}");
        }

        let raw_bytes = net::read_all_eof(&mut data, MAX_BODY).await?;
        drop(data);
        // 226 Transfer complete (best-effort)
        let _ = net::read_coded_reply(&mut ctrl).await;
        let _ = write_cmd(&mut ctrl, "QUIT").await;

        let extracted_text = String::from_utf8_lossy(&raw_bytes).into_owned();
        Ok(ProtocolPayload {
            raw_bytes,
            content_type: Some(if list {
                "text/plain".into()
            } else {
                "application/octet-stream".into()
            }),
            extracted_text,
            status: 200,
            title: Some(format!("ftp {host}{path}")),
            links: vec![],
            final_url: Some(url.to_string()),
        })
    }
}

async fn write_cmd(stream: &mut TcpStream, cmd: &str) -> Result<()> {
    net::write_all(stream, format!("{cmd}\r\n").as_bytes()).await
}

fn percent_decode(s: &str) -> String {
    let mut out = String::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16) {
                out.push(v as char);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

async fn open_data_socket(ctrl: &mut TcpStream, host: &str) -> Result<TcpStream> {
    write_cmd(ctrl, "EPSV").await?;
    let (code, text) = net::read_coded_reply(ctrl).await?;
    if (200..300).contains(&code) {
        if let Some(port) = parse_epsv(&text) {
            return net::tcp_connect(host, port).await;
        }
    }
    write_cmd(ctrl, "PASV").await?;
    let (code, text) = net::read_coded_reply(ctrl).await?;
    if !(200..300).contains(&code) {
        bail!("FTP PASV failed: {text}");
    }
    let (h, p) = parse_pasv(&text).context("parse PASV reply")?;
    net::tcp_connect(&h, p).await
}

pub(crate) fn parse_epsv(text: &str) -> Option<u16> {
    let start = text.find("(")?;
    let end = text[start..].find(')')?;
    let inner = &text[start + 1..start + end];
    let port = inner.trim_matches('|').trim();
    port.parse().ok()
}

pub(crate) fn parse_pasv(text: &str) -> Option<(String, u16)> {
    let start = text.find('(')?;
    let end = text[start..].find(')')?;
    let inner = &text[start + 1..start + end];
    let parts: Vec<&str> = inner.split(',').map(|s| s.trim()).collect();
    if parts.len() < 6 {
        return None;
    }
    let host = format!("{}.{}.{}.{}", parts[0], parts[1], parts[2], parts[3]);
    let p1: u16 = parts[4].parse().ok()?;
    let p2: u16 = parts[5].parse().ok()?;
    Some((host, p1 * 256 + p2))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn parses_pasv_and_epsv() {
        let (h, p) = parse_pasv("227 Entering Passive Mode (127,0,0,1,20,80)").unwrap();
        assert_eq!(h, "127.0.0.1");
        assert_eq!(p, 20 * 256 + 80);
        assert_eq!(parse_epsv("229 Entering Extended Passive Mode (|||12345|)"), Some(12345));
    }

    #[tokio::test]
    async fn lists_via_local_ftp_mock() {
        let control = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let data = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let ctrl_addr = control.local_addr().unwrap();
        let data_addr = data.local_addr().unwrap();
        let p1 = (data_addr.port() / 256) as u8;
        let p2 = (data_addr.port() % 256) as u8;

        tokio::spawn(async move {
            let (mut sock, _) = control.accept().await.unwrap();
            sock.write_all(b"220 mock ftp\r\n").await.unwrap();
            let mut buf = [0u8; 256];
            loop {
                let n = sock.read(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                let cmd = String::from_utf8_lossy(&buf[..n]).to_ascii_uppercase();
                if cmd.starts_with("USER") {
                    sock.write_all(b"331 need pass\r\n").await.unwrap();
                } else if cmd.starts_with("PASS") {
                    sock.write_all(b"230 logged in\r\n").await.unwrap();
                } else if cmd.starts_with("TYPE") {
                    sock.write_all(b"200 ok\r\n").await.unwrap();
                } else if cmd.starts_with("EPSV") {
                    sock.write_all(b"500 no epsv\r\n").await.unwrap();
                } else if cmd.starts_with("PASV") {
                    let reply = format!("227 Entering Passive Mode (127,0,0,1,{p1},{p2})\r\n");
                    sock.write_all(reply.as_bytes()).await.unwrap();
                } else if cmd.starts_with("LIST") {
                    sock.write_all(b"150 opening\r\n").await.unwrap();
                    let (mut ds, _) = data.accept().await.unwrap();
                    ds.write_all(b"-rw-r--r-- 1 ftp ftp 12 Jan 01 file.txt\r\n")
                        .await
                        .unwrap();
                    drop(ds);
                    sock.write_all(b"226 done\r\n").await.unwrap();
                } else if cmd.starts_with("QUIT") {
                    sock.write_all(b"221 bye\r\n").await.unwrap();
                    break;
                } else {
                    sock.write_all(b"200 ok\r\n").await.unwrap();
                }
            }
        });

        let url = Url::parse(&format!("ftp://127.0.0.1:{}/dir/", ctrl_addr.port())).unwrap();
        let payload = FtpHandler.fetch(&url).await.unwrap();
        assert!(payload.extracted_text.contains("file.txt"));
        assert_eq!(payload.status, 200);
    }
}
