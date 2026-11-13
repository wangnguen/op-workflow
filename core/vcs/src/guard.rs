//! Environment-only guard that stops AI processes from committing or pushing.
//!
//! Applied to every Claude/Codex process the app starts (headless and terminal panes), not to
//! the plain `shell` pane. Nothing is written to the repository's `.git/config`.
//!
//! - `url.no-push://blocked/.pushInsteadOf` for `https://`, `http://`, `ssh://`, `git://`, `git@`,
//!   `file://`: every push URL is rewritten to an unknown scheme, so `git push` fails even with
//!   `--no-verify`; fetch still works.
//! - `core.hooksPath` points at an app-owned directory whose `pre-commit` and `pre-push` refuse.

use std::path::Path;

/// Key/value pairs to add to the child process environment.
pub type GuardEnv = Vec<(String, String)>;

const BLOCKED_PUSH_PREFIXES: [&str; 6] = ["https://", "http://", "ssh://", "git://", "git@", "file://"];
const BLOCKED_URL: &str = "no-push://blocked-by-op-workflow/";

pub fn guard_env(hooks_dir: &Path) -> GuardEnv {
    let mut pairs: Vec<(String, String)> = BLOCKED_PUSH_PREFIXES
        .iter()
        .map(|p| (format!("url.{BLOCKED_URL}.pushInsteadOf"), (*p).to_owned()))
        .collect();
    pairs.push((
        "core.hooksPath".into(),
        hooks_dir.to_string_lossy().replace('\\', "/"),
    ));

    let mut env = vec![("GIT_CONFIG_COUNT".to_owned(), pairs.len().to_string())];
    for (i, (k, v)) in pairs.into_iter().enumerate() {
        env.push((format!("GIT_CONFIG_KEY_{i}"), k));
        env.push((format!("GIT_CONFIG_VALUE_{i}"), v));
    }
    env
}

/// Write the refusing hooks into `dir` (created if missing). Idempotent.
pub fn install_guard_hooks(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for (hook, action) in [("pre-commit", "commit"), ("pre-push", "push")] {
        let script = format!(
            "#!/bin/sh\necho \"op-workflow: AI processes may not {action}. Do it yourself in the shell pane.\" >&2\nexit 1\n"
        );
        let path = dir.join(hook);
        std::fs::write(&path, script)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Git;

    #[test]
    fn env_has_consistent_count() {
        let env = guard_env(Path::new("C:\\x\\hooks"));
        assert_eq!(env[0], ("GIT_CONFIG_COUNT".into(), "7".into()));
        assert_eq!(env.len(), 1 + 7 * 2);
        assert!(env.iter().any(|(_, v)| v == "C:/x/hooks"));
    }

    fn git_available() -> bool {
        std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    /// Same scenario as the manual test on 2026-11-13: push and commit fail, fetch works,
    /// the remote stays untouched.
    #[test]
    fn guard_blocks_commit_and_push_but_not_fetch() {
        if !git_available() {
            eprintln!("git not found; skipping");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let remote = tmp.path().join("remote.git");
        let work = tmp.path().join("work");
        let hooks = tmp.path().join("hooks");
        let ident = ["-c", "user.name=t", "-c", "user.email=t@t"];

        Git::new(tmp.path())
            .run(&["init", "-q", "--bare", remote.to_str().unwrap()])
            .unwrap();
        // a URL remote (like real https/ssh remotes), so pushInsteadOf applies
        let remote_url = format!(
            "file:///{}",
            remote
                .to_string_lossy()
                .replace('\\', "/")
                .trim_start_matches('/')
        );
        Git::new(tmp.path())
            .run(&["clone", "-q", &remote_url, work.to_str().unwrap()])
            .unwrap();
        let plain = Git::new(&work);
        plain
            .run(&[&ident[..], &["commit", "-q", "--allow-empty", "-m", "init"]].concat())
            .unwrap();
        plain.run(&["push", "-q", "origin", "HEAD"]).unwrap();
        let remote_before = Git::new(&remote).run(&["for-each-ref"]).unwrap();

        install_guard_hooks(&hooks).unwrap();
        let guarded = Git::new(&work).with_env(&guard_env(&hooks));

        assert!(
            guarded
                .run(&[&ident[..], &["commit", "--allow-empty", "-m", "x"]].concat())
                .is_err()
        );
        guarded.start_branch("feature/1", "HEAD").unwrap();
        assert_eq!(guarded.current_branch().unwrap(), "feature/1");
        assert!(guarded.run(&["push", "-u", "origin", "feature/1"]).is_err());
        assert!(guarded.run(&["push", "--no-verify", "origin", "HEAD"]).is_err());
        guarded.fetch().unwrap();

        assert_eq!(Git::new(&remote).run(&["for-each-ref"]).unwrap(), remote_before);
    }
}
