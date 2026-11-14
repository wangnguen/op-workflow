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
                })
                .collect(),
            unread: issue.unread,
            read_at: read_at.to_owned(),
        }
    }

    pub fn newest(&self) -> Option<&str> {
        self.comments.iter().map(|c| c.created_at.as_str()).max()
    }

    /// Comments newer than `seen` (none when nothing was seen yet: the first read is the
    /// baseline).
    pub fn new_after(&self, seen: Option<&str>) -> u32 {
        let Some(seen) = seen else { return 0 };
        count(self.comments.iter().filter(|c| c.created_at.as_str() > seen))
    }
}

fn count<T>(it: impl Iterator<Item = T>) -> u32 {
    u32::try_from(it.count()).unwrap_or(u32::MAX)
}

pub fn max_id(comments: &[Comment]) -> Option<u64> {
    comments.iter().map(|c| c.id).max()
}

/// OpenProject comments newer than the last one the user saw (0 before any baseline).
pub fn op_new(comments: &[Comment], seen: Option<u64>) -> u32 {
    seen.map_or(0, |s| count(comments.iter().filter(|c| c.id > s)))
}

/// Newest first, `is_new` against `seen`.
pub fn op_views(comments: &[Comment], seen: Option<u64>) -> Vec<CommentView> {
    let mut list: Vec<&Comment> = comments.iter().collect();
    list.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(b.id.cmp(&a.id)));
    list.into_iter()
        .map(|c| CommentView {
            id: c.id.to_string(),
            created_at: c.created_at.clone(),
            author: c.author.clone(),
            text: c.raw.clone(),
            is_new: seen.is_some_and(|s| c.id > s),
        })
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
                        is_new: seen.is_some_and(|s| c.created_at.as_str() > s),
                    })
                    .collect(),
                unread: issue.unread,
            }
        })
        .collect()
}

/// OpenProject has a comment newer than the newest one in the brief's comment section.
pub fn brief_stale(brief: Option<&str>, comments: &[Comment]) -> bool {
    let Some(newest) = comments.iter().map(|c| c.created_at.as_str()).max() else {
        return false;
    };
    match brief.and_then(newest_brief_comment) {
        Some(in_brief) => newest > in_brief.as_str(),
        None => brief.is_some(),
    }
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

/// Comments the user has not seen: OpenProject (`comments` = the cached list, if any) plus the
/// stored issues.
pub fn new_count(store: &Store, wp_id: u64, comments: Option<&[Comment]>) -> u32 {
    let op = comments.map_or(0, |c| op_new(c, seen_id(store, wp_id)));
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

    fn c(id: u64, at: &str) -> Comment {
        Comment {
            id,
            created_at: at.into(),
            raw: format!("c{id}"),
            author: Some("A".into()),
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
                })
                .collect(),
            unread: 0,
            read_at: "x".into(),
        }
    }

    #[test]
    fn op_comments_count_and_mark_new_against_the_baseline() {
        let list = [c(5, "2026-11-10T00:00:00Z"), c(9, "2026-11-12T00:00:00Z")];
        assert_eq!(op_new(&list, None), 0);
        assert_eq!(op_new(&list, Some(5)), 1);
        let views = op_views(&list, Some(5));
        assert_eq!(views[0].id, "9");
        assert!(views[0].is_new && !views[1].is_new);
        assert_eq!(views[0].author.as_deref(), Some("A"));
    }

    #[test]
    fn brief_is_stale_when_openproject_has_newer_comments() {
        let brief = "# x\n\n## Comment trên OpenProject\n### 2026-11-10T00:00:00Z\ncũ\n\n## Từ issue\n";
        assert!(!brief_stale(Some(brief), &[c(5, "2026-11-10T00:00:00Z")]));
        assert!(brief_stale(Some(brief), &[c(9, "2026-11-12T00:00:00Z")]));
        assert!(!brief_stale(Some(brief), &[]));
        assert!(!brief_stale(None, &[c(9, "2026-11-12T00:00:00Z")]));
    }

    #[test]
    fn issue_reads_baseline_then_count_new_comments() {
        let store = Store::open_in_memory().unwrap();
        let first = issue(&["2026-11-01T00:00:00Z"]);
        assert_eq!(save_issue_read(&store, 7, &[first], "t1"), 0);
        assert_eq!(new_count(&store, 7, None), 0);
        let second = issue(&["2026-11-01T00:00:00Z", "2026-11-05T00:00:00Z"]);
        assert_eq!(save_issue_read(&store, 7, &[second], "t2"), 1);
        assert_eq!(new_count(&store, 7, None), 1);
        let threads = issue_threads(&stored_issues(&store, 7), &issues_seen(&store, 7));
        assert!(threads[0].comments[0].is_new && !threads[0].comments[1].is_new);
        // OpenProject: no baseline yet, then one newer than seen
        let op = [c(3, "a"), c(4, "b")];
        mark_seen(&store, 7, &op[..1], "t3");
        assert_eq!(new_count(&store, 7, Some(&op)), 1);
        mark_seen(&store, 7, &op, "t4");
        assert_eq!(new_count(&store, 7, Some(&op)), 0);
        // seen never goes back
        raise(&store, 7, SEEN, 2, "t5");
        assert_eq!(seen_id(&store, 7), Some(4));
    }
}
