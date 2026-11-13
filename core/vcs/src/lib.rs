//! git operations the app is allowed to perform, and the guard for AI processes.
//!
//! Hard rule (PLAN.md principle 5): the app never commits or pushes, and new branches stay
//! local. There is deliberately no `push` or `commit` function in this crate.

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
