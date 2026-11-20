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

/// Where the installers put the CLIs, for one installed after the app started (the app keeps
/// the PATH it started with): Claude Code's native installer (`~/.local/bin`), npm's global
/// folder, and the user's PATH as Windows has it now.
fn later_path() -> std::ffi::OsString {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        dirs.push(PathBuf::from(home).join(".local").join("bin"));
    }
    if let Some(appdata) = std::env::var_os("APPDATA") {
        dirs.push(PathBuf::from(appdata).join("npm"));
    }
    dirs.extend(user_path_now());
    std::env::join_paths(dirs).unwrap_or_default()
}

/// The user's PATH as the registry holds it now (`HKCU\Environment`), `%VAR%` expanded.
#[cfg(windows)]
fn user_path_now() -> Vec<PathBuf> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let Ok(out) = std::process::Command::new("reg")
        .args(["query", "HKCU\\Environment", "/v", "Path"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    else {
        return Vec::new();
    };
    parse_reg_path(&String::from_utf8_lossy(&out.stdout))
        .split(';')
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(|d| PathBuf::from(expand_vars(d.trim_matches('"'))))
        .collect()
}

#[cfg(not(windows))]
fn user_path_now() -> Vec<PathBuf> {
    Vec::new()
}

/// The value of `Path` in `reg query` output (`    Path    REG_EXPAND_SZ    C:\a;%X%\b`).
fn parse_reg_path(out: &str) -> String {
    out.lines()
        .find_map(|line| {
            let line = line.trim_start();
            let rest = line.strip_prefix("Path")?.trim_start();
            let value = rest
                .strip_prefix("REG_EXPAND_SZ")
                .or_else(|| rest.strip_prefix("REG_SZ"))?;
            Some(value.trim().to_owned())
        })
        .unwrap_or_default()
}

/// `%NAME%` replaced by the variable's value (left as it is when unset).
fn expand_vars(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) => {
                let name = &after[..end];
                match std::env::var(name) {
                    Ok(v) if !name.is_empty() => out.push_str(&v),
                    _ => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

pub fn find_claude() -> Option<PathBuf> {
    find_in_path(&["claude.exe", "claude"], &path_var())
        .or_else(|| find_in_path(&["claude.exe", "claude"], &later_path()))
}

/// `codex.exe` on PATH, else the native binary next to npm's `codex.cmd` shim; the same in the
/// folders a later install uses.
pub fn find_codex() -> Option<PathBuf> {
    codex_in(&path_var()).or_else(|| codex_in(&later_path()))
}

/// npm on PATH (or where Node puts it), for installing Codex.
pub fn find_npm() -> Option<PathBuf> {
    find_in_path(&["npm.cmd", "npm"], &path_var()).or_else(|| {
        let mut dirs: Vec<PathBuf> = user_path_now();
        if let Some(pf) = std::env::var_os("ProgramFiles") {
            dirs.push(PathBuf::from(pf).join("nodejs"));
        }
        find_in_path(
            &["npm.cmd", "npm"],
            &std::env::join_paths(dirs).unwrap_or_default(),
        )
    })
}

fn codex_in(path: &OsStr) -> Option<PathBuf> {
    if let Some(exe) = find_in_path(&["codex.exe"], path) {
        return Some(exe);
    }
    let shim = find_in_path(&["codex.cmd", "codex"], path)?;
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
    fn the_user_path_comes_from_reg_query_with_variables_expanded() {
        let out = "\r\nHKEY_CURRENT_USER\\Environment\r\n    Path    REG_EXPAND_SZ    C:\\a;%OPW_TEST_NOPE%\\b;\r\n\r\n";
        assert_eq!(parse_reg_path(out), "C:\\a;%OPW_TEST_NOPE%\\b;");
        assert_eq!(parse_reg_path("nothing"), "");
        assert_eq!(expand_vars("%OPW_TEST_NOPE%\\x"), "%OPW_TEST_NOPE%\\x");
        assert_eq!(expand_vars("50%"), "50%");
    }

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
