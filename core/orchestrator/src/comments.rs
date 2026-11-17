//! The Comment tab and the "new comment" counts: OpenProject comments (read live) and the
//! GitHub issues of the last "Lấy issue" (stored, since reading them needs the user's Allow).
//!
//! What the user has seen lives in `task_kv`:
//! - `comments.seen`: highest OpenProject comment id the user saw (set at start, when the brief
//!   took the comments in, and when the Comment tab opens);
//! - `comments.notified`: highest id already announced in the bell;
//! - `issues.read` (key = issue URL): the issue as last read, JSON;
//! - `issues.seen` (key = issue URL): date of the newest issue comment the user saw;
//! - `nx.status`: the OpenProject status last seen, to notice Test failed / Reopened.
//!
//! The user's own comments (OpenProject: author = `users/me`; GitHub: `viewerDidAuthor`) never
//! count as new: no "cần xem" mark, no "+N comment", no bell. The tab shows them folded.

use std::collections::HashMap;

use opw_context::{BriefIssue, newest_brief_comment};
use opw_domain::view::{CommentView, IssueThread};
use opw_storage::Store;
use opw_tracker::Comment;
use serde::{Deserialize, Serialize};

pub const SEEN: &str = "comments.seen";
pub const NOTIFIED: &str = "comments.notified";
pub const ISSUES: &str = "issues.read";
pub const ISSUES_SEEN: &str = "issues.seen";
pub const STATUS: &str = "nx.status";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredComment {
    pub created_at: String,
    pub body: String,
    /// The user wrote it (reads saved before this field existed: `false`).
    #[serde(default)]
    pub mine: bool,
}

/// One issue as "Lấy issue" read it (no author names, like the brief).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredIssue {
    pub url: String,
    pub title: String,
    pub comments: Vec<StoredComment>,
    pub unread: u32,
    pub read_at: String,
}

impl StoredIssue {
    pub fn from_brief(issue: &BriefIssue, read_at: &str) -> Self {
        Self {
            url: issue.url.clone(),
            title: issue.title.clone(),
            comments: issue
                .comments
                .iter()
                .map(|c| StoredComment {
                    created_at: c.created_at.clone(),
                    body: c.body.clone(),
                    mine: c.mine,
                })
                .collect(),
            unread: issue.unread,
            read_at: read_at.to_owned(),
        }
    }

    pub fn newest(&self) -> Option<&str> {
        self.comments.iter().map(|c| c.created_at.as_str()).max()
    }

    /// Other people's comments newer than `seen` (none when nothing was seen yet: the first
    /// read is the baseline).
    pub fn new_after(&self, seen: Option<&str>) -> u32 {
        let Some(seen) = seen else { return 0 };
        count(
            self.comments
                .iter()
                .filter(|c| !c.mine && c.created_at.as_str() > seen),
        )
    }
}

fn count<T>(it: impl Iterator<Item = T>) -> u32 {
    u32::try_from(it.count()).unwrap_or(u32::MAX)
}

pub fn max_id(comments: &[Comment]) -> Option<u64> {
    comments.iter().map(|c| c.id).max()
}

/// Other people's OpenProject comments newer than the last one the user saw (0 before any
/// baseline). `me`: the user's OpenProject id, if known.
pub fn op_new(comments: &[Comment], seen: Option<u64>, me: Option<u64>) -> u32 {
    seen.map_or(0, |s| count(comments.iter().filter(|c| c.id > s && !c.is_by(me))))
}

/// The comment on its work package page: `<task url>/activity#activity-<N>` (the page's own
/// "#N" anchor; without a number, the activity tab).
pub fn op_link(task_url: &str, c: &Comment) -> String {
    match c.version {
        Some(n) => format!("{task_url}/activity#activity-{n}"),
        None => format!("{task_url}/activity"),
    }
}

/// Newest first, `is_new` against `seen` (never for the user's own comments). `task_url`:
/// `<OpenProject>/work_packages/<id>`, for each comment's link.
pub fn op_views(
    comments: &[Comment],
    seen: Option<u64>,
    me: Option<u64>,
    task_url: Option<&str>,
) -> Vec<CommentView> {
    let mut list: Vec<&Comment> = comments.iter().collect();
    list.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(b.id.cmp(&a.id)));
    list.into_iter()
        .map(|c| {
            let mine = c.is_by(me);
            CommentView {
                id: c.id.to_string(),
                created_at: c.created_at.clone(),
                author: c.author.clone(),
                text: c.raw.clone(),
                mine,
                is_new: !mine && seen.is_some_and(|s| c.id > s),
                url: task_url.map(|t| op_link(t, c)),
            }
        })
        .collect()
}

/// Comments to ring the bell for: other people's, newer than the last announced id.
pub fn to_announce(comments: &[Comment], notified: u64, me: Option<u64>) -> Vec<&Comment> {
    comments
        .iter()
        .filter(|c| c.id > notified && !c.is_by(me))
        .collect()
}

pub fn issue_threads(issues: &[StoredIssue], seen: &HashMap<String, String>) -> Vec<IssueThread> {
    issues
        .iter()
        .map(|issue| {
            let seen = seen.get(&issue.url).map(String::as_str);
            let mut comments: Vec<(usize, &StoredComment)> = issue.comments.iter().enumerate().collect();
            comments.sort_by(|a, b| b.1.created_at.cmp(&a.1.created_at));
            IssueThread {
                url: issue.url.clone(),
                title: issue.title.clone(),
                comments: comments
                    .into_iter()
                    .map(|(i, c)| CommentView {
                        id: format!("{}#{i}", issue.url),
                        created_at: c.created_at.clone(),
                        author: None,
                        text: c.body.clone(),
                        mine: c.mine,
                        is_new: !c.mine && seen.is_some_and(|s| c.created_at.as_str() > s),
                        url: Some(issue.url.clone()),
                    })
                    .collect(),
                unread: issue.unread,
            }
        })
        .collect()
}

/// Someone else wrote an OpenProject comment newer than the newest one in the brief's comment
/// section (the user's own replies alone do not ask for a brief update).
pub fn brief_stale(brief: Option<&str>, comments: &[Comment], me: Option<u64>) -> bool {
    let Some(newest) = comments
        .iter()
        .filter(|c| !c.is_by(me))
        .map(|c| c.created_at.as_str())
        .max()
    else {
        return false;
    };
    match brief.and_then(newest_brief_comment) {
        Some(in_brief) => newest > in_brief.as_str(),
        None => brief.is_some(),
    }
}

/// `url` is a page of this task the app links to: the work package itself (any sub-page or
/// anchor) or one of the task's issues as last read. Anything else is refused, so the UI can
/// never make the app open an arbitrary address.
pub fn is_task_link(url: &str, task_url: Option<&str>, issues: &[StoredIssue]) -> bool {
    if !url.starts_with("https://") || url.chars().any(char::is_whitespace) {
        return false;
    }
    let under = |base: &str| {
        url.strip_prefix(base)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(['/', '#', '?']))
    };
    task_url.is_some_and(under) || issues.iter().any(|i| under(&i.url))
}

// ---------- stored state ----------

pub fn seen_id(store: &Store, wp_id: u64) -> Option<u64> {
    store.kv_get(wp_id, SEEN, "").ok().flatten()?.parse().ok()
}

pub fn stored_issues(store: &Store, wp_id: u64) -> Vec<StoredIssue> {
    store
        .kv_all(wp_id, ISSUES)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(_, json)| serde_json::from_str(&json).ok())
        .collect()
}

pub fn issues_seen(store: &Store, wp_id: u64) -> HashMap<String, String> {
    store
        .kv_all(wp_id, ISSUES_SEEN)
        .unwrap_or_default()
        .into_iter()
        .collect()
}

/// Other people's comments the user has not seen: OpenProject (`comments` = the cached list, if
/// any) plus the stored issues.
pub fn new_count(store: &Store, wp_id: u64, comments: Option<&[Comment]>, me: Option<u64>) -> u32 {
    let op = comments.map_or(0, |c| op_new(c, seen_id(store, wp_id), me));
    let seen = issues_seen(store, wp_id);
    let issues: u32 = stored_issues(store, wp_id)
        .iter()
        .map(|i| i.new_after(seen.get(&i.url).map(String::as_str)))
        .sum();
    op.saturating_add(issues)
}

/// Raise `comments.seen` / `comments.notified` to `id` (never lower them).
pub fn raise(store: &Store, wp_id: u64, kind: &str, id: u64, at: &str) {
    let current: Option<u64> = store
        .kv_get(wp_id, kind, "")
        .ok()
        .flatten()
        .and_then(|v| v.parse().ok());
    if current.is_none_or(|c| id > c) {
        let _ = store.kv_set(wp_id, kind, "", &id.to_string(), at);
    }
}

/// Keep a fresh "Lấy issue" read. Returns how many comments are newer than the previous read
/// (0 the first time, which also becomes the "seen" baseline).
pub fn save_issue_read(store: &Store, wp_id: u64, issues: &[StoredIssue], at: &str) -> u32 {
    let before: HashMap<String, StoredIssue> = stored_issues(store, wp_id)
        .into_iter()
        .map(|i| (i.url.clone(), i))
        .collect();
    let seen = issues_seen(store, wp_id);
    let mut fresh = 0u32;
    for issue in issues {
        if let Some(prev) = before.get(&issue.url) {
            fresh = fresh.saturating_add(issue.new_after(Some(prev.newest().unwrap_or_default())));
        }
        if !seen.contains_key(&issue.url) {
            let _ = store.kv_set(
                wp_id,
                ISSUES_SEEN,
                &issue.url,
                issue.newest().unwrap_or_default(),
                at,
            );
        }
        if let Ok(json) = serde_json::to_string(issue) {
            let _ = store.kv_set(wp_id, ISSUES, &issue.url, &json, at);
        }
    }
    fresh
}

/// The user opened the Comment tab: everything shown counts as seen.
pub fn mark_seen(store: &Store, wp_id: u64, comments: &[Comment], at: &str) {
    if let Some(id) = max_id(comments) {
        raise(store, wp_id, SEEN, id, at);
        raise(store, wp_id, NOTIFIED, id, at);
    } else if seen_id(store, wp_id).is_none() {
        let _ = store.kv_set(wp_id, SEEN, "", "0", at);
    }
    mark_issues_seen(store, wp_id, at);
}

/// Every stored issue comment counts as seen.
pub fn mark_issues_seen(store: &Store, wp_id: u64, at: &str) {
    for issue in stored_issues(store, wp_id) {
        if let Some(newest) = issue.newest() {
            let _ = store.kv_set(wp_id, ISSUES_SEEN, &issue.url, newest, at);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The user's own OpenProject id in these tests.
    const ME: Option<u64> = Some(42);

    fn c(id: u64, at: &str) -> Comment {
        Comment {
            id,
            created_at: at.into(),
            raw: format!("c{id}"),
            author: Some("A".into()),
            author_id: Some(7),
            version: None,
        }
    }

    fn mine(id: u64, at: &str) -> Comment {
        Comment {
            author: Some("Me".into()),
            author_id: ME,
            ..c(id, at)
        }
    }

    fn issue(dates: &[&str]) -> StoredIssue {
        StoredIssue {
            url: "https://github.com/acme/web/issues/1".into(),
            title: "t".into(),
            comments: dates
                .iter()
                .map(|d| StoredComment {
                    created_at: (*d).into(),
                    body: "b".into(),
                    mine: false,
                })
                .collect(),
            unread: 0,
            read_at: "x".into(),
        }
    }

    #[test]
    fn op_comments_count_and_mark_new_against_the_baseline() {
        let list = [c(5, "2026-11-10T00:00:00Z"), c(9, "2026-11-12T00:00:00Z")];
        assert_eq!(op_new(&list, None, ME), 0);
        assert_eq!(op_new(&list, Some(5), ME), 1);
        let views = op_views(&list, Some(5), ME, None);
        assert_eq!(views[0].id, "9");
        assert!(views[0].is_new && !views[1].is_new);
        assert!(!views[0].mine && views[0].url.is_none());
        assert_eq!(views[0].author.as_deref(), Some("A"));
    }

    #[test]
    fn own_comments_are_never_new_nor_announced() {
        let list = [
            c(5, "2026-11-10T00:00:00Z"),
            mine(9, "2026-11-12T00:00:00Z"),
            c(11, "2026-11-13T00:00:00Z"),
        ];
        assert_eq!(op_new(&list, Some(5), ME), 1);
        let views = op_views(&list, Some(5), ME, None);
        assert_eq!(views[1].id, "9");
        assert!(views[1].mine && !views[1].is_new);
        assert!(views[0].is_new && !views[0].mine);
        let ids: Vec<u64> = to_announce(&list, 5, ME).iter().map(|c| c.id).collect();
        assert_eq!(ids, vec![11]);
        assert!(to_announce(&list[..2], 5, ME).is_empty());
        // unknown user (users/me not read yet): everything counts as someone else's
        assert_eq!(op_new(&list, Some(5), None), 2);
        assert_eq!(to_announce(&list, 5, None).len(), 2);
    }

    #[test]
    fn links_point_at_the_comment_and_only_task_pages_open() {
        let task = "https://op.example.com/work_packages/77";
        let with_n = Comment {
            version: Some(3),
            ..c(5, "a")
        };
        assert_eq!(op_link(task, &with_n), format!("{task}/activity#activity-3"));
        assert_eq!(op_link(task, &c(6, "b")), format!("{task}/activity"));
        let views = op_views(&[with_n], None, ME, Some(task));
        assert_eq!(
            views[0].url.as_deref(),
            Some("https://op.example.com/work_packages/77/activity#activity-3")
        );

        let issues = [issue(&[])];
        for ok in [
            task,
            "https://op.example.com/work_packages/77/activity#activity-3",
            "https://github.com/acme/web/issues/1",
            "https://github.com/acme/web/issues/1#issuecomment-9",
        ] {
            assert!(is_task_link(ok, Some(task), &issues), "{ok}");
        }
        for bad in [
            "https://op.example.com/work_packages/778",
            "https://op.example.com/work_packages/7",
            "https://op.example.com/projects/x",
            "https://github.com/acme/web/issues/12",
            "https://evil.example/https://op.example.com/work_packages/77",
            "http://op.example.com/work_packages/77",
            "https://op.example.com/work_packages/77 & calc",
            "",
        ] {
            assert!(!is_task_link(bad, Some(task), &issues), "{bad}");
        }
        assert!(!is_task_link(task, None, &[]));
    }

    #[test]
    fn brief_is_stale_when_openproject_has_newer_comments() {
        let brief = "# x\n\n## Comment trên OpenProject\n### 2026-11-10T00:00:00Z\ncũ\n\n## Từ issue\n";
        assert!(!brief_stale(Some(brief), &[c(5, "2026-11-10T00:00:00Z")], ME));
        assert!(brief_stale(Some(brief), &[c(9, "2026-11-12T00:00:00Z")], ME));
        assert!(!brief_stale(Some(brief), &[], ME));
        assert!(!brief_stale(None, &[c(9, "2026-11-12T00:00:00Z")], ME));
        // only the user's own reply is newer: no update asked
        let own = [c(5, "2026-11-10T00:00:00Z"), mine(9, "2026-11-12T00:00:00Z")];
        assert!(!brief_stale(Some(brief), &own, ME));
        assert!(brief_stale(Some(brief), &own, None));
    }

    #[test]
    fn own_issue_comments_are_not_new_and_old_reads_still_load() {
        let mut read = issue(&[
            "2026-11-01T00:00:00Z",
            "2026-11-05T00:00:00Z",
            "2026-11-06T00:00:00Z",
        ]);
        read.comments[1].mine = true;
        assert_eq!(read.new_after(Some("2026-11-02T00:00:00Z")), 1);
        let seen = HashMap::from([(read.url.clone(), "2026-11-02T00:00:00Z".to_owned())]);
        let thread = &issue_threads(std::slice::from_ref(&read), &seen)[0];
        // newest first: 06 (someone else, new), 05 (mine), 01 (old)
        assert!(thread.comments[0].is_new && !thread.comments[0].mine);
        assert!(thread.comments[1].mine && !thread.comments[1].is_new);
        assert_eq!(thread.comments[0].url.as_deref(), Some(read.url.as_str()));
        // a read saved before `mine` existed
        let old: StoredIssue = serde_json::from_str(
            r#"{"url":"u","title":"t","comments":[{"createdAt":"a","body":"b"}],"unread":0,"readAt":"x"}"#,
        )
        .unwrap();
        assert!(!old.comments[0].mine);
    }

    #[test]
    fn issue_reads_baseline_then_count_new_comments() {
        let store = Store::open_in_memory().unwrap();
        let first = issue(&["2026-11-01T00:00:00Z"]);
        assert_eq!(save_issue_read(&store, 7, &[first], "t1"), 0);
        assert_eq!(new_count(&store, 7, None, ME), 0);
        let second = issue(&["2026-11-01T00:00:00Z", "2026-11-05T00:00:00Z"]);
        assert_eq!(save_issue_read(&store, 7, &[second], "t2"), 1);
        assert_eq!(new_count(&store, 7, None, ME), 1);
        let threads = issue_threads(&stored_issues(&store, 7), &issues_seen(&store, 7));
        assert!(threads[0].comments[0].is_new && !threads[0].comments[1].is_new);
        // OpenProject: no baseline yet, then one newer than seen
        let op = [c(3, "a"), c(4, "b")];
        mark_seen(&store, 7, &op[..1], "t3");
        assert_eq!(new_count(&store, 7, Some(&op), ME), 1);
        mark_seen(&store, 7, &op, "t4");
        assert_eq!(new_count(&store, 7, Some(&op), ME), 0);
        // the user's own reply after that does not show as new
        let replied = [c(3, "a"), c(4, "b"), mine(6, "c")];
        assert_eq!(new_count(&store, 7, Some(&replied), ME), 0);
        // seen never goes back
        raise(&store, 7, SEEN, 2, "t5");
        assert_eq!(seen_id(&store, 7), Some(4));
    }
}
