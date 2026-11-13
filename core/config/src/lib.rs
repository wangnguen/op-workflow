//! Single place that resolves paths and loads configuration.
//!
//! Rules (from PLAN.md §11): unknown keys are ignored; a section with an invalid value falls
//! back to its defaults instead of failing the whole file; dev builds use a separate data dir.

pub mod secrets;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use opw_domain::finding::GatePolicy;
use opw_domain::task::DEFAULT_BRANCH_TEMPLATE;
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
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Config {
    pub tracker: TrackerConfig,
    pub quota: QuotaConfig,
    pub handoff: HandoffConfig,
    pub gate: GatePolicy,
    pub workspaces: Vec<String>,
    pub projects: BTreeMap<String, ProjectConfig>,
    pub repos: BTreeMap<String, RepoConfig>,
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
            gate: section(root, "gate", &mut warnings),
            workspaces: section(root, "workspaces", &mut warnings),
            projects: section(root, "projects", &mut warnings),
            repos: section(root, "repos", &mut warnings),
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
    if let Some(obj) = root.as_object_mut() {
        let tracker = obj
            .entry("tracker")
            .or_insert_with(|| Value::Object(Default::default()));
        if !tracker.is_object() {
            *tracker = Value::Object(Default::default());
        }
        if let Some(t) = tracker.as_object_mut() {
            t.insert("baseUrl".into(), Value::String(base_url.to_owned()));
        }
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

/// Expand a leading `~` the way config files write repo paths.
pub fn expand_home(path: &str) -> PathBuf {
    match (
        path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")),
        dirs::home_dir(),
    ) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(path),
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
    }
}
