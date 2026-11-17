//! Settings → Các bước AI: the models and effort levels the Claude and Codex CLIs on this
//! machine offer. Nothing here sends a prompt, so reading the lists costs no tokens:
//!
//! - Claude: `claude -p --input-format stream-json …` answers an `initialize` control request
//!   with `models` (`value`, `resolvedModel`, `displayName`, `supportedEffortLevels`, …).
//! - Codex: a short-lived `codex app-server` answers `model/list` (models with their
//!   `supportedReasoningEfforts`) and `config/read` (the configured model and effort).
//!
//! Permission modes come from `claude --help` (`--permission-mode` choices) and `codex exec
//! --help` (`--sandbox` values and the approval flags).
//!
//! What "Theo máy" turns out to be comes from Claude's `settings.json` (`model`, `effortLevel`,
//! or `ANTHROPIC_MODEL`) and Codex's config. Lists are cached for [`TTL`]. Each list a CLI gives
//! is also saved in the probe folder; when a CLI is missing or does not answer, the last saved
//! list is used, else only "Theo máy" is left, and the reason is shown. No model name is written
//! into the app, so new models and CLI updates need no code change.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use opw_agents::{claude, cli, codex};
use opw_domain::view::{AgentCatalog, AgentKind, AgentModel, StepModels};
use serde_json::{Value, json};

use crate::live::{hidden, line_reader};

/// How long a list read from the CLIs is reused before the page reads it again.
const TTL: Duration = Duration::from_secs(10 * 60);
const READ_TIMEOUT: Duration = Duration::from_secs(20);
/// `model/list` pages to follow at most.
const MAX_PAGES: u64 = 5;

/// `claude --effort` levels (`claude --help`, 2.1.325), for a model that says it takes effort
/// but does not list the levels.
pub const CLAUDE_EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

fn str_at<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn owned(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_owned()).collect()
}

// ---------- Claude ----------

/// The `initialize` answer of Claude Code → catalog. `settings` is Claude's `settings.json`;
/// `env_model` is `ANTHROPIC_MODEL`, which wins over it.
pub fn claude_catalog(
    init: &Value,
    settings: &Value,
    env_model: Option<&str>,
    read_at: &str,
) -> AgentCatalog {
    let mut models: Vec<AgentModel> = Vec::new();
    // every name the CLI accepts (aliases such as `opus` included) → the model id
    let mut names: Vec<(String, String)> = Vec::new();
    let mut recommended: Option<String> = None;
    for e in init.get("models").and_then(Value::as_array).into_iter().flatten() {
        let Some(value) = str_at(e, "value") else {
            continue;
        };
        let id = str_at(e, "resolvedModel").unwrap_or(value).to_owned();
        names.push((value.to_owned(), id.clone()));
        if value == "default" {
            recommended = Some(id);
            continue;
        }
        if models.iter().any(|m| m.id == id) {
            continue;
        }
        let efforts = if e.get("supportsEffort").and_then(Value::as_bool) == Some(true) {
            let levels: Vec<String> = e
                .get("supportedEffortLevels")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            if levels.is_empty() {
                owned(&CLAUDE_EFFORTS)
            } else {
                levels
            }
        } else {
            Vec::new()
        };
        models.push(AgentModel {
            label: str_at(e, "displayName").unwrap_or(value).to_owned(),
            description: str_at(e, "description").map(str::to_owned),
            efforts,
            default_effort: None,
            is_default: false,
            id,
        });
    }
    if let Some(m) = recommended
        .as_ref()
        .and_then(|r| models.iter_mut().find(|m| &m.id == r))
    {
        m.is_default = true;
    }
    let mut catalog = claude_defaults(models, settings, env_model, &names, recommended);
    catalog.from_cli = true;
    catalog.cli_version = str_at(init, "claude_code_version").map(str::to_owned);
    catalog.read_at = Some(read_at.to_owned());
    catalog
}

/// Fill in what "Theo máy" means. `opus[1m]` → id of `opus`, label "Opus 5.5 · 1M".
fn claude_defaults(
    models: Vec<AgentModel>,
    settings: &Value,
    env_model: Option<&str>,
    names: &[(String, String)],
    recommended: Option<String>,
) -> AgentCatalog {
    let configured = env_model
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .or_else(|| str_at(settings, "model"));
    let (default_id, default_label) = match configured {
        Some(raw) => {
            let (base, suffix) = match raw.find('[') {
                Some(i) if raw.ends_with(']') => (&raw[..i], Some(raw[i + 1..raw.len() - 1].to_uppercase())),
                _ => (raw, None),
            };
            let id = names
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(base))
                .map(|(_, id)| id.clone())
                .or_else(|| models.iter().find(|m| m.id == base).map(|m| m.id.clone()));
            let name = id
                .as_ref()
                .and_then(|id| models.iter().find(|m| &m.id == id))
                .map_or(base, |m| m.label.as_str());
            let label = match suffix {
                Some(s) => format!("{name} · {s}"),
                None => name.to_owned(),
            };
            (id, Some(label))
        }
        None => {
            let label = recommended
                .as_ref()
                .and_then(|r| models.iter().find(|m| &m.id == r))
                .map(|m| m.label.clone());
            (recommended, label)
        }
    };
    AgentCatalog {
        agent: AgentKind::Claude,
        default_model: configured.map(str::to_owned).or_else(|| default_id.clone()),
        default_id,
        default_label,
        default_effort: str_at(settings, "effortLevel").map(str::to_owned),
        from_cli: false,
        cli_version: None,
        error: None,
        read_at: None,
        models,
        modes: Vec::new(),
    }
}

/// Claude without a list (the CLI cannot be asked): only what "Theo máy" is, and why.
pub fn claude_unread(error: String, settings: &Value, env_model: Option<&str>) -> AgentCatalog {
    let mut catalog = claude_defaults(Vec::new(), settings, env_model, &[], None);
    catalog.error = Some(error);
    catalog
}

/// Claude's `settings.json` (`$CLAUDE_CONFIG_DIR` or `~/.claude`); `Null` when unreadable.
fn claude_settings() -> Value {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".claude")))
        .and_then(|d| std::fs::read_to_string(d.join("settings.json")).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(Value::Null)
}

/// Ask `claude` for its `initialize` answer. `dir` is an empty folder (no CLAUDE.md).
fn read_claude(exe: &Path, dir: &Path) -> Result<Value, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut child = hidden(
        Command::new(exe)
            .args(claude::catalog_args())
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null()),
    )
    .spawn()
    .map_err(|e| format!("không chạy được claude: {e}"))?;
    let result = (|| {
        let mut stdin = child.stdin.take().ok_or("no stdin")?;
        send_line(&mut stdin, claude::INITIALIZE_REQUEST)?;
        let rx = line_reader(&mut child).ok_or("no stdout")?;
        let deadline = Instant::now() + READ_TIMEOUT;
        loop {
            let v = next_json(&rx, deadline, "Claude CLI")?;
            if v.get("type").and_then(Value::as_str) != Some("control_response") {
                continue;
            }
            let resp = v.get("response").unwrap_or(&Value::Null);
            if resp.get("subtype").and_then(Value::as_str) == Some("error") {
                let why = str_at(resp, "error").unwrap_or("không rõ lý do");
                return Err(format!("Claude CLI báo lỗi: {why}"));
            }
            return Ok(resp.get("response").cloned().unwrap_or(Value::Null));
        }
        // stdin drops here, so the CLI ends by itself
    })();
    let _ = child.kill();
    let _ = child.wait();
    result
}

// ---------- Codex ----------

/// `model/list` entries and the `config/read` answer of Codex → catalog.
pub fn codex_catalog(
    list: &[Value],
    config: &Value,
    user_agent: Option<&str>,
    read_at: &str,
) -> AgentCatalog {
    let models: Vec<AgentModel> = list
        .iter()
        .filter(|m| m.get("hidden").and_then(Value::as_bool) != Some(true))
        .filter_map(|m| {
            let id = str_at(m, "model").or_else(|| str_at(m, "id"))?.to_owned();
            Some(AgentModel {
                label: str_at(m, "displayName").unwrap_or(&id).to_owned(),
                description: str_at(m, "description").map(str::to_owned),
                efforts: m
                    .get("supportedReasoningEfforts")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|e| str_at(e, "reasoningEffort").or_else(|| e.as_str()))
                    .map(str::to_owned)
                    .collect(),
                default_effort: str_at(m, "defaultReasoningEffort").map(str::to_owned),
                is_default: m.get("isDefault").and_then(Value::as_bool) == Some(true),
                id,
            })
        })
        .collect();
    let cfg = config.get("config").unwrap_or(config);
    let default_model = str_at(cfg, "model")
        .map(str::to_owned)
        .or_else(|| models.iter().find(|m| m.is_default).map(|m| m.id.clone()));
    let default_entry = default_model
        .as_ref()
        .and_then(|d| models.iter().find(|m| &m.id == d));
    AgentCatalog {
        agent: AgentKind::Codex,
        default_id: default_entry.map(|m| m.id.clone()),
        default_label: default_entry
            .map(|m| m.label.clone())
            .or_else(|| default_model.clone()),
        default_effort: str_at(cfg, "model_reasoning_effort")
            .map(str::to_owned)
            .or_else(|| default_entry.and_then(|m| m.default_effort.clone())),
        default_model,
        from_cli: true,
        // "op-workflow/0.169.0 (Windows …)" → "0.169.0"
        cli_version: user_agent
            .and_then(|ua| ua.split_whitespace().next())
            .and_then(|first| first.rsplit('/').next())
            .filter(|v| v.chars().next().is_some_and(|c| c.is_ascii_digit()))
            .map(str::to_owned),
        error: None,
        read_at: Some(read_at.to_owned()),
        models,
        modes: Vec::new(),
    }
}

/// Codex without a list (the CLI cannot be asked): only what "Theo máy" is, from `config.toml`.
pub fn codex_unread(error: String, toml: Option<&str>) -> AgentCatalog {
    let top_level = |key: &str| -> Option<String> {
        toml?
            .lines()
            .take_while(|l| !l.trim_start().starts_with('['))
            .filter_map(|l| l.split_once('='))
            .find(|(k, _)| k.trim() == key)
            .map(|(_, v)| {
                v.split('#')
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .trim_matches(['"', '\''])
                    .to_owned()
            })
            .filter(|v| !v.is_empty())
    };
    let default_model = top_level("model");
    AgentCatalog {
        agent: AgentKind::Codex,
        models: Vec::new(),
        default_id: None,
        default_label: default_model.clone(),
        default_model,
        default_effort: top_level("model_reasoning_effort"),
        from_cli: false,
        cli_version: None,
        error: Some(error),
        read_at: None,
        modes: Vec::new(),
    }
}

/// `~/.codex/config.toml` (or `$CODEX_HOME/config.toml`).
fn codex_config_toml() -> Option<String> {
    let home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".codex")))?;
    std::fs::read_to_string(home.join("config.toml")).ok()
}

/// One JSON-RPC call to the app-server; notifications in between are skipped.
fn rpc(
    stdin: &mut ChildStdin,
    rx: &Receiver<String>,
    id: u64,
    method: &str,
    params: &Value,
) -> Result<Value, String> {
    send_line(
        stdin,
        &json!({"id": id, "method": method, "params": params}).to_string(),
    )?;
    let deadline = Instant::now() + READ_TIMEOUT;
    loop {
        let v = next_json(rx, deadline, "Codex CLI")?;
        if v.get("id").and_then(Value::as_u64) != Some(id) {
            continue;
        }
        return match v.get("error") {
            Some(err) => Err(format!(
                "Codex CLI báo lỗi ở {method}: {}",
                str_at(err, "message").map_or_else(|| err.to_string(), str::to_owned)
            )),
            None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
        };
    }
}

/// Ask a short-lived `codex app-server` for its models, its config and its version.
fn read_codex(exe: &Path) -> Result<(Vec<Value>, Value, Option<String>), String> {
    let mut child = hidden(
        Command::new(exe)
            .arg("app-server")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null()),
    )
    .spawn()
    .map_err(|e| format!("không chạy được codex: {e}"))?;
    let result = (|| {
        let mut stdin = child.stdin.take().ok_or("no stdin")?;
        let rx = line_reader(&mut child).ok_or("no stdout")?;
        let init = rpc(
            &mut stdin,
            &rx,
            1,
            "initialize",
            &json!({
                "clientInfo": {"name": "op-workflow", "title": null, "version": env!("CARGO_PKG_VERSION")},
                "capabilities": null
            }),
        )?;
        send_line(&mut stdin, &json!({"method": "initialized"}).to_string())?;
        let mut models = Vec::new();
        let mut cursor: Option<String> = None;
        for page in 0..MAX_PAGES {
            let params = cursor
                .as_ref()
                .map_or_else(|| json!({}), |c| json!({"cursor": c}));
            let result = rpc(&mut stdin, &rx, 2 + page, "model/list", &params)?;
            models.extend(
                result
                    .get("data")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
            );
            cursor = str_at(&result, "nextCursor").map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }
        if models.is_empty() {
            return Err("Codex CLI trả danh sách model trống (đã đăng nhập chưa?)".to_owned());
        }
        let config = rpc(&mut stdin, &rx, 100, "config/read", &json!({})).unwrap_or(Value::Null);
        Ok((models, config, str_at(&init, "userAgent").map(str::to_owned)))
    })();
    let _ = child.kill();
    let _ = child.wait();
    result
}

// ---------- shared ----------

/// The permission modes a CLI's `--help` lists (`claude --help`, `codex exec --help`); empty when
/// the help cannot be read. Costs no tokens.
fn read_modes(exe: &Path, args: &[&str], parse: fn(&str) -> Vec<String>) -> Vec<String> {
    hidden(
        Command::new(exe)
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null()),
    )
    .output()
    .ok()
    .map(|o| parse(&String::from_utf8_lossy(&o.stdout)))
    .unwrap_or_default()
}

fn send_line(stdin: &mut ChildStdin, line: &str) -> Result<(), String> {
    writeln!(stdin, "{line}")
        .and_then(|()| stdin.flush())
        .map_err(|e| e.to_string())
}

fn next_json(rx: &Receiver<String>, deadline: Instant, who: &str) -> Result<Value, String> {
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let line = rx.recv_timeout(left).map_err(|e| match e {
            RecvTimeoutError::Timeout => format!("{who} không trả lời sau {} giây", READ_TIMEOUT.as_secs()),
            RecvTimeoutError::Disconnected => format!("{who} thoát trước khi trả danh sách model"),
        })?;
        if let Ok(v) = serde_json::from_str::<Value>(&line) {
            return Ok(v);
        }
    }
}

fn read_all(probe_dir: &Path) -> StepModels {
    let now = opw_quota::now_rfc3339();
    let env_model = std::env::var("ANTHROPIC_MODEL").ok();
    std::thread::scope(|s| {
        let claude = s.spawn(|| {
            let settings = claude_settings();
            let Some(exe) = cli::find_claude() else {
                return claude_unread(
                    "Chưa cài Claude CLI (không thấy claude trên PATH)".to_owned(),
                    &settings,
                    env_model.as_deref(),
                );
            };
            match read_claude(&exe, probe_dir) {
                Ok(init)
                    if init
                        .get("models")
                        .and_then(Value::as_array)
                        .is_some_and(|m| !m.is_empty()) =>
                {
                    let mut c = claude_catalog(&init, &settings, env_model.as_deref(), &now);
                    c.modes = read_modes(&exe, &["--help"], claude::permission_modes);
                    c
                }
                Ok(_) => claude_unread(
                    "Claude CLI không trả danh sách model".to_owned(),
                    &settings,
                    env_model.as_deref(),
                ),
                Err(e) => claude_unread(e, &settings, env_model.as_deref()),
            }
        });
        let codex = s.spawn(|| {
            let Some(exe) = cli::find_codex() else {
                return codex_unread(
                    "Chưa cài Codex CLI (không thấy codex trên PATH)".to_owned(),
                    codex_config_toml().as_deref(),
                );
            };
            match read_codex(&exe) {
                Ok((list, config, ua)) => {
                    let mut c = codex_catalog(&list, &config, ua.as_deref(), &now);
                    c.modes = read_modes(&exe, &["exec", "--help"], codex::permission_modes);
                    c
                }
                Err(e) => codex_unread(e, codex_config_toml().as_deref()),
            }
        });
        StepModels {
            claude: claude
                .join()
                .unwrap_or_else(|_| claude_unread("lỗi khi đọc".to_owned(), &Value::Null, None)),
            codex: codex
                .join()
                .unwrap_or_else(|_| codex_unread("lỗi khi đọc".to_owned(), None)),
        }
    })
}

/// Lists read from the CLIs, reused for [`TTL`].
#[derive(Debug, Default)]
pub struct Catalogs {
    value: Mutex<Option<(Instant, StepModels)>>,
    /// One read at a time; a second caller waits and gets the fresh lists.
    reading: Mutex<()>,
}

impl Catalogs {
    /// The cached lists, or fresh ones when there are none yet, they are older than [`TTL`] or
    /// `refresh` is set. Starts the CLIs (about 2 s), so call it off the UI thread.
    pub fn get(&self, probe_dir: &Path, refresh: bool) -> StepModels {
        let started = Instant::now();
        let _reading = self
            .reading
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((at, models)) = self.value.lock().ok().and_then(|v| v.clone()) {
            // a read that finished while this call waited counts as fresh
            if (!refresh && at.elapsed() < TTL) || at >= started {
                return models;
            }
        }
        let read = read_all(probe_dir);
        let fresh = StepModels {
            claude: keep_last_good(probe_dir, read.claude),
            codex: keep_last_good(probe_dir, read.codex),
        };
        if let Ok(mut v) = self.value.lock() {
            *v = Some((Instant::now(), fresh.clone()));
        }
        fresh
    }

    /// The last lists read, however old; never starts a CLI.
    pub fn cached(&self) -> Option<StepModels> {
        self.value.lock().ok()?.as_ref().map(|(_, m)| m.clone())
    }
}

/// Where the last list a CLI gave is kept.
fn saved_path(probe_dir: &Path, agent: AgentKind) -> PathBuf {
    let name = if agent == AgentKind::Codex {
        "codex"
    } else {
        "claude"
    };
    probe_dir.join(format!("models-{name}.json"))
}

/// A list read from the CLI is saved; a failed read gets the last saved list instead (still
/// `from_cli: false` with the new `error`; its `read_at` says when it was read). Without a saved
/// list the failed read stays as it is (no models, only "Theo máy").
pub fn keep_last_good(probe_dir: &Path, read: AgentCatalog) -> AgentCatalog {
    let path = saved_path(probe_dir, read.agent);
    if read.from_cli {
        if let Ok(text) = serde_json::to_string(&read) {
            let _ = std::fs::create_dir_all(probe_dir);
            let _ = std::fs::write(&path, text);
        }
        return read;
    }
    let saved = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str::<AgentCatalog>(&t).ok())
        .filter(|c| c.from_cli && c.agent == read.agent && !c.models.is_empty());
    match saved {
        Some(mut c) => {
            c.from_cli = false;
            c.error = read.error;
            c
        }
        None => read,
    }
}

/// Does the CLI take `effort` for `model` (`None` = the machine's default model)? Without a list
/// read from the CLI, any plain lowercase word passes; the CLI itself has the last say.
pub fn check_effort(catalog: Option<&AgentCatalog>, model: Option<&str>, effort: &str) -> Result<(), String> {
    let plain = !effort.is_empty() && effort.len() <= 16 && effort.chars().all(|c| c.is_ascii_lowercase());
    if !plain {
        return Err(format!("Effort không hợp lệ: {effort}"));
    }
    let Some(catalog) = catalog.filter(|c| c.from_cli) else {
        return Ok(());
    };
    let picked = model.or(catalog.default_id.as_deref());
    match picked.and_then(|m| catalog.models.iter().find(|x| x.id == m)) {
        Some(m) if m.efforts.is_empty() => Err(format!("{} không nhận effort", m.label)),
        Some(m) if m.efforts.iter().any(|e| e == effort) => Ok(()),
        Some(m) => Err(format!(
            "{} không có effort {effort} (có: {})",
            m.label,
            m.efforts.join(", ")
        )),
        // a saved model the list no longer has: any level some model takes
        None if catalog
            .models
            .iter()
            .any(|m| m.efforts.iter().any(|e| e == effort)) =>
        {
            Ok(())
        }
        None => Err(format!("Không model nào của máy có effort {effort}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claude_init() -> Value {
        json!({
            "claude_code_version": "2.1.325",
            "models": [
                {"value": "default", "resolvedModel": "claude-opus-5-5", "displayName": "Default (recommended)",
                 "supportsEffort": true, "supportedEffortLevels": ["low", "medium", "high", "xhigh", "max"]},
                {"value": "opus", "resolvedModel": "claude-opus-5-5", "displayName": "Opus 5.5",
                 "description": "For complex work", "supportsEffort": true,
                 "supportedEffortLevels": ["low", "medium", "high", "xhigh", "max"]},
                {"value": "sonnet", "resolvedModel": "claude-sonnet-5-5", "displayName": "Sonnet 5.5",
                 "supportsEffort": true, "supportedEffortLevels": ["low", "medium", "high", "xhigh", "max"]},
                {"value": "claude-opus-5-5", "resolvedModel": "claude-opus-5-5", "displayName": "Opus 5.5 again"},
                {"value": "claude-haiku-4-5-20251001", "resolvedModel": "claude-haiku-4-5-20251001",
                 "displayName": "Haiku 4.5", "supportsThinkingOff": true},
                {"value": "claude-opus-4-6", "resolvedModel": "claude-opus-4-6", "displayName": "Opus 4.6",
                 "supportsEffort": true, "supportedEffortLevels": ["low", "medium", "high", "max"]}
            ]
        })
    }

    #[test]
    fn claude_list_follows_the_cli() {
        let c = claude_catalog(&claude_init(), &Value::Null, None, "2026-11-16T09:00:00Z");
        let ids: Vec<&str> = c.models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "claude-opus-5-5",
                "claude-sonnet-5-5",
                "claude-haiku-4-5-20251001",
                "claude-opus-4-6"
            ]
        );
        assert_eq!(c.models[0].label, "Opus 5.5");
        assert!(c.models[0].is_default);
        assert!(c.models[2].efforts.is_empty(), "Haiku 4.5 takes no effort");
        assert_eq!(c.models[3].efforts, ["low", "medium", "high", "max"]);
        assert_eq!(c.default_id.as_deref(), Some("claude-opus-5-5"));
        assert_eq!(c.default_label.as_deref(), Some("Opus 5.5"));
        assert_eq!(c.cli_version.as_deref(), Some("2.1.325"));
        assert!(c.from_cli && c.error.is_none());
    }

    #[test]
    fn claude_default_comes_from_settings_or_env() {
        let settings = json!({"model": "opus[1m]", "effortLevel": "high"});
        let c = claude_catalog(&claude_init(), &settings, None, "t");
        assert_eq!(c.default_model.as_deref(), Some("opus[1m]"));
        assert_eq!(c.default_id.as_deref(), Some("claude-opus-5-5"));
        assert_eq!(c.default_label.as_deref(), Some("Opus 5.5 · 1M"));
        assert_eq!(c.default_effort.as_deref(), Some("high"));
        let env = claude_catalog(&claude_init(), &settings, Some("claude-opus-4-6"), "t");
        assert_eq!(env.default_label.as_deref(), Some("Opus 4.6"));
        let unknown = claude_catalog(&claude_init(), &json!({"model": "my-proxy-model"}), None, "t");
        assert_eq!(unknown.default_id, None);
        assert_eq!(unknown.default_label.as_deref(), Some("my-proxy-model"));
    }

    #[test]
    fn unread_lists_only_say_what_theo_may_is() {
        let c = claude_unread("Chưa cài Claude CLI".into(), &json!({"model": "sonnet"}), None);
        assert!(!c.from_cli);
        assert_eq!(c.error.as_deref(), Some("Chưa cài Claude CLI"));
        assert!(c.models.is_empty());
        assert_eq!(c.default_label.as_deref(), Some("sonnet"));
        let toml =
            "model = \"gpt-a\"\nmodel_reasoning_effort = \"high\"\n[profiles.fast]\nmodel = \"gpt-b\"\n";
        let x = codex_unread("không trả lời".into(), Some(toml));
        assert!(x.models.is_empty());
        assert_eq!(x.default_model.as_deref(), Some("gpt-a"));
        assert_eq!(x.default_label.as_deref(), Some("gpt-a"));
        assert_eq!(x.default_effort.as_deref(), Some("high"));
        assert_eq!(codex_unread("x".into(), None).default_model, None);
    }

    #[test]
    fn a_failed_read_uses_the_last_list_the_cli_gave() {
        let dir = tempfile::tempdir().unwrap();
        let unread = || codex_unread("Codex CLI không trả lời sau 20 giây".into(), None);
        // nothing saved yet: no models
        assert!(keep_last_good(dir.path(), unread()).models.is_empty());
        let good = codex_catalog(&codex_list(), &Value::Null, None, "2026-11-16T12:31:00+07:00");
        assert_eq!(keep_last_good(dir.path(), good.clone()), good);
        let later = keep_last_good(dir.path(), unread());
        assert!(!later.from_cli);
        assert_eq!(later.models, good.models);
        assert_eq!(later.read_at.as_deref(), Some("2026-11-16T12:31:00+07:00"));
        assert_eq!(
            later.error.as_deref(),
            Some("Codex CLI không trả lời sau 20 giây")
        );
        // each agent has its own file
        let claude = keep_last_good(dir.path(), claude_unread("x".into(), &Value::Null, None));
        assert!(claude.models.is_empty());
    }

    fn codex_list() -> Vec<Value> {
        let efforts = |levels: &[&str]| -> Value {
            levels
                .iter()
                .map(|l| json!({"reasoningEffort": l, "description": "…"}))
                .collect()
        };
        vec![
            json!({"id": "gpt-6.1-sol", "model": "gpt-6.1-sol", "displayName": "GPT-6.1-Sol", "hidden": false,
                   "supportedReasoningEfforts": efforts(&["low", "medium", "high", "xhigh", "max", "ultra"]),
                   "defaultReasoningEffort": "low", "isDefault": true}),
            json!({"id": "gpt-6-luna", "model": "gpt-6-luna", "displayName": "GPT-6-Luna",
                   "supportedReasoningEfforts": efforts(&["low", "medium", "high", "xhigh", "max"]),
                   "defaultReasoningEffort": "medium", "isDefault": false}),
            json!({"id": "secret", "model": "secret", "hidden": true, "supportedReasoningEfforts": []}),
        ]
    }

    #[test]
    fn codex_list_follows_the_app_server() {
        let config = json!({"config": {"model": "gpt-6-luna", "model_reasoning_effort": "xhigh"}});
        let ua = "op-workflow/0.169.0 (Windows 10.0.19045; x86_64) xterm-256color (op-workflow; 0.2.0)";
        let c = codex_catalog(&codex_list(), &config, Some(ua), "t");
        let ids: Vec<&str> = c.models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["gpt-6.1-sol", "gpt-6-luna"], "hidden models are left out");
        assert_eq!(c.models[0].efforts.last().map(String::as_str), Some("ultra"));
        assert_eq!(c.models[1].default_effort.as_deref(), Some("medium"));
        assert_eq!(c.default_label.as_deref(), Some("GPT-6-Luna"));
        assert_eq!(c.default_effort.as_deref(), Some("xhigh"));
        assert_eq!(c.cli_version.as_deref(), Some("0.169.0"));
        // without a config the CLI's own default model and its default effort
        let bare = codex_catalog(&codex_list(), &Value::Null, None, "t");
        assert_eq!(bare.default_id.as_deref(), Some("gpt-6.1-sol"));
        assert_eq!(bare.default_effort.as_deref(), Some("low"));
        assert_eq!(bare.cli_version, None);
    }

    /// Reads the real CLIs of this machine (no prompt, no tokens):
    /// `cargo test -p opw-orchestrator --lib models::tests::reads_the_real_clis -- --ignored --nocapture`
    #[test]
    #[ignore = "starts the real claude / codex CLIs"]
    fn reads_the_real_clis() {
        let dir = tempfile::tempdir().unwrap();
        let started = Instant::now();
        let m = Catalogs::default().get(dir.path(), true);
        for c in [&m.claude, &m.codex] {
            println!(
                "{:?} v{:?} from_cli={} error={:?} default={:?} ({:?}) effort={:?}",
                c.agent,
                c.cli_version,
                c.from_cli,
                c.error,
                c.default_model,
                c.default_label,
                c.default_effort
            );
            println!("  modes: {:?}", c.modes);
            for x in &c.models {
                println!(
                    "  {} | {} | {:?}{}",
                    x.id,
                    x.label,
                    x.efforts,
                    if x.is_default { " *" } else { "" }
                );
            }
        }
        println!("took {:?}", started.elapsed());
    }

    #[test]
    fn efforts_are_checked_against_the_model() {
        let claude = claude_catalog(&claude_init(), &Value::Null, None, "t");
        let codex = codex_catalog(&codex_list(), &Value::Null, None, "t");
        assert!(check_effort(Some(&claude), Some("claude-sonnet-5-5"), "max").is_ok());
        assert!(check_effort(Some(&claude), Some("claude-opus-4-6"), "xhigh").is_err());
        assert!(check_effort(Some(&claude), Some("claude-haiku-4-5-20251001"), "low").is_err());
        assert!(
            check_effort(Some(&claude), None, "xhigh").is_ok(),
            "default = Opus 5.5"
        );
        assert!(check_effort(Some(&codex), Some("gpt-6.1-sol"), "ultra").is_ok());
        assert!(check_effort(Some(&codex), Some("gpt-6-luna"), "ultra").is_err());
        assert!(check_effort(Some(&codex), Some("gpt-6.1-sol"), "minimal").is_err());
        assert!(check_effort(Some(&codex), Some("old-saved-name"), "ultra").is_ok());
        assert!(check_effort(Some(&codex), Some("old-saved-name"), "huge").is_err());
        // without a list from the CLI: any plain word, nothing that could break the flag
        let unread = codex_unread("x".into(), None);
        assert!(check_effort(Some(&unread), None, "minimal").is_ok());
        assert!(check_effort(None, None, "max").is_ok());
        for bad in ["", "High", "x\"y", "--yolo", "a b"] {
            assert!(check_effort(None, None, bad).is_err(), "{bad}");
        }
    }
}
