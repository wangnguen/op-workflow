//! Orchestrator: the one place that combines tracker, vcs, agents, quota and storage.
//!
//! [`AppService`] is the facade used by the Tauri commands. Without an OpenProject token it serves
//! the [`demo`] timeline; once a token is saved it serves the real task inbox from [`live`].
//! Quota is always live (it needs no token).

pub mod demo;
pub mod live;
pub mod selftest;

use std::path::PathBuf;
use std::time::Duration;

use opw_config::{Config, secrets};
use opw_domain::task::display_id;
use opw_domain::view::{
    ApiError, AppInfo, BoardSnapshot, DataMode, InboxItem, OrbitSnapshot, QuotaSnapshot, SettingsView,
    TaskCard, TaskColumn, TodayStats,
};
use opw_tracker::HttpTracker;

use crate::live::{Live, LiveSettings, LiveState};

pub const CORE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Facade used by the Tauri commands. Every method returns plain view DTOs.
#[derive(Debug, Default)]
pub struct AppService {
    config: Config,
    config_path: Option<PathBuf>,
    live: Option<Live>,
}

impl AppService {
    /// Demo only, no background threads (tests, browser previews).
    pub fn new() -> Self {
        Self::default()
    }

    /// Load the config, read the saved OpenProject token and start the live refresh threads.
    pub fn start() -> Self {
        let config_path = opw_config::config_path().ok();
        let config = config_path
            .as_deref()
            .map(|p| Config::load(p).unwrap_or_default())
            .unwrap_or_default();
        let token = secrets::get(&config.tracker.token).ok().flatten();
        let probe_dir = opw_config::data_dir()
            .map(|d| d.join("probe"))
            .unwrap_or_else(|_| std::env::temp_dir().join("opw-probe"));
        let settings = LiveSettings {
            poll: Duration::from_secs(u64::from(config.tracker.poll_min.max(1)) * 60),
            codex_poll: Duration::from_secs(u64::from(config.quota.codex_poll_sec.max(15))),
            claude_probe: Duration::from_secs(u64::from(config.quota.claude_probe_min.max(1)) * 60),
            claude_probe_model: config.quota.claude_probe_model.clone(),
            probe_dir,
        };
        let live = Live::start(settings, config.tracker.base_url.clone(), token);
        Self {
            config,
            config_path,
            live: Some(live),
        }
    }

    pub fn app_info(&self, app_version: &str) -> AppInfo {
        AppInfo {
            version: app_version.to_owned(),
            core: CORE_VERSION.to_owned(),
        }
    }

    fn live_state(&self) -> Option<LiveState> {
        self.live.as_ref().map(Live::snapshot)
    }

    pub fn board_snapshot(&self) -> BoardSnapshot {
        let Some(state) = self.live_state() else {
            return demo::board(demo::tick_now());
        };
        if state.token.is_none() {
            let mut board = demo::board(demo::tick_now());
            board.quota = live_quota(&state);
            return board;
        }
        let tasks = state
            .tasks
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|wp| TaskCard {
                id: display_id(&self.config.tracker.task_prefix, wp.id),
                wp_id: wp.id,
                title: wp.subject.clone(),
                project: wp.project.clone(),
                repo: None,
                branch: None,
                nx_status: wp.status.clone(),
                column: TaskColumn::Inbox,
                step: None,
                agent: None,
                cost_usd: None,
                context_pct: None,
                new_comments: 0,
            })
            .collect();
        let notice = match (&state.tasks_error, &state.tasks) {
            (Some(e), _) => Some(e.clone()),
            (None, None) => Some("Đang tải task từ OpenProject…".to_owned()),
            (None, Some(_)) => None,
        };
        BoardSnapshot {
            tasks,
            active_task_id: None,
            quota: live_quota(&state),
            today: TodayStats {
                tokens: 0,
                cost_usd: 0.0,
            },
            mode: DataMode::Live,
            notice,
        }
    }

    pub fn orbit_snapshot(&self) -> OrbitSnapshot {
        let Some(state) = self.live_state() else {
            return demo::orbit(demo::tick_now());
        };
        if state.token.is_none() {
            let mut orbit = demo::orbit(demo::tick_now());
            orbit.quota = live_quota(&state);
            return orbit;
        }
        OrbitSnapshot {
            active: None,
            inbox: state
                .tasks
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|wp| InboxItem {
                    task_id: display_id(&self.config.tracker.task_prefix, wp.id),
                    title: wp.subject.clone(),
                    project: wp.project.clone(),
                })
                .collect(),
            distant: Vec::new(),
            quota: live_quota(&state),
        }
    }

    // ---------- settings ----------

    pub fn settings(&self) -> SettingsView {
        let state = self.live_state().unwrap_or_else(|| LiveState {
            base_url: self.config.tracker.base_url.clone(),
            ..LiveState::default()
        });
        SettingsView {
            nx_base_url: state.base_url,
            nx_token_set: state.token.is_some(),
            nx_user: state.nx_user,
            nx_task_count: state.tasks.as_ref().map(Vec::len),
            nx_last_sync: state.last_sync,
            nx_error: state.tasks_error,
            claude_quota_at: state.claude.map(|q| q.observed_at),
            codex_quota_at: state.codex.map(|q| q.observed_at),
            quota_error: state.quota_error,
        }
    }

    /// Check URL + token against OpenProject (`GET users/me`), then keep the token in Windows
    /// Credential Manager and the URL in config.json. Blocking network call: run it off the UI
    /// thread.
    pub fn save_nx_token(&self, base_url: &str, token: &str) -> Result<SettingsView, ApiError> {
        let base_url = opw_tracker::normalize_base_url(base_url)
            .ok_or_else(|| ApiError::new("url.invalid", "URL phải bắt đầu bằng https://"))?;
        let token = token.trim();
        if token.is_empty() {
            return Err(ApiError::new("token.empty", "Token trống"));
        }
        let user = HttpTracker::new(&base_url, token)
            .me()
            .map_err(|e| ApiError::new("token.invalid", e.to_string()))?;
        secrets::set(&self.config.tracker.token, token)
            .map_err(|e| ApiError::new("token.store", e.to_string()))?;
        if let Some(path) = &self.config_path {
            opw_config::save_tracker_base_url(path, &base_url)
                .map_err(|e| ApiError::new("config.write", e.to_string()))?;
        }
        if let Some(live) = &self.live {
            live.set_token(base_url, token.to_owned(), user);
        }
        Ok(self.settings())
    }

    pub fn clear_nx_token(&self) -> Result<SettingsView, ApiError> {
        secrets::delete(&self.config.tracker.token)
            .map_err(|e| ApiError::new("token.store", e.to_string()))?;
        if let Some(live) = &self.live {
            live.clear_token();
        }
        Ok(self.settings())
    }

    pub fn refresh_now(&self) {
        if let Some(live) = &self.live {
            live.refresh();
        }
    }
}

fn live_quota(state: &LiveState) -> Vec<QuotaSnapshot> {
    state.claude.iter().chain(state.codex.iter()).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_live_threads_the_service_serves_demo_data() {
        let svc = AppService::new();
        assert_eq!(svc.board_snapshot().mode, DataMode::Demo);
        let s = svc.settings();
        assert!(!s.nx_token_set);
        assert_eq!(s.nx_base_url, "");
        assert_eq!(
            svc.save_nx_token("op.example.com", "t").unwrap_err().code,
            "url.invalid"
        );
        assert_eq!(
            svc.save_nx_token("https://op.example.com", "  ")
                .unwrap_err()
                .code,
            "token.empty"
        );
    }
}
