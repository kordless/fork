//! Finger protocol (RFC 1288) — TCP port 79.
//!
//! Fetches the classic status files a finger daemon serves from the user's
//! home directory:
//!   `.project`  — one-line project description (`Project:`)
//!   `.plan`     — free-form plan text (`Plan:`)
//!
//! URL forms:
//!   finger://hostname/username
//!   finger://username@hostname
//!   finger://hostname/username/.plan
//!   finger://hostname/username/.project
//!   finger://hostname/username?verbose=1   (`/W` verbose query)
//!   finger://hostname/                     (host listing)

use super::net::{self, MAX_BODY};
use super::{ProtocolHandler, ProtocolPayload};
use anyhow::Result;
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FingerDoc {
    All,
    Plan,
    Project,
}

pub struct FingerHandler;

#[async_trait::async_trait]
impl ProtocolHandler for FingerHandler {
    fn scheme(&self) -> &'static str {
        "finger"
    }

    async fn fetch(&self, url: &Url) -> Result<ProtocolPayload> {
        let host = net::url_host(url)?;
        let port = net::url_port(url, 79);
        let (user, doc, verbose) = finger_target(url);
        let query = finger_wire_query(&user, verbose);

        let mut stream = net::tcp_connect(host, port).await?;
        net::write_all(&mut stream, format!("{query}\r\n").as_bytes()).await?;
        let raw_bytes = net::read_all_eof(&mut stream, MAX_BODY).await?;
        let raw_text = String::from_utf8_lossy(&raw_bytes).into_owned();
        let (project, plan) = parse_status_files(&raw_text);

        let extracted_text = match doc {
            FingerDoc::Plan => plan.clone().unwrap_or_else(|| raw_text.clone()),
            FingerDoc::Project => project.clone().unwrap_or_else(|| raw_text.clone()),
            FingerDoc::All => format_status_extraction(&user, host, &project, &plan, &raw_text),
        };

        let title = match doc {
            FingerDoc::Plan => Some(format!("{user}@{host} .plan")),
            FingerDoc::Project => Some(format!("{user}@{host} .project")),
            FingerDoc::All if user.is_empty() => Some(format!("finger {host}")),
            FingerDoc::All => Some(format!("finger {user}@{host}")),
        };

        Ok(ProtocolPayload {
            raw_bytes,
            content_type: Some("text/plain".into()),
            extracted_text,
            status: 200,
            title,
            links: vec![],
            final_url: Some(url.to_string()),
        })
    }
}

fn finger_target(url: &Url) -> (String, FingerDoc, bool) {
    let verbose = url.query_pairs().any(|(k, v)| {
        k == "verbose" && (v == "1" || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("w"))
    }) || url.path().split('/').any(|p| p.eq_ignore_ascii_case("W") || p == "/W");

    let mut user = if !url.username().is_empty() {
        url.username().to_string()
    } else {
        String::new()
    };

    let mut doc = FingerDoc::All;
    for segment in url.path().split('/').filter(|s| !s.is_empty()) {
        let lower = segment.to_ascii_lowercase();
        if lower == ".plan" || lower == "plan" {
            doc = FingerDoc::Plan;
        } else if lower == ".project" || lower == "project" {
            doc = FingerDoc::Project;
        } else if lower == "w" {
            continue;
        } else if user.is_empty() {
            user = segment.to_string();
        }
    }

    if let Some((_, v)) = url.query_pairs().find(|(k, _)| k == "file") {
        match v.to_ascii_lowercase().as_str() {
            "plan" | ".plan" => doc = FingerDoc::Plan,
            "project" | ".project" => doc = FingerDoc::Project,
            _ => {}
        }
    }

    (user, doc, verbose)
}

fn finger_wire_query(user: &str, verbose: bool) -> String {
    match (verbose, user.is_empty()) {
        (true, true) => "/W".into(),
        (true, false) => format!("/W {user}"),
        (false, _) => user.to_string(),
    }
}

/// Split RFC 1288-style output into `.project` (Project:) and `.plan` (Plan:).
pub(crate) fn parse_status_files(text: &str) -> (Option<String>, Option<String>) {
    let mut project = None;
    let mut plan = None;
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        let lower = trimmed.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("project:") {
            let _ = rest;
            let value = trimmed
                .split_once(':')
                .map(|(_, r)| r.trim().to_string())
                .filter(|s| !s.is_empty());
            project = value;
        } else if lower.starts_with("plan:") {
            let mut body = String::new();
            if let Some((_, rest)) = trimmed.split_once(':') {
                let rest = rest.trim_start_matches(['\r', '\n']);
                if !rest.trim().is_empty() {
                    body.push_str(rest.trim_start());
                    body.push('\n');
                }
            }
            for rest in lines.by_ref() {
                body.push_str(rest);
                body.push('\n');
            }
            let body = body.trim_end().to_string();
            if !body.is_empty() {
                plan = Some(body);
            }
            break;
        }
    }
    (project, plan)
}

fn format_status_extraction(
    user: &str,
    host: &str,
    project: &Option<String>,
    plan: &Option<String>,
    raw: &str,
) -> String {
    let mut out = String::new();
    if user.is_empty() {
        out.push_str(&format!("# finger {host}\n"));
    } else {
        out.push_str(&format!("# finger {user}@{host}\n"));
    }
    match project {
        Some(p) => out.push_str(&format!("## .project\n{p}\n")),
        None => out.push_str("## .project\n(none)\n"),
    }
    match plan {
        Some(p) => out.push_str(&format!("## .plan\n{p}\n")),
        None => out.push_str("## .plan\n(none)\n"),
    }
    out.push_str("## raw\n");
    out.push_str(raw);
    if !raw.ends_with('\n') {
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn parses_user_path_userinfo_and_status_files() {
        let u = Url::parse("finger://example.com/alice").unwrap();
        assert_eq!(finger_target(&u), ("alice".into(), FingerDoc::All, false));

        let u = Url::parse("finger://bob@example.com/").unwrap();
        assert_eq!(finger_target(&u), ("bob".into(), FingerDoc::All, false));

        let u = Url::parse("finger://example.com/carol/.plan").unwrap();
        assert_eq!(finger_target(&u), ("carol".into(), FingerDoc::Plan, false));

        let u = Url::parse("finger://example.com/carol/.project").unwrap();
        assert_eq!(finger_target(&u), ("carol".into(), FingerDoc::Project, false));

        let u = Url::parse("finger://example.com/dave?verbose=1&file=plan").unwrap();
        assert_eq!(finger_target(&u), ("dave".into(), FingerDoc::Plan, true));

        let u = Url::parse("finger://example.com/").unwrap();
        assert_eq!(finger_target(&u), ("".into(), FingerDoc::All, false));
    }

    #[test]
    fn splits_rfc1288_project_and_plan() {
        let raw = "\
Login: alice\n\
Directory: /home/alice\n\
Shell: /bin/zsh\n\
Project: preserve human entropy\n\
Plan:\n\
hello from .plan\n\
second line\n";
        let (project, plan) = parse_status_files(raw);
        assert_eq!(project.as_deref(), Some("preserve human entropy"));
        assert_eq!(plan.as_deref(), Some("hello from .plan\nsecond line"));
    }

    #[test]
    fn verbose_wire_query() {
        assert_eq!(finger_wire_query("alice", false), "alice");
        assert_eq!(finger_wire_query("alice", true), "/W alice");
        assert_eq!(finger_wire_query("", true), "/W");
    }

    #[tokio::test]
    async fn fetches_plan_and_project_from_local_server() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 128];
            let n = sock.read(&mut buf).await.unwrap();
            assert!(std::str::from_utf8(&buf[..n]).unwrap().starts_with("alice"));
            sock.write_all(
                b"Login: alice\r\nProject: fork snapshots\r\nPlan:\r\nhello from finger\r\n",
            )
            .await
            .unwrap();
        });
        let url = Url::parse(&format!("finger://{addr}/alice")).unwrap();
        let payload = FingerHandler.fetch(&url).await.unwrap();
        assert_eq!(payload.status, 200);
        assert!(payload.extracted_text.contains("## .project"));
        assert!(payload.extracted_text.contains("fork snapshots"));
        assert!(payload.extracted_text.contains("## .plan"));
        assert!(payload.extracted_text.contains("hello from finger"));
    }

    #[tokio::test]
    async fn fetches_plan_file_only() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 128];
            let _ = sock.read(&mut buf).await;
            sock.write_all(b"Login: alice\r\nProject: ignored\r\nPlan:\r\nonly the plan\r\n")
                .await
                .unwrap();
        });
        let url = Url::parse(&format!("finger://{addr}/alice/.plan")).unwrap();
        let payload = FingerHandler.fetch(&url).await.unwrap();
        assert_eq!(payload.extracted_text.trim(), "only the plan");
        assert_eq!(payload.title.as_deref().unwrap().contains(".plan"), true);
    }
}
