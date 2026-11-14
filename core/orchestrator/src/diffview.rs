//! The Diff tab: the task's change from the fork point of its base, read with git (read-only),
//! plus "N file đổi từ lần xem trước" from the per-file fingerprints saved when the user last
//! opened the tab (`task_kv` kinds `diff.seen` and `diff.looked`).

use std::collections::HashMap;

use opw_domain::view::{
    ApiError, DiffChange, DiffFileView, DiffHunkView, DiffLineKind, DiffLineView, DiffView,
};
use opw_storage::{Store, TaskRow};
use opw_vcs::{FileChange, FileDiff, Git, LineKind, TaskDiff};

pub const SEEN: &str = "diff.seen";
pub const LOOKED: &str = "diff.looked";

/// What git should compare, and the note to show when it is not the working tree.
pub struct Target {
    pub repo: std::path::PathBuf,
    pub base: String,
    /// `None` = the working tree (the repo is on the task's branch).
    pub tip: Option<String>,
    pub note: Option<String>,
}

/// Where to diff for this task: its repo and base; the working tree when the repo is on the
/// task's branch, otherwise the branch's commits only.
pub fn target(row: &TaskRow) -> Result<Target, ApiError> {
    let repo = row
        .repo
        .as_deref()
        .map(opw_config::expand_home)
        .ok_or_else(|| ApiError::new("diff.noRepo", "Task chưa có repo: bấm Bắt đầu làm việc trước"))?;
    let base = row
        .base
        .clone()
        .ok_or_else(|| ApiError::new("diff.noBase", "Task chưa có nhánh gốc để so"))?;
    let branch = row.branch.clone().unwrap_or_default();
    let current = Git::new(&repo)
        .current_branch()
        .map_err(|e| ApiError::new("diff.git", e.to_string()))?;
    if branch.is_empty() || current == branch {
        return Ok(Target {
            repo,
            base,
            tip: None,
            note: None,
        });
    }
    let shown = if current.is_empty() {
        "HEAD rời (detached)".to_owned()
    } else {
        format!("nhánh {current}")
    };
    Ok(Target {
        repo,
        base,
        note: Some(format!(
            "Repo đang ở {shown}, nên chỉ hiện phần đã commit trên {branch} (không có thay đổi chưa commit)."
        )),
        tip: Some(branch),
    })
}

fn change(c: FileChange) -> DiffChange {
    match c {
        FileChange::Added => DiffChange::Added,
        FileChange::Deleted => DiffChange::Deleted,
        FileChange::Modified => DiffChange::Modified,
        FileChange::Renamed => DiffChange::Renamed,
    }
}

fn line_kind(k: LineKind) -> DiffLineKind {
    match k {
        LineKind::Context => DiffLineKind::Context,
        LineKind::Added => DiffLineKind::Added,
        LineKind::Removed => DiffLineKind::Removed,
    }
}

fn file_view(f: FileDiff, changed: bool) -> DiffFileView {
    DiffFileView {
        path: f.path,
        old_path: f.old_path,
        change: change(f.change),
        binary: f.binary,
        untracked: f.untracked,
        added: f.added,
        removed: f.removed,
        truncated: f.truncated,
        changed,
        hunks: f
            .hunks
            .into_iter()
            .map(|h| DiffHunkView {
                section: h.section,
                lines: h
                    .lines
                    .into_iter()
                    .map(|l| DiffLineView {
                        kind: line_kind(l.kind),
                        old: l.old,
                        new: l.new,
                        text: l.text,
                    })
                    .collect(),
            })
            .collect(),
    }
}

/// Build the tab's view; `seen` = fingerprints from the last look (`None` = never looked).
pub fn view(
    diff: TaskDiff,
    base: &str,
    note: Option<String>,
    seen: Option<&HashMap<String, String>>,
) -> DiffView {
    let short: String = diff.fork_point.chars().take(8).collect();
    let mut added = 0u32;
    let mut removed = 0u32;
    let mut changed_count = 0u32;
    let files = diff
        .files
        .into_iter()
        .map(|f| {
            added = added.saturating_add(f.added);
            removed = removed.saturating_add(f.removed);
            let changed = seen.is_some_and(|s| s.get(&f.path) != Some(&f.fingerprint));
            if changed {
                changed_count += 1;
            }
            file_view(f, changed)
        })
        .collect();
    DiffView {
        base: base.to_owned(),
        fork_point: short,
        files,
        added,
        removed,
        changed_since_seen: changed_count,
        seen_before: seen.is_some(),
        note,
        truncated: diff.truncated,
    }
}

/// Read the diff (git; run it off the UI thread) and, when `mark_seen`, remember it as seen.
pub fn task_diff(
    store: Option<&std::sync::Mutex<Store>>,
    row: &TaskRow,
    mark_seen: bool,
    now: &str,
) -> Result<DiffView, ApiError> {
    let t = target(row)?;
    let diff = Git::new(&t.repo)
        .task_diff(&t.base, t.tip.as_deref())
        .map_err(|e| ApiError::new("diff.git", e.to_string()))?;
    let fingerprints: Vec<(String, String)> = diff
        .files
        .iter()
        .map(|f| (f.path.clone(), f.fingerprint.clone()))
        .collect();
    let store = store.and_then(|s| s.lock().ok());
    let seen = store.as_ref().and_then(|s| {
        s.kv_get(row.wp_id, LOOKED, "").ok().flatten()?;
        Some(
            s.kv_all(row.wp_id, SEEN)
                .unwrap_or_default()
                .into_iter()
                .collect::<HashMap<_, _>>(),
        )
    });
    let out = view(diff, &t.base, t.note, seen.as_ref());
    if let (true, Some(s)) = (mark_seen, store.as_ref()) {
        let _ = s.kv_replace(row.wp_id, SEEN, &fingerprints, now);
        let _ = s.kv_set(row.wp_id, LOOKED, "", now, now);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use opw_vcs::diff::{parse_unified, untracked_file};

    #[test]
    fn files_changed_since_the_last_look_are_flagged() {
        let text = "diff --git a/a.ts b/a.ts\n--- a/a.ts\n+++ b/a.ts\n@@ -1 +1 @@\n-x\n+y\n";
        let mut files = parse_unified(text);
        files.push(untracked_file("b.ts", Some("new\n")));
        let fp_a = files[0].fingerprint.clone();
        let diff = TaskDiff {
            fork_point: "0123456789abcdef".into(),
            files,
            truncated: false,
        };
        let first = view(diff.clone(), "origin/main", None, None);
        assert_eq!((first.changed_since_seen, first.seen_before), (0, false));
        assert_eq!(
            (first.added, first.removed, first.fork_point.as_str()),
            (2, 1, "01234567")
        );
        let seen: HashMap<String, String> = [("a.ts".to_owned(), fp_a)].into();
        let again = view(diff, "origin/main", None, Some(&seen));
        assert_eq!(again.changed_since_seen, 1);
        assert!(!again.files[0].changed && again.files[1].changed);
        assert_eq!(again.files[1].hunks[0].lines[0].kind, DiffLineKind::Added);
    }
}
