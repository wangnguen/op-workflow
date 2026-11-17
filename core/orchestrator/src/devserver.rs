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
use opw_domain::view::DevServer;

/// How long the app waits for the URL to answer after starting the command.
pub const READY_WAIT: Duration = Duration::from_secs(180);
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

/// Check what the user typed before it is saved or run. Empty URL = no dev server.
pub fn check(server: &DevServer) -> Result<(), String> {
    let url = server.url.trim();
    if url.is_empty() {
        return Ok(());
    }
    address(url).ok_or_else(|| format!("URL phải dạng http://localhost:5173 (đang là `{url}`)"))?;
    let command = server.command.trim();
    if command.contains(['\n', '\r']) || command.chars().count() > 300 {
        return Err("Lệnh chạy app phải là một dòng, tối đa 300 ký tự".into());
    }
    let dir = server.dir.trim();
    if !dir.is_empty() {
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
pub enum Up {
    /// The app was already running: used as it is, never stopped by the app.
    Running,
    /// The app started it; it answered after this long. Stop it when the step ends.
    Started(Box<dyn AppProcess>, Duration),
}

/// Make sure the app answers at `server.url`: use it when it is up already, else run the command
/// and wait up to `wait` for the URL. `Err` = (code, message) for the step's error.
pub fn bring_up(
    starter: &dyn AppStarter,
    repo: &Path,
    server: &DevServer,
    log: &Path,
    wait: Duration,
    stop: &AtomicBool,
) -> Result<Up, (&'static str, String)> {
    let url = server.url.trim();
    if starter.is_up(url) {
        return Ok(Up::Running);
    }
    let command = server.command.trim();
    if command.is_empty() {
        return Err((
            "devserver.down",
            format!(
                "Không mở được {url} và chưa có lệnh chạy app. Bật app rồi bấm Thử lại, hoặc ghi lệnh ở mục Test trình duyệt (tab Plan)."
            ),
        ));
    }
    let cwd = match server.dir.trim() {
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
            return Ok(Up::Started(proc, started.elapsed()));
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

    #[test]
    fn what_the_user_types_is_checked() {
        let s = |command: &str, url: &str, dir: &str| DevServer {
            command: command.into(),
            url: url.into(),
            dir: dir.into(),
        };
        assert!(check(&s("npm run dev", "http://localhost:5173", "apps/web")).is_ok());
        assert!(check(&s("", "", "")).is_ok(), "nothing = the AI starts the app");
        assert!(
            check(&s("", "http://localhost:5173", "")).is_ok(),
            "URL only = already running"
        );
        for bad in [
            s("npm run dev", "localhost:5173", ""),
            s("npm run dev\nrm -rf /", "http://localhost:5173", ""),
            s("npm run dev", "http://localhost:5173", "../other"),
            s("npm run dev", "http://localhost:5173", "C:\\other"),
            s("npm run dev", "http://localhost:5173", "/etc"),
        ] {
            assert!(check(&bad).is_err(), "{bad:?}");
        }
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
        let server = DevServer {
            command: "npm run dev".into(),
            url: "http://localhost:5173".into(),
            dir: "apps/web".into(),
        };
        let go = AtomicBool::new(false);
        // already up: nothing started
        let running = fake(Some(0), None);
        assert!(matches!(
            bring_up(&running, dir.path(), &server, &log, READY_WAIT, &go),
            Ok(Up::Running)
        ));
        assert!(running.started.lock().unwrap().is_empty());
        // up on the third look: started from the package folder
        let later = fake(Some(2), None);
        let Ok(Up::Started(proc, _)) = bring_up(&later, dir.path(), &server, &log, READY_WAIT, &go) else {
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
        let Err((code, message)) = bring_up(&dies, dir.path(), &server, &log, READY_WAIT, &go) else {
            panic!("should fail")
        };
        assert_eq!(code, "devserver.exited");
        assert!(message.contains("Cannot find module 'vite'"), "{message}");
        // never answers: stopped after the wait
        let never = fake(None, None);
        let Err((code, _)) = bring_up(&never, dir.path(), &server, &log, Duration::ZERO, &go) else {
            panic!("should time out")
        };
        assert_eq!(code, "devserver.timeout");
        assert_eq!(*never.stopped.lock().unwrap(), 1);
        // URL only and nothing answers
        let url_only = DevServer {
            command: String::new(),
            ..server.clone()
        };
        let Err((code, _)) = bring_up(&fake(None, None), dir.path(), &url_only, &log, READY_WAIT, &go) else {
            panic!("should fail")
        };
        assert_eq!(code, "devserver.down");
        // Dừng while waiting
        let stopped = AtomicBool::new(true);
        let waiting = fake(None, None);
        let Err((code, _)) = bring_up(&waiting, dir.path(), &server, &log, READY_WAIT, &stopped) else {
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
