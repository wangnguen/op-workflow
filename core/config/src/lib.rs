//! Single place that resolves paths and loads configuration.
//!
//! Rules (from PLAN.md §11): unknown keys are ignored; a section with an invalid value falls
//! back to its defaults instead of failing the whole file; dev builds use a separate data dir.

pub mod secrets;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use opw_domain::finding::GatePolicy;
use opw_domain::task::DEFAULT_BRANCH_TEMPLATE;
use opw_domain::view::{NotifyKind, NotifySettings};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const ENV_CONFIG_PATH: &str = "OPW_CONFIG_PATH";
pub const ENV_DATA_DIR: &str = "OPW_DATA_DIR";
const DATA_DIR_NAME: &str = ".op-workflow";
const DEV_DATA_DIR_NAME: &str = ".op-workflow-dev";
const CONFIG_FILE: &str = "config.json";

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot find the user's home directory")]
    NoHome,
    #[error("cannot read {path}: {source}")]
    Read { path: PathBuf, source: std::io::Error },
    #[error("{path} is not valid JSON: {source}")]
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("cannot write {path}: {source}")]
    Write { path: PathBuf, source: std::io::Error },
}

pub type ConfigResult<T> = Result<T, ConfigError>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TrackerConfig {
    /// OpenProject URL, entered in Settings (empty until then).
    pub base_url: String,
    /// Prefix shown before work package ids, e.g. `WP` → `WP-187716`.
    pub task_prefix: String,
    /// Reference into Windows Credential Manager, never the token itself.
    pub token: String,
    pub poll_min: u32,
}

impl Default for TrackerConfig {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            task_prefix: "WP".into(),
            token: "credman:op-workflow/openproject".into(),
            poll_min: 5,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct QuotaConfig {
    pub codex_poll_sec: u32,
    pub claude_probe_min: u32,
    pub claude_probe_model: String,
    pub warn_at: Vec<u8>,
    /// Undocumented OAuth usage endpoints; off unless the user opts in.
    pub oauth_endpoints: bool,
}

impl Default for QuotaConfig {
    fn default() -> Self {
        Self {
            codex_poll_sec: 60,
            // each probe costs ~735 Haiku tokens; 15 min keeps it small (headless steps
            // also report rate_limit_event for free)
            claude_probe_min: 15,
            claude_probe_model: "haiku".into(),
            warn_at: vec![80, 95],
            oauth_endpoints: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HandoffConfig {
    pub claude_at_tokens: u64,
    pub codex_at_percent: u8,
}

impl Default for HandoffConfig {
    fn default() -> Self {
        Self {
            claude_at_tokens: 500_000,
            codex_at_percent: 80,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProjectConfig {
    pub repos: Vec<String>,
    pub branch: Option<String>,
}

impl ProjectConfig {
    pub fn branch_template(&self) -> &str {
        self.branch.as_deref().unwrap_or(DEFAULT_BRANCH_TEMPLATE)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RepoConfig {
    /// Base picked last time; pre-selected in the "Bắt đầu làm việc" dialog.
    pub last_base: Option<String>,
    pub deny_paths: Vec<String>,
    pub post_edit: Vec<String>,
    /// Commands the AI may run while coding in this repo (`npm run lint *`, `npx tsc *`), on top
    /// of the repo's own `.claude/settings.json` allow list. Anything else is refused.
    pub allow_bash: Vec<String>,
}

/// Limits of one workflow step (`steps.<step>` in config). Unset fields use the app's defaults.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StepConfig {
    /// `claude` or `codex` (cross-review only for now).
    pub agent: Option<String>,
    /// Model the user picked in Settings (`claude-sonnet-5-5`, a Codex model). `None` = whatever
    /// the CLI on this machine uses; the app never picks one by itself.
    pub model: Option<String>,
    /// `low` … `max` (Claude `--effort`) or Codex `model_reasoning_effort`. `None` = the machine's.
    pub effort: Option<String>,
    pub max_budget_usd: Option<f64>,
    /// Cross-review ⇄ fix rounds before the final review.
    pub max_rounds: Option<u8>,
    pub timeout_min: Option<u32>,
}

/// Steps 2-8 (`steps` in config).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StepsConfig {
    /// Run the next step by itself when one finishes. Off by default: PLAN.md §12 asks for a few
    /// real tasks in "stop after each step" mode first. Each task can turn it on for itself.
    pub auto_advance: bool,
    pub analyze: StepConfig,
    pub implement: StepConfig,
    pub review_local: StepConfig,
    pub fix: StepConfig,
    pub cross_review: StepConfig,
    /// Model and effort of the final review. Its budget and timeout fall back to `reviewLocal`
    /// (both used to share that section).
    pub final_review: StepConfig,
    pub push_summary: StepConfig,
}

/// What the user picked for one step in Settings → Các bước AI.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StepChoice<'a> {
    /// `analyze`, `reviewLocal`, … (the key under `steps`).
    pub step: &'a str,
    pub agent: Option<&'a str>,
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
}

/// Write `steps.<step>.agent/model/effort`; `None` removes the key (back to the machine's
/// default). Budget, timeout and other keys stay.
pub fn save_step_choice(path: &Path, c: &StepChoice<'_>) -> ConfigResult<()> {
    edit_file(path, |root| {
        let step = object_at(object_at(root, "steps"), c.step);
        for (key, value) in [("agent", c.agent), ("model", c.model), ("effort", c.effort)] {
            match value.map(str::trim).filter(|v| !v.is_empty()) {
                Some(v) => {
                    step.insert(key.into(), Value::String(v.to_owned()));
                }
                None => {
                    step.remove(key);
                }
            }
        }
    })
}

/// New-version check against the public GitHub Releases of the app (read-only GET).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UpdateConfig {
    pub check: bool,
    /// `owner/repo` on GitHub.
    pub repo: String,
    /// Count pre-releases as updates (every release so far is a pre-release).
    pub include_prerelease: bool,
    pub every_hours: u32,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            check: true,
            repo: "wangnguen/op-workflow".into(),
            include_prerelease: true,
            every_hours: 6,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Config {
    pub tracker: TrackerConfig,
    pub quota: QuotaConfig,
    pub handoff: HandoffConfig,
    pub update: UpdateConfig,
    pub gate: GatePolicy,
    pub steps: StepsConfig,
    /// Windows notifications by kind (`notify`); all on by default.
    pub notify: NotifySettings,
    pub workspaces: Vec<String>,
    pub projects: BTreeMap<String, ProjectConfig>,
    pub repos: BTreeMap<String, RepoConfig>,
    /// Folder holding `<task id>/brief.md` (`tasksDir`, `~` allowed). `None` = the user has not
    /// chosen yet: briefs go to `<data dir>/tasks` and the app offers the Desktop once.
    pub tasks_dir: Option<String>,
    /// Sections that were invalid and replaced by defaults (shown as a warning in the UI).
    pub warnings: Vec<String>,
}

impl Config {
    /// Parse config JSON. Never fails on content: bad sections become defaults + a warning.
    pub fn from_value(root: &Value) -> Self {
        let mut warnings = Vec::new();
        let mut cfg = Config {
            tracker: section(root, "tracker", &mut warnings),
            quota: section(root, "quota", &mut warnings),
            handoff: section(root, "handoff", &mut warnings),
            update: section(root, "update", &mut warnings),
            gate: section(root, "gate", &mut warnings),
            steps: section(root, "steps", &mut warnings),
            notify: section(root, "notify", &mut warnings),
            workspaces: section(root, "workspaces", &mut warnings),
            projects: section(root, "projects", &mut warnings),
            repos: section(root, "repos", &mut warnings),
            tasks_dir: section::<Option<String>>(root, "tasksDir", &mut warnings)
                .filter(|d| !d.trim().is_empty()),
            warnings: Vec::new(),
        };
        if cfg.workspaces.is_empty() {
            cfg.workspaces = vec!["~/Desktop".into()];
        }
        cfg.warnings = warnings;
        cfg
    }

    /// Load from `path`; a missing file means "all defaults".
    pub fn load(path: &Path) -> ConfigResult<Self> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::from_value(&Value::Null)),
            Err(source) => {
                return Err(ConfigError::Read {
                    path: path.to_owned(),
                    source,
                });
            }
        };
        let root: Value = serde_json::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.to_owned(),
            source,
        })?;
        Ok(Self::from_value(&root))
    }
}

fn section<T: DeserializeOwned + Default>(root: &Value, key: &str, warnings: &mut Vec<String>) -> T {
    match root.get(key) {
        None | Some(Value::Null) => T::default(),
        Some(v) => serde_json::from_value(v.clone()).unwrap_or_else(|e| {
            warnings.push(format!("config `{key}` is invalid ({e}); using defaults"));
            T::default()
        }),
    }
}

/// Write `tracker.baseUrl` into the config file, keeping every other key as it is.
pub fn save_tracker_base_url(path: &Path, base_url: &str) -> ConfigResult<()> {
    edit_file(path, |root| {
        object_at(root, "tracker").insert("baseUrl".into(), Value::String(base_url.to_owned()));
    })
}

/// Turn one kind of Windows notification on or off (`notify.<kind>`), keeping every other key.
pub fn save_notify(path: &Path, kind: NotifyKind, on: bool) -> ConfigResult<()> {
    let key = match kind {
        NotifyKind::Comments => "comments",
        NotifyKind::Steps => "steps",
        NotifyKind::Quota => "quota",
    };
    edit_file(path, |root| {
        object_at(root, "notify").insert(key.into(), Value::Bool(on));
    })
}

/// Write `tasksDir` (where briefs live), keeping every other key as it is.
pub fn save_tasks_dir(path: &Path, dir: &str) -> ConfigResult<()> {
    edit_file(path, |root| {
        root.insert("tasksDir".into(), Value::String(dir.to_owned()));
    })
}

/// What "Bắt đầu làm việc" remembers for next time (PLAN.md §0 "app nhớ cho lần sau").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartMemory<'a> {
    /// OpenProject project name.
    pub project: &'a str,
    /// Repo path as written in config (`~/Desktop/acme-web`).
    pub repo: &'a str,
    /// Branch template derived from the name the user confirmed, if it contains the task id.
    pub branch_template: Option<&'a str>,
    pub base: &'a str,
}

/// Remember the repo (first in `projects.<project>.repos`), the branch template of the project
/// and the base of the repo. Other keys stay untouched.
pub fn remember_start(path: &Path, m: &StartMemory<'_>) -> ConfigResult<()> {
    edit_file(path, |root| {
        let project = object_at(object_at(root, "projects"), m.project);
        let mut repos: Vec<Value> = project
            .get("repos")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        repos.retain(|r| r.as_str().is_none_or(|s| !same_path(s, m.repo)));
        repos.insert(0, Value::String(m.repo.to_owned()));
        project.insert("repos".into(), Value::Array(repos));
        if let Some(t) = m.branch_template {
            project.insert("branch".into(), Value::String(t.to_owned()));
        }
        let repos = object_at(root, "repos");
        let key = repos
            .keys()
            .find(|k| same_path(k, m.repo))
            .cloned()
            .unwrap_or_else(|| m.repo.to_owned());
        object_at(repos, &key).insert("lastBase".into(), Value::String(m.base.to_owned()));
    })
}

/// Same repo path, whatever the `~`, slash direction or (on Windows) letter case.
pub fn same_path(a: &str, b: &str) -> bool {
    let norm = |p: &str| {
        let s = expand_home(p).to_string_lossy().replace('\\', "/");
        let s = s.trim_end_matches('/').to_owned();
        if cfg!(windows) { s.to_lowercase() } else { s }
    };
    norm(a) == norm(b)
}

/// `C:\Users\me\Desktop\web` → `~/Desktop/web`; paths outside home keep their form, with `/`.
pub fn contract_home(path: &Path) -> String {
    let as_slashes = |p: &Path| p.to_string_lossy().replace('\\', "/");
    if let Some(home) = dirs::home_dir() {
        if let Ok(rest) = path.strip_prefix(&home) {
            let rest = as_slashes(rest);
            return if rest.is_empty() {
                "~".into()
            } else {
                format!("~/{rest}")
            };
        }
    }
    as_slashes(path)
}

/// The object under `key`, created (or replacing a non-object) when needed.
fn object_at<'a>(
    obj: &'a mut serde_json::Map<String, Value>,
    key: &str,
) -> &'a mut serde_json::Map<String, Value> {
    let slot = obj
        .entry(key.to_owned())
        .or_insert_with(|| Value::Object(Default::default()));
    if !slot.is_object() {
        *slot = Value::Object(Default::default());
    }
    match slot {
        Value::Object(map) => map,
        _ => unreachable!("slot was just made an object"),
    }
}

/// Read-modify-write of the config file; unknown keys survive, a missing file starts empty.
fn edit_file(path: &Path, f: impl FnOnce(&mut serde_json::Map<String, Value>)) -> ConfigResult<()> {
    let mut root: Value = match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.to_owned(),
            source,
        })?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Value::Object(Default::default()),
        Err(source) => {
            return Err(ConfigError::Read {
                path: path.to_owned(),
                source,
            });
        }
    };
    if !root.is_object() {
        root = Value::Object(Default::default());
    }
    if let Value::Object(obj) = &mut root {
        f(obj);
    }
    let write_err = |source| ConfigError::Write {
        path: path.to_owned(),
        source,
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(write_err)?;
    }
    let text = serde_json::to_string_pretty(&root).unwrap_or_default();
    std::fs::write(path, text + "\n").map_err(write_err)
}

/// `~/.op-workflow` (release) or `~/.op-workflow-dev` (debug), overridable by `OPW_DATA_DIR`.
pub fn data_dir() -> ConfigResult<PathBuf> {
    if let Some(dir) = std::env::var_os(ENV_DATA_DIR) {
        return Ok(PathBuf::from(dir));
    }
    let name = if cfg!(debug_assertions) {
        DEV_DATA_DIR_NAME
    } else {
        DATA_DIR_NAME
    };
    dirs::home_dir().map(|h| h.join(name)).ok_or(ConfigError::NoHome)
}

/// `OPW_CONFIG_PATH` or `<data_dir>/config.json`.
pub fn config_path() -> ConfigResult<PathBuf> {
    match std::env::var_os(ENV_CONFIG_PATH) {
        Some(p) => Ok(PathBuf::from(p)),
        None => Ok(data_dir()?.join(CONFIG_FILE)),
    }
}

/// Expand a leading `~` the way config files write repo paths, with the platform's own
/// separators: config files use `/`, and on Windows a mixed path such as
/// `C:\Users\me\Desktop/op-workflow-tasks\WP-1` works for file I/O but not for Explorer
/// (`/select,` then opens Documents instead).
pub fn expand_home(path: &str) -> PathBuf {
    let native = |p: &str| {
        if cfg!(windows) {
            p.replace('/', "\\")
        } else {
            p.to_owned()
        }
    };
    let home = dirs::home_dir();
    if path == "~" {
        if let Some(home) = home {
            return home;
        }
    }
    match (path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")), home) {
        (Some(rest), Some(home)) => home.join(native(rest)),
        _ => PathBuf::from(native(path)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opw_domain::finding::Severity;
    use serde_json::json;

    #[test]
    fn empty_config_gives_defaults() {
        let cfg = Config::from_value(&Value::Null);
        assert_eq!(cfg.tracker.base_url, "");
        assert_eq!(cfg.tracker.task_prefix, "WP");
        assert_eq!(cfg.handoff.claude_at_tokens, 500_000);
        assert_eq!(cfg.gate.block, vec![Severity::Critical, Severity::High]);
        assert_eq!(cfg.workspaces, vec!["~/Desktop"]);
        assert!(cfg.warnings.is_empty());
    }

    #[test]
    fn unknown_keys_ignored_and_bad_section_falls_back() {
        let cfg = Config::from_value(&json!({
            "somethingNew": 1,
            "quota": { "codexPollSec": 30, "extra": true },
            "handoff": { "claudeAtTokens": "lots" },
            "projects": { "Web Platform": { "repos": ["~/code/acme-web"] } }
        }));
        assert_eq!(cfg.quota.codex_poll_sec, 30);
        assert_eq!(
            cfg.quota.claude_probe_min,
            QuotaConfig::default().claude_probe_min
        );
        assert_eq!(cfg.handoff, HandoffConfig::default());
        assert_eq!(cfg.warnings.len(), 1);
        assert_eq!(cfg.projects["Web Platform"].branch_template(), "feature/{taskId}");
    }

    #[test]
    fn steps_section_keeps_what_is_set() {
        let cfg = Config::from_value(&json!({
            "steps": {
                "autoAdvance": true,
                "analyze": { "maxBudgetUsd": 2.5 },
                "crossReview": { "agent": "claude", "maxRounds": 2 }
            },
            "repos": { "~/code/web": { "allowBash": ["npm run lint *"] } }
        }));
        assert!(cfg.steps.auto_advance);
        assert_eq!(cfg.steps.analyze.max_budget_usd, Some(2.5));
        assert_eq!(cfg.steps.cross_review.agent.as_deref(), Some("claude"));
        assert_eq!(cfg.steps.cross_review.max_rounds, Some(2));
        assert_eq!(cfg.steps.fix, StepConfig::default());
        assert_eq!(cfg.repos["~/code/web"].allow_bash, ["npm run lint *"]);
        assert!(!Config::from_value(&Value::Null).steps.auto_advance);
    }

    #[test]
    fn step_choices_are_saved_and_cleared_without_touching_limits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{"steps":{"autoAdvance":true,"fix":{"maxBudgetUsd":3,"model":"old"}}}"#,
        )
        .unwrap();
        save_step_choice(
            &path,
            &StepChoice {
                step: "fix",
                agent: None,
                model: Some("claude-sonnet-5-5"),
                effort: Some("high"),
            },
        )
        .unwrap();
        save_step_choice(
            &path,
            &StepChoice {
                step: "finalReview",
                agent: None,
                model: Some(" "),
                effort: Some("low"),
            },
        )
        .unwrap();
        let cfg = Config::load(&path).unwrap();
        assert!(cfg.steps.auto_advance);
        assert_eq!(cfg.steps.fix.model.as_deref(), Some("claude-sonnet-5-5"));
        assert_eq!(cfg.steps.fix.effort.as_deref(), Some("high"));
        assert_eq!(cfg.steps.fix.max_budget_usd, Some(3.0));
        assert_eq!(cfg.steps.final_review.model, None);
        assert_eq!(cfg.steps.final_review.effort.as_deref(), Some("low"));

        save_step_choice(
            &path,
            &StepChoice {
                step: "fix",
                ..StepChoice::default()
            },
        )
        .unwrap();
        let cfg = Config::load(&path).unwrap();
        assert_eq!((cfg.steps.fix.model, cfg.steps.fix.effort), (None, None));
        assert_eq!(cfg.steps.fix.max_budget_usd, Some(3.0));
    }

    #[test]
    fn missing_file_is_defaults_and_bad_json_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let missing = Config::load(&dir.path().join("none.json")).unwrap();
        assert_eq!(missing.quota, QuotaConfig::default());
        let bad = dir.path().join("bad.json");
        std::fs::write(&bad, "{ not json").unwrap();
        assert!(matches!(Config::load(&bad), Err(ConfigError::Parse { .. })));
    }

    #[test]
    fn saving_the_base_url_keeps_other_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("config.json");
        save_tracker_base_url(&path, "https://op.example.com").unwrap();
        assert_eq!(
            Config::load(&path).unwrap().tracker.base_url,
            "https://op.example.com"
        );
        std::fs::write(
            &path,
            r#"{"tracker":{"taskPrefix":"XY","baseUrl":"old"},"workspaces":["~/w"]}"#,
        )
        .unwrap();
        save_tracker_base_url(&path, "https://new.example.com").unwrap();
        let cfg = Config::load(&path).unwrap();
        assert_eq!(cfg.tracker.base_url, "https://new.example.com");
        assert_eq!(cfg.tracker.task_prefix, "XY");
        assert_eq!(cfg.workspaces, vec!["~/w"]);
        assert_eq!(cfg.tasks_dir, None);

        save_tasks_dir(&path, "~/Desktop/op-workflow-tasks").unwrap();
        let cfg = Config::load(&path).unwrap();
        assert_eq!(cfg.tasks_dir.as_deref(), Some("~/Desktop/op-workflow-tasks"));
        assert_eq!(cfg.tracker.task_prefix, "XY");
    }

    #[test]
    fn remembering_a_start_moves_the_repo_first_and_keeps_other_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{"tracker":{"taskPrefix":"XY"},
                "projects":{"Web Platform":{"repos":["~/code/api","~/code/web"],"branch":"old/{taskId}"}},
                "repos":{"~/code/web":{"denyPaths":[".env"]}}}"#,
        )
        .unwrap();
        remember_start(
            &path,
            &StartMemory {
                project: "Web Platform",
                repo: "~\\code\\web\\",
                branch_template: Some("f/XY-{taskId}"),
                base: "origin/development",
            },
        )
        .unwrap();
        let cfg = Config::load(&path).unwrap();
        let p = &cfg.projects["Web Platform"];
        assert_eq!(p.repos, vec!["~\\code\\web\\", "~/code/api"]);
        assert_eq!(p.branch_template(), "f/XY-{taskId}");
        // the existing repo key is reused, its other settings kept
        assert_eq!(cfg.repos.len(), 1);
        let web = &cfg.repos["~/code/web"];
        assert_eq!(web.last_base.as_deref(), Some("origin/development"));
        assert_eq!(web.deny_paths, vec![".env"]);
        assert_eq!(cfg.tracker.task_prefix, "XY");

        remember_start(
            &path,
            &StartMemory {
                project: "Mobile App",
                repo: "~/code/app",
                branch_template: None,
                base: "main",
            },
        )
        .unwrap();
        let cfg = Config::load(&path).unwrap();
        assert_eq!(cfg.projects["Mobile App"].branch, None);
        assert_eq!(cfg.repos["~/code/app"].last_base.as_deref(), Some("main"));
    }

    #[test]
    fn paths_compare_across_home_and_slashes() {
        assert!(same_path("~/code/web", "~\\code\\web\\"));
        let home = dirs::home_dir().unwrap();
        let abs = home.join("code").join("web");
        assert!(same_path("~/code/web", &abs.to_string_lossy()));
        assert_eq!(contract_home(&abs), "~/code/web");
        assert!(!same_path("~/code/web", "~/code/web2"));
    }

    /// Explorer's `/select,` needs one kind of separator (the "Xem brief" bug, 2026-11-14).
    #[test]
    fn expanded_paths_use_native_separators() {
        let home = dirs::home_dir().unwrap();
        let tasks = expand_home("~/Desktop/op-workflow-tasks");
        assert_eq!(tasks, home.join("Desktop").join("op-workflow-tasks"));
        assert_eq!(expand_home("~"), home);
        if cfg!(windows) {
            let brief = tasks.join("WP-1").join("brief.md");
            assert!(!brief.to_string_lossy().contains('/'), "{}", brief.display());
            assert_eq!(expand_home("C:/code/web"), PathBuf::from("C:\\code\\web"));
        }
    }
}
