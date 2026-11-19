//! The repository a task works in. Bắt đầu never refuses a checkout because its origin differs
//! from the issue links (a secondary repo, a fork, another repo entirely): the AI of Phân tích
//! decides whether it is the right place, and the user can talk it over at the gate. The only
//! hard stop is a folder or origin that changed under a started task.

use std::path::Path;

use opw_config::{Config, same_path};
use opw_domain::view::ApiError;
use opw_tracker::extract_issue_links;
use opw_vcs::{Git, remote_slug};
use serde::{Deserialize, Serialize};

pub(crate) const KEY: &str = "task.repoBinding";

/// The checkout a task was started in, checked again before and after every step.
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

    pub fn remote(&self) -> Option<&str> {
        self.remote.as_deref()
    }

    /// The folder and origin are still the ones the task started with.
    pub fn recheck(&self, repo: &Path) -> Result<(), ApiError> {
        let current = self.rebind(repo)?;
        if same_path(&self.root, &current.root) && self.remote == current.remote {
            return Ok(());
        }
        let origin = |r: &Option<String>| r.clone().unwrap_or_else(|| "không có origin".into());
        Err(ApiError::new(
            "repo.changed",
            format!(
                "Thư mục hoặc origin của repo đã đổi từ lúc bắt đầu task (lúc đầu {} · {}, giờ {} · {}). \
                 Repo hiện tại đúng thì bấm Dùng repo hiện tại; không thì trả repo về như cũ rồi Thử lại.",
                self.root,
                origin(&self.remote),
                current.root,
                origin(&current.remote),
            ),
        ))
    }

    /// The same task bound to the checkout as it is now ("Dùng repo hiện tại").
    pub fn rebind(&self, repo: &Path) -> Result<Self, ApiError> {
        let mut now = Self::read(&Git::new(repo), "")?;
        now.issue_repos.clone_from(&self.issue_repos);
        Ok(now)
    }

    /// The origin is not one of the issue repos (or the task names none): the analysis checks
    /// that this checkout is the right place.
    pub fn needs_check(&self) -> bool {
        self.remote
            .as_ref()
            .is_none_or(|remote| !self.issue_repos.contains(remote))
    }

    /// Pairs to remember once the user approves a plan made in this checkout.
    pub fn pairs(&self) -> Vec<(String, String)> {
        match &self.remote {
            Some(remote) if self.needs_check() => self
                .issue_repos
                .iter()
                .map(|issue| (issue.clone(), remote.clone()))
                .collect(),
            _ => Vec::new(),
        }
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

/// Issue repos the user already worked on in a checkout with this origin (from earlier tasks
/// of the project).
pub(crate) fn remembered(config: &Config, project: &str, b: &Binding) -> Vec<String> {
    let Some(remote) = b.remote() else {
        return Vec::new();
    };
    config
        .projects
        .get(project)
        .into_iter()
        .flat_map(|p| &p.repo_aliases)
        .filter(|(_, origins)| origins.iter().any(|o| slug(o).as_deref() == Some(remote)))
        .filter_map(|(issue, _)| slug(issue))
        .collect()
}

fn slug(entry: &str) -> Option<String> {
    let entry = entry.trim();
    remote_slug(entry).or_else(|| remote_slug(&format!("https://{entry}")))
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
