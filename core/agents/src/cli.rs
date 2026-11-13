//! Locate the `claude` and `codex` executables.
//!
//! The app starts the real binaries directly instead of npm's `codex.cmd` shim, so no
//! `cmd.exe` sits between the app and the CLI (cleaner process-tree kill, no console flash).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Where npm puts the native Codex binary, relative to the folder that holds `codex.cmd`.
/// Seen with codex-cli 0.169.0 installed by `npm i -g @openai/codex` (PLAN.md §1).
pub const CODEX_NPM_EXE: &str = "node_modules/@openai/codex/node_modules/@openai/codex-win32-x64/vendor/x86_64-pc-windows-msvc/bin/codex.exe";

/// First existing `dir/name` over the entries of a PATH-style value, trying names in order
/// for each directory.
pub fn find_in_path(names: &[&str], path_var: &OsStr) -> Option<PathBuf> {
    std::env::split_paths(path_var).find_map(|dir| {
        names
            .iter()
            .map(|n| dir.join(n))
            .find(|candidate| candidate.is_file())
    })
}

fn path_var() -> std::ffi::OsString {
    std::env::var_os("PATH").unwrap_or_default()
}

pub fn find_claude() -> Option<PathBuf> {
    find_in_path(&["claude.exe", "claude"], &path_var())
}

/// `codex.exe` on PATH, else the native binary next to npm's `codex.cmd` shim.
pub fn find_codex() -> Option<PathBuf> {
    let path = path_var();
    if let Some(exe) = find_in_path(&["codex.exe"], &path) {
        return Some(exe);
    }
    let shim = find_in_path(&["codex.cmd", "codex"], &path)?;
    let native = shim.parent().map(|d| d.join(CODEX_NPM_EXE))?;
    if native.is_file() {
        Some(native)
    } else if is_native_binary(&shim) {
        Some(shim)
    } else {
        None
    }
}

/// On Unix the `codex` found on PATH is the real binary; `.cmd` shims only exist on Windows.
fn is_native_binary(p: &Path) -> bool {
    !cfg!(windows) && p.extension().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_first_match_in_path_order() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        std::fs::write(b.path().join("claude.exe"), b"").unwrap();
        std::fs::write(a.path().join("other.exe"), b"").unwrap();
        let path = std::env::join_paths([a.path(), b.path()]).unwrap();
        assert_eq!(
            find_in_path(&["claude.exe"], &path),
            Some(b.path().join("claude.exe"))
        );
        assert_eq!(find_in_path(&["missing.exe"], &path), None);
    }
}
