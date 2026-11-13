//! Finding repos on disk and reading their `origin` remote without running git (fast enough to
//! scan a whole workspace folder each time the "Bắt đầu làm việc" dialog opens).

use std::path::{Path, PathBuf};

/// Stop scanning after this many folders, so a huge workspace cannot freeze the dialog.
const MAX_DIRS_SCANNED: usize = 4000;

/// Folders never worth descending into.
const SKIP_DIRS: &[&str] = &["node_modules", "target", "dist", "build", "vendor", "AppData"];

/// Git repos (folders containing `.git`) directly under each root, or up to `max_depth` levels
/// below it. A repo's own subfolders are not scanned. Sorted by folder name, case-insensitive.
pub fn discover_repos(roots: &[PathBuf], max_depth: usize) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut budget = MAX_DIRS_SCANNED;
    for root in roots {
        if is_repo(root) {
            found.push(root.clone());
            continue;
        }
        scan(root, max_depth, &mut budget, &mut found);
    }
    found.sort_by_key(|p| {
        p.file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    });
    found.dedup();
    found
}

fn is_repo(dir: &Path) -> bool {
    dir.join(".git").exists()
}

fn scan(dir: &Path, depth: usize, budget: &mut usize, found: &mut Vec<PathBuf>) {
    if depth == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if *budget == 0 {
            return;
        }
        *budget -= 1;
        let Ok(kind) = entry.file_type() else { continue };
        if !kind.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name.starts_with('$') || SKIP_DIRS.contains(&name.as_ref()) {
            continue;
        }
        let path = entry.path();
        if is_repo(&path) {
            found.push(path);
        } else {
            scan(&path, depth - 1, budget, found);
        }
    }
}

/// `remote.origin.url` read straight from `.git/config`. `None` when there is no origin, or when
/// `.git` is a file (worktree, submodule); callers then show no remote.
pub fn origin_url(repo: &Path) -> Option<String> {
    let text = std::fs::read_to_string(repo.join(".git").join("config")).ok()?;
    let mut in_origin = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_origin = line.replace(char::is_whitespace, "") == "[remote\"origin\"]";
            continue;
        }
        if !in_origin {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            if key.trim().eq_ignore_ascii_case("url") {
                return Some(value.trim().to_owned());
            }
        }
    }
    None
}

/// `host/path` of a remote URL, lower-case, without `.git`, credentials or port, so it can be
/// compared with an issue link (`github.com/acme/web`). Handles https, ssh:// and scp-like
/// `git@host:path` forms.
pub fn remote_slug(url: &str) -> Option<String> {
    let url = url.trim();
    let (host, path) = if let Some((_, rest)) = url.split_once("://") {
        let (authority, path) = rest.split_once('/')?;
        let host = authority.rsplit('@').next()?;
        (host.split(':').next()?, path)
    } else {
        // scp-like: [user@]host:path (a Windows path like C:/x has a one-letter "host")
        let (authority, path) = url.split_once(':')?;
        let host = authority.rsplit('@').next()?;
        if authority.contains(['/', '\\']) || host.len() < 2 {
            return None;
        }
        (host, path)
    };
    let path = path.trim_matches('/').trim_end_matches(".git").trim_matches('/');
    if host.is_empty() || path.is_empty() {
        return None;
    }
    Some(format!("{}/{}", host.to_lowercase(), path.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_of_common_remote_forms() {
        for (url, want) in [
            ("https://github.com/Acme/Web.git", "github.com/acme/web"),
            ("https://user:tok@github.com/acme/web/", "github.com/acme/web"),
            ("git@github.com:acme/web.git", "github.com/acme/web"),
            (
                "ssh://git@gitlab.example.com:2222/group/sub/project.git",
                "gitlab.example.com/group/sub/project",
            ),
        ] {
            assert_eq!(remote_slug(url).as_deref(), Some(want), "{url}");
        }
        assert_eq!(remote_slug("C:/local/path"), None);
        assert_eq!(remote_slug(""), None);
    }

    #[test]
    fn discovers_repos_and_reads_origin() {
        let root = tempfile::tempdir().unwrap();
        let mk = |rel: &str| std::fs::create_dir_all(root.path().join(rel)).unwrap();
        mk("web/.git");
        mk("group/api/.git");
        mk("group/deeper/too/.git");
        mk("node_modules/pkg/.git");
        mk("notes");
        std::fs::write(
            root.path().join("web/.git/config"),
            "[core]\n\tbare = false\n[remote \"upstream\"]\n\turl = https://github.com/other/web\n\
             [remote \"origin\"]\n\turl = git@github.com:acme/web.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n",
        )
        .unwrap();
        let repos = discover_repos(&[root.path().to_path_buf()], 2);
        let names: Vec<_> = repos
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["api", "web"]);
        assert_eq!(
            origin_url(&root.path().join("web")).as_deref(),
            Some("git@github.com:acme/web.git")
        );
        assert_eq!(origin_url(&root.path().join("group/api")), None);
    }
}
