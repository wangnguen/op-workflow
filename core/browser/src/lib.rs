//! Read pages through the user's own running Chrome, read-only.
//!
//! The user turns on remote debugging once at `chrome://inspect/#remote-debugging` (Chrome 144+).
//! Chrome then writes `DevToolsActivePort` (port + websocket path) in its data folder and asks
//! the user to **Allow** every time a program connects. We open a background tab, load one page,
//! run one read-only script, close the tab. No clicks, no typing, no cookies read.
//!
//! The old `--remote-debugging-port` switch is ignored for the default profile since Chrome 136,
//! which is why we connect to the browser the user already runs instead of starting one.

mod cdp;
mod github;

use std::path::PathBuf;
use std::time::SystemTime;

pub use github::{IssueComment, IssueError, IssuePage, IssueRead, read_github_issues, strip_mentions};

/// Page in Chrome where the user allows remote debugging.
pub const DEBUG_SETTINGS_URL: &str = "chrome://inspect/#remote-debugging";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BrowserError {
    #[error("Chrome chưa bật cho phép kết nối: mở chrome://inspect/#remote-debugging và bật lên")]
    NotEnabled,
    #[error("Chrome đang tắt: mở Chrome rồi thử lại")]
    NotRunning,
    #[error("Chrome không cho kết nối: chưa bấm Allow kịp, hoặc đã bấm từ chối")]
    Denied,
    #[error("Lỗi khi nói chuyện với Chrome: {0}")]
    Protocol(String),
}

/// Where a running browser listens, read from its `DevToolsActivePort` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugPort {
    /// "Chrome", "Chrome Beta", "Edge"
    pub browser: &'static str,
    pub port: u16,
    /// `/devtools/browser/<id>`
    pub path: String,
}

/// User data folders of the browsers we know, per OS.
fn data_dirs() -> Vec<(&'static str, PathBuf)> {
    let mut out = Vec::new();
    if cfg!(windows) {
        if let Some(base) = dirs::data_local_dir() {
            out.push(("Chrome", base.join("Google").join("Chrome").join("User Data")));
            out.push((
                "Chrome Beta",
                base.join("Google").join("Chrome Beta").join("User Data"),
            ));
            out.push(("Edge", base.join("Microsoft").join("Edge").join("User Data")));
        }
    } else if cfg!(target_os = "macos") {
        if let Some(base) = dirs::data_dir() {
            out.push(("Chrome", base.join("Google").join("Chrome")));
            out.push(("Chrome Beta", base.join("Google").join("Chrome Beta")));
            out.push(("Edge", base.join("Microsoft Edge")));
        }
    } else if let Some(base) = dirs::config_dir() {
        out.push(("Chrome", base.join("google-chrome")));
        out.push(("Chrome Beta", base.join("google-chrome-beta")));
        out.push(("Edge", base.join("microsoft-edge")));
    }
    out
}

/// `DevToolsActivePort`: first line the port, second line the browser websocket path.
fn parse_port_file(text: &str) -> Option<(u16, String)> {
    let mut lines = text.lines().map(str::trim);
    let port = lines.next()?.parse().ok().filter(|p| *p > 0)?;
    let path = lines.next()?;
    path.starts_with("/devtools/browser/")
        .then(|| (port, path.to_owned()))
}

/// The browser with remote debugging on; the most recently started one when several are.
pub fn find_debug_port() -> Option<DebugPort> {
    data_dirs()
        .into_iter()
        .filter_map(|(browser, dir)| {
            let file = dir.join("DevToolsActivePort");
            let modified = std::fs::metadata(&file)
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            let (port, path) = parse_port_file(&std::fs::read_to_string(&file).ok()?)?;
            Some((modified, DebugPort { browser, port, path }))
        })
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, port)| port)
}

/// Chrome's own executable, for opening [`DEBUG_SETTINGS_URL`] (a `chrome://` page cannot be
/// opened through the default-browser handler) and the browser test's own window.
pub fn chrome_exe() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        for var in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
            if let Some(base) = std::env::var_os(var) {
                candidates.push(
                    PathBuf::from(base)
                        .join("Google")
                        .join("Chrome")
                        .join("Application")
                        .join("chrome.exe"),
                );
            }
        }
    } else if cfg!(target_os = "macos") {
        candidates.push(PathBuf::from(
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        ));
    } else {
        candidates.push(PathBuf::from("/usr/bin/google-chrome"));
    }
    candidates.into_iter().find(|p| p.is_file())
}

/// Open `chrome://inspect/#remote-debugging` in Chrome (a new tab of the running window).
pub fn open_debug_settings() -> Result<(), String> {
    let exe = chrome_exe().ok_or("Không tìm thấy Chrome trên máy")?;
    std::process::Command::new(exe)
        .arg(DEBUG_SETTINGS_URL)
        .spawn()
        .map(drop)
        .map_err(|e| format!("Không mở được Chrome: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_file_needs_a_port_and_a_browser_path() {
        assert_eq!(
            parse_port_file("9222\n/devtools/browser/abc-123\n"),
            Some((9222, "/devtools/browser/abc-123".into()))
        );
        assert_eq!(
            parse_port_file("9222\r\n/devtools/browser/x\r\n"),
            Some((9222, "/devtools/browser/x".into()))
        );
        assert_eq!(parse_port_file("9222\n"), None);
        assert_eq!(parse_port_file("0\n/devtools/browser/x"), None);
        assert_eq!(parse_port_file("abc\n/devtools/browser/x"), None);
        assert_eq!(parse_port_file("9222\n/devtools/page/x"), None);
    }
}
