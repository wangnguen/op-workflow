//! The repository a task works in. Bắt đầu never refuses a checkout because its origin differs
//! from the issue links (a secondary repo, a fork, another repo entirely): related repos are
//! talked over in the session's chat. When the folder or origin changes under a started task, the
//! app binds the task to the checkout as it is now and opens a new session there.

use std::path::Path;

use opw_config::same_path;
use opw_domain::view::ApiError;
use opw_tracker::extract_issue_links;
use opw_vcs::{Git, remote_slug};
use serde::{Deserialize, Serialize};

pub(crate) const KEY: &str = "task.repoBinding";

/// The checkout a task was started in, checked again before every step.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Binding {
    root: String,
    remote: Option<String>,
    /// Repos named by the task's issue links when it was started (host/owner/repo).
    #[serde(default)]
    pub issue_repos: Vec<String>,
}

impl Binding {
    /// The checkout as it is now, for a task whose issue links are in `issue_source`.
    pub fn read(git: &Git, issue_source: &str) -> Result<Self, ApiError> {
        let root = git
            .toplevel()
            .map_err(|_| ApiError::new("repo.notGit", "Không đọc được thư mục gốc Git."))?;
        let remote = git
            .run(&["remote", "get-url", "origin"])
            .ok()
            .and_then(|url| remote_slug(&url));
        Ok(Self {
            root: root.to_string_lossy().into_owned(),
            remote,
            issue_repos: issue_repos(issue_source),
        })
    }

    /// The checkout as it is now when its folder or origin is no longer the one the task
    /// started with, and a line saying what changed; `None` when nothing did.
    pub fn moved(&self, repo: &Path) -> Result<Option<(Self, String)>, ApiError> {
        let current = self.rebind(repo)?;
        if same_path(&self.root, &current.root) && self.remote == current.remote {
            return Ok(None);
        }
        let origin = |r: &Option<String>| r.clone().unwrap_or_else(|| "không có origin".into());
        let what = format!(
            "lúc đầu {} · {}, giờ {} · {}",
            self.root,
            origin(&self.remote),
            current.root,
            origin(&current.remote),
        );
        Ok(Some((current, what)))
    }

    /// The same task bound to the checkout as it is now.
    pub fn rebind(&self, repo: &Path) -> Result<Self, ApiError> {
        let mut now = Self::read(&Git::new(repo), "")?;
        now.issue_repos.clone_from(&self.issue_repos);
        Ok(now)
    }
}

/// host/owner/repo of every issue link in `text`, lowercase, in order, without repeats.
pub(crate) fn issue_repos(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for link in extract_issue_links(text) {
        let slug = format!("{}/{}", link.domain, link.repo).to_lowercase();
        if !out.contains(&slug) {
            out.push(slug);
        }
    }
    out
}

/// Backwards compatibility for tasks opened before repository bindings were stored. Read only
/// the generated issue metadata rather than interpreting arbitrary links in issue/comment text.
pub(crate) fn legacy_issues(brief: &str) -> &str {
    brief
        .lines()
        .nth(1)
        .filter(|line| line.starts_with("Task: "))
        .and_then(|line| line.split_once(" · Issue: "))
        .and_then(|(_, rest)| rest.split_once(" · Repo: "))
        .map_or("", |(issues, _)| issues)
}
