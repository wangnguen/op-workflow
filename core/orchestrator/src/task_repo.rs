//! Bind a task to the repository confirmed at intake, before Git or AI write anything.

use std::collections::BTreeSet;
use std::path::Path;

use opw_config::{Config, same_path};
use opw_domain::view::ApiError;
use opw_tracker::extract_issue_links;
use opw_vcs::{Git, remote_slug};
use serde::{Deserialize, Serialize};

pub(crate) const KEY: &str = "task.repoBinding";

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Binding {
    root: String,
    remote: Option<String>,
}

impl Binding {
    fn read(git: &Git) -> Result<Self, ApiError> {
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
        })
    }

    pub fn recheck(&self, repo: &Path) -> Result<(), ApiError> {
        let current = Self::read(&Git::new(repo))?;
        if !same_path(&self.root, &current.root) || self.remote != current.remote {
            return Err(ApiError::new(
                "repo.mismatch",
                "Repo hoặc origin đã thay đổi từ lúc gắn với task. Kiểm tra lại trong Bắt đầu làm việc.",
            ));
        }
        Ok(())
    }
}

pub(crate) fn check(
    config: &Config,
    project: &str,
    issue_source: &str,
    git: &Git,
    confirm_unlinked: bool,
) -> Result<Binding, ApiError> {
    let binding = Binding::read(git)?;
    let project_config = config.projects.get(project);
    let linked: BTreeSet<String> = extract_issue_links(issue_source)
        .into_iter()
        .map(|link| format!("{}/{}", link.domain, link.repo).to_lowercase())
        .collect();
    let expected: BTreeSet<String> = if linked.is_empty() {
        project_config
            .into_iter()
            .flat_map(|p| &p.allowed_remotes)
            .filter_map(|remote| slug(remote))
            .collect()
    } else {
        linked
            .into_iter()
            .flat_map(|repo| {
                let mut candidates: Vec<String> = project_config
                    .into_iter()
                    .flat_map(|p| &p.repo_aliases)
                    .filter(|(issue_repo, _)| slug(issue_repo).as_ref() == Some(&repo))
                    .flat_map(|(_, remotes)| remotes)
                    .filter_map(|remote| slug(remote))
                    .collect();
                if candidates.is_empty() {
                    candidates.push(repo);
                }
                candidates
            })
            .collect()
    };
    if expected.is_empty() {
        return if confirm_unlinked {
            Ok(binding)
        } else {
            Err(ApiError::new(
                "repo.unverified",
                "Task chưa có link issue hoặc repo đã xác nhận. Kiểm tra repo và xác nhận trong Bắt đầu làm việc.",
            ))
        };
    }
    if binding
        .remote
        .as_ref()
        .is_some_and(|remote| expected.contains(remote))
    {
        return if expected.len() == 1 || confirm_unlinked {
            Ok(binding)
        } else {
            Err(ApiError::new(
                "repo.unverified",
                "Task hoặc project có nhiều repo phù hợp. Kiểm tra và xác nhận repo của task này.",
            ))
        };
    }
    Err(ApiError::new(
        "repo.mismatch",
        format!(
            "Repo không khớp task thuộc project {project}. Origin đang chọn là {}; repo dự kiến là {}. Chọn repo đúng hoặc khai báo mirror cho repo của issue trong projects.<project>.repoAliases.",
            binding.remote.as_deref().unwrap_or("không xác định"),
            expected.iter().cloned().collect::<Vec<_>>().join(", "),
        ),
    ))
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
