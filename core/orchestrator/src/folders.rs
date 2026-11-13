//! Where the task folders (`<task id>/brief.md`) live, and moving them when the user picks
//! another place (e.g. the Desktop, so briefs are easy to find).

use std::path::{Path, PathBuf};

/// Name of the folder offered on the Desktop. Not `op-workflow`: that is often the app's repo.
pub const DESKTOP_FOLDER: &str = "op-workflow-tasks";

/// `<Desktop>/op-workflow-tasks` (follows a Desktop moved into OneDrive).
pub fn desktop_suggestion() -> Option<PathBuf> {
    dirs::desktop_dir().map(|d| d.join(DESKTOP_FOLDER))
}

/// Create `dir` and prove the app may write there (Windows "Controlled folder access" or a
/// read-only folder fail here, before anything moves).
pub fn ensure_writable(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("Không tạo được thư mục {}: {e}", dir.display()))?;
    let probe = dir.join(".opw-write-test");
    std::fs::write(&probe, b"ok")
        .and_then(|()| std::fs::remove_file(&probe))
        .map_err(|e| format!("Không ghi được vào {}: {e}", dir.display()))
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Move every task folder of `from` into `to`. A folder already present in `to` is left where
/// it was (never overwritten). Returns how many moved and what stayed behind.
pub fn move_task_folders(from: &Path, to: &Path) -> (u32, Vec<String>) {
    let Ok(entries) = std::fs::read_dir(from) else {
        return (0, Vec::new());
    };
    let mut moved = 0;
    let mut warnings = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let name = entry.file_name();
        let dest = to.join(&name);
        let label = name.to_string_lossy();
        if dest.exists() {
            warnings.push(format!(
                "{label}: chỗ mới đã có thư mục cùng tên, giữ bản ở chỗ cũ"
            ));
            continue;
        }
        let src = entry.path();
        let result = std::fs::rename(&src, &dest).or_else(|_| {
            // another drive: copy, then remove the old one
            copy_dir(&src, &dest).and_then(|()| std::fs::remove_dir_all(&src))
        });
        match result {
            Ok(()) => moved += 1,
            Err(e) => warnings.push(format!("{label}: không chuyển được ({e})")),
        }
    }
    (moved, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_folders_move_and_existing_ones_are_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join("old");
        let new = tmp.path().join("new");
        for id in ["WP-1", "WP-2"] {
            std::fs::create_dir_all(old.join(id)).unwrap();
            std::fs::write(old.join(id).join("brief.md"), id).unwrap();
        }
        std::fs::write(old.join("stray.txt"), "x").unwrap();
        std::fs::create_dir_all(new.join("WP-2")).unwrap();
        ensure_writable(&new).unwrap();

        let (moved, warnings) = move_task_folders(&old, &new);
        assert_eq!(moved, 1);
        assert_eq!(
            std::fs::read_to_string(new.join("WP-1").join("brief.md")).unwrap(),
            "WP-1"
        );
        assert!(!old.join("WP-1").exists());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].starts_with("WP-2"));
        assert!(old.join("WP-2").join("brief.md").exists());
        assert!(!new.join(".opw-write-test").exists());
        // nothing to move from a folder that does not exist
        assert_eq!(move_task_folders(&tmp.path().join("none"), &new), (0, Vec::new()));
    }
}
