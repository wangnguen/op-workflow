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
            .raw_arg(format!("/select,\"{}\"", explorer_path(file)))
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
    return Command::new("explorer.exe")
        .arg(explorer_path(dir))
        .spawn()
        .map(drop);
    #[cfg(target_os = "macos")]
    return Command::new("open").arg(dir).spawn().map(drop);
    #[cfg(all(unix, not(target_os = "macos")))]
    return Command::new("xdg-open").arg(dir).spawn().map(drop);
}

/// Open a console window in `dir` running `exe args…`, so the user can continue an AI session
/// there. `args` must be flags and ids the app built (letters, digits, `-`, `_`, `.`); anything
/// else is refused because it goes through `cmd.exe`. `env` is added (the git guard) and the
/// API-key variables are dropped, as for every AI process the app starts.
pub fn open_console(dir: &Path, exe: &Path, args: &[&str], env: &[(String, String)]) -> std::io::Result<()> {
    let plain = |a: &&str| {
        !a.is_empty()
            && a.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    if !args.iter().all(plain) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "tham số không hợp lệ",
        ));
    }
    if !dir.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("{} không còn", dir.display()),
        ));
    }
    #[cfg(windows)]
    let mut cmd = {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
        let mut cmd = Command::new("cmd.exe");
        // cmd /k strips the outer quotes, runs "<exe>" <args> and keeps the window open after
        cmd.raw_arg(format!("/k \"\"{}\" {}\"", exe.display(), args.join(" ")))
            .creation_flags(CREATE_NEW_CONSOLE);
        cmd
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut cmd = Command::new(exe);
        cmd.args(args);
        cmd
    };
    cmd.current_dir(dir);
    for (k, _) in std::env::vars() {
        if opw_agents::env::is_stripped(&k, opw_agents::env::Account::Subscription) {
            cmd.env_remove(k);
        }
    }
    cmd.envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    cmd.spawn().map(drop)
}

/// Explorer misreads a path with `/` in it (it opens Documents instead), so always hand it `\`.
#[cfg(windows)]
fn explorer_path(path: &Path) -> String {
    path.to_string_lossy().replace('/', "\\")
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn explorer_gets_backslashes_only() {
        let mixed = Path::new("C:\\Users\\me\\Desktop/op-workflow-tasks\\WP-1\\brief.md");
        assert_eq!(
            explorer_path(mixed),
            "C:\\Users\\me\\Desktop\\op-workflow-tasks\\WP-1\\brief.md"
        );
    }
}
