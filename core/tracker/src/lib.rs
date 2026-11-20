//! OpenProject API v3 (verified against a real instance on 2026-11-13).
//!
//! Pure parts (response models, URL building, issue link extraction, comment filtering) live
//! here; the read-only HTTP client is [`HttpTracker`] behind [`TrackerApi`].
//! Read-only: the app never writes to OpenProject. The user changes task status by hand.

pub mod http;

pub use http::HttpTracker;

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Every message is a sentence the user can act on (what went wrong and what to do), never a
/// bare HTTP status or a library's English error.
#[derive(Debug, thiserror::Error)]
pub enum TrackerError {
    #[error(
        "OpenProject trả về dữ liệu app không đọc được ({0}). Kiểm tra URL có đúng là OpenProject không."
    )]
    Shape(String),
    #[error("{}", http_problem(*.status, .message))]
    Http { status: u16, message: String },
    #[error("Không kết nối được tới OpenProject: {0}.")]
    Network(String),
    #[error(
        "URL này trả về một trang web chứ không phải API OpenProject. Chỉ nhập địa chỉ gốc, ví dụ https://openproject.cong-ty.vn."
    )]
    NotApi,
}

/// What an HTTP error status from OpenProject means for the user.
fn http_problem(status: u16, message: &str) -> String {
    let said = message.trim();
    let detail = if said.is_empty() || said.starts_with('<') {
        String::new()
    } else {
        format!(" OpenProject ghi: “{said}”.")
    };
    let what = match status {
        401 => "OpenProject không nhận token (lỗi 401): token sai, đã hết hạn hoặc bị thu hồi. Tạo token mới ở OpenProject → My account → Access tokens rồi dán vào Cài đặt → OpenProject.".to_owned(),
        403 => "Tài khoản của token không có quyền xem mục này trên OpenProject (lỗi 403). Nhờ quản trị cấp quyền trong project.".to_owned(),
        404 => "OpenProject không thấy mục này (lỗi 404): URL OpenProject sai, hoặc task đã bị xoá / bạn không còn quyền xem.".to_owned(),
        408 | 504 => format!("OpenProject trả lời quá chậm (lỗi {status}). Thử lại sau ít phút."),
        429 => "OpenProject tạm chặn vì app hỏi quá dày (lỗi 429). App tự thử lại sau.".to_owned(),
        300..=399 => format!("URL OpenProject đang chuyển hướng sang chỗ khác (lỗi {status}): kiểm tra lại URL, đúng https:// và đúng địa chỉ."),
        500..=599 => format!("Máy chủ OpenProject đang lỗi (lỗi {status}). Không phải do bạn; thử lại sau ít phút."),
        _ => format!("OpenProject từ chối yêu cầu (lỗi {status})."),
    };
    format!("{what}{detail}")
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
    /// Priority name as OpenProject shows it (`_links.priority.title`, e.g. `High`); empty when
    /// the response has none.
    pub priority: String,
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
    priority: Option<RawLink>,
}

#[derive(Deserialize)]
struct RawLink {
    title: Option<String>,
}

/// OpenProject's editor leaves bits of HTML in the markdown it stores (`<br>`, `&nbsp;`,
/// `<p>`, `<span …>`): turn them into plain markdown so the brief, the AI and the app read
/// clean text. Code (fenced blocks and `` `spans` ``) stays as written; tables and other tags
/// stay too.
pub fn clean_markdown(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut fence: Option<&str> = None;
    for (i, line) in raw.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let start = line.trim_start();
        let marker = ["```", "~~~"].into_iter().find(|m| start.starts_with(m));
        match (fence, marker) {
            (Some(open), Some(m)) if open == m => {
                fence = None;
                out.push_str(line);
                continue;
            }
            (Some(_), _) => {
                out.push_str(line);
                continue;
            }
            (None, Some(m)) => {
                fence = Some(m);
                out.push_str(line);
                continue;
            }
            (None, None) => {}
        }
        // outside `code spans` only: the even pieces between backticks
        for (j, piece) in line.split('`').enumerate() {
            if j > 0 {
                out.push('`');
            }
            if j % 2 == 0 {
                out.push_str(&clean_html(piece));
            } else {
                out.push_str(piece);
            }
        }
    }
    out
}

fn clean_html(text: &str) -> String {
    static BREAK: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)<br\s*/?>|</(p|div)\s*>").expect("valid regex"));
    static INLINE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)</?(p|div|span|font|strong|b|em|i|u|ins|mark|figure|figcaption)(\s[^<>]*)?>")
            .expect("valid regex")
    });
    static NUMERIC: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"&#(x[0-9a-fA-F]{1,6}|[0-9]{1,7});").expect("valid regex"));
    if !text.contains(['<', '&']) {
        return text.to_owned();
    }
    let text = BREAK.replace_all(text, "\n");
    let text = INLINE.replace_all(&text, |c: &regex::Captures<'_>| {
        // bold and italics stay bold and italics
        let tag = c[1].to_ascii_lowercase();
        match tag.as_str() {
            "strong" | "b" => "**",
            "em" | "i" => "_",
            _ => "",
        }
        .to_owned()
    });
    let text = NUMERIC.replace_all(&text, |c: &regex::Captures<'_>| {
        let code = &c[1];
        let n = match code.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok(),
            None => code.parse().ok(),
        };
        n.and_then(char::from_u32).filter(|ch| *ch != '\0').map_or_else(
            || c[0].to_owned(),
            |ch| {
                if ch == '\u{a0}' {
                    " ".to_owned()
                } else {
                    ch.to_string()
                }
            },
        )
    });
    text.replace("&nbsp;", " ")
        .replace('\u{a0}', " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

pub fn parse_work_package(v: &Value) -> TrackerResult<WorkPackage> {
    let raw: RawWp = serde_json::from_value(v.clone()).map_err(|e| TrackerError::Shape(e.to_string()))?;
    Ok(WorkPackage {
        id: raw.id,
        subject: raw.subject,
        lock_version: raw.lock_version,
        status: raw.links.status.title.unwrap_or_default(),
        project: raw.links.project.title.unwrap_or_default(),
        description: clean_markdown(&raw.description.and_then(|d| d.raw).unwrap_or_default()),
        updated_at: raw.updated_at.unwrap_or_default(),
        priority: raw.links.priority.and_then(|p| p.title).unwrap_or_default(),
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

/// Also saved as JSON (the Comment tab's last read, `task_kv` kind `comments.op`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub id: u64,
    pub created_at: String,
    pub raw: String,
    /// Name of the writer, for the Comment tab only (the brief leaves it out).
    pub author: Option<String>,
    /// OpenProject user id of the writer (from `_links.user.href`), to tell the user's own
    /// comments from other people's.
    pub author_id: Option<u64>,
    /// The activity's number on its work package (`version`), for the `#activity-N` link.
    pub version: Option<u64>,
}

impl Comment {
    /// Written by the user `me` (the token owner). Unknown on either side → not theirs, so a
    /// comment is never hidden from the alerts by mistake.
    pub fn is_by(&self, me: Option<u64>) -> bool {
        me.is_some() && self.author_id == me
    }
}

/// The token owner (`GET /api/v3/users/me`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Me {
    pub id: u64,
    pub name: String,
}

pub fn parse_me(v: &Value) -> TrackerResult<Me> {
    let id = v
        .get("id")
        .and_then(Value::as_u64)
        .ok_or_else(|| TrackerError::Shape("users/me without id".into()))?;
    let name = v
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| TrackerError::Shape("users/me without name".into()))?;
    Ok(Me {
        id,
        name: name.to_owned(),
    })
}

/// `/api/v3/users/9` → 9.
fn user_id(href: &str) -> Option<u64> {
    href.rsplit_once("/api/v3/users/")?
        .1
        .trim_end_matches('/')
        .parse()
        .ok()
}

/// `GET /api/v3/work_packages/{id}/activities` → only activities that carry a comment.
pub fn comments_from_activities(v: &Value) -> Vec<Comment> {
    v.pointer("/_embedded/elements")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|a| {
            let raw = clean_markdown(a.pointer("/comment/raw")?.as_str()?);
            let raw = raw.trim();
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
                author_id: a
                    .pointer("/_links/user/href")
                    .and_then(Value::as_str)
                    .and_then(user_id),
                version: a.get("version").and_then(Value::as_u64),
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

/// `GET /api/v3/statuses` (or `/priorities`) → (name, `#rrggbb` colour) as set by the
/// OpenProject admin. Entries without a colour are left out.
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

/// What `GET /api/v3/priorities` says: colours set by the admin, and the names in OpenProject's
/// own order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Priorities {
    /// name → `#rrggbb`; priorities without a colour are left out.
    pub colors: Vec<(String, String)>,
    /// Active priorities from the lowest to the highest (`position`).
    pub order: Vec<String>,
}

pub fn parse_priorities(v: &Value) -> Priorities {
    let mut order: Vec<(u64, String)> = v
        .pointer("/_embedded/elements")
        .and_then(Value::as_array)
        .map(|els| {
            els.iter()
                .filter(|e| e.get("isActive").and_then(Value::as_bool) != Some(false))
                .filter_map(|e| Some((e.get("position")?.as_u64()?, e.get("name")?.as_str()?.to_owned())))
                .collect()
        })
        .unwrap_or_default();
    order.sort_by_key(|(position, _)| *position);
    Priorities {
        colors: parse_status_colors(v),
        order: order.into_iter().map(|(_, name)| name).collect(),
    }
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

    #[test]
    fn html_left_by_the_editor_becomes_plain_markdown() {
        let raw = "以下について<br>確認&nbsp;ください。<br/>\n<p>補足:</p>\n<strong>Bổ sung</strong> &amp; &lt;x&gt; &#39;a&#39; &#x41;\n`<br>` stays\n```\n<br> &nbsp;\n```\n<table><tr><td>a</td></tr></table>";
        let clean = clean_markdown(raw);
        assert_eq!(
            clean,
            "以下について\n確認 ください。\n\n補足:\n\n**Bổ sung** & <x> 'a' A\n`<br>` stays\n```\n<br> &nbsp;\n```\n<table><tr><td>a</td></tr></table>"
        );
        assert_eq!(clean_markdown("plain text"), "plain text");
        assert_eq!(clean_markdown("&amp;lt;"), "&lt;");
    }

    #[test]
    fn http_errors_say_what_to_do() {
        let e = TrackerError::Http {
            status: 401,
            message: "Unauthenticated".into(),
        }
        .to_string();
        assert!(
            e.contains("401") && e.contains("Access tokens") && e.contains("Unauthenticated"),
            "{e}"
        );
        let e = TrackerError::Http {
            status: 502,
            message: "<html>".into(),
        }
        .to_string();
        assert!(e.contains("502") && !e.contains("<html>"), "{e}");
        assert!(TrackerError::NotApi.to_string().contains("API OpenProject"));
    }
    use serde_json::json;

    #[test]
    fn priorities_come_in_openproject_order_without_inactive_ones() {
        let v = json!({"_embedded": {"elements": [
            {"name": "High", "position": 3, "color": "#FF922B", "isActive": true},
            {"name": "Low", "position": 1, "isActive": true},
            {"name": "Old", "position": 2, "isActive": false},
            {"name": "Normal", "position": 2},
            {"name": "Immediate", "position": 4, "color": "#e03131"}
        ]}});
        let p = parse_priorities(&v);
        assert_eq!(p.order, vec!["Low", "Normal", "High", "Immediate"]);
        assert_eq!(
            p.colors,
            vec![
                ("High".to_owned(), "#ff922b".to_owned()),
                ("Immediate".to_owned(), "#e03131".to_owned())
            ]
        );
        assert_eq!(parse_priorities(&json!({})), Priorities::default());
    }

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
        assert_eq!(parsed[0].priority, "");
        let with_priority = json!({"id": 1, "subject": "S", "lockVersion": 0,
            "_links": {"status": {"title": "Open"}, "project": {"title": "P"},
                       "priority": {"href": "/api/v3/priorities/9", "title": "Immediate"}}});
        assert_eq!(parse_work_package(&with_priority).unwrap().priority, "Immediate");
        assert_eq!(parsed[0].updated_at, "");
        assert_eq!(parsed[0].lock_version, 3);

        let acts = json!({"_embedded": {"elements": [
            {"id": 1, "createdAt": "2026-11-12T01:00:00Z", "comment": {"raw": ""}},
            {"id": 2, "version": 4, "createdAt": "2026-11-12T02:00:00Z", "comment": {"raw": " Lỗi khi lưu "},
             "_links": {"user": {"href": "/api/v3/users/9", "title": "Tester A"}}}]}});
        let comments = comments_from_activities(&acts);
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].raw, "Lỗi khi lưu");
        assert_eq!(comments[0].author.as_deref(), Some("Tester A"));
        assert_eq!(comments[0].author_id, Some(9));
        assert_eq!(comments[0].version, Some(4));
        assert!(comments[0].is_by(Some(9)));
        assert!(!comments[0].is_by(Some(10)) && !comments[0].is_by(None));
    }

    #[test]
    fn me_and_user_ids() {
        let me = parse_me(&json!({"_type": "User", "id": 9, "name": "Dev C", "login": "devc"})).unwrap();
        assert_eq!(
            me,
            Me {
                id: 9,
                name: "Dev C".into()
            }
        );
        assert!(parse_me(&json!({"name": "x"})).is_err());
        assert_eq!(user_id("/api/v3/users/9"), Some(9));
        assert_eq!(user_id("https://op.example.com/api/v3/users/12"), Some(12));
        assert_eq!(user_id("/api/v3/users/me"), None);
        assert_eq!(user_id("/api/v3/groups/3"), None);
        // a comment without a user link (system) is nobody's
        let acts = json!({"_embedded": {"elements": [
            {"id": 1, "createdAt": "x", "comment": {"raw": "auto"}}]}});
        let c = &comments_from_activities(&acts)[0];
        assert_eq!(c.author_id, None);
        assert!(!c.is_by(Some(9)));
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
