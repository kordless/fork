//! DNS handler — UDP port 53, with TXT and CHAOS-class queries.
//!
//! URL forms:
//!   dns://example.com                  (IN TXT by default)
//!   dns://example.com/A
//!   dns://example.com/TXT
//!   dns://example.com?type=MX&server=1.1.1.1
//!   dns://version.bind?class=CH        (CHAOS TXT — BIND version)
//!   dns://hostname.bind/TXT/CH
//!   dns://id.server/CHAOS

use super::net::IO_TIMEOUT;
use super::{ProtocolHandler, ProtocolPayload};
use anyhow::{bail, Context, Result};
use std::net::SocketAddr;
use tokio::net::UdpSocket;
use tokio::time::timeout;
use url::Url;

const TYPE_A: u16 = 1;
const TYPE_NS: u16 = 2;
const TYPE_CNAME: u16 = 5;
const TYPE_SOA: u16 = 6;
const TYPE_PTR: u16 = 12;
const TYPE_MX: u16 = 15;
const TYPE_TXT: u16 = 16;
const TYPE_AAAA: u16 = 28;
const TYPE_ANY: u16 = 255;
const CLASS_IN: u16 = 1;
const CLASS_CH: u16 = 3;
const CLASS_HS: u16 = 4;
const DEFAULT_SERVERS: &[&str] = &["1.1.1.1:53", "8.8.8.8:53"];

#[derive(Default)]
pub struct DnsHandler;

#[async_trait::async_trait]
impl ProtocolHandler for DnsHandler {
    fn scheme(&self) -> &'static str {
        "dns"
    }

    async fn fetch(&self, url: &Url) -> Result<ProtocolPayload> {
        let name = url.host_str().context("dns URL needs a hostname to query")?;
        let (qtype, qclass) = query_type_class(url)?;
        let servers = resolver_addrs(url)?;
        let id = 0x464B; // 'FK'
        let query = encode_query(id, name, qtype, qclass)?;

        let mut last_err = None;
        for server in &servers {
            match query_server(*server, &query).await {
                Ok(raw) => {
                    let text = format_message(name, qtype, qclass, &raw)?;
                    return Ok(ProtocolPayload {
                        raw_bytes: raw,
                        content_type: Some("application/dns-message".into()),
                        extracted_text: text,
                        status: 200,
                        title: Some(format!("{name} {} {}", class_name(qclass), type_name(qtype))),
                        links: vec![],
                        final_url: Some(url.to_string()),
                    });
                }
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("no DNS resolvers configured")))
    }
}

fn query_type_class(url: &Url) -> Result<(u16, u16)> {
    let mut qtype = TYPE_TXT;
    let mut qclass = CLASS_IN;

    let path = url.path().trim_matches('/');
    if !path.is_empty() {
        for segment in path.split('/') {
            if segment.is_empty() {
                continue;
            }
            if let Ok(c) = parse_qclass(segment) {
                qclass = c;
                continue;
            }
            qtype = parse_qtype(segment)?;
        }
    }

    for (k, v) in url.query_pairs() {
        match k.as_ref() {
            "type" => qtype = parse_qtype(&v)?,
            "class" => qclass = parse_qclass(&v)?,
            _ => {}
        }
    }
    Ok((qtype, qclass))
}

fn parse_qtype(s: &str) -> Result<u16> {
    Ok(match s.to_ascii_uppercase().as_str() {
        "A" => TYPE_A,
        "NS" => TYPE_NS,
        "CNAME" => TYPE_CNAME,
        "SOA" => TYPE_SOA,
        "PTR" => TYPE_PTR,
        "MX" => TYPE_MX,
        "TXT" => TYPE_TXT,
        "AAAA" => TYPE_AAAA,
        "ANY" | "*" => TYPE_ANY,
        other => {
            if let Ok(n) = other.parse::<u16>() {
                n
            } else {
                bail!("unknown DNS type {other}");
            }
        }
    })
}

fn parse_qclass(s: &str) -> Result<u16> {
    match s.to_ascii_uppercase().as_str() {
        "IN" | "INTERNET" => Ok(CLASS_IN),
        "CH" | "CHAOS" => Ok(CLASS_CH),
        "HS" | "HESIOD" => Ok(CLASS_HS),
        other => {
            if let Ok(n) = other.parse::<u16>() {
                Ok(n)
            } else {
                bail!("unknown DNS class {other}")
            }
        }
    }
}

fn type_name(t: u16) -> &'static str {
    match t {
        TYPE_A => "A",
        TYPE_NS => "NS",
        TYPE_CNAME => "CNAME",
        TYPE_SOA => "SOA",
        TYPE_PTR => "PTR",
        TYPE_MX => "MX",
        TYPE_TXT => "TXT",
        TYPE_AAAA => "AAAA",
        TYPE_ANY => "ANY",
        _ => "TYPE",
    }
}

fn class_name(c: u16) -> &'static str {
    match c {
        CLASS_IN => "IN",
        CLASS_CH => "CH",
        CLASS_HS => "HS",
        _ => "CLASS",
    }
}

fn resolver_addrs(url: &Url) -> Result<Vec<SocketAddr>> {
    if let Some((_, v)) = url.query_pairs().find(|(k, _)| k == "server") {
        let with_port = if v.contains(':') {
            v.to_string()
        } else {
            format!("{v}:53")
        };
        return Ok(vec![with_port
            .parse()
            .with_context(|| format!("bad DNS server {with_port}"))?]);
    }
    DEFAULT_SERVERS
        .iter()
        .map(|s| s.parse().context("builtin resolver"))
        .collect()
}

pub(crate) fn encode_qname(name: &str) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let trimmed = name.trim_end_matches('.');
    if trimmed.is_empty() {
        out.push(0);
        return Ok(out);
    }
    for label in trimmed.split('.') {
        if label.is_empty() {
            bail!("empty DNS label in {name}");
        }
        if label.len() > 63 {
            bail!("DNS label too long");
        }
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
    Ok(out)
}

pub(crate) fn encode_query(id: u16, name: &str, qtype: u16, qclass: u16) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    buf.extend_from_slice(&id.to_be_bytes());
    buf.extend_from_slice(&0x0100u16.to_be_bytes()); // recursion desired
    buf.extend_from_slice(&1u16.to_be_bytes());
    buf.extend_from_slice(&0u16.to_be_bytes());
    buf.extend_from_slice(&0u16.to_be_bytes());
    buf.extend_from_slice(&0u16.to_be_bytes());
    buf.extend(encode_qname(name)?);
    buf.extend_from_slice(&qtype.to_be_bytes());
    buf.extend_from_slice(&qclass.to_be_bytes());
    Ok(buf)
}

async fn query_server(server: SocketAddr, query: &[u8]) -> Result<Vec<u8>> {
    let sock = UdpSocket::bind("0.0.0.0:0").await.context("bind UDP")?;
    timeout(IO_TIMEOUT, sock.send_to(query, server))
        .await
        .context("DNS send timeout")?
        .context("DNS send")?;
    let mut buf = vec![0u8; 4096];
    let (n, _) = timeout(IO_TIMEOUT, sock.recv_from(&mut buf))
        .await
        .context("DNS recv timeout")?
        .context("DNS recv")?;
    buf.truncate(n);
    if n < 12 {
        bail!("DNS response too short");
    }
    Ok(buf)
}

fn read_u16(msg: &[u8], i: &mut usize) -> Result<u16> {
    if *i + 2 > msg.len() {
        bail!("truncated DNS message");
    }
    let v = u16::from_be_bytes([msg[*i], msg[*i + 1]]);
    *i += 2;
    Ok(v)
}

fn read_u32(msg: &[u8], i: &mut usize) -> Result<u32> {
    if *i + 4 > msg.len() {
        bail!("truncated DNS message");
    }
    let v = u32::from_be_bytes([msg[*i], msg[*i + 1], msg[*i + 2], msg[*i + 3]]);
    *i += 4;
    Ok(v)
}

fn decode_name(msg: &[u8], i: &mut usize) -> Result<String> {
    let mut labels = Vec::new();
    let mut jumped = false;
    let mut cursor = *i;
    let mut hops = 0usize;
    loop {
        if hops > 20 {
            bail!("DNS name pointer loop");
        }
        if cursor >= msg.len() {
            bail!("truncated QNAME");
        }
        let len = msg[cursor];
        if len == 0 {
            cursor += 1;
            break;
        }
        if len & 0xC0 == 0xC0 {
            if cursor + 1 >= msg.len() {
                bail!("truncated compression pointer");
            }
            let ptr = (((len as usize) & 0x3F) << 8) | msg[cursor + 1] as usize;
            if !jumped {
                *i = cursor + 2;
                jumped = true;
            }
            cursor = ptr;
            hops += 1;
            continue;
        }
        if len & 0xC0 != 0 {
            bail!("unsupported name encoding");
        }
        cursor += 1;
        let end = cursor + len as usize;
        if end > msg.len() {
            bail!("truncated label");
        }
        labels.push(String::from_utf8_lossy(&msg[cursor..end]).into_owned());
        cursor = end;
    }
    if !jumped {
        *i = cursor;
    }
    Ok(labels.join("."))
}

fn format_rdata(qtype: u16, msg: &[u8], rdata: &[u8], rdata_off: usize) -> String {
    match qtype {
        TYPE_A if rdata.len() == 4 => {
            format!("{}.{}.{}.{}", rdata[0], rdata[1], rdata[2], rdata[3])
        }
        TYPE_AAAA if rdata.len() == 16 => {
            let mut segs = Vec::new();
            for c in rdata.chunks(2) {
                segs.push(format!("{:x}", u16::from_be_bytes([c[0], c[1]])));
            }
            segs.join(":")
        }
        TYPE_NS | TYPE_CNAME | TYPE_PTR => {
            let mut i = rdata_off;
            decode_name(msg, &mut i).unwrap_or_else(|_| String::from_utf8_lossy(rdata).into_owned())
        }
        TYPE_MX if rdata.len() >= 2 => {
            let pref = u16::from_be_bytes([rdata[0], rdata[1]]);
            let mut i = rdata_off + 2;
            let name = decode_name(msg, &mut i).unwrap_or_default();
            format!("{pref} {name}")
        }
        TYPE_TXT => decode_txt(rdata),
        TYPE_SOA => {
            let mut i = rdata_off;
            let mname = decode_name(msg, &mut i).unwrap_or_default();
            let rname = decode_name(msg, &mut i).unwrap_or_default();
            format!("{mname} {rname}")
        }
        _ => hex_bytes(rdata),
    }
}

fn decode_txt(rdata: &[u8]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < rdata.len() {
        let n = rdata[i] as usize;
        i += 1;
        if i + n > rdata.len() {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&String::from_utf8_lossy(&rdata[i..i + n]));
        i += n;
    }
    format!("\"{out}\"")
}

fn hex_bytes(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn format_message(qname: &str, qtype: u16, qclass: u16, msg: &[u8]) -> Result<String> {
    if msg.len() < 12 {
        bail!("short DNS message");
    }
    let flags = u16::from_be_bytes([msg[2], msg[3]]);
    let rcode = flags & 0xF;
    let qd = u16::from_be_bytes([msg[4], msg[5]]);
    let an = u16::from_be_bytes([msg[6], msg[7]]);
    let ns = u16::from_be_bytes([msg[8], msg[9]]);
    let ar = u16::from_be_bytes([msg[10], msg[11]]);
    let mut i = 12usize;
    let mut out = String::new();
    out.push_str(&format!(
        ";; QUESTION {qname} {} {}\n;; rcode={rcode} answers={an} authority={ns} additional={ar}\n",
        class_name(qclass),
        type_name(qtype)
    ));
    for _ in 0..qd {
        let _ = decode_name(msg, &mut i)?;
        i = i.saturating_add(4);
    }
    let sections = [("ANSWER", an), ("AUTHORITY", ns), ("ADDITIONAL", ar)];
    for (label, count) in sections {
        if count == 0 {
            continue;
        }
        out.push_str(&format!(";; {label}\n"));
        for _ in 0..count {
            let name = decode_name(msg, &mut i)?;
            let typ = read_u16(msg, &mut i)?;
            let class = read_u16(msg, &mut i)?;
            let ttl = read_u32(msg, &mut i)?;
            let rdlen = read_u16(msg, &mut i)? as usize;
            if i + rdlen > msg.len() {
                bail!("truncated rdata");
            }
            let rendered = format_rdata(typ, msg, &msg[i..i + rdlen], i);
            i += rdlen;
            out.push_str(&format!(
                "{name}. {ttl} {} {} {rendered}\n",
                class_name(class),
                type_name(typ)
            ));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::UdpSocket;

    #[test]
    fn encodes_qname_and_in_txt_query() {
        let q = encode_query(0x1234, "example.com", TYPE_TXT, CLASS_IN).unwrap();
        assert_eq!(&q[0..2], &[0x12, 0x34]);
        assert_eq!(q[12], 7);
        assert_eq!(&q[13..20], b"example");
        let end = q.len();
        assert_eq!(&q[end - 4..], &[0, 16, 0, 1]); // TXT IN
    }

    #[test]
    fn encodes_chaos_txt_query() {
        let q = encode_query(0x464B, "version.bind", TYPE_TXT, CLASS_CH).unwrap();
        assert_eq!(&q[end_qtype_class(&q)], &[0, 16, 0, 3]); // TXT CH
    }

    fn end_qtype_class(q: &[u8]) -> std::ops::RangeFrom<usize> {
        (q.len() - 4)..
    }

    #[test]
    fn parses_type_and_class_from_path_and_query() {
        let u = Url::parse("dns://example.com").unwrap();
        assert_eq!(query_type_class(&u).unwrap(), (TYPE_TXT, CLASS_IN));

        let u = Url::parse("dns://example.com/MX").unwrap();
        assert_eq!(query_type_class(&u).unwrap(), (TYPE_MX, CLASS_IN));

        let u = Url::parse("dns://example.com?type=aaaa").unwrap();
        assert_eq!(query_type_class(&u).unwrap(), (TYPE_AAAA, CLASS_IN));

        let u = Url::parse("dns://version.bind?class=CH").unwrap();
        assert_eq!(query_type_class(&u).unwrap(), (TYPE_TXT, CLASS_CH));

        let u = Url::parse("dns://hostname.bind/TXT/CHAOS").unwrap();
        assert_eq!(query_type_class(&u).unwrap(), (TYPE_TXT, CLASS_CH));

        let u = Url::parse("dns://id.server/CHAOS").unwrap();
        assert_eq!(query_type_class(&u).unwrap(), (TYPE_TXT, CLASS_CH));
    }

    #[test]
    fn decodes_compressed_name() {
        let mut msg = vec![0u8; 12];
        msg.extend_from_slice(&[7]);
        msg.extend_from_slice(b"example");
        msg.extend_from_slice(&[3]);
        msg.extend_from_slice(b"com");
        msg.push(0);
        let ptr_at = msg.len();
        msg.extend_from_slice(&[0xC0, 12]);
        let mut i = 12;
        assert_eq!(decode_name(&msg, &mut i).unwrap(), "example.com");
        let mut j = ptr_at;
        assert_eq!(decode_name(&msg, &mut j).unwrap(), "example.com");
        assert_eq!(j, ptr_at + 2);
    }

    #[tokio::test]
    async fn fetches_txt_from_local_udp_server() {
        let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = sock.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buf = [0u8; 512];
            let (n, peer) = sock.recv_from(&mut buf).await.unwrap();
            let mut resp = buf[..n].to_vec();
            resp[2] = 0x81;
            resp[3] = 0x80;
            resp[6] = 0;
            resp[7] = 1;
            // TXT "fork"  (rdlen=6, len-prefixed)
            resp.extend_from_slice(&[0xC0, 12, 0, 16, 0, 1, 0, 0, 0, 60, 0, 6, 5]);
            resp.extend_from_slice(b"fork!");
            let _ = sock.send_to(&resp, peer).await;
        });

        let url = Url::parse(&format!("dns://example.com/TXT?server={addr}")).unwrap();
        let payload = DnsHandler.fetch(&url).await.unwrap();
        assert_eq!(payload.status, 200);
        assert!(payload.extracted_text.contains("fork!"));
        assert!(payload.extracted_text.contains("TXT"));
    }

    #[tokio::test]
    async fn fetches_chaos_txt_from_local_udp_server() {
        let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = sock.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buf = [0u8; 512];
            let (n, peer) = sock.recv_from(&mut buf).await.unwrap();
            // last 2 bytes of question should be class CH (3)
            assert_eq!(&buf[n - 2..n], &[0, 3]);
            let mut resp = buf[..n].to_vec();
            resp[2] = 0x81;
            resp[3] = 0x80;
            resp[6] = 0;
            resp[7] = 1;
            // TXT CH "9.18.0"
            let txt = b"9.18.0";
            resp.extend_from_slice(&[0xC0, 12, 0, 16, 0, 3, 0, 0, 0, 0, 0, (txt.len() + 1) as u8, txt.len() as u8]);
            resp.extend_from_slice(txt);
            let _ = sock.send_to(&resp, peer).await;
        });

        let url = Url::parse(&format!("dns://version.bind?class=CH&server={addr}")).unwrap();
        let payload = DnsHandler.fetch(&url).await.unwrap();
        assert_eq!(payload.status, 200);
        assert!(payload.extracted_text.contains("9.18.0"));
        assert!(payload.extracted_text.contains("CH"));
        assert!(payload.title.as_deref().unwrap().contains("CH"));
    }
}
