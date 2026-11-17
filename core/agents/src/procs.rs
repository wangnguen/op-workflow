//! What an AI run leaves behind (PLAN.md §12 part 5d): dev servers, node, Chrome started by the
//! AI keep running after its own process ends, often holding a port. After every run the app
//! finds every process descended from the run's process (also orphans whose parent already
//! exited: Windows keeps their parent id), kills them, and checks the ports they listened on.
//!
//! No `unsafe` (the workspace forbids it), so no Job Object: the process table and the listening
//! ports come from one PowerShell / CIM query. A process whose parent already exited keeps that
//! parent's id, so while a run lasts a [`Watch`] notes every process below it now and then: a
//! dev server whose shell is gone is still found through the shell's id.

use std::collections::{HashMap, HashSet};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// First look at the run's processes, then every [`WATCH_EVERY`].
const WATCH_FIRST: Duration = Duration::from_secs(10);
const WATCH_EVERY: Duration = Duration::from_secs(20);

/// One process of the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proc {
    pub pid: u32,
    pub parent: u32,
    pub name: String,
    /// Start time, ms since the Unix epoch (0 = unknown).
    pub created_ms: u64,
}

/// What a sweep killed and which ports are still taken afterwards.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sweep {
    /// `node.exe`, `chrome.exe`, … with their pid.
    pub killed: Vec<(u32, String)>,
    /// Ports the killed processes listened on.
    pub ports: Vec<u16>,
    /// Of those, the ones still listening a moment after the kill.
    pub still_busy: Vec<u16>,
}

impl Sweep {
    pub fn is_empty(&self) -> bool {
        self.killed.is_empty()
    }

    /// "đã tắt 2 tiến trình AI để lại: node.exe (cổng 5173), chrome.exe"
    pub fn describe(&self) -> Option<String> {
        if self.killed.is_empty() {
            return None;
        }
        let mut names: Vec<String> = Vec::new();
        for (_, name) in &self.killed {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
        let mut text = format!(
            "đã tắt {} tiến trình AI để lại: {}",
            self.killed.len(),
            names.join(", ")
        );
        if !self.ports.is_empty() {
            let ports: Vec<String> = self.ports.iter().map(u16::to_string).collect();
            text.push_str(&format!(" (cổng {})", ports.join(", ")));
        }
        Some(text)
    }
}

/// Now, in ms since the Unix epoch.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Every process below `root` (not `root` itself) that started after `since_ms` (minus a little
/// clock slack), so a pid reused by an unrelated process is never taken for a child. `known` =
/// processes seen below `root` earlier (pid → start time): their children count even when they
/// already exited, unless their pid now belongs to another process.
pub fn descendants(all: &[Proc], root: u32, since_ms: u64, known: &HashMap<u32, u64>) -> Vec<Proc> {
    let slack = since_ms.saturating_sub(2_000);
    let mut children: HashMap<u32, Vec<&Proc>> = HashMap::new();
    let alive: HashMap<u32, &Proc> = all.iter().map(|p| (p.pid, p)).collect();
    for p in all {
        if p.pid != p.parent {
            children.entry(p.parent).or_default().push(p);
        }
    }
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut todo = vec![root];
    for (pid, created) in known {
        match alive.get(pid) {
            // still running: found again through its parent, or taken as it is when orphaned
            Some(p) if p.created_ms == *created => {
                if seen.insert(*pid) {
                    out.push((*p).clone());
                }
                todo.push(*pid);
            }
            // reused by another process: neither it nor its children are ours
            Some(_) => {}
            None => todo.push(*pid),
        }
    }
    while let Some(pid) = todo.pop() {
        for c in children.get(&pid).into_iter().flatten() {
            if c.created_ms != 0 && c.created_ms < slack {
                continue;
            }
            if seen.insert(c.pid) {
                out.push((*c).clone());
                todo.push(c.pid);
            }
        }
    }
    out
}

/// Read the query's lines: `P<TAB>pid<TAB>parent<TAB>name<TAB>created ms` and
/// `L<TAB>port<TAB>pid`.
pub fn parse_table(text: &str) -> (Vec<Proc>, Listening) {
    let mut procs = Vec::new();
    let mut ports = Vec::new();
    for line in text.lines() {
        let parts: Vec<&str> = line.trim_end_matches('\r').split('\t').collect();
        match parts.as_slice() {
            ["P", pid, parent, name, created] => {
                if let (Ok(pid), Ok(parent)) = (pid.parse(), parent.parse()) {
                    procs.push(Proc {
                        pid,
                        parent,
                        name: (*name).to_owned(),
                        created_ms: created.trim().parse::<f64>().map_or(0, |v| v.max(0.0) as u64),
                    });
                }
            }
            ["L", port, pid] => {
                if let (Ok(port), Ok(pid)) = (port.parse(), pid.trim().parse()) {
                    ports.push((port, pid));
                }
            }
            _ => {}
        }
    }
    (procs, ports)
}

#[cfg(windows)]
const PROCS_SCRIPT: &str = "$ErrorActionPreference='SilentlyContinue';\
$e=[datetime]'1970-01-01';\
Get-CimInstance Win32_Process -Property ProcessId,ParentProcessId,Name,CreationDate | ForEach-Object { $c=0; if ($_.CreationDate) { $c=[long]($_.CreationDate.ToUniversalTime()-$e).TotalMilliseconds }; \"P`t$($_.ProcessId)`t$($_.ParentProcessId)`t$($_.Name)`t$c\" };";

#[cfg(windows)]
const PORTS_SCRIPT: &str =
    "Get-NetTCPConnection -State Listen | ForEach-Object { \"L`t$($_.LocalPort)`t$($_.OwningProcess)\" }";

#[cfg(windows)]
fn powershell(script: &str) -> Option<String> {
    let mut cmd = Command::new("powershell.exe");
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        script,
    ])
    .stdin(Stdio::null())
    .stderr(Stdio::null());
    crate::run::hide_window(&mut cmd);
    let out = cmd.output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Listening TCP ports with the pid that owns each.
pub type Listening = Vec<(u16, u32)>;

/// The process table and, with `ports`, the listening TCP ports (`None` off Windows or when the
/// query fails).
pub fn table(ports: bool) -> Option<(Vec<Proc>, Listening)> {
    #[cfg(windows)]
    {
        let script = if ports {
            format!("{PROCS_SCRIPT}{PORTS_SCRIPT}")
        } else {
            PROCS_SCRIPT.to_owned()
        };
        powershell(&script).map(|t| parse_table(&t))
    }
    #[cfg(not(windows))]
    {
        let _ = ports;
        None
    }
}

/// Notes the processes below a running process now and then, so the final [`sweep`] still finds
/// those whose parent exited in between. Stops with [`Watch::finish`].
#[derive(Debug)]
pub struct Watch {
    done: Arc<AtomicBool>,
    known: Arc<Mutex<HashMap<u32, u64>>>,
    thread: Option<JoinHandle<()>>,
}

impl Watch {
    pub fn start(root: u32, since_ms: u64) -> Self {
        let done = Arc::new(AtomicBool::new(false));
        let known = Arc::new(Mutex::new(HashMap::new()));
        let (flag, seen) = (done.clone(), known.clone());
        let thread = std::thread::Builder::new()
            .name("opw-procs-watch".into())
            .spawn(move || {
                let mut next = Instant::now() + WATCH_FIRST;
                while !flag.load(Ordering::Relaxed) {
                    if Instant::now() < next {
                        std::thread::sleep(Duration::from_millis(250));
                        continue;
                    }
                    if let Some((procs, _)) = table(false) {
                        let mut map = seen.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                        let below = descendants(&procs, root, since_ms, &map);
                        for p in below {
                            map.insert(p.pid, p.created_ms);
                        }
                    }
                    next = Instant::now() + WATCH_EVERY;
                }
            })
            .ok();
        Self { done, known, thread }
    }

    /// Stop watching; what was seen below the process so far.
    pub fn finish(mut self) -> HashMap<u32, u64> {
        self.done.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        self.known.lock().map(|m| m.clone()).unwrap_or_default()
    }
}

impl Drop for Watch {
    /// A run that ended early (an I/O error) still stops its watcher.
    fn drop(&mut self) {
        self.done.store(true, Ordering::Relaxed);
    }
}

fn kill(pids: &[u32]) {
    if pids.is_empty() {
        return;
    }
    #[cfg(windows)]
    {
        let mut cmd = Command::new("taskkill");
        cmd.args(["/T", "/F"]);
        for pid in pids {
            cmd.args(["/PID", &pid.to_string()]);
        }
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        crate::run::hide_window(&mut cmd);
        let _ = cmd.status();
    }
    #[cfg(not(windows))]
    {
        for pid in pids {
            let _ = Command::new("kill")
                .args(["-9", &pid.to_string()])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}

/// Kill everything `root` started since `since_ms` and report it (`known` = what a [`Watch`]
/// saw below it). Best effort: when the process table cannot be read, nothing is killed (the
/// run's own kill still happened).
pub fn sweep(root: u32, since_ms: u64, known: &HashMap<u32, u64>) -> Sweep {
    let Some((procs, listening)) = table(true) else {
        return Sweep::default();
    };
    let left = descendants(&procs, root, since_ms, known);
    if left.is_empty() {
        return Sweep::default();
    }
    let pids: HashSet<u32> = left.iter().map(|p| p.pid).collect();
    let mut ports: Vec<u16> = listening
        .iter()
        .filter(|(_, pid)| pids.contains(pid))
        .map(|(port, _)| *port)
        .collect();
    ports.sort_unstable();
    ports.dedup();
    let list: Vec<u32> = left.iter().map(|p| p.pid).collect();
    kill(&list);
    let still_busy = if ports.is_empty() {
        Vec::new()
    } else {
        std::thread::sleep(Duration::from_millis(800));
        table(true)
            .map(|(_, now)| {
                let mut busy: Vec<u16> = now
                    .iter()
                    .map(|(port, _)| *port)
                    .filter(|p| ports.contains(p))
                    .collect();
                busy.sort_unstable();
                busy.dedup();
                busy
            })
            .unwrap_or_default()
    };
    Sweep {
        killed: left.into_iter().map(|p| (p.pid, p.name)).collect(),
        ports,
        still_busy,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, parent: u32, name: &str, created_ms: u64) -> Proc {
        Proc {
            pid,
            parent,
            name: name.into(),
            created_ms,
        }
    }

    #[test]
    fn finds_children_and_orphans_but_not_reused_pids() {
        let all = vec![
            p(10, 1, "claude.exe", 100_000),
            p(11, 10, "bash.exe", 101_000),
            p(12, 11, "node.exe", 102_000),
            // its parent bash already exited: still found through the recorded parent id
            p(13, 99, "chrome.exe", 103_000),
            p(99, 10, "cmd.exe", 102_500),
            // pid 10 reused long ago by something else's parent link
            p(20, 10, "old.exe", 10_000),
            p(30, 2, "explorer.exe", 1_000),
        ];
        let none = HashMap::new();
        let mut found: Vec<u32> = descendants(&all, 10, 100_000, &none)
            .iter()
            .map(|p| p.pid)
            .collect();
        found.sort_unstable();
        assert_eq!(found, vec![11, 12, 13, 99]);
        assert!(descendants(&all, 30, 0, &none).is_empty());
    }

    #[test]
    fn a_watched_shell_that_exited_still_leads_to_its_server() {
        // claude 10 ran bash 11, which started node 12 and then exited
        let all = vec![
            p(10, 1, "claude.exe", 100_000),
            p(12, 11, "node.exe", 102_000),
            // pid 40 was seen below the run, then reused by an unrelated process
            p(40, 2, "notepad.exe", 150_000),
            p(41, 40, "child.exe", 151_000),
        ];
        let none = HashMap::new();
        assert!(descendants(&all, 10, 100_000, &none).is_empty());
        let known = HashMap::from([(11, 101_000), (40, 103_000)]);
        let found: Vec<u32> = descendants(&all, 10, 100_000, &known)
            .iter()
            .map(|p| p.pid)
            .collect();
        assert_eq!(found, vec![12]);
        // a watched orphan still running is taken as it is
        let known = HashMap::from([(12, 102_000)]);
        let found: Vec<u32> = descendants(&all, 10, 100_000, &known)
            .iter()
            .map(|p| p.pid)
            .collect();
        assert_eq!(found, vec![12]);
    }

    #[test]
    fn reads_the_table_and_describes_a_sweep() {
        let text = "P\t10\t1\tclaude.exe\t1700000000000\r\nP\t11\t10\tnode.exe\t0\nL\t5173\t11\nnoise\nP\tx\t1\ty\t0\n";
        let (procs, ports) = parse_table(text);
        assert_eq!(procs.len(), 2);
        assert_eq!(procs[0].created_ms, 1_700_000_000_000);
        assert_eq!(ports, vec![(5173, 11)]);
        let s = Sweep {
            killed: vec![
                (11, "node.exe".into()),
                (12, "node.exe".into()),
                (13, "chrome.exe".into()),
            ],
            ports: vec![5173],
            still_busy: vec![],
        };
        assert_eq!(
            s.describe().as_deref(),
            Some("đã tắt 3 tiến trình AI để lại: node.exe, chrome.exe (cổng 5173)")
        );
        assert_eq!(Sweep::default().describe(), None);
    }
}
