//! "Đã push xong": is the task branch on origin? `git fetch origin <branch>`, then the local
//! branch against `origin/<branch>`. Read only: fetching reads the remote, nothing is pushed.

use std::path::Path;

use opw_domain::view::PushCheck;
use opw_storage::Store;
use opw_vcs::Git;

/// Longest fetch error kept.
const ERROR_CHARS: usize = 300;
/// `task_kv` kind of the last check (the "Đã xong" card shows it).
const KV: &str = "push.check";

/// Remember the last check of a task.
pub fn save(store: &Store, wp_id: u64, check: &PushCheck) {
    if let Ok(json) = serde_json::to_string(check) {
        let _ = store.kv_set(wp_id, KV, "", &json, &check.checked_at);
    }
}

/// The last check of a task, if any.
pub fn last(store: &Store, wp_id: u64) -> Option<PushCheck> {
    let json = store.kv_get(wp_id, KV, "").ok().flatten()?;
    serde_json::from_str(&json).ok()
}

fn short(text: &str) -> String {
    let t = text.trim();
    if t.chars().count() <= ERROR_CHARS {
        t.to_owned()
    } else {
        format!("{}…", t.chars().take(ERROR_CHARS).collect::<String>())
    }
}

/// Count of the lines git printed (`rev-list --count` gives one number).
fn number(out: &str) -> u32 {
    out.trim().parse().unwrap_or(0)
}

/// Check `branch` of `repo` against origin. Never fails: what could not be read is in the result.
pub fn check(repo: &Path, branch: &str, now: &str) -> PushCheck {
    let git = Git::new(repo);
    let mut fetch_error = None;
    let mut remote_gone = false;
    if let Err(e) = git.run(&["fetch", "--no-tags", "origin", branch]) {
        let msg = e.to_string();
        let lower = msg.to_lowercase();
        if lower.contains("couldn't find remote ref") || lower.contains("could not find remote ref") {
            remote_gone = true;
        } else {
            fetch_error = Some(short(&msg));
        }
    }
    let local = format!("refs/heads/{branch}");
    let tracking = format!("refs/remotes/origin/{branch}");
    let head = git
        .run(&["rev-parse", "--short", "--verify", "--quiet", &local])
        .ok()
        .map(|h| h.trim().to_owned())
        .filter(|h| !h.is_empty());
    let remote = !remote_gone && git.run(&["rev-parse", "--verify", "--quiet", &tracking]).is_ok();
    let (ahead, behind) = match (&head, remote) {
        (Some(_), true) => {
            let range = format!("{local}...{tracking}");
            let out = git
                .run(&["rev-list", "--left-right", "--count", &range])
                .unwrap_or_default();
            let mut parts = out.split_whitespace();
            (
                number(parts.next().unwrap_or("0")),
                number(parts.next().unwrap_or("0")),
            )
        }
        // not on origin: the commits of the branch that no origin branch has
        (Some(_), false) => (
            number(
                &git.run(&["rev-list", "--count", &local, "--not", "--remotes=origin"])
                    .unwrap_or_default(),
            ),
            0,
        ),
        (None, _) => (0, 0),
    };
    let current = git.current_branch().unwrap_or_default();
    let (uncommitted, other_branch) = if current == branch {
        let n = git.dirty_entries().map(|d| d.len()).unwrap_or(0);
        (u32::try_from(n).unwrap_or(u32::MAX), None)
    } else {
        (0, Some(current).filter(|c| !c.is_empty()))
    };
    PushCheck {
        branch: branch.to_owned(),
        pushed: remote && head.is_some() && ahead == 0,
        remote,
        ahead,
        behind,
        uncommitted,
        head,
        fetch_error,
        other_branch,
        checked_at: now.to_owned(),
        forced: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn finds_unpushed_commits_pushed_branches_and_uncommitted_files() {
        if Command::new("git").arg("--version").output().is_err() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        // a local bare repo stands in for origin (the app itself never pushes)
        let origin = tmp.path().join("origin.git");
        std::fs::create_dir_all(&origin).unwrap();
        git(&origin, &["init", "-q", "--bare", "-b", "main"]);
        let repo = tmp.path().join("web");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["remote", "add", "origin", &origin.to_string_lossy()]);
        std::fs::write(repo.join("a.ts"), "1\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "init"]);
        git(&repo, &["push", "-q", "origin", "main"]);
        git(&repo, &["switch", "-q", "-c", "f/WP-1"]);
        std::fs::write(repo.join("a.ts"), "2\n").unwrap();
        git(&repo, &["commit", "-q", "-am", "fix"]);

        let c = check(&repo, "f/WP-1", "now");
        assert!(!c.pushed && !c.remote, "{c:?}");
        assert_eq!((c.ahead, c.uncommitted, c.fetch_error.as_deref()), (1, 0, None));
        assert!(c.head.is_some());

        git(&repo, &["push", "-q", "origin", "f/WP-1"]);
        let c = check(&repo, "f/WP-1", "now");
        assert!(c.pushed && c.remote, "{c:?}");
        assert_eq!((c.ahead, c.behind), (0, 0));

        std::fs::write(repo.join("a.ts"), "3\n").unwrap();
        git(&repo, &["commit", "-q", "-am", "more"]);
        std::fs::write(repo.join("b.ts"), "new\n").unwrap();
        let c = check(&repo, "f/WP-1", "now");
        assert!(!c.pushed && c.remote);
        assert_eq!((c.ahead, c.uncommitted), (1, 1));

        std::fs::remove_file(repo.join("b.ts")).unwrap();
        git(&repo, &["switch", "-q", "main"]);
        let c = check(&repo, "f/WP-1", "now");
        assert_eq!(c.other_branch.as_deref(), Some("main"));
        assert_eq!(c.uncommitted, 0);

        // origin unreachable: the last known state is used and the error is kept
        git(
            &repo,
            &[
                "remote",
                "set-url",
                "origin",
                &tmp.path().join("gone.git").to_string_lossy(),
            ],
        );
        let c = check(&repo, "f/WP-1", "now");
        assert!(c.fetch_error.is_some());
        assert!(c.remote && c.ahead == 1);
        let none = check(&repo, "f/none", "now");
        assert!(!none.pushed && none.head.is_none());
    }
}
