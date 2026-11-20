//! What sits next to a Workspace session's chat (PLAN.md §12): the repo's files for `@`, the
//! commands for `/`, and the repo's package scripts.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use opw_domain::session::{RepoScript, SlashCommand};
use serde_json::Value;

/// Lower case, forward slashes, no trailing slash: how repo paths are compared.
fn norm(path: &str) -> String {
    path.replace('\\', "/").trim_end_matches('/').to_lowercase()
}

// ---------- files for @ ----------

/// A repo's file list and when git gave it.
type FileList = (Instant, Arc<Vec<String>>);

/// The repo's files (git's list: tracked and untracked, not ignored), kept a little while.
#[derive(Default)]
pub(crate) struct FileLists {
    lists: Mutex<HashMap<String, FileList>>,
}

const FILES_FOR: Duration = Duration::from_secs(20);
const FILES_MAX: usize = 50_000;

impl FileLists {
    pub(crate) fn find(&self, repo: &Path, query: &str, limit: usize) -> Vec<String> {
        let key = norm(&repo.to_string_lossy());
        let cached = self
            .lists
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&key)
            .filter(|(at, _)| at.elapsed() < FILES_FOR)
            .map(|(_, l)| l.clone());
        let list = cached.unwrap_or_else(|| {
            let list = Arc::new(repo_files(repo));
            self.lists
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(key, (Instant::now(), list.clone()));
            list
        });
        match_files(&list, query, limit)
    }
}

fn repo_files(repo: &Path) -> Vec<String> {
    let out = opw_vcs::Git::new(repo).run(&[
        "-c",
        "core.quotepath=false",
        "ls-files",
        "--cached",
        "--others",
        "--exclude-standard",
    ]);
    let mut files: Vec<String> = out
        .map(|o| o.lines().map(str::to_owned).take(FILES_MAX).collect())
        .unwrap_or_default();
    files.sort();
    files.dedup();
    files
}

/// Files whose path holds `query` (any case): name matches first, then shorter paths.
pub(crate) fn match_files(files: &[String], query: &str, limit: usize) -> Vec<String> {
    let q = query.trim().replace('\\', "/").to_lowercase();
    let mut hits: Vec<(u8, usize, &String)> = files
        .iter()
        .filter_map(|f| {
            let lower = f.to_lowercase();
            let name = lower.rsplit('/').next().unwrap_or(&lower);
            let rank = if q.is_empty() {
                2
            } else if name.starts_with(&q) {
                0
            } else if name.contains(&q) {
                1
            } else if lower.contains(&q) {
                2
            } else {
                return None;
            };
            Some((rank, f.len(), f))
        })
        .collect();
    hits.sort();
    hits.into_iter().take(limit).map(|(_, _, f)| f.clone()).collect()
}

// ---------- commands for / ----------

/// Built-in commands that work in a session the app runs, with what they do.
const BUILT_IN: [(&str, &str); 8] = [
    ("compact", "Tóm tắt hội thoại để có lại chỗ trong context"),
    ("context", "Xem context của phiên đang dùng bao nhiêu"),
    ("cost", "Chi phí và token của phiên"),
    ("init", "Viết CLAUDE.md cho repo"),
    ("review", "Review một pull request"),
    ("security-review", "Soát bảo mật các thay đổi đang có"),
    ("pr-comments", "Lấy comment của một pull request"),
    ("todos", "Danh sách việc của phiên"),
];

/// `/` commands: what the running CLI announced (`known`, from its `init`), else the built-in
/// ones; plus the custom commands and skills of the repo and of the user, with their
/// descriptions.
pub(crate) fn slash_commands(repo: &Path, known: &[String], home: Option<&Path>) -> Vec<SlashCommand> {
    let mut out: Vec<SlashCommand> = Vec::new();
    let mut seen = HashSet::new();
    let mut custom = Vec::new();
    let user = home.map(Path::to_path_buf);
    for (dir, source) in [(Some(repo.join(".claude")), "repo"), (user.clone(), "user")] {
        let Some(dir) = dir else { continue };
        command_files(&dir.join("commands"), &dir.join("commands"), source, &mut custom);
        skills(&dir.join("skills"), &mut custom);
    }
    let described: HashMap<String, SlashCommand> =
        custom.iter().map(|c| (c.name.clone(), c.clone())).collect();
    let names: Vec<String> = if known.is_empty() {
        BUILT_IN.iter().map(|(n, _)| (*n).to_owned()).collect()
    } else {
        known.to_vec()
    };
    for name in names {
        if !seen.insert(name.clone()) {
            continue;
        }
        let entry = described.get(&name).cloned().unwrap_or_else(|| SlashCommand {
            description: BUILT_IN
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, d)| (*d).to_owned()),
            source: if name.contains(':') { "plugin" } else { "claude" }.to_owned(),
            name: name.clone(),
        });
        out.push(entry);
    }
    for c in custom {
        if seen.insert(c.name.clone()) {
            out.push(c);
        }
    }
    out
}

/// `.claude/commands/**/*.md`: `a/b.md` is `/a:b`.
fn command_files(root: &Path, dir: &Path, source: &str, out: &mut Vec<SlashCommand>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            command_files(root, &path, source, out);
        } else if path.extension().is_some_and(|e| e == "md") {
            let Ok(rel) = path.with_extension("").strip_prefix(root).map(Path::to_path_buf) else {
                continue;
            };
            let name = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(":");
            out.push(SlashCommand {
                name,
                description: describe(&path),
                source: source.to_owned(),
            });
        }
    }
}

/// `.claude/skills/<name>/SKILL.md`.
fn skills(dir: &Path, out: &mut Vec<SlashCommand>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let file = entry.path().join("SKILL.md");
        if file.is_file() {
            out.push(SlashCommand {
                name: entry.file_name().to_string_lossy().into_owned(),
                description: describe(&file),
                source: "skill".to_owned(),
            });
        }
    }
}

/// The `description:` of the front matter, else the first line of text.
fn describe(file: &Path) -> Option<String> {
    let text = std::fs::read_to_string(file).ok()?;
    let mut lines = text.lines();
    let mut first_text = None;
    if text.starts_with("---") {
        lines.next();
        for line in lines.by_ref() {
            if line.trim() == "---" {
                break;
            }
            if let Some(d) = line.trim().strip_prefix("description:") {
                let d = d.trim().trim_matches(['"', '\'']);
                if !d.is_empty() {
                    return Some(cut(d));
                }
            }
        }
    }
    for line in lines {
        let line = line.trim().trim_start_matches('#').trim();
        if !line.is_empty() {
            first_text = Some(cut(line));
            break;
        }
    }
    first_text
}

fn cut(text: &str) -> String {
    const MAX: usize = 140;
    if text.chars().count() <= MAX {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(MAX).collect();
    out.push('…');
    out
}

// ---------- package scripts ----------

/// The repo's package.json scripts with how to run each (the package manager its lock file
/// or `packageManager` names).
pub(crate) fn scripts(repo: &Path) -> Vec<RepoScript> {
    let Ok(text) = std::fs::read_to_string(repo.join("package.json")) else {
        return Vec::new();
    };
    let Ok(pkg) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    let manager = package_manager(repo, &pkg);
    pkg["scripts"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(name, command)| {
            Some(RepoScript {
                name: name.clone(),
                command: command.as_str()?.to_owned(),
                run: format!("{manager} run {name}"),
            })
        })
        .collect()
}

fn package_manager(repo: &Path, pkg: &Value) -> &'static str {
    let by_lock = [
        ("pnpm-lock.yaml", "pnpm"),
        ("yarn.lock", "yarn"),
        ("bun.lockb", "bun"),
        ("bun.lock", "bun"),
        ("package-lock.json", "npm"),
    ];
    if let Some((_, m)) = by_lock.iter().find(|(f, _)| repo.join(f).is_file()) {
        return m;
    }
    match pkg["packageManager"].as_str().and_then(|p| p.split('@').next()) {
        Some("pnpm") => "pnpm",
        Some("yarn") => "yarn",
        Some("bun") => "bun",
        _ => "npm",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_match_by_name_first() {
        let files: Vec<String> = [
            "src/app/App.tsx",
            "src/apple.ts",
            "docs/app-notes.md",
            "README.md",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        assert_eq!(
            match_files(&files, "app", 10),
            vec!["src/apple.ts", "src/app/App.tsx", "docs/app-notes.md"]
        );
        assert_eq!(match_files(&files, "", 2).len(), 2);
    }

    #[test]
    fn commands_carry_their_description_and_scripts_their_manager() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        std::fs::create_dir_all(repo.join(".claude/commands/git")).unwrap();
        std::fs::write(
            repo.join(".claude/commands/review-local.md"),
            "---\ndescription: Review the local change\n---\nDo it",
        )
        .unwrap();
        std::fs::write(repo.join(".claude/commands/git/sync.md"), "# Sync the branch\n").unwrap();
        let list = slash_commands(repo, &["compact".into(), "review-local".into()], None);
        let find = |n: &str| list.iter().find(|c| c.name == n).cloned();
        assert_eq!(find("compact").unwrap().source, "claude");
        assert_eq!(
            find("review-local").unwrap().description.as_deref(),
            Some("Review the local change")
        );
        assert_eq!(
            find("git:sync").unwrap().description.as_deref(),
            Some("Sync the branch")
        );

        std::fs::write(
            repo.join("package.json"),
            r#"{"scripts":{"dev":"vite","test":"vitest"},"packageManager":"pnpm@9.0.0"}"#,
        )
        .unwrap();
        let s = scripts(repo);
        assert_eq!(
            s.iter().map(|s| s.run.as_str()).collect::<Vec<_>>(),
            vec!["pnpm run dev", "pnpm run test"]
        );
    }
}
