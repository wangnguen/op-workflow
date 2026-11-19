//! "Bắt đầu làm việc" (Intake, step 1): pick a repo and base, create the branch locally, write
//! the brief. Plain functions over explicit inputs so they can be tested with temp repos;
//! [`crate::AppService`] wires them to config, storage and the live task cache.
//!
//! Never commits, never pushes on its own, never writes to OpenProject. The branch list can
//! delete a branch the user picked and confirmed (local, or `origin/<branch>`).

use std::path::{Path, PathBuf};

use opw_config::{Config, StartMemory, contract_home, expand_home, same_path};
use opw_context::{BriefComment, BriefInput, render_brief, render_comment_section, replace_comment_section};
use opw_domain::task::{
    DEFAULT_BRANCH_TEMPLATE, display_id, infer_branch_template, render_branch_name, template_from_branch,
    validate_branch_name,
};
use opw_domain::view::{
    ApiError, BranchOption, DeleteBranchRequest, RepoOption, RepoState, StartOptions, StartRequest,
    StartResult,
};
use opw_storage::TaskRow;
use opw_tracker::{Comment, WorkPackage, extract_issue_links};
use opw_vcs::{Git, VcsError, discover_repos, is_protected_branch, origin_url, remote_slug, short_branch};

/// How deep to look for repos under each workspace folder (`~/Desktop/<repo>` or
/// `~/Desktop/<group>/<repo>`).
const WORKSPACE_DEPTH: usize = 2;
/// Dirty entries listed in the dialog; the count is always exact.
const MAX_DIRTY_SHOWN: usize = 50;

pub const BRANCH_SAVED: &str = "saved";
pub const BRANCH_PROJECT: &str = "project";
pub const BRANCH_DEFAULT: &str = "default";

/// Dialog data for one task. Scans the workspace folders; no network.
pub fn options(config: &Config, wp: &WorkPackage, saved: Option<&TaskRow>) -> StartOptions {
    let links = extract_issue_links(&wp.description);
    let issue_slugs = crate::task_repo::issue_repos(&wp.description);
    // checkouts where issues of these repos were worked on before (a secondary repo, a fork)
    let paired: Vec<String> = config
        .projects
        .get(&wp.project)
        .into_iter()
        .flat_map(|p| &p.repo_aliases)
        .filter(|(issue, _)| issue_slugs.iter().any(|s| s.eq_ignore_ascii_case(issue.trim())))
        .flat_map(|(_, origins)| origins.iter().map(|o| o.trim().to_lowercase()))
        .collect();
    let remembered: Vec<String> = config
        .projects
        .get(&wp.project)
        .map(|p| p.repos.clone())
        .unwrap_or_default();

    let roots: Vec<PathBuf> = config.workspaces.iter().map(|w| expand_home(w)).collect();
    let mut paths = discover_repos(&roots, WORKSPACE_DEPTH);
    let extra = remembered
        .iter()
        .map(String::as_str)
        .chain(saved.and_then(|s| s.repo.as_deref()))
        .map(expand_home)
        .filter(|p| p.join(".git").exists());
    for p in extra {
        if !paths
            .iter()
            .any(|q| same_path(&q.to_string_lossy(), &p.to_string_lossy()))
        {
            paths.push(p);
        }
    }

    let mut repos: Vec<(usize, RepoOption)> = paths
        .iter()
        .map(|path| {
            let path_text = path.to_string_lossy().into_owned();
            let remote = origin_url(path);
            let slug = remote.as_deref().and_then(remote_slug);
            let suggested = slug
                .as_ref()
                .is_some_and(|s| issue_slugs.contains(s) || paired.contains(s));
            let rank = remembered
                .iter()
                .position(|r| same_path(r, &path_text))
                .unwrap_or(usize::MAX);
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path_text.clone());
            (
                rank,
                RepoOption {
                    path: path_text,
                    name,
                    remote: slug,
                    suggested,
                    remembered: rank != usize::MAX,
                },
            )
        })
        .collect();
    repos.sort_by(|(ra, a), (rb, b)| {
        b.suggested
            .cmp(&a.suggested)
            .then(ra.cmp(rb))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    let repos: Vec<RepoOption> = repos.into_iter().map(|(_, r)| r).collect();

    let saved_repo = saved
        .and_then(|s| s.repo.as_deref())
        .and_then(|r| repos.iter().find(|o| same_path(&o.path, r)));
    let repo = saved_repo
        .or_else(|| repos.iter().find(|r| r.suggested))
        .or_else(|| repos.iter().find(|r| r.remembered))
        .map(|r| r.path.clone());

    let project_template = config.projects.get(&wp.project).and_then(|p| p.branch.clone());
    let (branch, branch_source) = match (saved.and_then(|s| s.branch.clone()), project_template) {
        (Some(b), _) => (b, BRANCH_SAVED),
        (None, Some(t)) => match render_branch_name(&t, wp.id) {
            Ok(b) => (b, BRANCH_PROJECT),
            Err(_) => (default_branch(wp.id), BRANCH_DEFAULT),
        },
        (None, None) => (default_branch(wp.id), BRANCH_DEFAULT),
    };

    StartOptions {
        wp_id: wp.id,
        task_id: display_id(&config.tracker.task_prefix, wp.id),
        title: wp.subject.clone(),
        project: wp.project.clone(),
        nx_status: wp.status.clone(),
        repos,
        repo,
        branch,
        branch_source: branch_source.to_owned(),
        issue_links: links.into_iter().map(|l| l.url).collect(),
        issue_repos: issue_slugs,
        workspaces: config.workspaces.clone(),
    }
}

fn default_branch(wp_id: u64) -> String {
    DEFAULT_BRANCH_TEMPLATE.replace("{taskId}", &wp_id.to_string())
}

/// The brief's "Branch: … (base: …)" line with the branch the task starts on now.
fn with_branch_line(brief: &str, branch: &str, base: &str) -> String {
    let mut done = false;
    let mut out = String::with_capacity(brief.len());
    for line in brief.split_inclusive('\n') {
        if !done && line.starts_with("Branch: ") {
            done = true;
            out.push_str(&format!("Branch: {branch} (base: {base})"));
            out.push_str(if line.ends_with('\n') { "\n" } else { "" });
        } else {
            out.push_str(line);
        }
    }
    out
}

fn with_repo_line(brief: &str, repo: &str) -> String {
    let mut lines: Vec<&str> = brief.split_inclusive('\n').collect();
    let Some(line) = lines.get_mut(1).filter(|line| line.starts_with("Task: ")) else {
        return brief.to_owned();
    };
    let Some((before, _)) = line.rsplit_once(" · Repo: ") else {
        return brief.to_owned();
    };
    let updated = format!(
        "{before} · Repo: {repo}{}",
        if line.ends_with('\n') { "\n" } else { "" }
    );
    *line = &updated;
    lines.concat()
}

fn git_error(e: VcsError) -> ApiError {
    match e {
        VcsError::Dirty(n) => ApiError::new(
            "repo.dirty",
            format!("Working tree có {n} file chưa commit. Hãy tự commit hoặc stash trước."),
        ),
        VcsError::Spawn(e) => ApiError::new("git.missing", format!("Không chạy được git: {e}")),
        VcsError::Git { stderr, .. } => ApiError::new("git", stderr),
        VcsError::NotMerged(_) => ApiError::new(
            "branch.notMerged",
            "Nhánh còn commit chưa merge vào đâu. Bấm “Xoá hẳn” nếu chắc chắn bỏ các commit đó.",
        ),
        VcsError::Refused(why) => ApiError::new("branch.protected", format!("Không xoá: {why}")),
    }
}

/// Open `path` as a repo: the toplevel of the working tree that contains it.
fn open_repo(path: &str) -> Result<(Git, PathBuf), ApiError> {
    let dir = expand_home(path.trim());
    if !dir.is_dir() {
        return Err(ApiError::new(
            "repo.missing",
            format!("Không có thư mục {}", dir.display()),
        ));
    }
    let top = Git::new(&dir).toplevel().map_err(|e| match e {
        VcsError::Git { .. } => {
            ApiError::new("repo.notGit", format!("{} không phải repo git", dir.display()))
        }
        other => git_error(other),
    })?;
    // git prints C:/x/y on Windows; keep the platform form for display and config
    let top = PathBuf::from(top.to_string_lossy().replace('/', std::path::MAIN_SEPARATOR_STR));
    Ok((Git::new(&top), top))
}

/// Branches (local + `origin/*`, newest first) and working-tree state of one repo.
pub fn repo_state(config: &Config, path: &str, wp_id: Option<u64>) -> Result<RepoState, ApiError> {
    let (git, top) = open_repo(path)?;
    let refs = git.branches().map_err(git_error)?;
    let remote_default = git.remote_default();
    let branches: Vec<BranchOption> = refs
        .into_iter()
        .map(|b| {
            let track = b.track.trim_matches(['[', ']']).trim().to_owned();
            BranchOption {
                protected: is_protected_branch(&b.name, remote_default.as_deref()),
                name: b.name,
                remote: b.remote,
                committed_at: (!b.committed_at.is_empty()).then_some(b.committed_at),
                track: (!track.is_empty()).then_some(track),
            }
        })
        .collect();
    let dirty = git.dirty_entries().map_err(git_error)?;
    let top_text = top.to_string_lossy().into_owned();
    let last_base = config
        .repos
        .iter()
        .find(|(k, _)| same_path(k, &top_text))
        .and_then(|(_, r)| r.last_base.clone())
        .filter(|b| branches.iter().any(|o| &o.name == b));
    let suggested_branch = wp_id.and_then(|id| {
        let template = infer_branch_template(branches.iter().map(|b| b.name.as_str()))?;
        render_branch_name(&template, id).ok()
    });
    Ok(RepoState {
        name: top
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| top_text.clone()),
        path: top_text,
        branches,
        current: git.current_branch().ok().filter(|b| !b.is_empty()),
        dirty_count: dirty.len(),
        dirty: dirty.into_iter().take(MAX_DIRTY_SHOWN).collect(),
        last_base,
        suggested_branch,
    })
}

/// Delete a branch the user picked and confirmed: local (`git branch -d`, or `-D` when
/// `force`), or `origin/<branch>` (`git push origin --delete`). Main branches, the checked-out
/// branch and the branch of a task in "Đang làm" are refused. Returns the fresh repo state.
pub fn delete_branch(
    config: &Config,
    active: &[TaskRow],
    req: &DeleteBranchRequest,
) -> Result<RepoState, ApiError> {
    let (git, top) = open_repo(&req.repo)?;
    let name = short_branch(req.branch.trim());
    validate_branch_name(name).map_err(|e| ApiError::new("branch.invalid", e.to_string()))?;
    if !req.remote {
        let key = contract_home(&top);
        let owner = active.iter().find(|t| {
            t.branch.as_deref() == Some(name) && t.repo.as_deref().is_some_and(|r| same_path(r, &key))
        });
        if let Some(t) = owner {
            return Err(ApiError::new(
                "branch.inUse",
                format!(
                    "Đây là nhánh của task {} đang làm. Tạm dừng task trước rồi xoá.",
                    t.display_id
                ),
            ));
        }
        git.delete_local_branch(name, req.force).map_err(git_error)?;
    } else {
        git.delete_remote_branch(name).map_err(git_error)?;
    }
    repo_state(config, &top.to_string_lossy(), req.wp_id)
}

/// `git fetch --prune origin` (reads the remote, never writes), then the fresh state.
pub fn fetch(config: &Config, path: &str, wp_id: Option<u64>) -> Result<RepoState, ApiError> {
    let (git, top) = open_repo(path)?;
    if origin_url(&top).is_none() && git.run(&["remote", "get-url", "origin"]).is_err() {
        return Err(ApiError::new("repo.noOrigin", "Repo không có remote origin"));
    }
    git.fetch().map_err(git_error)?;
    repo_state(config, &top.to_string_lossy(), wp_id)
}

/// Where the brief of a task lives: `<tasks dir>/<task id>/brief.md`.
pub fn brief_path(tasks_dir: &Path, task_id: &str) -> PathBuf {
    tasks_dir.join(task_id).join("brief.md")
}

/// Everything `start` needs besides the request.
pub struct StartInput<'a> {
    pub config: &'a Config,
    pub config_path: Option<&'a Path>,
    /// Folder of the task folders (`tasksDir`, or `<data dir>/tasks`).
    pub tasks_dir: Option<&'a Path>,
    pub wp: &'a WorkPackage,
    /// Fresh description + comments from OpenProject, or why they could not be read.
    pub details: Result<(WorkPackage, Vec<Comment>), String>,
    pub task_url: String,
    pub now: String,
}

/// Create (or switch to) the branch, remember the choices, write the brief. Returns the result
/// and the row to store as the active task.
pub(crate) fn start(
    input: StartInput<'_>,
    req: &StartRequest,
) -> Result<(StartResult, TaskRow, crate::task_repo::Binding), ApiError> {
    let StartInput {
        config,
        config_path,
        tasks_dir,
        wp,
        details,
        task_url,
        now,
    } = input;
    let branch = req.branch.trim();
    validate_branch_name(branch).map_err(|e| ApiError::new("branch.invalid", e.to_string()))?;
    let base = req.base.trim();
    if base.is_empty() {
        return Err(ApiError::new("base.empty", "Chưa chọn base"));
    }
    let (git, top) = open_repo(&req.repo)?;
    let source = details.as_ref().map_or(wp, |(fresh, _)| fresh);
    // any checkout may start the task: Phân tích decides whether it is the right place
    let binding = crate::task_repo::Binding::read(&git, &source.description)?;
    let created = git.start_branch(branch, base).map_err(git_error)?;

    let mut warnings = Vec::new();
    let repo_key = contract_home(&top);
    let template = template_from_branch(branch, wp.id);
    if let Some(path) = config_path {
        let memory = StartMemory {
            project: &source.project,
            repo: &repo_key,
            branch_template: template.as_deref(),
            base,
        };
        if let Err(e) = opw_config::remember_start(path, &memory) {
            warnings.push(format!("Không lưu được lựa chọn vào config: {e}"));
        }
    }

    let task_id = display_id(&config.tracker.task_prefix, wp.id);
    let repo_name = top
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| repo_key.clone());
    let comments_read = details.is_ok();
    let (fresh, comments) = match details {
        Ok((fresh, comments)) => (fresh, comments),
        Err(e) => {
            warnings.push(format!(
                "Brief chỉ có mô tả đã lưu, chưa có comment (không đọc được OpenProject: {e})"
            ));
            (wp.clone(), Vec::new())
        }
    };
    let brief_comments: Vec<BriefComment> = comments
        .iter()
        .map(|c| BriefComment {
            created_at: c.created_at.clone(),
            text: c.raw.clone(),
        })
        .collect();
    let issue_links: Vec<String> = extract_issue_links(&fresh.description)
        .into_iter()
        .chain(comments.iter().flat_map(|c| extract_issue_links(&c.raw)))
        .map(|l| l.url)
        .fold(Vec::new(), |mut acc, url| {
            if !acc.contains(&url) {
                acc.push(url);
            }
            acc
        });
    let github_issue_count = u32::try_from(
        issue_links
            .iter()
            .filter(|u| u.starts_with("https://github.com/"))
            .count()
            .min(crate::issues::MAX_ISSUES),
    )
    .unwrap_or(0);
    let brief = render_brief(&BriefInput {
        task_id: &task_id,
        title: &fresh.subject,
        task_url: &task_url,
        repo: &repo_name,
        branch,
        base,
        description: &fresh.description,
        comments: &brief_comments,
        issue_links: &issue_links,
        redact_terms: &[],
    });
    let brief_file = tasks_dir.map(|d| brief_path(d, &task_id));
    let brief_path = match &brief_file {
        Some(file) => {
            // A task started before keeps its brief (your edits, issues read with "Lấy issue");
            // only the branch line and, when OpenProject answered, the comments are refreshed.
            let text = match std::fs::read_to_string(file) {
                Ok(old) => {
                    let kept = with_repo_line(&with_branch_line(&old, branch, base), &repo_name);
                    if comments_read {
                        replace_comment_section(&kept, &render_comment_section(&brief_comments, &[]))
                    } else {
                        kept
                    }
                }
                Err(_) => brief,
            };
            let written = file
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::write(file, &text));
            match written {
                Ok(()) => Some(file.to_string_lossy().into_owned()),
                Err(e) => {
                    warnings.push(format!("Không ghi được brief: {e}"));
                    None
                }
            }
        }
        None => None,
    };

    let row = TaskRow {
        wp_id: wp.id,
        display_id: task_id.clone(),
        title: fresh.subject.clone(),
        project: fresh.project.clone(),
        repo: Some(repo_key),
        branch: Some(branch.to_owned()),
        base: Some(base.to_owned()),
        column: crate::COLUMN_ACTIVE.into(),
        updated_at: now,
    };
    Ok((
        StartResult {
            task_id,
            branch: branch.to_owned(),
            created,
            brief_path,
            warnings,
            issue_links: github_issue_count,
        },
        row,
        binding,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git_ok(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// `<root>/web` (origin github.com/acme/web, branches f/WP-1001, f/WP-1002) and `<root>/api`.
    /// `root.path()` is the long path form git prints (Windows %TEMP% is often an 8.3 name).
    struct Ws {
        _dir: tempfile::TempDir,
        root: PathBuf,
    }

    impl Ws {
        fn path(&self) -> &Path {
            &self.root
        }
    }

    fn workspace() -> Ws {
        let dir = tempfile::tempdir().unwrap();
        for name in ["web", "api"] {
            let p = dir.path().join(name);
            std::fs::create_dir_all(&p).unwrap();
            git_ok(&p, &["init", "-q", "-b", "development"]);
            git_ok(&p, &["config", "user.email", "test@example.invalid"]);
            git_ok(&p, &["config", "user.name", "test"]);
            git_ok(&p, &["config", "commit.gpgsign", "false"]);
            std::fs::write(p.join("a.txt"), name).unwrap();
            git_ok(&p, &["add", "a.txt"]);
            git_ok(&p, &["commit", "-q", "-m", "init"]);
        }
        let web = dir.path().join("web");
        git_ok(&web, &["remote", "add", "origin", "git@github.com:acme/web.git"]);
        git_ok(&web, &["branch", "f/WP-1001"]);
        git_ok(&web, &["branch", "f/WP-1002"]);
        let (_, top) = open_repo(&web.to_string_lossy()).unwrap();
        let root = top.parent().unwrap().to_path_buf();
        Ws { _dir: dir, root }
    }

    fn wp(id: u64, description: &str) -> WorkPackage {
        WorkPackage {
            id,
            subject: "Không lưu được đơn".into(),
            lock_version: 1,
            status: "Open".into(),
            project: "Web Platform".into(),
            description: description.into(),
            updated_at: "2026-11-13T08:00:00Z".into(),
            priority: "High".into(),
        }
    }

    fn config_for(root: &Path) -> Config {
        Config {
            workspaces: vec![root.to_string_lossy().into_owned()],
            ..Config::default()
        }
    }

    #[test]
    fn delete_branch_keeps_main_and_in_progress_branches() {
        let root = workspace();
        let cfg = config_for(root.path());
        let web = root.path().join("web");
        let repo = web.to_string_lossy().into_owned();
        let req = |branch: &str, force: bool| DeleteBranchRequest {
            repo: repo.clone(),
            branch: branch.into(),
            remote: false,
            force,
            wp_id: None,
        };
        let (_, top) = open_repo(&repo).unwrap();
        let active = vec![TaskRow {
            wp_id: 1001,
            display_id: "WP-1001".into(),
            title: "t".into(),
            project: "p".into(),
            repo: Some(contract_home(&top)),
            branch: Some("f/WP-1001".into()),
            base: Some("development".into()),
            column: "active".into(),
            updated_at: "2026-11-13T00:00:00Z".into(),
        }];

        let err = delete_branch(&cfg, &active, &req("f/WP-1001", true)).unwrap_err();
        assert_eq!(err.code, "branch.inUse");
        assert_eq!(
            delete_branch(&cfg, &active, &req("development", true))
                .unwrap_err()
                .code,
            "branch.protected"
        );
        let state = delete_branch(&cfg, &active, &req("f/WP-1002", false)).unwrap();
        assert!(!state.branches.iter().any(|b| b.name == "f/WP-1002"));
        assert!(
            state
                .branches
                .iter()
                .any(|b| b.name == "development" && b.protected)
        );
        assert!(
            state
                .branches
                .iter()
                .any(|b| b.name == "f/WP-1001" && !b.protected)
        );
    }

    #[test]
    fn options_suggest_the_repo_from_the_issue_link() {
        let root = workspace();
        let cfg = config_for(root.path());
        let task = wp(188001, "Xem https://github.com/Acme/web/issues/12");
        let o = options(&cfg, &task, None);
        let names: Vec<_> = o.repos.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["web", "api"]);
        assert!(o.repos[0].suggested && !o.repos[1].suggested);
        assert!(o.repo.as_deref().is_some_and(|p| p.ends_with("web")));
        assert_eq!(o.task_id, "WP-188001");
        assert_eq!(o.branch, "feature/188001");
        assert_eq!(o.branch_source, BRANCH_DEFAULT);
        assert_eq!(o.issue_links, vec!["https://github.com/Acme/web/issues/12"]);

        // no link: nothing pre-selected, alphabetical
        let o = options(&cfg, &wp(188001, ""), None);
        assert_eq!(o.repo, None);
        assert_eq!(o.repos[0].name, "api");
    }

    #[test]
    fn repo_state_lists_branches_and_guesses_the_convention() {
        let root = workspace();
        let cfg = config_for(root.path());
        let web = root.path().join("web");
        let s = repo_state(&cfg, &web.to_string_lossy(), Some(188001)).unwrap();
        assert_eq!(s.name, "web");
        assert_eq!(s.branches.len(), 3);
        assert_eq!(s.current.as_deref(), Some("development"));
        assert_eq!(s.dirty_count, 0);
        assert_eq!(s.suggested_branch.as_deref(), Some("f/WP-188001"));
        let err = repo_state(&cfg, &root.path().to_string_lossy(), None).unwrap_err();
        assert_eq!(err.code, "repo.notGit");
        let api = root.path().join("api");
        assert_eq!(
            fetch(&cfg, &api.to_string_lossy(), None).unwrap_err().code,
            "repo.noOrigin"
        );
    }

    #[test]
    fn start_creates_the_branch_remembers_and_writes_the_brief() {
        let root = workspace();
        let data = tempfile::tempdir().unwrap();
        let cfg_path = data.path().join("config.json");
        let cfg = config_for(root.path());
        let web = root.path().join("web");
        let task = wp(188001, "Mô tả https://github.com/acme/web/issues/12");
        let req = StartRequest {
            wp_id: 188001,
            repo: web.to_string_lossy().into_owned(),
            base: "development".into(),
            branch: "f/WP-188001".into(),
            carry_on: None,
        };
        let input = || StartInput {
            config: &cfg,
            config_path: Some(&cfg_path),
            tasks_dir: Some(data.path()),
            wp: &task,
            details: Ok((
                task.clone(),
                vec![Comment {
                    id: 1,
                    created_at: "2026-11-12T01:00:00Z".into(),
                    raw: "Vẫn lỗi".into(),
                    author: None,
                    author_id: None,
                    version: None,
                }],
            )),
            task_url: "https://op.example.com/work_packages/188001".into(),
            now: "2026-11-13T09:00:00Z".into(),
        };
        // a checkout that is not the issue's repo is not refused: Phân tích checks it
        let api = root.path().join("api");
        let other = crate::task_repo::Binding::read(&Git::new(&api), &task.description).unwrap();
        assert!(
            other.needs_check() && other.pairs().is_empty(),
            "no origin, nothing to pair"
        );
        let upstream = crate::task_repo::Binding::read(
            &Git::new(&web),
            "https://github.com/acme/upstream/issues/12 https://github.com/acme/upstream/issues/13",
        )
        .unwrap();
        assert_eq!(upstream.issue_repos, ["github.com/acme/upstream"]);
        assert!(upstream.needs_check());
        let pair = (
            "github.com/acme/upstream".to_owned(),
            "github.com/acme/web".to_owned(),
        );
        assert_eq!(upstream.pairs(), [pair]);
        let mut alias_cfg = cfg.clone();
        alias_cfg
            .projects
            .entry(task.project.clone())
            .or_default()
            .repo_aliases
            .insert(
                "github.com/acme/upstream".into(),
                vec!["github.com/acme/web".into()],
            );
        assert_eq!(
            crate::task_repo::remembered(&alias_cfg, &task.project, &upstream),
            ["github.com/acme/upstream"]
        );
        // a remembered secondary checkout is suggested for the next task of that issue repo
        let next = wp(188009, "https://github.com/acme/upstream/issues/40");
        let opts = options(&alias_cfg, &next, None);
        assert_eq!(opts.issue_repos, ["github.com/acme/upstream"]);
        assert!(opts.repos.iter().any(|r| r.name == "web" && r.suggested));
        let (res, row, _) = start(input(), &req).unwrap();
        assert!(res.created);
        assert!(res.warnings.is_empty(), "{:?}", res.warnings);
        assert_eq!(Git::new(&web).current_branch().unwrap(), "f/WP-188001");
        assert_eq!(row.column, "active");
        assert_eq!(row.base.as_deref(), Some("development"));
        let brief = std::fs::read_to_string(res.brief_path.unwrap()).unwrap();
        assert!(brief.contains("# [WP-188001] Không lưu được đơn"));
        assert!(brief.contains("Vẫn lỗi"));

        let saved = Config::load(&cfg_path).unwrap();
        let project = &saved.projects["Web Platform"];
        assert_eq!(project.branch.as_deref(), Some("f/WP-{taskId}"));
        assert!(same_path(&project.repos[0], &web.to_string_lossy()));
        assert_eq!(
            saved.repos.values().next().unwrap().last_base.as_deref(),
            Some("development")
        );

        // next time: same repo and template come back pre-filled
        let o = options(&saved_with_workspace(&saved, root.path()), &wp(188002, ""), None);
        assert!(
            o.repo
                .as_deref()
                .is_some_and(|p| same_path(p, &web.to_string_lossy()))
        );
        assert_eq!(o.branch, "f/WP-188002");
        assert_eq!(o.branch_source, BRANCH_PROJECT);

        // starting again only switches, and keeps what the user added to the brief
        let brief_file = data.path().join("WP-188001").join("brief.md");
        let edited = std::fs::read_to_string(&brief_file).unwrap() + "\n## Ghi chú của tôi\nđừng đổi API\n";
        std::fs::write(&brief_file, edited).unwrap();
        git_ok(&web, &["switch", "development"]);
        let (res, _, _) = start(input(), &req).unwrap();
        assert!(!res.created);
        let again = std::fs::read_to_string(&brief_file).unwrap();
        assert!(again.contains("đừng đổi API"), "{again}");
        assert_eq!(again.matches("Vẫn lỗi").count(), 1, "{again}");

        // dirty tree and bad names are refused before touching git
        std::fs::write(web.join("b.txt"), "x").unwrap();
        let other = StartRequest {
            branch: "f/WP-188003".into(),
            ..req.clone()
        };
        assert_eq!(start(input(), &other).unwrap_err().code, "repo.dirty");
        let bad = StartRequest {
            branch: "f/WP 1".into(),
            ..req
        };
        assert_eq!(start(input(), &bad).unwrap_err().code, "branch.invalid");
    }

    #[test]
    fn starting_again_keeps_the_brief_and_refreshes_comments_and_branch() {
        let brief = "# [WP-1] T\nTask: u · Issue: — · Repo: web\nBranch: f/old (base: main)\n\n## Yêu cầu\nsửa tay\n\n";
        let kept = with_branch_line(brief, "f/new", "development");
        assert!(kept.contains("Branch: f/new (base: development)\n"));
        assert!(!kept.contains("f/old"));
        assert!(kept.contains("sửa tay"));
        // no branch line: left as it is
        assert_eq!(with_branch_line("# x\n", "b", "m"), "# x\n");
    }

    fn saved_with_workspace(saved: &Config, root: &Path) -> Config {
        let mut c = saved.clone();
        c.workspaces = vec![root.to_string_lossy().into_owned()];
        c
    }

    #[test]
    fn brief_falls_back_when_openproject_is_unreachable() {
        let root = workspace();
        let data = tempfile::tempdir().unwrap();
        let cfg = config_for(root.path());
        let task = wp(188004, "Mô tả đã lưu");
        let req = StartRequest {
            wp_id: 188004,
            repo: root.path().join("api").to_string_lossy().into_owned(),
            base: "development".into(),
            branch: "feature/188004".into(),
            carry_on: None,
        };
        let (res, _, _) = start(
            StartInput {
                config: &cfg,
                config_path: None,
                tasks_dir: Some(data.path()),
                wp: &task,
                details: Err("HTTP 503".into()),
                task_url: String::new(),
                now: "2026-11-13T09:00:00Z".into(),
            },
            &req,
        )
        .unwrap();
        assert_eq!(res.warnings.len(), 1);
        let brief = std::fs::read_to_string(res.brief_path.unwrap()).unwrap();
        assert!(brief.contains("Mô tả đã lưu"));
    }
}
