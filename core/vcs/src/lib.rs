//! git operations the app is allowed to perform, and the guard for AI processes.
//!
//! Hard rule (PLAN.md principle 5): the app never commits or pushes on its own, and new
//! branches stay local. There is no `commit` and no general `push` here; the one remote write is
//! [`Git::delete_remote_branch`], run only for a branch the user picked and confirmed by typing
//! its name. AI processes keep the push guard either way.

use std::path::{Path, PathBuf};
use std::process::Command;

mod discover;
mod guard;

pub use discover::{discover_repos, origin_url, remote_slug};
pub use guard::{GuardEnv, guard_env, install_guard_hooks};

#[derive(Debug, thiserror::Error)]
pub enum VcsError {
    #[error("cannot run git: {0}")]
    Spawn(std::io::Error),
    #[error("git {args} failed: {stderr}")]
    Git { args: String, stderr: String },
    #[error("working tree has {0} uncommitted change(s)")]
    Dirty(usize),
    /// `git branch -d` refused: the branch has commits that are not merged.
    #[error("branch {0} is not fully merged")]
    NotMerged(String),
    #[error("{0}")]
    Refused(String),
}

/// Branch names a delete button never touches, whatever the remote's default is.
pub const PROTECTED_BRANCHES: [&str; 7] = [
    "main",
    "master",
    "develop",
    "development",
    "production",
    "staging",
    "release",
];

/// `origin/development` → `development`; local names stay as they are.
pub fn short_branch(name: &str) -> &str {
    name.strip_prefix("origin/").unwrap_or(name)
}

/// Main branches (see [`PROTECTED_BRANCHES`]) and the remote's default branch.
pub fn is_protected_branch(name: &str, remote_default: Option<&str>) -> bool {
    let short = short_branch(name);
    PROTECTED_BRANCHES.contains(&short) || remote_default == Some(short)
}

pub type VcsResult<T> = Result<T, VcsError>;

/// `git for-each-ref` format used for the base-branch picker.
pub const BRANCH_FORMAT: &str = "%(refname)|%(refname:short)|%(committerdate:iso-strict)|%(upstream:track)";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchRef {
    /// Short name as git shows it, e.g. `development` or `origin/development`.
    pub name: String,
    pub remote: bool,
    pub committed_at: String,
    /// e.g. `[ahead 2]`, empty when in sync or no upstream.
    pub track: String,
}

/// Parse `git for-each-ref --format=BRANCH_FORMAT` output; drops `origin/HEAD`.
pub fn parse_branches(output: &str) -> Vec<BranchRef> {
    output
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(4, '|');
            let full = parts.next()?.trim();
            let short = parts.next()?.trim();
            let date = parts.next().unwrap_or_default().trim();
            let track = parts.next().unwrap_or_default().trim();
            if short.is_empty() || full.ends_with("/HEAD") {
                return None;
            }
            Some(BranchRef {
                name: short.to_owned(),
                remote: full.starts_with("refs/remotes/"),
                committed_at: date.to_owned(),
                track: track.to_owned(),
            })
        })
        .collect()
}

/// Runs git with an argument array (never through a shell) in one repository.
#[derive(Debug, Clone)]
pub struct Git {
    repo: PathBuf,
    env: Vec<(String, String)>,
}

impl Git {
    pub fn new(repo: impl Into<PathBuf>) -> Self {
        Self {
            repo: repo.into(),
            env: Vec::new(),
        }
    }

    pub fn repo(&self) -> &Path {
        &self.repo
    }

    /// Extra environment for every git call (used by tests to apply the guard).
    pub fn with_env(mut self, env: &[(String, String)]) -> Self {
        self.env.extend_from_slice(env);
        self
    }

    pub fn run(&self, args: &[&str]) -> VcsResult<String> {
        let mut cmd = Command::new("git");
        cmd.arg("-C")
            .arg(&self.repo)
            .args(args)
            // never wait for a username/password on a terminal nobody sees
            .env("GIT_TERMINAL_PROMPT", "0")
            .envs(self.env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        #[cfg(windows)]
        {
            // the app has no console; without this every git call flashes a window
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let out = cmd.output().map_err(VcsError::Spawn)?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(VcsError::Git {
                args: args.join(" "),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
            })
        }
    }

    /// `fetch --prune origin`. Reading from the remote is allowed; writing is not.
    pub fn fetch(&self) -> VcsResult<()> {
        self.run(&["fetch", "--prune", "origin"]).map(drop)
    }

    /// Local and `origin/*` branches, newest commit first.
    pub fn branches(&self) -> VcsResult<Vec<BranchRef>> {
        let format = format!("--format={BRANCH_FORMAT}");
        let out = self.run(&[
            "for-each-ref",
            "--sort=-committerdate",
            &format,
            "refs/heads",
            "refs/remotes/origin",
        ])?;
        Ok(parse_branches(&out))
    }

    /// Root of the working tree that contains `repo` (fails when it is not a git repo).
    pub fn toplevel(&self) -> VcsResult<PathBuf> {
        Ok(PathBuf::from(self.run(&["rev-parse", "--show-toplevel"])?.trim()))
    }

    pub fn has_local_branch(&self, name: &str) -> bool {
        self.run(&["rev-parse", "--verify", "--quiet", &format!("refs/heads/{name}")])
            .is_ok()
    }

    pub fn current_branch(&self) -> VcsResult<String> {
        Ok(self.run(&["branch", "--show-current"])?.trim().to_owned())
    }

    pub fn head(&self) -> VcsResult<String> {
        Ok(self.run(&["rev-parse", "HEAD"])?.trim().to_owned())
    }

    /// Uncommitted entries (`git status --porcelain`).
    pub fn dirty_entries(&self) -> VcsResult<Vec<String>> {
        Ok(self
            .run(&["status", "--porcelain"])?
            .lines()
            .map(str::to_owned)
            .collect())
    }

    /// Default branch of origin (`origin/HEAD` → `development`), when the clone knows it.
    pub fn remote_default(&self) -> Option<String> {
        let out = self
            .run(&["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"])
            .ok()?;
        let name = short_branch(out.trim()).to_owned();
        (!name.is_empty()).then_some(name)
    }

    /// Delete a local branch. `force` (`-D`) also drops commits that are not merged anywhere;
    /// without it git refuses and this returns [`VcsError::NotMerged`]. Never the checked-out
    /// branch, never a main branch.
    pub fn delete_local_branch(&self, name: &str, force: bool) -> VcsResult<()> {
        if is_protected_branch(name, self.remote_default().as_deref()) {
            return Err(VcsError::Refused(format!("{name} là nhánh chính")));
        }
        if self.current_branch().ok().as_deref() == Some(name) {
            return Err(VcsError::Refused(format!("đang đứng ở nhánh {name}")));
        }
        let flag = if force { "-D" } else { "-d" };
        match self.run(&["branch", flag, "--", name]) {
            Err(VcsError::Git { stderr, .. }) if stderr.contains("not fully merged") => {
                Err(VcsError::NotMerged(name.to_owned()))
            }
            other => other.map(drop),
        }
    }

    /// Delete `origin/<name>` (`git push origin --delete refs/heads/<name>`). The only write to
    /// a remote in this crate: call it only for a branch the user picked and confirmed. Repo
    /// hooks (pre-push) still run. Never a main branch.
    pub fn delete_remote_branch(&self, name: &str) -> VcsResult<()> {
        let short = short_branch(name);
        if is_protected_branch(short, self.remote_default().as_deref()) {
            return Err(VcsError::Refused(format!("{short} là nhánh chính")));
        }
        self.run(&["push", "origin", "--delete", &format!("refs/heads/{short}")])
            .map(drop)
    }

    /// Create `name` from `base` locally and switch to it; if it already exists, just switch.
    /// `--no-track` keeps `base` from becoming the upstream, so a later push can never land on it.
    /// Refuses to run on a dirty working tree. Never pushes. Returns `true` when it created the
    /// branch, `false` when it only switched to an existing one.
    pub fn start_branch(&self, name: &str, base: &str) -> VcsResult<bool> {
        let dirty = self.dirty_entries()?;
        if !dirty.is_empty() {
            return Err(VcsError::Dirty(dirty.len()));
        }
        if self.has_local_branch(name) {
            self.run(&["switch", name]).map(|_| false)
        } else {
            self.run(&["switch", "-c", name, "--no-track", base])
                .map(|_| true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git_ok(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Repo with one commit on `development`, identity set locally.
    fn temp_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git_ok(p, &["init", "-q", "-b", "development"]);
        git_ok(p, &["config", "user.email", "test@example.invalid"]);
        git_ok(p, &["config", "user.name", "test"]);
        git_ok(p, &["config", "commit.gpgsign", "false"]);
        std::fs::write(p.join("a.txt"), "a").unwrap();
        git_ok(p, &["add", "a.txt"]);
        git_ok(p, &["commit", "-q", "-m", "init"]);
        dir
    }

    #[test]
    fn start_branch_creates_then_switches_and_refuses_dirty_trees() {
        let dir = temp_repo();
        let git = Git::new(dir.path());
        assert!(git.start_branch("f/WP-1", "development").unwrap());
        assert_eq!(git.current_branch().unwrap(), "f/WP-1");
        git.run(&["switch", "development"]).unwrap();
        assert!(!git.start_branch("f/WP-1", "development").unwrap());
        assert_eq!(git.current_branch().unwrap(), "f/WP-1");
        // no upstream: a later push can never land on the base by accident
        assert!(
            git.run(&["rev-parse", "--abbrev-ref", "f/WP-1@{upstream}"])
                .is_err()
        );

        std::fs::write(dir.path().join("b.txt"), "b").unwrap();
        assert!(matches!(
            git.start_branch("f/WP-2", "development"),
            Err(VcsError::Dirty(1))
        ));
        assert!(!git.has_local_branch("f/WP-2"));
        let names: Vec<_> = git.branches().unwrap().into_iter().map(|b| b.name).collect();
        assert!(names.contains(&"development".to_owned()));
        assert!(names.contains(&"f/WP-1".to_owned()));
    }

    #[test]
    fn deletes_local_and_remote_branches_but_never_main_ones() {
        // a bare "origin" next to a clone, so the remote delete is real but stays in temp
        let origin = temp_repo();
        let bare = tempfile::tempdir().unwrap();
        git_ok(
            bare.path(),
            &["clone", "-q", "--bare", &origin.path().to_string_lossy(), "."],
        );
        let work = tempfile::tempdir().unwrap();
        git_ok(work.path(), &["clone", "-q", &bare.path().to_string_lossy(), "."]);
        git_ok(work.path(), &["config", "user.email", "test@example.invalid"]);
        git_ok(work.path(), &["config", "user.name", "test"]);
        git_ok(work.path(), &["config", "commit.gpgsign", "false"]);
        let git = Git::new(work.path());
        assert_eq!(git.remote_default().as_deref(), Some("development"));

        // merged branch: plain delete works
        git.run(&["branch", "f/WP-1"]).unwrap();
        git.delete_local_branch("f/WP-1", false).unwrap();
        assert!(!git.has_local_branch("f/WP-1"));

        // unmerged branch: refused until forced
        git.run(&["switch", "-q", "-c", "f/WP-2"]).unwrap();
        std::fs::write(work.path().join("b.txt"), "b").unwrap();
        git.run(&["add", "b.txt"]).unwrap();
        git.run(&["commit", "-q", "-m", "wip"]).unwrap();
        assert!(matches!(
            git.delete_local_branch("f/WP-2", false),
            Err(VcsError::Refused(_))
        ));
        git.run(&["switch", "-q", "development"]).unwrap();
        assert!(matches!(
            git.delete_local_branch("f/WP-2", false),
            Err(VcsError::NotMerged(_))
        ));
        git.delete_local_branch("f/WP-2", true).unwrap();
        assert!(!git.has_local_branch("f/WP-2"));

        // main branches are never deleted, here or on the remote
        assert!(matches!(
            git.delete_local_branch("development", true),
            Err(VcsError::Refused(_))
        ));
        assert!(matches!(
            git.delete_remote_branch("origin/main"),
            Err(VcsError::Refused(_))
        ));

        // remote: push a branch into the temp bare repo, then delete it there
        git.run(&["push", "-q", "origin", "development:refs/heads/f/WP-3"])
            .unwrap();
        git.fetch().unwrap();
        assert!(git.branches().unwrap().iter().any(|b| b.name == "origin/f/WP-3"));
        git.delete_remote_branch("origin/f/WP-3").unwrap();
        let in_bare = Git::new(bare.path())
            .run(&["branch", "--list", "f/WP-3"])
            .unwrap();
        assert!(in_bare.trim().is_empty());
        assert!(!git.branches().unwrap().iter().any(|b| b.name == "origin/f/WP-3"));
    }

    #[test]
    fn protected_names_cover_main_branches_and_the_remote_default() {
        assert!(is_protected_branch("origin/main", None));
        assert!(is_protected_branch("development", None));
        assert!(is_protected_branch("origin/trunk", Some("trunk")));
        assert!(!is_protected_branch("f/WP-1", Some("development")));
        assert_eq!(short_branch("origin/f/WP-1"), "f/WP-1");
    }

    #[test]
    fn parses_branch_listing() {
        let out = "refs/heads/feature/187641|feature/187641|2026-11-12T10:00:00+07:00|[ahead 2]\n\
                   refs/remotes/origin/HEAD|origin|2026-11-12T09:00:00+07:00|\n\
                   refs/remotes/origin/development|origin/development|2026-11-13T08:00:00+07:00|\n";
        let b = parse_branches(out);
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].track, "[ahead 2]");
        assert!(!b[0].remote);
        assert!(b[1].remote);
        assert_eq!(b[1].name, "origin/development");
    }
}
