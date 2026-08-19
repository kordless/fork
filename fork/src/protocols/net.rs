//! Shared TCP/UDP helpers for protocol handlers.

use anyhow::{bail, Context, Result};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

pub const IO_TIMEOUT: Duration = Duration::from_secs(20);
pub const MAX_BODY: usize = 8 * 1024 * 1024;

pub fn url_host(url: &url::Url) -> Result<&str> {
    url.host_str().context("URL is missing a host")
}

pub fn url_port(url: &url::Url, default: u16) -> u16 {
    url.port().unwrap_or(default)
}

pub async fn tcp_connect(host: &str, port: u16) -> Result<TcpStream> {
    timeout(IO_TIMEOUT, TcpStream::connect((host, port)))
        .await
        .with_context(|| format!("timeout connecting to {host}:{port}"))?
        .with_context(|| format!("connect {host}:{port}"))
}

pub async fn write_all(stream: &mut TcpStream, data: &[u8]) -> Result<()> {
    timeout(IO_TIMEOUT, stream.write_all(data))
        .await
        .context("write timeout")?
        .context("write failed")?;
    Ok(())
}

pub async fn read_all_eof(stream: &mut TcpStream, max: usize) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        let n = timeout(IO_TIMEOUT, stream.read(&mut tmp))
            .await
            .context("read timeout")?
            .context("read failed")?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.len() > max {
            bail!("response exceeds {max} bytes");
        }
    }
    Ok(buf)
}

pub async fn read_line(stream: &mut TcpStream, max: usize) -> Result<String> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = timeout(IO_TIMEOUT, stream.read(&mut byte))
            .await
            .context("read timeout")?
            .context("read failed")?;
        if n == 0 {
            break;
        }
        buf.push(byte[0]);
        if buf.len() >= 2 && buf.ends_with(b"\r\n") {
            break;
        }
        if buf.ends_with(b"\n") {
            break;
        }
        if buf.len() > max {
            bail!("line exceeds {max} bytes");
        }
    }
    Ok(String::from_utf8_lossy(&buf).to_string())
}

/// Read an FTP or NNTP reply, including multi-line `NNN-` / `NNN ` forms.
pub async fn read_coded_reply(stream: &mut TcpStream) -> Result<(u16, String)> {
    let first = read_line(stream, 4096).await?;
    let code: u16 = first
        .get(..3)
        .and_then(|s| s.trim().parse().ok())
        .with_context(|| format!("invalid control reply: {first:?}"))?;
    let mut text = first.clone();
    if first.as_bytes().get(3) == Some(&b'-') {
        let prefix = format!("{code}");
        loop {
            let line = read_line(stream, 4096).await?;
            text.push_str(&line);
            if line.starts_with(&prefix) && line.as_bytes().get(3) == Some(&b' ') {
                break;
            }
        }
    }
    Ok((code, text))
}

/// NNTP multi-line body terminated by a lone `.` line. Handles dot-stuffing.
pub async fn read_dot_body(stream: &mut TcpStream, max: usize) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let line = read_line(stream, 8192).await?;
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed == "." {
            break;
        }
        let payload = if let Some(rest) = trimmed.strip_prefix("..") {
            rest
        } else {
            trimmed
        };
        out.extend_from_slice(payload.as_bytes());
        out.extend_from_slice(b"\n");
        if out.len() > max {
            bail!("dot-body exceeds {max} bytes");
        }
    }
    Ok(out)
}
