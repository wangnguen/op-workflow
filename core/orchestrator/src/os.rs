//! Hand a URL or a file to the operating system (default browser, file manager). Callers only
//! pass URLs and paths the app built itself, never text typed by a user or read from a task.

use std::path::Path;
use std::process::Command;

/// Open an `https://` URL in the default browser.
pub fn open_url(url: &str) -> std::io::Result<()> {
    if !url.starts_with("https://") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "only https URLs are opened",
        ));
    }
    #[cfg(windows)]
    let mut cmd = Command::new("explorer.exe");
    #[cfg(target_os = "macos")]
    let mut cmd = Command::new("open");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = Command::new("xdg-open");
    // explorer.exe exits with 1 even when it worked, so only a failed spawn is an error
    cmd.arg(url).spawn().map(drop)
}

/// Show `file` selected in the file manager (Explorer on Windows).
pub fn reveal(file: &Path) -> std::io::Result<()> {
    if !file.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("{} không còn", file.display()),
        ));
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // explorer wants exactly /select,"<path>"; std's own quoting would wrap the comma too
        Command::new("explorer.exe")
            .raw_arg(format!("/select,\"{}\"", file.display()))
            .spawn()
            .map(drop)
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg("-R").arg(file).spawn().map(drop)
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Command::new("xdg-open")
            .arg(file.parent().unwrap_or(file))
            .spawn()
            .map(drop)
    }
}

/// Open a folder in the file manager.
pub fn open_folder(dir: &Path) -> std::io::Result<()> {
    if !dir.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("{} không còn", dir.display()),
        ));
    }
    #[cfg(windows)]
    let mut cmd = Command::new("explorer.exe");
    #[cfg(target_os = "macos")]
    let mut cmd = Command::new("open");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = Command::new("xdg-open");
    cmd.arg(dir).spawn().map(drop)
}
