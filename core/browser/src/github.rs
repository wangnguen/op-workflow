//! Read GitHub issues (title, markdown body, comment bodies) from the issue page's embedded
//! data. Authors, assignees, labels and the activity list are never collected, and `@mentions`
//! in the text are blanked: the brief only needs what the issue says.

use std::sync::LazyLock;
use std::time::{Duration, Instant};

use regex::Regex;
use serde_json::{Value, json};

use crate::cdp::{Cdp, CdpError};
use crate::{BrowserError, find_debug_port};

/// How long the user has to click Allow in Chrome.
const ALLOW_WAIT: Duration = Duration::from_secs(90);
/// Load + read of one issue page.
const PAGE_WAIT: Duration = Duration::from_secs(45);
const CLOSE_WAIT: Duration = Duration::from_secs(5);
/// The data script is in the HTML, but give a slow page a few more looks.
const READ_TRIES: u32 = 5;
const READ_RETRY: Duration = Duration::from_millis(800);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueComment {
    pub created_at: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuePage {
    pub url: String,
    pub title: String,
    pub body: String,
    /// Visible comments, oldest first (hidden/minimized ones are skipped).
    pub comments: Vec<IssueComment>,
    /// What GitHub counts, including comments the page did not load or that are hidden.
    pub total_comments: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IssueError {
    #[error("Chrome chưa đăng nhập GitHub (trang báo không tìm thấy)")]
    SignedOut,
    #[error("GitHub báo không tìm thấy: link sai hoặc tài khoản không xem được repo này")]
    NotFound,
    #[error("chỉ đọc link issue GitHub dạng github.com/<org>/<repo>/issues/<số>")]
    NotAnIssueLink,
    #[error("trang tải quá lâu")]
    Timeout,
    #[error("không tải được trang: {0}")]
    Load(String),
    #[error("trang không có dữ liệu issue như mong đợi (GitHub có thể đã đổi giao diện)")]
    Unreadable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueRead {
    pub url: String,
    pub result: Result<IssuePage, IssueError>,
}

static ISSUE_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^https://github\.com/([\w.-]+)/([\w.-]+)/issues/(\d+)(?:[/?#].*)?$").expect("valid regex")
});

/// The only pages we ever open: `https://github.com/<org>/<repo>/issues/<n>`, rebuilt clean.
fn canonical_issue_url(url: &str) -> Option<String> {
    let c = ISSUE_URL.captures(url.trim())?;
    Some(format!("https://github.com/{}/{}/issues/{}", &c[1], &c[2], &c[3]))
}

/// Runs in the issue page. Returns only the issue's title/body and its comments' dates and
/// bodies (+ the HTML, to find which `@words` GitHub rendered as mentions).
const EXTRACT: &str = r#"(() => {
  const login = document.querySelector('meta[name="user-login"]')?.content || "";
  const base = { signedIn: login !== "", notFound: /not found/i.test(document.title) };
  const el = document.querySelector('script[type="application/json"][data-target="react-app.embeddedData"]');
  let issue = null;
  try { issue = JSON.parse(el.textContent).payload.issueViewerRoute.data.repository.issue; } catch (e) {}
  if (!issue || typeof issue.title !== "string") return { ...base, found: false };
  const seen = new Set();
  const comments = [];
  for (const key of ["timelineItems", "backTimelineItems"]) {
    for (const edge of (issue[key] && issue[key].edges) || []) {
      const n = edge && edge.node;
      if (!n || (n.id && seen.has(n.id))) continue;
      if (n.id) seen.add(n.id);
      if (n.__typename !== "IssueComment" || n.isHidden || n.isMinimized) continue;
      comments.push({ createdAt: n.createdAt || "", body: n.body || "", bodyHTML: n.bodyHTML || "" });
    }
  }
  return { ...base, found: true, title: issue.title, body: issue.body || "", bodyHTML: issue.bodyHTML || "",
           totalComments: issue.totalCommentsCount ?? comments.length, comments };
})()"#;

#[derive(Debug, PartialEq, Eq)]
enum Extracted {
    Page(IssuePage),
    /// no data yet; look again
    NotYet,
    Failed(IssueError),
}

/// Logins (and `org/team` names) that GitHub rendered as mentions in `html`.
fn mentioned(html: &str) -> Vec<String> {
    static MENTION: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"class="[^"]*\b(?:user|team)-mention\b[^"]*"[^>]*>@([^<\s]+)<"#).expect("valid regex")
    });
    let mut names: Vec<String> = MENTION.captures_iter(html).map(|c| c[1].to_owned()).collect();
    names.sort_by_key(|n| std::cmp::Reverse(n.len()));
    names.dedup();
    names
}

/// Replace `@name` for every mention GitHub marked in `html` with `@…`; other `@` text (emails,
/// decorators in code) stays.
pub fn strip_mentions(markdown: &str, html: &str) -> String {
    let mut out = markdown.to_owned();
    for name in mentioned(html) {
        let needle = format!("@{name}");
        let mut result = String::with_capacity(out.len());
        let mut rest = out.as_str();
        while let Some(at) = rest.find(&needle) {
            let prev = rest[..at].chars().last().or_else(|| result.chars().last());
            let before_ok = !prev.is_some_and(|c| c.is_alphanumeric() || c == '_');
            let after = &rest[at + needle.len()..];
            let after_ok = !after
                .chars()
                .next()
                .is_some_and(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '/');
            result.push_str(&rest[..at]);
            result.push_str(if before_ok && after_ok { "@…" } else { &needle });
            rest = after;
        }
        result.push_str(rest);
        out = result;
    }
    out
}

fn text(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_default().to_owned()
}

fn parse_extracted(v: &Value, url: &str) -> Extracted {
    let flag = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
    if !flag("found") {
        return match (flag("notFound"), flag("signedIn")) {
            (true, false) => Extracted::Failed(IssueError::SignedOut),
            (true, true) => Extracted::Failed(IssueError::NotFound),
            _ => Extracted::NotYet,
        };
    }
    let comments: Vec<IssueComment> = v
        .get("comments")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|c| IssueComment {
                    created_at: text(c, "createdAt"),
                    body: strip_mentions(&text(c, "body"), &text(c, "bodyHTML")),
                })
                .collect()
        })
        .unwrap_or_default();
    let total = v
        .get("totalComments")
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(0);
    Extracted::Page(IssuePage {
        url: url.to_owned(),
        title: text(v, "title"),
        body: strip_mentions(&text(v, "body"), &text(v, "bodyHTML")),
        total_comments: total.max(u32::try_from(comments.len()).unwrap_or(u32::MAX)),
        comments,
    })
}

fn load_error(e: CdpError) -> IssueError {
    match e {
        CdpError::Timeout => IssueError::Timeout,
        CdpError::Closed => IssueError::Load("Chrome đã đóng kết nối".into()),
        CdpError::Remote(m) | CdpError::Protocol(m) => IssueError::Load(m),
    }
}

fn str_field(v: &Value, key: &str) -> Result<String, IssueError> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| IssueError::Load(format!("Chrome không trả {key}")))
}

/// Load `url` in a background tab of the user's Chrome, read it, close the tab.
fn read_one(cdp: &mut Cdp, url: &str) -> Result<IssuePage, IssueError> {
    let deadline = Instant::now() + PAGE_WAIT;
    let created = cdp
        .call(
            "Target.createTarget",
            json!({ "url": "about:blank", "background": true }),
            None,
            deadline,
        )
        .map_err(load_error)?;
    let target = str_field(&created, "targetId")?;
    let result = read_in_tab(cdp, &target, url, deadline);
    let _ = cdp.call(
        "Target.closeTarget",
        json!({ "targetId": target }),
        None,
        Instant::now() + CLOSE_WAIT,
    );
    result
}

fn read_in_tab(cdp: &mut Cdp, target: &str, url: &str, deadline: Instant) -> Result<IssuePage, IssueError> {
    let attached = cdp
        .call(
            "Target.attachToTarget",
            json!({ "targetId": target, "flatten": true }),
            None,
            deadline,
        )
        .map_err(load_error)?;
    let session = str_field(&attached, "sessionId")?;
    let s = Some(session.as_str());
    cdp.call("Page.enable", json!({}), s, deadline)
        .map_err(load_error)?;
    let nav = cdp
        .call("Page.navigate", json!({ "url": url }), s, deadline)
        .map_err(load_error)?;
    if let Some(err) = nav.get("errorText").and_then(Value::as_str) {
        return Err(IssueError::Load(err.to_owned()));
    }
    cdp.wait_event("Page.loadEventFired", &session, deadline)
        .map_err(load_error)?;
    for attempt in 1..=READ_TRIES {
        let r = cdp
            .call(
                "Runtime.evaluate",
                json!({ "expression": EXTRACT, "returnByValue": true }),
                s,
                deadline,
            )
            .map_err(load_error)?;
        let value = r.pointer("/result/value").cloned().unwrap_or(Value::Null);
        match parse_extracted(&value, url) {
            Extracted::Page(page) => return Ok(page),
            Extracted::Failed(e) => return Err(e),
            Extracted::NotYet if attempt < READ_TRIES => std::thread::sleep(READ_RETRY),
            Extracted::NotYet => {}
        }
    }
    Err(IssueError::Unreadable)
}

/// Read each GitHub issue through the user's running Chrome. One connection for all of them,
/// so Chrome asks Allow once. `Err` = could not talk to Chrome at all.
pub fn read_github_issues(urls: &[String]) -> Result<Vec<IssueRead>, BrowserError> {
    let port = find_debug_port().ok_or(BrowserError::NotEnabled)?;
    let mut cdp = Cdp::connect(port.port, &port.path, Instant::now() + ALLOW_WAIT)?;
    cdp.call("Browser.getVersion", json!({}), None, Instant::now() + ALLOW_WAIT)
        .map_err(|e| match e {
            CdpError::Timeout | CdpError::Closed => BrowserError::Denied,
            CdpError::Remote(m) | CdpError::Protocol(m) => BrowserError::Protocol(m),
        })?;
    let reads = urls
        .iter()
        .map(|raw| IssueRead {
            url: raw.clone(),
            result: match canonical_issue_url(raw) {
                Some(url) => read_one(&mut cdp, &url),
                None => Err(IssueError::NotAnIssueLink),
            },
        })
        .collect();
    cdp.close();
    Ok(reads)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_clean_github_issue_urls_are_opened() {
        assert_eq!(
            canonical_issue_url("https://github.com/acme/web/issues/12#issuecomment-9"),
            Some("https://github.com/acme/web/issues/12".into())
        );
        assert_eq!(canonical_issue_url("https://github.com/acme/web/pull/12"), None);
        assert_eq!(canonical_issue_url("http://github.com/acme/web/issues/12"), None);
        assert_eq!(
            canonical_issue_url("https://github.com.evil.io/a/b/issues/1"),
            None
        );
        assert_eq!(canonical_issue_url("https://github.com/a/b/issues/1x"), None);
    }

    #[test]
    fn mentions_are_blanked_but_emails_and_code_stay() {
        let md = "cc @dev-a @acme/backend: see @dev-ab, mail x@dev-a.io, `@Component`";
        let html = r#"cc <a class="user-mention notranslate" data-hovercard-type="user" href="/dev-a">@dev-a</a> <a class="team-mention js-team-mention" href="/orgs/acme/teams/backend">@acme/backend</a>: see"#;
        assert_eq!(
            strip_mentions(md, html),
            "cc @… @…: see @dev-ab, mail x@dev-a.io, `@Component`"
        );
        assert_eq!(strip_mentions("plain @x", ""), "plain @x");
    }

    #[test]
    fn extracted_page_keeps_text_only() {
        let v = json!({
            "signedIn": true, "notFound": false, "found": true,
            "title": "保存できない", "body": "手順 @dev-a", "bodyHTML": "<a class=\"user-mention\">@dev-a</a>",
            "totalComments": 3,
            "comments": [{ "createdAt": "2026-11-10T01:00:00Z", "body": "補足", "bodyHTML": "" }]
        });
        let Extracted::Page(page) = parse_extracted(&v, "https://github.com/a/b/issues/1") else {
            panic!("page expected");
        };
        assert_eq!(page.title, "保存できない");
        assert_eq!(page.body, "手順 @…");
        assert_eq!(page.comments.len(), 1);
        assert_eq!(page.comments[0].body, "補足");
        assert_eq!(page.total_comments, 3);
    }

    /// Live, read-only, needs a click on Allow in Chrome. Prints sizes only, never the text:
    /// `OPW_ISSUE_LINKS=<file with one url per line> cargo test -p opw-browser -- --ignored live --nocapture`
    #[test]
    #[ignore = "talks to the user's Chrome"]
    fn live_read_issues() {
        let file = std::env::var("OPW_ISSUE_LINKS").expect("OPW_ISSUE_LINKS");
        let urls: Vec<String> = std::fs::read_to_string(file)
            .unwrap()
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .collect();
        for read in read_github_issues(&urls).unwrap() {
            match read.result {
                Ok(p) => println!(
                    "ok: title {} chars, body {} chars, {} comments of {}, mentions left {}",
                    p.title.chars().count(),
                    p.body.chars().count(),
                    p.comments.len(),
                    p.total_comments,
                    p.body.matches("@…").count()
                ),
                Err(e) => println!("error: {e}"),
            }
        }
    }

    #[test]
    fn not_found_pages_tell_signed_out_from_no_access() {
        let page = |signed_in: bool| json!({ "signedIn": signed_in, "notFound": true, "found": false });
        assert_eq!(
            parse_extracted(&page(false), "u"),
            Extracted::Failed(IssueError::SignedOut)
        );
        assert_eq!(
            parse_extracted(&page(true), "u"),
            Extracted::Failed(IssueError::NotFound)
        );
        assert_eq!(
            parse_extracted(
                &json!({ "signedIn": true, "notFound": false, "found": false }),
                "u"
            ),
            Extracted::NotYet
        );
        assert_eq!(parse_extracted(&Value::Null, "u"), Extracted::NotYet);
    }
}
