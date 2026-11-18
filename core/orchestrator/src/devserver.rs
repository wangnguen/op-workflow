//! Test trình duyệt (part 6c): the app starts the repo's dev server itself, waits until its URL
//! answers, opens the test's own Chrome window (a profile per repo, visible, never the user's
//! Chrome) and lets the AI drive that window through Chrome DevTools MCP. The window stays open
//! while the AI waits for the user (a login), and both are stopped when the test ends. The
//! command and URL are the user's (confirmed in the Plan tab, remembered per repo in
//! `repos.<repo>.devServer`); the AI only proposes them.

use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use opw_agents::procs;
use opw_domain::view::{DevCommand, DevServer};

/// How long the app waits for the URL to answer after starting the command.
pub const READY_WAIT: Duration = Duration::from_secs(180);
/// At most this many commands for one dev server.
pub const MAX_COMMANDS: usize = 5;
const POLL: Duration = Duration::from_millis(500);
const CONNECT: Duration = Duration::from_millis(400);
/// The log's last bytes shown when the command stops before the URL answers.
const LOG_TAIL: usize = 1500;

/// Starts dev servers and asks whether a URL answers (a fake in tests).
pub trait AppStarter: Send + Sync {
    /// Something accepts connections at the URL's host and port.
    fn is_up(&self, url: &str) -> bool;
    /// Run `command` from `cwd` in the background, its output into `log`.
    fn start(&self, cwd: &Path, command: &str, log: &Path) -> Result<Box<dyn AppProcess>, String>;
    /// Open the test's Chrome window on `url` with its own `profile` folder and a debugging
    /// port; the port comes back for Chrome DevTools MCP (`--browserUrl`).
    fn open_browser(&self, profile: &Path, url: &str) -> Result<(Box<dyn AppProcess>, u16), String>;
}

/// A dev server the app started.
pub trait AppProcess: Send {
    /// `Some(code)` once the command has ended (`None` inside = no exit code).
    fn exited(&mut self) -> Option<Option<i32>>;
    /// Stop it and everything it started; one line on what was stopped, if anything.
    fn stop(self: Box<Self>) -> Option<String>;
}

/// What the user typed, trimmed: folders without leading / trailing slashes, a bare port to wait
/// for written as `http://localhost:<port>`, rows left empty dropped.
pub fn tidy(url: &str, commands: &[DevCommand]) -> DevServer {
    let folder = |d: &str| d.trim().trim_matches(['/', '\\']).to_owned();
    DevServer {
        url: url.trim().to_owned(),
        commands: commands
            .iter()
            .filter(|c| !c.command.trim().is_empty() || !c.wait.trim().is_empty())
            .map(|c| DevCommand {
                command: c.command.trim().to_owned(),
                dir: folder(&c.dir),
                wait: wait_url(&c.wait).unwrap_or_else(|| c.wait.trim().to_owned()),
            })
            .collect(),
    }
}

/// What a command is waited for: a URL as written, or a bare port (`3000`, `:3000`) on
/// localhost. `None` = neither.
pub fn wait_url(wait: &str) -> Option<String> {
    let wait = wait.trim();
    let port = wait.strip_prefix(':').unwrap_or(wait);
    if let Ok(port) = port.parse::<u16>() {
        return (port > 0).then(|| format!("http://localhost:{port}"));
    }
    address(wait).map(|_| wait.to_owned())
}

/// Check what the user typed before it is saved or run. Empty URL = no dev server. A command
/// may leave Chờ empty (= wait for the URL) only when it is the last one: the ones above it
/// must say when they are up, or the next would never start.
pub fn check(server: &DevServer) -> Result<(), String> {
    let url = server.url.trim();
    if url.is_empty() {
        return if server.commands.is_empty() {
            Ok(())
        } else {
            Err("Có lệnh chạy thì cần URL của app để mở trong Chrome".into())
        };
    }
    address(url).ok_or_else(|| format!("URL phải dạng http://localhost:5173 (đang là `{url}`)"))?;
    if server.commands.len() > MAX_COMMANDS {
        return Err(format!("Tối đa {MAX_COMMANDS} lệnh"));
    }
    let last = server.commands.len().saturating_sub(1);
    for (i, c) in server.commands.iter().enumerate() {
        let n = i + 1;
        if c.command.trim().is_empty() {
            return Err(format!("Lệnh {n} chưa có lệnh"));
        }
        let command = c.command.trim();
        if command.contains(['\n', '\r']) || command.chars().count() > 300 {
            return Err(format!("Lệnh {n} phải là một dòng, tối đa 300 ký tự"));
        }
        check_dir(&c.dir).map_err(|e| format!("Lệnh {n}: {e}"))?;
        let wait = c.wait.trim();
        if wait.is_empty() && i != last {
            return Err(format!(
                "Lệnh {n}: ghi cổng chờ (ví dụ 3000); chỉ lệnh cuối được để trống ô Chờ (chờ URL)"
            ));
        }
        if !wait.is_empty() && wait_url(wait).is_none() {
            return Err(format!(
                "Lệnh {n}: ô Chờ là cổng (ví dụ 3000) hoặc URL (http://localhost:3000), đang là `{wait}`"
            ));
        }
    }
    Ok(())
}

fn check_dir(dir: &str) -> Result<(), String> {
    let dir = dir.trim();
    if dir.is_empty() {
        return Ok(());
    }
    let p = Path::new(dir);
    let outside = p.is_absolute()
        || dir.starts_with(['/', '\\'])
        || p.components()
            .any(|c| matches!(c, std::path::Component::ParentDir));
    if outside || dir.contains(':') {
        return Err(format!(
            "Thư mục phải nằm trong repo, viết tương đối (ví dụ `apps/web`), đang là `{dir}`"
        ));
    }
    Ok(())
}

/// `host:port` of an http(s) URL (port 80 / 443 when not written).
pub fn address(url: &str) -> Option<(String, u16)> {
    let (scheme, rest) = url.trim().split_once("://")?;
    let default = match scheme.to_ascii_lowercase().as_str() {
        "http" => 80,
        "https" => 443,
        _ => return None,
    };
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?;
    if authority.is_empty() {
        return None;
    }
    // [::1]:5173
    if let Some(v6) = authority.strip_prefix('[') {
        let (host, tail) = v6.split_once(']')?;
        let port = match tail.strip_prefix(':') {
            Some(p) => p.parse().ok()?,
            None if tail.is_empty() => default,
            None => return None,
        };
        return Some((host.to_owned(), port));
    }
    match authority.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() => Some((host.to_owned(), port.parse().ok()?)),
        Some(_) => None,
        None => Some((authority.to_owned(), default)),
    }
}

/// What [`bring_up`] did.
pub struct Up {
    /// What the app started (all of it, as one): stop it when the step ends. `None` = everything
    /// was running already and is left as it is.
    pub started: Option<Box<dyn AppProcess>>,
    /// One line per command for the step's log.
    pub lines: Vec<String>,
}

/// What the app runs, in order, for the tool line ("api: npm run dev → web: npm run dev"); the
/// URL when there is no command.
pub fn what_runs(server: &DevServer) -> String {
    if server.commands.is_empty() {
        return server.url.trim().to_owned();
    }
    server
        .commands
        .iter()
        .map(|c| match c.dir.trim() {
            "" => c.command.trim().to_owned(),
            d => format!("{d}: {}", c.command.trim()),
        })
        .collect::<Vec<_>>()
        .join(" → ")
}

/// Make sure the app answers at `server.url`: run its commands top to bottom, each one waited for
/// (its Chờ, else the URL); one that already answers is used as it is. No command = the app must
/// be running already. A failure stops what this call started. `Err` = (code, message).
pub fn bring_up(
    starter: &dyn AppStarter,
    repo: &Path,
    server: &DevServer,
    log: &Path,
    wait: Duration,
    stop: &AtomicBool,
) -> Result<Up, Failure> {
    fn fail(started: Vec<Box<dyn AppProcess>>, e: Failure) -> Result<Up, Failure> {
        for p in started.into_iter().rev() {
            p.stop();
        }
        Err(e)
    }
    let url = server.url.trim();
    let in_dir = |dir: &str| match dir.trim() {
        "" => String::new(),
        d => format!(" trong {d}"),
    };
    let many = server.commands.len() > 1;
    let mut started: Vec<Box<dyn AppProcess>> = Vec::new();
    let mut lines = Vec::new();
    for (i, c) in server.commands.iter().enumerate() {
        let target = match c.wait.trim() {
            "" => url.to_owned(),
            w => match wait_url(w) {
                Some(u) => u,
                None => {
                    let e = format!("Lệnh {}: không biết chờ cổng / URL nào (`{w}`)", i + 1);
                    return fail(started, ("devserver.invalid", e));
                }
            },
        };
        let log = if many {
            numbered(log, i + 1)
        } else {
            log.to_path_buf()
        };
        match start_one(starter, repo, &c.command, &c.dir, &target, &log, wait, stop) {
            Ok(Some((p, after))) => {
                lines.push(format!(
                    "Dev server: app chạy `{}`{}, {target} mở sau {} giây",
                    c.command.trim(),
                    in_dir(&c.dir),
                    after.as_secs()
                ));
                started.push(p);
            }
            Ok(None) => lines.push(format!("Dùng {target} đang chạy sẵn (app không tắt nó)")),
            Err(e) => return fail(started, e),
        }
    }
    if server.commands.is_empty() {
        if !starter.is_up(url) {
            return Err((
                "devserver.down",
                format!(
                    "Không mở được {url} và chưa có lệnh chạy app. Bật app rồi bấm Thử lại, hoặc ghi lệnh ở hộp Mở app để test (tab Plan)."
                ),
            ));
        }
        lines.push(format!("Dùng app đang chạy ở {url} (app không tắt nó)"));
    } else if let Err(e) = wait_for(starter, url, wait, stop) {
        // every command answered at its own port, the page itself did not
        return fail(started, e);
    }
    let started = match started.len() {
        0 => None,
        1 => started.pop(),
        _ => Some(Box::new(Group(started)) as Box<dyn AppProcess>),
    };
    Ok(Up { started, lines })
}

/// Wait up to `wait` for the URL once all commands run (it answers at once when the last command
/// waited for it).
fn wait_for(starter: &dyn AppStarter, url: &str, wait: Duration, stop: &AtomicBool) -> Result<(), Failure> {
    let since = Instant::now();
    loop {
        if starter.is_up(url) {
            return Ok(());
        }
        if stop.load(Ordering::Relaxed) {
            return Err((
                "step.stopped",
                "Bạn đã dừng bước này. Bấm Thử lại để chạy lại.".into(),
            ));
        }
        if since.elapsed() >= wait {
            return Err((
                "devserver.timeout",
                format!(
                    "Các lệnh đã chạy nhưng {url} vẫn chưa mở sau {} giây; app đã tắt những gì nó bật. Xem log dev-server trong thư mục browser-test của task rồi sửa lệnh / URL ở tab Plan.",
                    wait.as_secs()
                ),
            ));
        }
        std::thread::sleep(POLL);
    }
}

/// `dev-server.log` → `dev-server-2.log`, when there are several commands.
fn numbered(log: &Path, n: usize) -> PathBuf {
    let stem = log
        .file_stem()
        .map_or_else(|| "dev-server".into(), |s| s.to_string_lossy().into_owned());
    log.with_file_name(format!("{stem}-{n}.log"))
}

/// Several commands the app started for one test, stopped together (last started first).
struct Group(Vec<Box<dyn AppProcess>>);

impl AppProcess for Group {
    fn exited(&mut self) -> Option<Option<i32>> {
        self.0.iter_mut().find_map(|p| p.exited())
    }

    fn stop(self: Box<Self>) -> Option<String> {
        let lines: Vec<String> = self.0.into_iter().rev().filter_map(|p| p.stop()).collect();
        (!lines.is_empty()).then(|| lines.join("; "))
    }
}

/// A step error: (code, message).
type Failure = (&'static str, String);
/// A command the app started, and how long it took to answer.
type Started = (Box<dyn AppProcess>, Duration);

/// One command: `None` when its URL answers already, else started and waited for.
#[allow(clippy::too_many_arguments)]
fn start_one(
    starter: &dyn AppStarter,
    repo: &Path,
    command: &str,
    dir: &str,
    url: &str,
    log: &Path,
    wait: Duration,
    stop: &AtomicBool,
) -> Result<Option<Started>, Failure> {
    if starter.is_up(url) {
        return Ok(None);
    }
    let command = command.trim();
    if command.is_empty() {
        return Err((
            "devserver.down",
            format!(
                "Không mở được {url} và chưa có lệnh chạy app. Bật app rồi bấm Thử lại, hoặc ghi lệnh ở hộp Mở app để test (tab Plan)."
            ),
        ));
    }
    let cwd = match dir.trim() {
        "" => repo.to_path_buf(),
        d => repo.join(d),
    };
    if !cwd.is_dir() {
        return Err((
            "devserver.dir",
            format!("Không thấy thư mục {} để chạy `{command}`", cwd.display()),
        ));
    }
    let started = Instant::now();
    let mut proc = starter
        .start(&cwd, command, log)
        .map_err(|e| ("devserver.start", format!("Không chạy được `{command}`: {e}")))?;
    loop {
        if stop.load(Ordering::Relaxed) {
            proc.stop();
            return Err((
                "step.stopped",
                "Bạn đã dừng bước này. Bấm Thử lại để chạy lại.".into(),
            ));
        }
        if starter.is_up(url) {
            return Ok(Some((proc, started.elapsed())));
        }
        if let Some(code) = proc.exited() {
            let code = code.map_or_else(|| "?".into(), |c| c.to_string());
            return Err((
                "devserver.exited",
                format!(
                    "`{command}` dừng (mã {code}) trước khi {url} mở được. Cuối log ({}):\n{}",
                    log.display(),
                    log_tail(log)
                ),
            ));
        }
        if started.elapsed() >= wait {
            let stopped = proc.stop();
            return Err((
                "devserver.timeout",
                format!(
                    "Đã chạy `{command}` {} giây mà {url} vẫn chưa mở; app đã tắt nó{}. Xem log {} rồi sửa lệnh / URL ở tab Plan, hoặc bấm Thử lại.",
                    wait.as_secs(),
                    stopped.map(|s| format!(" ({s})")).unwrap_or_default(),
                    log.display()
                ),
            ));
        }
        std::thread::sleep(POLL);
    }
}

/// The last part of the dev server's log.
fn log_tail(log: &Path) -> String {
    let text = std::fs::read(log).unwrap_or_default();
    let start = text.len().saturating_sub(LOG_TAIL);
    String::from_utf8_lossy(&text[start..]).trim().to_owned()
}

// ---------- the real thing ----------

/// Starts commands through the system shell (`cmd /c` on Windows, so `npm run dev` finds
/// `npm.cmd`) with no window, and checks URLs with a plain TCP connect.
#[derive(Debug, Default)]
pub struct ShellStarter;

impl AppStarter for ShellStarter {
    fn is_up(&self, url: &str) -> bool {
        let Some((host, port)) = address(url) else {
            return false;
        };
        let addrs: Vec<SocketAddr> = (host.as_str(), port)
            .to_socket_addrs()
            .map(Iterator::collect)
            .unwrap_or_default();
        addrs
            .iter()
            .any(|a| TcpStream::connect_timeout(a, CONNECT).is_ok())
    }

    fn start(&self, cwd: &Path, command: &str, log: &Path) -> Result<Box<dyn AppProcess>, String> {
        if let Some(dir) = log.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let out = std::fs::File::create(log).map_err(|e| e.to_string())?;
        let err = out.try_clone().map_err(|e| e.to_string())?;
        let mut cmd = shell(command);
        cmd.current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::from(out))
            .stderr(Stdio::from(err))
            .env("BROWSER", "none")
            .env("FORCE_COLOR", "0");
        let since = procs::now_ms();
        let child = crate::live::hidden(&mut cmd).spawn().map_err(|e| e.to_string())?;
        let watch = procs::Watch::start(child.id(), since);
        Ok(Box::new(ShellProcess {
            child,
            since,
            watch: Some(watch),
            log: log.to_path_buf(),
        }))
    }

    fn open_browser(&self, profile: &Path, url: &str) -> Result<(Box<dyn AppProcess>, u16), String> {
        Self::launch_chrome(profile, url)
    }
}

/// How long Chrome may take to write its debugging port.
const CHROME_WAIT: Duration = Duration::from_secs(20);

/// The `DevToolsActivePort` file Chrome writes in its profile folder: the port, first line.
fn chrome_port(profile: &Path) -> Option<u16> {
    let text = std::fs::read_to_string(profile.join("DevToolsActivePort")).ok()?;
    text.lines().next()?.trim().parse().ok().filter(|p| *p > 0)
}

impl ShellStarter {
    fn launch_chrome(profile: &Path, url: &str) -> Result<(Box<dyn AppProcess>, u16), String> {
        let exe = opw_browser::chrome_exe().ok_or("Không tìm thấy Chrome trên máy")?;
        std::fs::create_dir_all(profile).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_file(profile.join("DevToolsActivePort"));
        let since = procs::now_ms();
        let mut cmd = Command::new(exe);
        cmd.arg(format!("--user-data-dir={}", profile.display()))
            .args([
                "--remote-debugging-port=0",
                "--no-first-run",
                "--no-default-browser-check",
                "--new-window",
            ])
            .arg(if url.trim().is_empty() {
                "about:blank"
            } else {
                url.trim()
            })
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = cmd.spawn().map_err(|e| format!("không mở được Chrome: {e}"))?;
        let deadline = Instant::now() + CHROME_WAIT;
        loop {
            if let Some(port) = chrome_port(profile) {
                let watch = procs::Watch::start(child.id(), since);
                return Ok((
                    Box::new(ShellProcess {
                        child,
                        since,
                        watch: Some(watch),
                        log: profile.to_path_buf(),
                    }),
                    port,
                ));
            }
            if let Ok(Some(_)) = child.try_wait() {
                return Err(
                    "Chrome thoát ngay khi mở (profile của bước test đang bị một cửa sổ Chrome khác giữ?)"
                        .into(),
                );
            }
            if Instant::now() >= deadline {
                kill_tree(child.id());
                let _ = child.kill();
                return Err(format!(
                    "Chrome không mở cổng debug sau {} giây",
                    CHROME_WAIT.as_secs()
                ));
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

#[cfg(windows)]
fn shell(command: &str) -> Command {
    use std::os::windows::process::CommandExt;
    let mut cmd = Command::new("cmd");
    // `/s /c "<command>"`: cmd keeps the command line as it is, quotes included
    cmd.arg("/d")
        .arg("/s")
        .arg("/c")
        .raw_arg(format!("\"{command}\""));
    cmd
}

#[cfg(not(windows))]
fn shell(command: &str) -> Command {
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(command);
    cmd
}

struct ShellProcess {
    child: Child,
    since: u64,
    watch: Option<procs::Watch>,
    #[allow(dead_code)]
    log: PathBuf,
}

impl AppProcess for ShellProcess {
    fn exited(&mut self) -> Option<Option<i32>> {
        match self.child.try_wait() {
            Ok(Some(status)) => Some(status.code()),
            Ok(None) => None,
            Err(_) => Some(None),
        }
    }

    fn stop(mut self: Box<Self>) -> Option<String> {
        let known = self.watch.take().map(procs::Watch::finish).unwrap_or_default();
        let root = self.child.id();
        // everything below the shell (node, vite, …), then whatever it left behind
        let sweep = procs::sweep(root, self.since, &known);
        kill_tree(root);
        let _ = self.child.kill();
        let _ = self.child.wait();
        let mut text = sweep.describe();
        if !sweep.still_busy.is_empty() {
            let ports: Vec<String> = sweep.still_busy.iter().map(u16::to_string).collect();
            let busy = format!("cổng {} vẫn bị chiếm", ports.join(", "));
            text = Some(match text {
                Some(t) => format!("{t}; {busy}"),
                None => busy,
            });
        }
        text
    }
}

fn kill_tree(pid: u32) {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("taskkill");
        cmd.args(["/T", "/F", "/PID", &pid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let _ = crate::live::hidden(&mut cmd).status();
    }
    #[cfg(not(windows))]
    let _ = pid;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn urls_give_a_host_and_port() {
        assert_eq!(
            address("http://localhost:5173/files?x=1"),
            Some(("localhost".into(), 5173))
        );
        assert_eq!(
            address("https://app.example.com/"),
            Some(("app.example.com".into(), 443))
        );
        assert_eq!(address("http://127.0.0.1"), Some(("127.0.0.1".into(), 80)));
        assert_eq!(address("http://[::1]:3000/"), Some(("::1".into(), 3000)));
        assert_eq!(
            address("http://u:p@localhost:8080"),
            Some(("localhost".into(), 8080))
        );
        for bad in [
            "localhost:5173",
            "ftp://x",
            "http://",
            "http://:80",
            "http://x:port",
        ] {
            assert_eq!(address(bad), None, "{bad}");
        }
    }

    /// A dev server command.
    fn cmd(command: &str, dir: &str, wait: &str) -> DevCommand {
        DevCommand {
            command: command.into(),
            dir: dir.into(),
            wait: wait.into(),
        }
    }

    fn server(url: &str, commands: Vec<DevCommand>) -> DevServer {
        DevServer {
            url: url.into(),
            commands,
        }
    }

    #[test]
    fn what_the_user_types_is_checked() {
        let web = |dir: &str| cmd("npm run dev", dir, "");
        assert!(check(&server("http://localhost:5173", vec![web("apps/web")])).is_ok());
        assert!(
            check(&server("", vec![])).is_ok(),
            "nothing = the AI starts the app"
        );
        assert!(
            check(&server("http://localhost:5173", vec![])).is_ok(),
            "URL only = already running"
        );
        // an API then the web: the API says when it is up, the last may wait for the URL
        let api = cmd("npm run dev", "api", "3000");
        assert!(check(&server("http://localhost:3003", vec![api.clone(), web("web")])).is_ok());
        assert!(
            check(&server(
                "http://localhost:3003",
                vec![
                    cmd("npm run dev", "api", "http://localhost:3000/health"),
                    web("web")
                ]
            ))
            .is_ok()
        );
        for bad in [
            server("localhost:5173", vec![web("")]),
            server(
                "http://localhost:5173",
                vec![cmd("npm run dev\nrm -rf /", "", "")],
            ),
            server("http://localhost:5173", vec![web("../other")]),
            server("http://localhost:5173", vec![web("C:\\other")]),
            server("http://localhost:5173", vec![web("/etc")]),
            server("http://localhost:5173", vec![cmd("", "api", "3000")]),
            // only the last command may leave Chờ empty
            server("http://localhost:3003", vec![web("api"), web("web")]),
            server(
                "http://localhost:3003",
                vec![cmd("npm run dev", "api", "localhost"), web("web")],
            ),
            server("http://localhost:3003", vec![api.clone(); MAX_COMMANDS + 1]),
            server("", vec![web("web")]),
        ] {
            assert!(check(&bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn what_is_typed_is_tidied() {
        let typed = [
            cmd(" npm run dev ", "/api/", ":3000"),
            DevCommand::default(),
            cmd(" npm run dev ", "web\\", ""),
        ];
        let t = tidy(" http://localhost:3003 ", &typed);
        assert_eq!(t.url, "http://localhost:3003");
        assert_eq!(
            t.commands,
            vec![
                cmd("npm run dev", "api", "http://localhost:3000"),
                cmd("npm run dev", "web", ""),
            ],
            "empty rows dropped, a bare port waits on localhost"
        );
        assert_eq!(wait_url("3000").as_deref(), Some("http://localhost:3000"));
        assert_eq!(
            wait_url("http://127.0.0.1:4000/x").as_deref(),
            Some("http://127.0.0.1:4000/x")
        );
        assert_eq!(wait_url("0"), None);
        assert_eq!(wait_url("api"), None);
        assert_eq!(what_runs(&t), "api: npm run dev → web: npm run dev");
        assert_eq!(
            what_runs(&server("http://localhost:3003", vec![])),
            "http://localhost:3003"
        );
    }

    #[test]
    fn a_dev_server_saved_with_one_command_reads_as_a_list() {
        let old: DevServer =
            serde_json::from_str(r#"{"command":"pnpm dev","url":"http://localhost:5173","dir":"web"}"#)
                .unwrap();
        assert_eq!(
            old,
            server("http://localhost:5173", vec![cmd("pnpm dev", "web", "")])
        );
        let url_only: DevServer =
            serde_json::from_str(r#"{"command":"","url":"http://localhost:5173","dir":""}"#).unwrap();
        assert!(url_only.commands.is_empty());
        let new = server(
            "http://localhost:3003",
            vec![cmd("npm run dev", "api", "http://localhost:3000")],
        );
        let back: DevServer = serde_json::from_str(&serde_json::to_string(&new).unwrap()).unwrap();
        assert_eq!(back, new);
    }

    /// Up = running from the start, or its command was started (`runs`: URL → command).
    struct Ports {
        up: Vec<&'static str>,
        runs: Vec<(&'static str, &'static str)>,
        started: Mutex<Vec<(PathBuf, String)>>,
        stopped: Arc<Mutex<Vec<String>>>,
    }

    struct PortProc {
        command: String,
        stopped: Arc<Mutex<Vec<String>>>,
    }

    impl AppProcess for PortProc {
        fn exited(&mut self) -> Option<Option<i32>> {
            None
        }
        fn stop(self: Box<Self>) -> Option<String> {
            self.stopped.lock().unwrap().push(self.command.clone());
            Some(format!("đã tắt {}", self.command))
        }
    }

    impl AppStarter for Ports {
        fn is_up(&self, url: &str) -> bool {
            let started = self.started.lock().unwrap();
            self.up.contains(&url)
                || self
                    .runs
                    .iter()
                    .any(|(u, c)| *u == url && started.iter().any(|(_, s)| s == c))
        }
        fn start(&self, cwd: &Path, command: &str, _log: &Path) -> Result<Box<dyn AppProcess>, String> {
            self.started
                .lock()
                .unwrap()
                .push((cwd.to_path_buf(), command.into()));
            Ok(Box::new(PortProc {
                command: command.into(),
                stopped: self.stopped.clone(),
            }))
        }
        fn open_browser(&self, _profile: &Path, _url: &str) -> Result<(Box<dyn AppProcess>, u16), String> {
            Err("no browser in this test".into())
        }
    }

    #[test]
    fn several_commands_start_top_to_bottom_and_stop_together() {
        let dir = tempfile::tempdir().unwrap();
        for d in ["api", "web"] {
            std::fs::create_dir_all(dir.path().join(d)).unwrap();
        }
        let log = dir.path().join("dev-server.log");
        let two = server(
            "http://localhost:3003",
            vec![cmd("ts-node-dev", "api", "3000"), cmd("vite", "web", "")],
        );
        let go = AtomicBool::new(false);
        let ports = |up: Vec<&'static str>, runs: Vec<(&'static str, &'static str)>| Ports {
            up,
            runs,
            started: Mutex::new(Vec::new()),
            stopped: Arc::new(Mutex::new(Vec::new())),
        };
        let both = vec![
            ("http://localhost:3000", "ts-node-dev"),
            ("http://localhost:3003", "vite"),
        ];
        // nothing running: the API first (from its folder), then the web; stopped last-first
        let fresh = ports(vec![], both.clone());
        let up = bring_up(&fresh, dir.path(), &two, &log, READY_WAIT, &go).unwrap();
        assert_eq!(
            *fresh.started.lock().unwrap(),
            vec![
                (dir.path().join("api"), "ts-node-dev".to_owned()),
                (dir.path().join("web"), "vite".to_owned()),
            ]
        );
        assert_eq!(up.lines.len(), 2);
        assert!(
            up.lines[0].starts_with("Dev server: app chạy `ts-node-dev` trong api, http://localhost:3000"),
            "{:?}",
            up.lines
        );
        assert_eq!(
            up.started.unwrap().stop().as_deref(),
            Some("đã tắt vite; đã tắt ts-node-dev")
        );
        assert_eq!(*fresh.stopped.lock().unwrap(), ["vite", "ts-node-dev"]);
        // the API runs already: left alone, only the web started
        let api_up = ports(vec!["http://localhost:3000"], both.clone());
        let up = bring_up(&api_up, dir.path(), &two, &log, READY_WAIT, &go).unwrap();
        assert_eq!(api_up.started.lock().unwrap().len(), 1);
        assert!(up.lines[0].contains("đang chạy sẵn"), "{:?}", up.lines);
        up.started.unwrap().stop();
        assert_eq!(*api_up.stopped.lock().unwrap(), ["vite"]);
        // everything up: nothing to stop later
        let all_up = ports(vec!["http://localhost:3000", "http://localhost:3003"], vec![]);
        let up = bring_up(&all_up, dir.path(), &two, &log, READY_WAIT, &go).unwrap();
        assert!(up.started.is_none());
        // the web never answers: the API the app started is stopped too
        let web_dead = ports(vec![], vec![("http://localhost:3000", "ts-node-dev")]);
        let Err((code, _)) = bring_up(&web_dead, dir.path(), &two, &log, Duration::ZERO, &go) else {
            panic!("should time out")
        };
        assert_eq!(code, "devserver.timeout");
        assert_eq!(*web_dead.stopped.lock().unwrap(), ["vite", "ts-node-dev"]);
        // every command waits on its own port and the page never answers: all stopped
        let own_ports = server(
            "http://localhost:3003",
            vec![cmd("ts-node-dev", "api", "3000"), cmd("vite", "web", "5000")],
        );
        let no_page = ports(
            vec![],
            vec![
                ("http://localhost:3000", "ts-node-dev"),
                ("http://localhost:5000", "vite"),
            ],
        );
        let Err((code, _)) = bring_up(&no_page, dir.path(), &own_ports, &log, Duration::ZERO, &go) else {
            panic!("the page never answered")
        };
        assert_eq!(code, "devserver.timeout");
        assert_eq!(*no_page.stopped.lock().unwrap(), ["vite", "ts-node-dev"]);
    }

    /// Up after `up_after` polls; the process ends with `exit` after `exit_after` polls.
    struct Fake {
        polls: Mutex<u32>,
        up_after: Option<u32>,
        exit_after: Option<u32>,
        started: Mutex<Vec<(PathBuf, String)>>,
        stopped: Arc<Mutex<u32>>,
    }

    struct FakeProc {
        polls: u32,
        exit_after: Option<u32>,
        stopped: Arc<Mutex<u32>>,
    }

    impl AppProcess for FakeProc {
        fn exited(&mut self) -> Option<Option<i32>> {
            self.polls += 1;
            self.exit_after.filter(|n| self.polls >= *n).map(|_| Some(1))
        }
        fn stop(self: Box<Self>) -> Option<String> {
            *self.stopped.lock().unwrap() += 1;
            Some("đã tắt 1 tiến trình".into())
        }
    }

    impl AppStarter for Fake {
        fn is_up(&self, _url: &str) -> bool {
            let mut p = self.polls.lock().unwrap();
            *p += 1;
            self.up_after.is_some_and(|n| *p > n)
        }
        fn start(&self, cwd: &Path, command: &str, _log: &Path) -> Result<Box<dyn AppProcess>, String> {
            self.started
                .lock()
                .unwrap()
                .push((cwd.to_path_buf(), command.into()));
            Ok(Box::new(FakeProc {
                polls: 0,
                exit_after: self.exit_after,
                stopped: self.stopped.clone(),
            }))
        }
        fn open_browser(&self, _profile: &Path, _url: &str) -> Result<(Box<dyn AppProcess>, u16), String> {
            Err("no browser in this test".into())
        }
    }

    fn fake(up_after: Option<u32>, exit_after: Option<u32>) -> Fake {
        Fake {
            polls: Mutex::new(0),
            up_after,
            exit_after,
            started: Mutex::new(Vec::new()),
            stopped: Arc::new(Mutex::new(0)),
        }
    }

    #[test]
    fn the_app_starts_the_server_waits_for_it_and_uses_one_already_running() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("apps/web")).unwrap();
        let log = dir.path().join("dev.log");
        let one = server("http://localhost:5173", vec![cmd("npm run dev", "apps/web", "")]);
        let go = AtomicBool::new(false);
        // already up: nothing started
        let running = fake(Some(0), None);
        let up = bring_up(&running, dir.path(), &one, &log, READY_WAIT, &go).unwrap();
        assert!(up.started.is_none());
        assert!(running.started.lock().unwrap().is_empty());
        // up on the third look: started from the package folder
        let later = fake(Some(2), None);
        let Ok(Up {
            started: Some(proc), ..
        }) = bring_up(&later, dir.path(), &one, &log, READY_WAIT, &go)
        else {
            panic!("not started")
        };
        assert_eq!(
            later.started.lock().unwrap()[0],
            (dir.path().join("apps/web"), "npm run dev".to_owned())
        );
        assert_eq!(proc.stop().as_deref(), Some("đã tắt 1 tiến trình"));
        // the command dies first: its log tail in the error
        std::fs::write(&log, "Error: Cannot find module 'vite'").unwrap();
        let dies = fake(None, Some(1));
        let Err((code, message)) = bring_up(&dies, dir.path(), &one, &log, READY_WAIT, &go) else {
            panic!("should fail")
        };
        assert_eq!(code, "devserver.exited");
        assert!(message.contains("Cannot find module 'vite'"), "{message}");
        // never answers: stopped after the wait
        let never = fake(None, None);
        let Err((code, _)) = bring_up(&never, dir.path(), &one, &log, Duration::ZERO, &go) else {
            panic!("should time out")
        };
        assert_eq!(code, "devserver.timeout");
        assert_eq!(*never.stopped.lock().unwrap(), 1);
        // URL only and nothing answers
        let url_only = server("http://localhost:5173", vec![]);
        let Err((code, _)) = bring_up(&fake(None, None), dir.path(), &url_only, &log, READY_WAIT, &go) else {
            panic!("should fail")
        };
        assert_eq!(code, "devserver.down");
        // Dừng while waiting
        let stopped = AtomicBool::new(true);
        let waiting = fake(None, None);
        let Err((code, _)) = bring_up(&waiting, dir.path(), &one, &log, READY_WAIT, &stopped) else {
            panic!("should stop")
        };
        assert_eq!(code, "step.stopped");
        assert_eq!(*waiting.stopped.lock().unwrap(), 1);
    }

    #[test]
    fn chrome_writes_its_port_in_the_profile() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(chrome_port(dir.path()), None);
        std::fs::write(
            dir.path().join("DevToolsActivePort"),
            "51234\n/devtools/browser/abc\n",
        )
        .unwrap();
        assert_eq!(chrome_port(dir.path()), Some(51234));
        std::fs::write(dir.path().join("DevToolsActivePort"), "0\n").unwrap();
        assert_eq!(chrome_port(dir.path()), None);
    }

    #[test]
    fn a_listening_port_counts_as_up() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(ShellStarter.is_up(&format!("http://127.0.0.1:{port}/")));
        drop(listener);
        assert!(!ShellStarter.is_up("http://127.0.0.1:1/"));
    }
}
