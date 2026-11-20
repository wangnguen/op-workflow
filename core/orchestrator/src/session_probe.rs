//! `op-workflow-desktop --session-probe <folder>` (PLAN.md §12, phiên làm lõi A2): Claude Code
//! started the way the VS Code extension starts it (stream-json in and out, permission requests
//! over stdio, the user's setting sources), driven the way the app's chat window will drive a
//! task's session. It chains messages, answers permission requests (deny, then allow), a question
//! from the AI and a plan approval, interrupts a turn, queues a message during a turn, closes the
//! session and resumes it, and reports what the app could see. Haiku in a throwaway folder.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use opw_agents::claude::DISALLOWED_GIT;
use opw_agents::cli;
use opw_agents::env::{Account, is_stripped};
use opw_domain::view::Provider;
use opw_domain::workflow::StepKind;
use serde_json::{Value, json};

use crate::flow_mcp;

const MARK: &str = "PONG-ALPHA";
const MD_MARK: &str = "CLAUDE-MD-5521";
const MCP_MARK: &str = "MCP-OK-7391";
const QUEUED_MARK: &str = "QUEUED-OK";
/// "cli" is turned into "sdk-cli" by the CLI when it runs without a TTY; other values stay.
const ENTRYPOINT: &str = "op-workflow";
const NO_STATUS: &str = "(no status file)";
const TURN_TIMEOUT: Duration = Duration::from_secs(150);
const POLL: Duration = Duration::from_millis(200);
const STDERR_KEEP: usize = 16 * 1024;

type Checks = Vec<(String, bool, String)>;

fn check(checks: &mut Checks, name: &str, ok: bool, detail: impl Into<String>) {
    checks.push((name.to_owned(), ok, detail.into()));
}

/// `<data dir>/session-probe`: outside `%TEMP%`, where Claude Code ignores CLAUDE.md files.
pub fn default_folder() -> PathBuf {
    opw_config::data_dir()
        .map(|d| d.join("session-probe"))
        .unwrap_or_else(|_| std::env::temp_dir().join("opw-session-probe"))
}

/// Run the probe in `folder` (created if missing) and return a plain-text report.
pub fn run(folder: &Path) -> String {
    let mut r = String::new();
    let _ = writeln!(
        r,
        "op-workflow session probe (PLAN.md §12 A2, the extension's way)"
    );
    let Some(exe) = cli::find_claude() else {
        let _ = writeln!(r, "claude not found on PATH");
        return r;
    };
    if in_temp(folder) {
        let _ = writeln!(
            r,
            "note: the folder is under %TEMP%; Claude Code ignores CLAUDE.md there, so the \
             CLAUDE.md check fails"
        );
    }
    if let Err(e) = std::fs::create_dir_all(folder) {
        let _ = writeln!(r, "cannot create {}: {e}", folder.display());
        return r;
    }
    let _ = std::fs::write(
        folder.join("README.md"),
        "Throwaway folder for the op-workflow session probe.\n",
    );
    let _ = std::fs::write(
        folder.join("CLAUDE.md"),
        format!(
            "# Probe project\n\nThe project marker is {MD_MARK}. Mention it only when asked for \
             the project marker.\n"
        ),
    );
    for f in ["probe.txt", "plan-ok.txt"] {
        let _ = std::fs::remove_file(folder.join(f));
    }
    let _ = writeln!(r, "claude: {}\nfolder: {}", exe.display(), folder.display());

    let mcp = match flow_mcp::Run::start(StepKind::Analyze, folder, json!({ "probe": MCP_MARK })) {
        Ok(m) => m,
        Err(e) => {
            let _ = writeln!(r, "app MCP server did not start: {e}");
            return r;
        }
    };
    // Without an entrypoint the CLI writes "sdk-cli", and the extension's (and `/resume`'s)
    // session list hides those; the extension sets "claude-vscode", the app its own name.
    let env = vec![
        (flow_mcp::TOKEN_ENV.to_owned(), mcp.token().to_owned()),
        ("CLAUDE_CODE_ENTRYPOINT".to_owned(), ENTRYPOINT.to_owned()),
    ];

    let id = new_uuid();
    let _ = writeln!(r, "session id: {id}");
    let t0 = Instant::now();
    let mut checks: Checks = Vec::new();
    // total_cost_usd in a result is the running total of the session process
    let mut cost1 = 0.0_f64;
    let mut cost2 = 0.0_f64;

    // Folder trust, as the extension reads it before it starts a chat (no tokens).
    let trust0 = workspace_trust(&exe, folder);
    let trusted0 = trust0.as_ref().is_some_and(|v| v["trusted"] == true);
    check(
        &mut checks,
        "workspace-trust report read",
        trust0.is_some(),
        trust0.as_ref().map(Value::to_string).unwrap_or_default(),
    );
    // The user agreed to this throwaway folder; in the app the user answers in the UI.
    let attest = (!trusted0)
        .then(|| {
            trust0
                .as_ref()
                .and_then(|v| v["directory"].as_str().map(str::to_owned))
        })
        .flatten();

    // part 1: a new session
    let mut args = base_args(&mcp);
    args.extend(["--session-id".to_owned(), id.clone()]);
    let mut s = match Session::open(&exe, folder, &args, &env, &id, t0) {
        Ok(s) => s,
        Err(e) => {
            let _ = writeln!(r, "spawn failed: {e}");
            return r;
        }
    };
    let mut init_req = json!({ "subtype": "initialize" });
    if let Some(dir) = &attest {
        init_req["workspaceTrust"] = json!({ "accepted": true, "directory": dir });
    }
    let init_resp = s.request(init_req);
    check(
        &mut checks,
        "initialize answered",
        init_resp.as_ref().is_some_and(|v| v["subtype"] == "success"),
        init_resp
            .as_ref()
            .map(|v| keys(&v["response"]))
            .unwrap_or_default(),
    );
    if attest.is_some() {
        let recorded = init_resp
            .as_ref()
            .map(|v| v["response"]["workspace_trust_recorded"].clone())
            .unwrap_or(Value::Null);
        let after = workspace_trust(&exe, folder);
        check(
            &mut checks,
            "trust recorded via initialize (folder was untrusted)",
            after.as_ref().is_some_and(|v| v["trusted"] == true),
            format!(
                "workspace_trust_recorded={recorded}, report after: {}",
                after.unwrap_or_default()
            ),
        );
    }

    // 1: a plain message; project CLAUDE.md loaded?
    let t = s.turn(
        &format!(
            "Reply with exactly {MARK} on the first line. On the second line write the project \
             marker if your instructions include one, otherwise NONE. Do not use any tools."
        ),
        Extra::None,
        Bash::Allow,
    );
    cost1 = cost1.max(t.cost());
    let text = t.text();
    check(
        &mut checks,
        "message 1 answered",
        t.finished && text.contains(MARK),
        text.clone(),
    );
    check(
        &mut checks,
        "project CLAUDE.md loaded (marker in reply)",
        text.contains(MD_MARK),
        "",
    );
    check(
        &mut checks,
        "live text (--include-partial-messages)",
        t.deltas > 0,
        format!(
            "{} deltas, first after {:.1}s",
            t.deltas,
            t.first_delta.unwrap_or_default()
        ),
    );
    check(
        &mut checks,
        "own message echoed (--replay-user-messages)",
        t.replayed,
        "",
    );
    let init = s.init.clone().unwrap_or(Value::Null);
    check(
        &mut checks,
        "system/init received",
        !init.is_null(),
        describe_init(&init),
    );
    let user_mcp = init["mcp_servers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m["name"].as_str())
        .any(|n| n != flow_mcp::NAME);
    check(
        &mut checks,
        "user settings loaded (an MCP server besides the app's)",
        user_mcp,
        "",
    );

    // 2: the app's MCP tool (pre-allowed by --allowedTools, as the app does)
    let t = s.turn(
        "Call the get_step_contract tool of the op_workflow MCP server, then reply with only the \
         value of context.probe from its result. Do not call report_step_result.",
        Extra::None,
        Bash::Allow,
    );
    cost1 = cost1.max(t.cost());
    check(
        &mut checks,
        "app MCP tool called",
        t.text().contains(MCP_MARK),
        t.text(),
    );
    check(
        &mut checks,
        "no permission request for the pre-allowed MCP tool",
        t.asks.is_empty(),
        t.asks.join(" | "),
    );

    // 3: a permission request, denied
    let t = s.turn(
        "Use the Bash tool to run exactly this command: echo opw-probe > probe.txt   If the \
         command is refused, do not retry; reply with only the word REFUSED. Otherwise reply with \
         only DONE.",
        Extra::None,
        Bash::Deny,
    );
    cost1 = cost1.max(t.cost());
    let wrote = folder.join("probe.txt").exists();
    check(
        &mut checks,
        "permission request reached the app (mode default)",
        t.asks.iter().any(|a| a.starts_with("Bash")),
        t.asks.join(" | "),
    );
    check(
        &mut checks,
        "deny: command did not run, AI went on",
        !wrote && t.finished,
        t.text(),
    );

    // 4: the same request, allowed
    let t = s.turn(
        "Run that same command once more with the Bash tool now: echo opw-probe > probe.txt   Then \
         reply with only DONE (or REFUSED if it is refused again).",
        Extra::None,
        Bash::Allow,
    );
    cost1 = cost1.max(t.cost());
    check(
        &mut checks,
        "allow: command ran",
        folder.join("probe.txt").exists(),
        format!("{} || {}", t.asks.join(" | "), t.text()),
    );

    // 5: a question from the AI
    let t = s.turn(
        "Use the AskUserQuestion tool to ask me one question, \"Which color should the probe \
         use?\", with the two options Red and Blue. After I answer, reply with only the color I \
         picked.",
        Extra::None,
        Bash::Allow,
    );
    cost1 = cost1.max(t.cost());
    check(
        &mut checks,
        "AskUserQuestion came as a permission request",
        t.asks.iter().any(|a| a.starts_with("AskUserQuestion")),
        t.asks.join(" | "),
    );
    check(
        &mut checks,
        "AI got the app's answer (Blue)",
        t.text().to_ascii_lowercase().contains("blue"),
        t.text(),
    );

    // 6: plan mode and a plan approval
    let mode = s.request(json!({ "subtype": "set_permission_mode", "mode": "plan" }));
    check(
        &mut checks,
        "set_permission_mode plan",
        mode.as_ref().is_some_and(|v| v["subtype"] == "success"),
        mode.map(|v| v.to_string()).unwrap_or_default(),
    );
    let t = s.turn(
        "Plan how to create a file named plan-ok.txt in the current folder containing the single \
         word hi. Keep the plan to two lines and present it with ExitPlanMode. Once it is \
         approved, create the file and reply with only CREATED.",
        Extra::None,
        Bash::Allow,
    );
    cost1 = cost1.max(t.cost());
    check(
        &mut checks,
        "plan approval came as a permission request",
        t.asks.iter().any(|a| a.starts_with("ExitPlanMode")),
        t.asks.join(" | "),
    );
    check(
        &mut checks,
        "approved plan carried out without a Write prompt",
        folder.join("plan-ok.txt").exists() && !t.asks.iter().any(|a| a.starts_with("Write")),
        t.text(),
    );
    let _ = s.request(json!({ "subtype": "set_permission_mode", "mode": "default" }));

    // 7: stop in the middle of a turn
    let t = s.turn(
        "Write the numbers from 1 to 400, one per line, and nothing else.",
        Extra::Interrupt,
        Bash::Allow,
    );
    cost1 = cost1.max(t.cost());
    check(
        &mut checks,
        "interrupt ended the turn",
        t.finished && t.stopped_after.is_some(),
        format!(
            "result {} {:.1}s after the interrupt, ack {}",
            t.result["subtype"],
            t.stopped_after.unwrap_or_default(),
            t.ack
        ),
    );

    // 8: a message typed while the AI is still answering
    let t = s.turn(
        "Write the numbers from 1 to 150, one per line, without using any tools.",
        Extra::Followup(&format!(
            "After that, reply with only {QUEUED_MARK} on its own line."
        )),
        Bash::Allow,
    );
    cost1 = cost1.max(t.cost());
    check(
        &mut checks,
        "message sent mid-turn was handled",
        t.text().contains(QUEUED_MARK),
        format!("{} result(s), last: {}", t.results, short(&t.text(), 120)),
    );

    let statuses1 = s.statuses.join(", ");
    let types1 = format!("{:?}", s.types);
    let (log1, err1) = s.close();

    // part 2: close and resume
    let trust1 = workspace_trust(&exe, folder);
    let mut args = base_args(&mcp);
    args.extend(["--resume".to_owned(), id.clone()]);
    let mut log2 = String::new();
    let mut err2 = String::new();
    match Session::open(&exe, folder, &args, &env, &id, t0) {
        Ok(mut s) => {
            let _ = s.request(json!({ "subtype": "initialize" }));
            let t = s.turn(
                "What exact word did I ask you to reply with on the first line of my first \
                 message? Reply with only that word.",
                Extra::None,
                Bash::Allow,
            );
            cost2 = cost2.max(t.cost());
            let same = s.init.as_ref().is_some_and(|v| v["session_id"] == id.as_str());
            check(
                &mut checks,
                "resume: same session id",
                same,
                s.init
                    .as_ref()
                    .map(|v| v["session_id"].to_string())
                    .unwrap_or_default(),
            );
            check(
                &mut checks,
                "resume: remembers message 1",
                t.text().contains(MARK),
                t.text(),
            );
            (log2, err2) = s.close();
        }
        Err(e) => check(&mut checks, "resume: spawn", false, e),
    }
    drop(mcp);

    let path = transcript(&id);
    let tx = path.as_ref().map(|p| transcript_summary(p)).unwrap_or_default();
    check(
        &mut checks,
        "transcript has the whole conversation",
        tx.contains(MARK) && tx.contains(QUEUED_MARK),
        path.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
    );
    let entrypoints = path.as_ref().map(|p| entrypoints(p)).unwrap_or_default();
    check(
        &mut checks,
        "transcript entrypoint lets the extension list it",
        !entrypoints.is_empty() && entrypoints.iter().all(|e| e == ENTRYPOINT),
        format!("{entrypoints:?}"),
    );

    let _ = writeln!(r, "\nCHECKS");
    for (name, ok, detail) in &checks {
        let _ = write!(r, "  [{}] {name}", if *ok { "OK" } else { "--" });
        if !detail.is_empty() {
            let _ = write!(r, ": {}", short(detail, 260));
        }
        let _ = writeln!(r);
    }
    let _ = writeln!(r, "\ntrust after part 1: {}", trust1.unwrap_or_default());
    let _ = writeln!(r, "status file values seen (part 1): {statuses1}");
    let _ = writeln!(r, "message types (part 1): {types1}");
    let _ = writeln!(
        r,
        "total_cost_usd (running total of each process): part 1 ${cost1:.4}, resume ${cost2:.4}"
    );
    if let Some(p) = &path {
        let (input, cache_w, cache_r, output) = usage(p);
        let _ = writeln!(
            r,
            "tokens (transcript): input {input}, cache write {cache_w}, cache read {cache_r}, output {output}"
        );
    }
    let _ = writeln!(r, "total time: {:.0}s", t0.elapsed().as_secs_f32());
    let _ = writeln!(
        r,
        "\nOPEN IT ELSEWHERE (check by hand that the whole conversation shows):\n  terminal: cd \"{}\" ; claude --resume {id}\n  VS Code:  vscode://anthropic.claude-code/open?session={id}",
        folder.display()
    );
    let _ = writeln!(r, "\nTRANSCRIPT\n{tx}");
    let _ = writeln!(r, "TIMELINE (new session)\n{log1}\nTIMELINE (resume)\n{log2}");
    let _ = writeln!(
        r,
        "STDERR (new session, tail)\n{}\nSTDERR (resume, tail)\n{}",
        tail(&err1, 1500),
        tail(&err2, 800)
    );
    r
}

/// Save the report next to the app data.
pub fn save(report: &str) -> Option<PathBuf> {
    let dir = opw_config::data_dir().ok()?;
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("session-probe.txt");
    std::fs::write(&path, report).ok()?;
    Some(path)
}

/// The extension's flags (Claude Code 2.1.327, trusted folder) plus the app's MCP and git guard.
/// Left out: `--debug`, `--debug-to-stderr`, `--enable-auth-status` (extension diagnostics),
/// thinking / effort (Haiku here).
fn base_args(mcp: &flow_mcp::Run) -> Vec<String> {
    // `configure` expects the program's first argument at index 0
    let mut args = vec!["-".to_owned()];
    mcp.configure(Provider::Claude, &mut args);
    args.remove(0);
    let own = [
        "--output-format",
        "stream-json",
        "--verbose",
        "--input-format",
        "stream-json",
        "--model",
        "haiku",
        "--permission-prompt-tool=stdio",
        "--disallowedTools",
        DISALLOWED_GIT,
        "--setting-sources",
        "user,project,local",
        "--permission-mode",
        "default",
        "--include-partial-messages",
        "--no-chrome",
        "--replay-user-messages",
    ];
    own.iter().map(|s| (*s).to_owned()).chain(args).collect()
}

/// Is `folder` inside the temp folder (8.3 short and long names compared after resolving)?
fn in_temp(folder: &Path) -> bool {
    let long = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    long(folder).starts_with(long(&std::env::temp_dir()))
}

fn stripped_env() -> Vec<String> {
    std::env::vars()
        .map(|(k, _)| k)
        .filter(|k| is_stripped(k, Account::Subscription))
        .collect()
}

fn hide_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

/// `claude --setting-sources user workspace-trust` in `folder`, as the extension asks before a
/// chat: `{trusted, directory, root?}`.
fn workspace_trust(exe: &Path, folder: &Path) -> Option<Value> {
    let mut cmd = Command::new(exe);
    cmd.args(["--setting-sources", "user", "workspace-trust", "--json"])
        .current_dir(folder)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for k in stripped_env() {
        cmd.env_remove(k);
    }
    hide_window(&mut cmd);
    let out = cmd.output().ok()?;
    String::from_utf8_lossy(&out.stdout).lines().rev().find_map(|l| {
        serde_json::from_str::<Value>(l.trim())
            .ok()
            .filter(Value::is_object)
    })
}

#[derive(Clone, Copy)]
enum Bash {
    Allow,
    Deny,
}

enum Extra<'a> {
    None,
    /// Send a control `interrupt` once the answer streams.
    Interrupt,
    /// Send another user message once the answer streams.
    Followup(&'a str),
}

#[derive(Default)]
struct Turn {
    /// What the assistant said and which tools it called, in order.
    reply: Vec<String>,
    /// Permission requests: "<tool> <input> → <answer> (status <file status>)".
    asks: Vec<String>,
    result: Value,
    results: u32,
    deltas: u32,
    first_delta: Option<f32>,
    replayed: bool,
    finished: bool,
    stopped_after: Option<f32>,
    ack: String,
}

impl Turn {
    fn text(&self) -> String {
        self.reply.join(" | ")
    }

    fn cost(&self) -> f64 {
        self.result["total_cost_usd"].as_f64().unwrap_or(0.0)
    }
}

struct Session {
    child: Child,
    stdin: Option<ChildStdin>,
    rx: Receiver<Value>,
    eof: bool,
    stderr: Arc<Mutex<String>>,
    id: String,
    t0: Instant,
    log: String,
    next_id: u32,
    pending: Vec<Value>,
    last_status: String,
    statuses: Vec<String>,
    types: BTreeMap<String, u32>,
    init: Option<Value>,
}

impl Session {
    fn open(
        exe: &Path,
        cwd: &Path,
        args: &[String],
        env: &[(String, String)],
        id: &str,
        t0: Instant,
    ) -> Result<Self, String> {
        let mut cmd = Command::new(exe);
        cmd.args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for k in stripped_env() {
            cmd.env_remove(k);
        }
        cmd.envs(env.iter().map(|(k, v)| (k, v)));
        hide_window(&mut cmd);
        let mut child = cmd.spawn().map_err(|e| e.to_string())?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let stderr_pipe = child.stderr.take().ok_or("no stderr")?;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let v =
                    serde_json::from_str(line).unwrap_or_else(|_| json!({ "type": "_raw", "text": line }));
                if tx.send(v).is_err() {
                    break;
                }
            }
        });
        let stderr = Arc::new(Mutex::new(String::new()));
        {
            let stderr = stderr.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stderr_pipe).lines() {
                    let Ok(line) = line else { break };
                    if let Ok(mut s) = stderr.lock() {
                        s.push_str(&line);
                        s.push('\n');
                        if s.len() > STDERR_KEEP * 2 {
                            let cut = s.len() - STDERR_KEEP;
                            let cut = (cut..s.len()).find(|i| s.is_char_boundary(*i)).unwrap_or(0);
                            s.drain(..cut);
                        }
                    }
                }
            });
        }
        let mut s = Self {
            child,
            stdin,
            rx,
            eof: false,
            stderr,
            id: id.to_owned(),
            t0,
            log: String::new(),
            next_id: 0,
            pending: Vec::new(),
            last_status: String::new(),
            statuses: Vec::new(),
            types: BTreeMap::new(),
            init: None,
        };
        let first = args
            .iter()
            .position(|a| a == "--resume" || a == "--session-id")
            .map_or("", |i| args[i].as_str());
        s.note(&format!("spawned: claude … {first} (pid {})", s.child.id()));
        Ok(s)
    }

    fn note(&mut self, text: &str) {
        let _ = writeln!(self.log, "  +{:>6.1}s {text}", self.t0.elapsed().as_secs_f32());
    }

    fn send(&mut self, v: &Value) {
        let Some(stdin) = self.stdin.as_mut() else {
            return;
        };
        let line = format!("{v}\n");
        if stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.flush())
            .is_err()
        {
            self.note("stdin closed");
        }
    }

    fn say(&mut self, text: &str) {
        self.note(&format!("user: {}", short(text, 80)));
        let msg = json!({
            "type": "user",
            "message": { "role": "user", "content": [{ "type": "text", "text": text }] },
            "parent_tool_use_id": null,
            "session_id": "",
        });
        self.send(&msg);
    }

    fn control(&mut self, body: Value) -> String {
        self.next_id += 1;
        let rid = format!("req_{}_probe", self.next_id);
        self.note(&format!("control_request {}", body["subtype"]));
        let msg = json!({ "type": "control_request", "request_id": rid, "request": body });
        self.send(&msg);
        rid
    }

    /// Send a control request and wait for its response (messages in between are kept).
    fn request(&mut self, body: Value) -> Option<Value> {
        let rid = self.control(body);
        let start = Instant::now();
        let mut kept = Vec::new();
        let mut found = None;
        while start.elapsed() < Duration::from_secs(60) && !self.eof {
            let Some(v) = self.recv(POLL) else {
                continue;
            };
            if v["type"] == "control_response" && v["response"]["request_id"] == rid.as_str() {
                let r = v["response"].clone();
                self.note(&format!(
                    "control_response {} in {:.1}s",
                    r["subtype"],
                    start.elapsed().as_secs_f32()
                ));
                found = Some(r);
                break;
            }
            kept.push(v);
        }
        self.pending.splice(0..0, kept);
        found
    }

    fn recv(&mut self, wait: Duration) -> Option<Value> {
        if !self.pending.is_empty() {
            return Some(self.pending.remove(0));
        }
        match self.rx.recv_timeout(wait) {
            Ok(v) => {
                self.observe(&v);
                Some(v)
            }
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => {
                if !self.eof {
                    self.eof = true;
                    self.note("stdout closed");
                }
                None
            }
        }
    }

    fn observe(&mut self, v: &Value) {
        let mut kind = v["type"].as_str().unwrap_or("?").to_owned();
        if let Some(sub) = v["subtype"].as_str() {
            let _ = write!(kind, "/{sub}");
        } else if v["type"] == "control_request" {
            let _ = write!(kind, "/{}", v["request"]["subtype"].as_str().unwrap_or("?"));
        }
        *self.types.entry(kind).or_default() += 1;
        if v["type"] == "system" && v["subtype"] == "init" && self.init.is_none() {
            self.init = Some(v.clone());
            self.note(&format!("system/init: session {}", v["session_id"]));
        } else if v["type"] == "system" && v["subtype"] != "init" {
            self.note(&format!("system/{}", v["subtype"].as_str().unwrap_or("?")));
        }
    }

    /// The session's status as `~/.claude/sessions/<pid>.json` says, noted when it changes.
    fn status(&mut self) -> String {
        let s = read_status(&self.id, Some(self.child.id()))
            .map(|v| describe_status(&v))
            .unwrap_or_else(|| NO_STATUS.to_owned());
        if s != self.last_status {
            self.note(&format!("status file: {s}"));
            if !self.statuses.contains(&s) {
                self.statuses.push(s.clone());
            }
            self.last_status.clone_from(&s);
        }
        s
    }

    /// Send one message and read until its `result`, answering permission requests.
    fn turn(&mut self, text: &str, extra: Extra<'_>, bash: Bash) -> Turn {
        let mut t = Turn::default();
        let start = Instant::now();
        self.say(text);
        let mut acted_at: Option<Instant> = None;
        let mut interrupt_id = String::new();
        let mut first_result: Option<Instant> = None;
        let mut last_poll = Instant::now();
        loop {
            if start.elapsed() > TURN_TIMEOUT {
                self.note("turn timed out");
                break;
            }
            if self.eof {
                self.note("claude exited during the turn");
                break;
            }
            if last_poll.elapsed() >= POLL {
                self.status();
                last_poll = Instant::now();
            }
            // act once the answer streams: a follow-up at once, an interrupt 1.5 s in
            let wait = if matches!(extra, Extra::Interrupt) {
                1.5
            } else {
                0.0
            };
            if acted_at.is_none()
                && t.first_delta
                    .is_some_and(|d| start.elapsed().as_secs_f32() - d >= wait)
            {
                match extra {
                    Extra::None => {}
                    Extra::Interrupt => {
                        interrupt_id = self.control(json!({ "subtype": "interrupt" }));
                    }
                    Extra::Followup(more) => self.say(more),
                }
                acted_at = Some(Instant::now());
            }
            if let Some(at) = first_result {
                let done = match extra {
                    Extra::Followup(_) => {
                        t.text().contains(QUEUED_MARK)
                            || t.results >= 2
                            || at.elapsed() > Duration::from_secs(25)
                    }
                    _ => true,
                };
                if done {
                    break;
                }
            }
            let Some(v) = self.recv(POLL) else {
                continue;
            };
            match v["type"].as_str().unwrap_or("") {
                "control_request" => self.answer(&v, bash, &mut t),
                "control_response" => {
                    if v["response"]["request_id"] == interrupt_id.as_str() {
                        t.ack = v["response"]["subtype"].as_str().unwrap_or("?").to_owned();
                        self.note(&format!("interrupt acknowledged: {}", t.ack));
                    }
                }
                "stream_event" => {
                    if v["event"]["type"] == "content_block_delta" {
                        t.deltas += 1;
                        if t.first_delta.is_none() {
                            let at = start.elapsed().as_secs_f32();
                            t.first_delta = Some(at);
                            self.note(&format!("first delta after {at:.1}s"));
                        }
                    }
                }
                "assistant" => {
                    for item in v["message"]["content"].as_array().into_iter().flatten() {
                        match item["type"].as_str() {
                            Some("text") => {
                                let s = item["text"].as_str().unwrap_or("").trim().to_owned();
                                if !s.is_empty() {
                                    t.reply.push(s);
                                }
                            }
                            Some("tool_use") => {
                                let name = item["name"].as_str().unwrap_or("?").to_owned();
                                self.note(&format!("tool_use {name}"));
                                t.reply.push(format!("[tool {name}]"));
                            }
                            _ => {}
                        }
                    }
                }
                "user" => {
                    let content = &v["message"]["content"];
                    let echoed = content.as_str() == Some(text)
                        || content
                            .as_array()
                            .is_some_and(|a| a.iter().any(|c| c["text"].as_str() == Some(text)));
                    if echoed {
                        t.replayed = true;
                    }
                    for c in content.as_array().into_iter().flatten() {
                        if c["type"] == "tool_result" {
                            let body = c["content"].to_string();
                            self.note(&format!(
                                "tool_result{}: {}",
                                if c["is_error"] == true { " (error)" } else { "" },
                                short(&body, 90)
                            ));
                        }
                    }
                }
                "result" => {
                    t.results += 1;
                    t.finished = true;
                    if let Some(at) = acted_at.filter(|_| matches!(extra, Extra::Interrupt)) {
                        t.stopped_after.get_or_insert(at.elapsed().as_secs_f32());
                    }
                    self.note(&format!(
                        "result {} is_error={} ${:.4} turns={} in {:.1}s",
                        v["subtype"],
                        v["is_error"],
                        v["total_cost_usd"].as_f64().unwrap_or(0.0),
                        v["num_turns"],
                        start.elapsed().as_secs_f32()
                    ));
                    t.result = v;
                    first_result.get_or_insert_with(Instant::now);
                }
                _ => {}
            }
        }
        self.note(&format!("turn over: {}", short(&t.text(), 120)));
        t
    }

    /// Answer `can_use_tool` the way the extension's webview does.
    fn answer(&mut self, v: &Value, bash: Bash, t: &mut Turn) {
        let rid = v["request_id"].clone();
        let req = &v["request"];
        if req["subtype"] != "can_use_tool" {
            self.note(&format!(
                "control_request {} from claude: not handled",
                req["subtype"]
            ));
            let msg = json!({ "type": "control_response", "response": {
                "subtype": "error", "request_id": rid, "error": "not supported by the probe" } });
            self.send(&msg);
            return;
        }
        let tool = req["tool_name"].as_str().unwrap_or("?").to_owned();
        let input = req["input"].clone();
        let (mut response, said) = match (tool.as_str(), bash) {
            ("AskUserQuestion", _) => {
                let mut answers = serde_json::Map::new();
                for q in input["questions"].as_array().into_iter().flatten() {
                    let labels: Vec<&str> = q["options"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|o| o["label"].as_str())
                        .collect();
                    let pick = labels
                        .iter()
                        .find(|l| l.to_ascii_lowercase().contains("blue"))
                        .or(labels.first())
                        .copied()
                        .unwrap_or("Blue");
                    answers.insert(q["question"].as_str().unwrap_or("").to_owned(), json!(pick));
                }
                let mut updated = input.clone();
                updated["answers"] = Value::Object(answers.clone());
                (
                    json!({ "behavior": "allow", "updatedInput": updated }),
                    format!("allow + answers {}", Value::Object(answers)),
                )
            }
            ("ExitPlanMode", _) => {
                // the extension's "approve": switch to auto (acceptEdits when auto is refused),
                // then allow the tool
                let mut mode = "auto";
                let ok = |r: &Option<Value>, m: &str| {
                    r.as_ref().is_some_and(|v| {
                        v["subtype"] == "success" && v["response"]["mode"].as_str().is_none_or(|x| x == m)
                    })
                };
                let r = self.request(json!({ "subtype": "set_permission_mode", "mode": mode }));
                if !ok(&r, mode) {
                    self.note(&format!(
                        "auto refused: {}",
                        r.map(|v| v.to_string()).unwrap_or_default()
                    ));
                    mode = "acceptEdits";
                    let r = self.request(json!({ "subtype": "set_permission_mode", "mode": mode }));
                    if !ok(&r, mode) {
                        mode = "(mode change failed)";
                    }
                }
                (
                    json!({ "behavior": "allow", "updatedInput": input }),
                    format!("set mode {mode}, allow"),
                )
            }
            ("Bash", Bash::Deny) => (
                json!({ "behavior": "deny", "message": "The user declined this command in op-workflow." }),
                "deny".to_owned(),
            ),
            _ => (
                json!({ "behavior": "allow", "updatedInput": input }),
                "allow".to_owned(),
            ),
        };
        if let Some(id) = req["tool_use_id"].as_str() {
            response["toolUseID"] = json!(id);
        }
        let extra_keys: Vec<&str> = req
            .as_object()
            .map(|o| {
                o.keys()
                    .map(String::as_str)
                    .filter(|k| !["subtype", "tool_name", "input", "tool_use_id"].contains(k))
                    .collect()
            })
            .unwrap_or_default();
        let line = format!(
            "{tool} {} → {said} (status {}; request keys {extra_keys:?}{})",
            short(&input.to_string(), 160),
            self.last_status,
            req.get("permission_suggestions")
                .map(|p| format!("; suggestions {}", short(&p.to_string(), 160)))
                .unwrap_or_default()
        );
        self.note(&format!("can_use_tool {line}"));
        t.asks.push(line);
        let msg = json!({ "type": "control_response",
            "response": { "subtype": "success", "request_id": rid, "response": response } });
        self.send(&msg);
    }

    /// Close stdin the way the SDK ends a query; kill the tree if claude does not exit.
    fn close(mut self) -> (String, String) {
        self.note("close: stdin EOF");
        drop(self.stdin.take());
        let start = Instant::now();
        loop {
            match self.child.try_wait() {
                Ok(Some(st)) => {
                    self.note(&format!(
                        "claude exited ({:?}) {:.1}s after EOF",
                        st.code(),
                        start.elapsed().as_secs_f32()
                    ));
                    break;
                }
                Ok(None) if start.elapsed() < Duration::from_secs(10) => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                _ => {
                    self.note("still running 10s after EOF: kill tree");
                    kill_tree(self.child.id());
                    let _ = self.child.wait();
                    break;
                }
            }
        }
        let err = self.stderr.lock().map(|s| s.clone()).unwrap_or_default();
        (self.log, err)
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
        hide_window(&mut cmd);
        let _ = cmd.status();
    }
    #[cfg(not(windows))]
    {
        let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
    }
}

/// The parts of `system/init` that say what the session loaded.
fn describe_init(v: &Value) -> String {
    if v.is_null() {
        return String::new();
    }
    let names = |key: &str, field: &str| -> String {
        v[key]
            .as_array()
            .into_iter()
            .flatten()
            .map(|m| {
                let name = m[field].as_str().or(m.as_str()).unwrap_or("?");
                match m["status"].as_str() {
                    Some(st) => format!("{name}={st}"),
                    None => name.to_owned(),
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    let count = |key: &str| v[key].as_array().map_or(0, Vec::len);
    format!(
        "model {}, permissionMode {}, version {}, tools {}, mcp [{}], plugins [{}], skills {}, \
         slash commands {}, agents {}, keys {}",
        v["model"],
        v["permissionMode"],
        v["claude_code_version"],
        count("tools"),
        names("mcp_servers", "name"),
        names("plugins", "name"),
        count("skills"),
        count("slash_commands"),
        count("agents"),
        keys(v)
    )
}

fn keys(v: &Value) -> String {
    v.as_object()
        .map(|o| o.keys().cloned().collect::<Vec<_>>().join(","))
        .unwrap_or_default()
}

/// Claude Code's own folder (`$CLAUDE_CONFIG_DIR` or `~/.claude`).
fn claude_home() -> Option<PathBuf> {
    crate::browsertest::claude_projects().and_then(|p| p.parent().map(Path::to_path_buf))
}

/// The status file of session `id` (`~/.claude/sessions/<pid>.json`), else the one of `pid`.
fn read_status(id: &str, pid: Option<u32>) -> Option<Value> {
    let dir = claude_home()?.join("sessions");
    let parse = |p: &Path| -> Option<Value> { serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok() };
    if !id.is_empty() {
        for entry in std::fs::read_dir(&dir).ok()?.flatten() {
            let p = entry.path();
            if p.extension().is_some_and(|e| e == "json") {
                if let Some(v) = parse(&p).filter(|v| v["sessionId"] == id) {
                    return Some(v);
                }
            }
        }
    }
    pid.and_then(|pid| parse(&dir.join(format!("{pid}.json"))))
}

/// `status` plus any field that says what the session waits for or who runs it.
fn describe_status(v: &Value) -> String {
    let mut s = v["status"].as_str().unwrap_or("?").to_owned();
    if let Some(obj) = v.as_object() {
        for (k, val) in obj {
            let key = k.to_ascii_lowercase();
            if ["wait", "reason", "block", "pending", "prompt", "entry", "kind"]
                .iter()
                .any(|w| key.contains(w))
            {
                let _ = write!(s, " {k}={val}");
            }
        }
    }
    s
}

fn transcript(id: &str) -> Option<PathBuf> {
    let projects = crate::browsertest::claude_projects()?;
    std::fs::read_dir(projects)
        .ok()?
        .flatten()
        .map(|d| d.path().join(format!("{id}.jsonl")))
        .find(|p| p.is_file())
}

/// One line per user text / assistant text / tool call in the transcript.
fn transcript_summary(path: &Path) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut out = String::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let who = v["type"].as_str().unwrap_or("");
        if who != "user" && who != "assistant" {
            continue;
        }
        let content = &v["message"]["content"];
        if let Some(s) = content.as_str() {
            let _ = writeln!(out, "  {who}: {}", short(s, 110));
            continue;
        }
        for item in content.as_array().into_iter().flatten() {
            match item["type"].as_str() {
                Some("text") => {
                    let _ = writeln!(
                        out,
                        "  {who}: {}",
                        short(item["text"].as_str().unwrap_or(""), 110)
                    );
                }
                Some("tool_use") => {
                    let _ = writeln!(out, "  {who}: [tool {}]", item["name"].as_str().unwrap_or("?"));
                }
                Some("tool_result") => {
                    let _ = writeln!(
                        out,
                        "  {who}: [result{}] {}",
                        if item["is_error"] == true { " error" } else { "" },
                        short(&item["content"].to_string(), 80)
                    );
                }
                _ => {}
            }
        }
    }
    out
}

/// The distinct `entrypoint` values written in the transcript.
fn entrypoints(path: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(e) = v["entrypoint"].as_str() {
            if !out.iter().any(|o| o == e) {
                out.push(e.to_owned());
            }
        }
    }
    out
}

fn usage(path: &Path) -> (u64, u64, u64, u64) {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut sum = (0, 0, 0, 0);
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let u = &v["message"]["usage"];
        if v["type"] == "assistant" && u.is_object() {
            sum.0 += u["input_tokens"].as_u64().unwrap_or(0);
            sum.1 += u["cache_creation_input_tokens"].as_u64().unwrap_or(0);
            sum.2 += u["cache_read_input_tokens"].as_u64().unwrap_or(0);
            sum.3 += u["output_tokens"].as_u64().unwrap_or(0);
        }
    }
    sum
}

fn new_uuid() -> String {
    let mut b = [0_u8; 16];
    let _ = getrandom::fill(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

fn short(s: &str, max: usize) -> String {
    let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= max {
        one
    } else {
        format!("{}…", one.chars().take(max).collect::<String>())
    }
}

fn tail(s: &str, max: usize) -> String {
    let n = s.chars().count();
    s.chars().skip(n.saturating_sub(max)).collect()
}
