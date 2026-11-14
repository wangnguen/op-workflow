//! OpenProject API v3 (verified against a real instance on 2026-11-13).
//!
//! Pure parts (response models, URL building, issue link extraction, comment filtering) live
//! here; the read-only HTTP client is [`HttpTracker`] behind [`TrackerApi`].
//! Read-only: the app never writes to OpenProject. The user changes task status by hand.

pub mod http;

pub use http::HttpTracker;

use std::sync::LazyLock;

use regex::Regex;
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum TrackerError {
    #[error("unexpected OpenProject response: {0}")]
    Shape(String),
    #[error("OpenProject HTTP {status}: {message}")]
    Http { status: u16, message: String },
    #[error("cannot reach OpenProject: {0}")]
    Network(String),
}

pub type TrackerResult<T> = Result<T, TrackerError>;

/// Minimal work package fields the app uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkPackage {
    pub id: u64,
    pub subject: String,
    pub lock_version: u64,
    pub status: String,
    pub project: String,
    pub description: String,
    /// `updatedAt`: changes with any edit (a comment, the status, …), so the app only reloads
    /// comments of tasks whose value moved.
    pub updated_at: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawWp {
    id: u64,
    subject: String,
    lock_version: u64,
    description: Option<RawFormattable>,
    updated_at: Option<String>,
    #[serde(rename = "_links")]
    links: RawLinks,
}

#[derive(Deserialize)]
struct RawFormattable {
    raw: Option<String>,
}

#[derive(Deserialize)]
struct RawLinks {
    status: RawLink,
    project: RawLink,
}

#[derive(Deserialize)]
struct RawLink {
    title: Option<String>,
}

pub fn parse_work_package(v: &Value) -> TrackerResult<WorkPackage> {
    let raw: RawWp = serde_json::from_value(v.clone()).map_err(|e| TrackerError::Shape(e.to_string()))?;
    Ok(WorkPackage {
        id: raw.id,
        subject: raw.subject,
        lock_version: raw.lock_version,
        status: raw.links.status.title.unwrap_or_default(),
        project: raw.links.project.title.unwrap_or_default(),
        description: raw.description.and_then(|d| d.raw).unwrap_or_default(),
        updated_at: raw.updated_at.unwrap_or_default(),
    })
}

/// `GET /api/v3/work_packages` collection → work packages.
pub fn parse_work_package_collection(v: &Value) -> TrackerResult<Vec<WorkPackage>> {
    v.pointer("/_embedded/elements")
        .and_then(Value::as_array)
        .ok_or_else(|| TrackerError::Shape("missing _embedded.elements".into()))?
        .iter()
        .map(parse_work_package)
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    pub id: u64,
    pub created_at: String,
    pub raw: String,
    /// Name of the writer, for the Comment tab only (the brief leaves it out).
    pub author: Option<String>,
}

/// `GET /api/v3/work_packages/{id}/activities` → only activities that carry a comment.
pub fn comments_from_activities(v: &Value) -> Vec<Comment> {
    v.pointer("/_embedded/elements")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|a| {
            let raw = a.pointer("/comment/raw")?.as_str()?.trim();
            (!raw.is_empty()).then(|| Comment {
                id: a.get("id").and_then(Value::as_u64).unwrap_or_default(),
                created_at: a
                    .get("createdAt")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                raw: raw.to_owned(),
                author: a
                    .pointer("/_links/user/title")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            })
        })
        .collect()
}

/// Clean up a user-entered OpenProject URL: trimmed, no trailing slash, `https://` only
/// (plain `http://` is allowed for localhost).
pub fn normalize_base_url(input: &str) -> Option<String> {
    let url = input.trim().trim_end_matches('/');
    let ok = url.starts_with("https://") && url.len() > "https://".len()
        || url.starts_with("http://localhost")
        || url.starts_with("http://127.0.0.1");
    (ok && !url.contains(char::is_whitespace)).then(|| url.to_owned())
}

/// Inbox query: open work packages assigned to the token owner, all projects.
pub fn inbox_url(base_url: &str, page_size: u32) -> String {
    let filters =
        r#"[{"assignee":{"operator":"=","values":["me"]}},{"status":{"operator":"o","values":[]}}]"#;
    format!(
        "{}/api/v3/work_packages?pageSize={page_size}&filters={}",
        base_url.trim_end_matches('/'),
        encode_component(filters)
    )
}

fn encode_component(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueHost {
    GitHub,
    GitLab,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueLink {
    pub host: IssueHost,
    /// `github.com` or the GitLab host, e.g. `gitlab.example.com`
    pub domain: String,
    /// `org/repo` or `group/sub/project`
    pub repo: String,
    pub number: u64,
    pub url: String,
}

static GITHUB_ISSUE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https?://(github\.com)/([\w.-]+/[\w.-]+)/issues/(\d+)").expect("valid regex")
});
static GITLAB_ISSUE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https?://([\w.-]+)/([\w.-]+(?:/[\w.-]+)+)/-/issues/(\d+)").expect("valid regex")
});

/// Issue links in a task description or comment, de-duplicated, in order of appearance.
pub fn extract_issue_links(text: &str) -> Vec<IssueLink> {
    let mut found: Vec<(usize, IssueLink)> = Vec::new();
    for (re, host) in [
        (&*GITHUB_ISSUE, IssueHost::GitHub),
        (&*GITLAB_ISSUE, IssueHost::GitLab),
    ] {
        for c in re.captures_iter(text) {
            let (Some(all), Some(domain), Some(repo), Some(num)) = (c.get(0), c.get(1), c.get(2), c.get(3))
            else {
                continue;
            };
            let Ok(number) = num.as_str().parse() else {
                continue;
            };
            found.push((
                all.start(),
                IssueLink {
                    host,
                    domain: domain.as_str().to_owned(),
                    repo: repo.as_str().to_owned(),
                    number,
                    url: all.as_str().to_owned(),
                },
            ));
        }
    }
    found.sort_by_key(|(pos, _)| *pos);
    let mut out: Vec<IssueLink> = Vec::new();
    for (_, link) in found {
        if !out
            .iter()
            .any(|l| l.domain == link.domain && l.repo == link.repo && l.number == link.number)
        {
            out.push(link);
        }
    }
    out
}

/// `GET /api/v3/statuses` → (status name, `#rrggbb` colour) as set by the OpenProject admin.
/// Statuses without a colour are left out.
pub fn parse_status_colors(v: &Value) -> Vec<(String, String)> {
    v.pointer("/_embedded/elements")
        .and_then(Value::as_array)
        .map(|els| {
            els.iter()
                .filter_map(|e| {
                    let name = e.get("name")?.as_str()?;
                    let color = e.get("color")?.as_str()?;
                    let ok = color.len() == 7
                        && color.starts_with('#')
                        && color[1..].chars().all(|c| c.is_ascii_hexdigit());
                    ok.then(|| (name.to_owned(), color.to_ascii_lowercase()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Network side (read-only). [`HttpTracker`] implements it; the token comes from Windows
/// Credential Manager.
pub trait TrackerApi {
    fn inbox(&self) -> TrackerResult<Vec<WorkPackage>>;
    fn work_package(&self, id: u64) -> TrackerResult<WorkPackage>;
    fn comments(&self, id: u64) -> TrackerResult<Vec<Comment>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn status_colors_keep_valid_hex_only() {
        let v = json!({"_embedded": {"elements": [
            {"name": "New", "color": "#1A67A3"},
            {"name": "In progress", "color": "#ff922b"},
            {"name": "Closed"},
            {"name": "Odd", "color": "red"}
        ]}});
        assert_eq!(
            parse_status_colors(&v),
            vec![
                ("New".to_owned(), "#1a67a3".to_owned()),
                ("In progress".to_owned(), "#ff922b".to_owned())
            ]
        );
        assert!(parse_status_colors(&json!({})).is_empty());
    }

    #[test]
    fn extracts_github_and_gitlab_links_deduped_in_order() {
        let text = "Xem https://github.com/acme/web/issues/456 và \
                    https://gitlab.example.com/acme/desktop/-/issues/12, lặp lại \
                    https://github.com/acme/web/issues/456#issuecomment-1 \
                    rồi https://github.com/acme/web/issues/457";
        let links = extract_issue_links(text);
        let got: Vec<_> = links
            .iter()
            .map(|l| (l.host, l.repo.as_str(), l.number))
            .collect();
        assert_eq!(
            got,
            vec![
                (IssueHost::GitHub, "acme/web", 456),
                (IssueHost::GitLab, "acme/desktop", 12),
                (IssueHost::GitHub, "acme/web", 457),
            ]
        );
        assert!(extract_issue_links("no links, only #456").is_empty());
    }

    #[test]
    fn parses_work_package_and_comments() {
        let wp = json!({"id": 187716, "subject": "S", "lockVersion": 3,
            "description": {"format": "markdown", "raw": "desc"},
            "_links": {"status": {"href": "/api/v3/statuses/1", "title": "Open"},
                       "project": {"href": "/api/v3/projects/208", "title": "Web Platform"}}});
        let parsed = parse_work_package_collection(&json!({"_embedded": {"elements": [wp]}})).unwrap();
        assert_eq!(parsed[0].status, "Open");
        assert_eq!(parsed[0].updated_at, "");
        assert_eq!(parsed[0].lock_version, 3);

        let acts = json!({"_embedded": {"elements": [
            {"id": 1, "createdAt": "2026-11-12T01:00:00Z", "comment": {"raw": ""}},
            {"id": 2, "createdAt": "2026-11-12T02:00:00Z", "comment": {"raw": " Lỗi khi lưu "},
             "_links": {"user": {"href": "/api/v3/users/9", "title": "Tester A"}}}]}});
        let comments = comments_from_activities(&acts);
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].raw, "Lỗi khi lưu");
        assert_eq!(comments[0].author.as_deref(), Some("Tester A"));
    }

    #[test]
    fn base_url_must_be_https() {
        assert_eq!(
            normalize_base_url(" https://op.example.com/ ").as_deref(),
            Some("https://op.example.com")
        );
        assert_eq!(
            normalize_base_url("http://localhost:8080").as_deref(),
            Some("http://localhost:8080")
        );
        for bad in [
            "",
            "op.example.com",
            "http://op.example.com",
            "https://",
            "https://a b",
        ] {
            assert!(normalize_base_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn inbox_url_encodes_filters() {
        let url = inbox_url("https://openproject.example.com/", 100);
        assert!(url.starts_with(
            "https://openproject.example.com/api/v3/work_packages?pageSize=100&filters=%5B%7B%22assignee"
        ));
        assert!(!url.contains('"'));
    }
}
