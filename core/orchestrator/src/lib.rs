//! Orchestrator: the one place that combines tracker, vcs, agents, quota and storage.
//!
//! [`AppService`] is the facade used by the Tauri commands. Without an OpenProject token it serves
//! the [`demo`] timeline; once a token is saved it serves the real task inbox from [`live`], plus
//! the task the user started ([`start`]) and its workflow steps 2-8 ([`flow`]). Quota and the
//! update check need no token.

pub mod demo;
pub mod flow;
pub mod folders;
pub mod issues;
pub mod live;
pub mod os;
pub mod selftest;
pub mod start;
pub mod updater;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use opw_config::{Config, secrets};
use opw_domain::task::display_id;
use opw_domain::view::{
    ApiError, AppInfo, BoardSnapshot, DataMode, DeleteBranchRequest, FindingDecision, FlowAction, InboxItem,
    IssueFetch, OrbitSnapshot, ProjectMemory, Provider, QuotaSnapshot, RepoState, SettingsView, StartOptions,
    StartRequest, StartResult, StepRef, StepView, TaskCard, TaskColumn, TaskDetail, TaskSystem,
    TasksDirChange, TodayStats, UpdateView,
};
use opw_domain::workflow::{StepKind, StepState};
use opw_storage::{Store, TaskRow};
use opw_tracker::{HttpTracker, TrackerApi, WorkPackage};

use crate::flow::{CliLauncher, Flow, TaskCtx};
use crate::live::{Live, LiveSettings, LiveState};
use crate::updater::Updater;

pub const CORE_VERSION: &str = env!("CARGO_PKG_VERSION");

const DB_FILE: &str = "opw.db";
const NO_URL_NOTICE: &str = "Chưa có URL OpenProject cho bản này. Mở Cài đặt → OpenProject, nhập URL rồi bấm Kiểm tra & lưu (token đã lưu thì không cần dán lại).";
pub(crate) const COLUMN_ACTIVE: &str = "active";
const COLUMN_INBOX: &str = "inbox";
/// The user pushed: the task leaves the board.
const COLUMN_DONE: &str = "done";
/// "Hôm nay" on the bottom bar: the last 24 hours.
const TODAY_HOURS: u32 = 24;

/// Facade used by the Tauri commands. Every method returns plain view DTOs.
#[derive(Debug, Default)]
pub struct AppService {
    /// Reloaded after "Bắt đầu làm việc" remembers a repo, base or branch template.
    config: Mutex<Config>,
    config_path: Option<PathBuf>,
    data_dir: Option<PathBuf>,
    live: Option<Arc<Live>>,
    store: Option<Arc<Mutex<Store>>>,
    store_error: Option<String>,
    /// Cache of the tasks in the "Đang làm" column (most recent first), so snapshots do not
    /// hit SQLite. Several tasks can be in progress at once.
    active: Mutex<Vec<TaskRow>>,
    updater: Option<Updater>,
    /// Steps 2-8; `None` without local storage.
    flow: Option<Flow>,
}

impl AppService {
    /// Demo only, no background threads (tests, browser previews).
    pub fn new() -> Self {
        Self::default()
    }

    /// Load the config, read the saved OpenProject token, open the local database and start
    /// the live refresh threads.
    pub fn start() -> Self {
        let config_path = opw_config::config_path().ok();
        let config = config_path
            .as_deref()
            .map(|p| Config::load(p).unwrap_or_default())
            .unwrap_or_default();
        let token = secrets::get(&config.tracker.token).ok().flatten();
        let data_dir = opw_config::data_dir().ok();
        let probe_dir = data_dir
            .as_ref()
            .map(|d| d.join("probe"))
            .unwrap_or_else(|| std::env::temp_dir().join("opw-probe"));
        let settings = LiveSettings {
            poll: Duration::from_secs(u64::from(config.tracker.poll_min.max(1)) * 60),
            codex_poll: Duration::from_secs(u64::from(config.quota.codex_poll_sec.max(15))),
            claude_probe: Duration::from_secs(u64::from(config.quota.claude_probe_min.max(1)) * 60),
            claude_probe_model: config.quota.claude_probe_model.clone(),
            probe_dir,
        };
        let live = Arc::new(Live::start(settings, config.tracker.base_url.clone(), token));
        let (store, store_error) = match open_store(data_dir.as_deref()) {
            Ok(s) => (Some(s), None),
            Err(e) => (None, Some(e)),
        };
        let active = store
            .as_ref()
            .and_then(|s| s.active_tasks().ok())
            .unwrap_or_default();
        let store = store.map(|s| Arc::new(Mutex::new(s)));
        let flow = store.as_ref().and_then(|store| {
            let dir = data_dir.as_ref()?;
            let launcher = CliLauncher::new(dir.join("guard-hooks")).ok()?;
            let flow = Flow::new(store.clone(), Arc::new(launcher), Some(dir.join("runs")));
            let quota = live.clone();
            flow.set_quota_sink(move |snap| quota.observe_claude(snap));
            Some(flow)
        });
        let updater = Updater::start(config.update.clone(), CORE_VERSION);
        Self {
            config: Mutex::new(config),
            config_path,
            data_dir,
            live: Some(live),
            store,
            store_error,
            active: Mutex::new(active),
            updater: Some(updater),
            flow,
        }
    }

    /// For tests: a service on this store and flow, demo data otherwise.
    #[cfg(test)]
    fn with_flow(store: Arc<Mutex<Store>>, flow: Flow, config: Config, tasks_dir: PathBuf) -> Self {
        let active = store
            .lock()
            .ok()
            .and_then(|s| s.active_tasks().ok())
            .unwrap_or_default();
        Self {
            config: Mutex::new(Config {
                tasks_dir: Some(tasks_dir.to_string_lossy().into_owned()),
                ..config
            }),
            store: Some(store),
            active: Mutex::new(active),
            flow: Some(flow),
            ..Self::default()
        }
    }

    pub fn app_info(&self, app_version: &str) -> AppInfo {
        AppInfo {
            version: app_version.to_owned(),
            core: CORE_VERSION.to_owned(),
        }
    }

    fn config(&self) -> Config {
        self.config.lock().map(|c| c.clone()).unwrap_or_default()
    }

    fn live_state(&self) -> Option<LiveState> {
        self.live.as_ref().map(|l| l.snapshot())
    }

    fn active_rows(&self) -> Vec<TaskRow> {
        self.active.lock().map(|a| a.clone()).unwrap_or_default()
    }

    fn task_card(prefix: &str, wp: &WorkPackage, state: &LiveState) -> TaskCard {
        TaskCard {
            id: display_id(prefix, wp.id),
            wp_id: wp.id,
            title: wp.subject.clone(),
            project: wp.project.clone(),
            repo: None,
            branch: None,
            nx_status: wp.status.clone(),
            nx_status_color: state.status_color(&wp.status),
            column: TaskColumn::Inbox,
            step: None,
            agent: None,
            cost_usd: None,
            context_pct: None,
            new_comments: 0,
            gate: None,
            flow_note: None,
            login: None,
        }
    }

    pub fn board_snapshot(&self) -> BoardSnapshot {
        let Some(state) = self.live_state() else {
            return demo::board(demo::tick_now());
        };
        if state.token.is_none() {
            let mut board = demo::board(demo::tick_now());
            board.quota = live_quota(&state);
            board.links = state.links();
            return board;
        }
        let config = self.config();
        let prefix = config.tracker.task_prefix.clone();
        let done = self.done_ids();
        let mut tasks: Vec<TaskCard> = state
            .tasks
            .as_deref()
            .unwrap_or_default()
            .iter()
            .filter(|wp| !done.contains(&wp.id))
            .map(|wp| Self::task_card(&prefix, wp, &state))
            .collect();
        let active = self.active_rows();
        for row in &active {
            let pos = match tasks.iter().position(|c| c.wp_id == row.wp_id) {
                Some(i) => i,
                None => {
                    // no longer open/assigned on OpenProject, but the user is still on it
                    tasks.push(TaskCard {
                        id: row.display_id.clone(),
                        wp_id: row.wp_id,
                        title: row.title.clone(),
                        project: row.project.clone(),
                        repo: None,
                        branch: None,
                        nx_status: "—".into(),
                        nx_status_color: None,
                        column: TaskColumn::Inbox,
                        step: None,
                        agent: None,
                        cost_usd: None,
                        context_pct: None,
                        new_comments: 0,
                        gate: None,
                        flow_note: None,
                        login: None,
                    });
                    tasks.len() - 1
                }
            };
            let card = &mut tasks[pos];
            card.column = TaskColumn::Active;
            card.repo = row.repo.as_deref().map(repo_name);
            card.branch = row.branch.clone();
            card.step = Some(StepRef {
                kind: StepKind::Intake,
                index: StepKind::Intake.index(),
                state: StepState::Done,
                since: row.updated_at.clone(),
            });
            if let (Some(flow), Some(ctx)) = (&self.flow, self.ctx_for(row, &config)) {
                flow.ensure(&ctx);
                if let Some(f) = flow.card(&ctx) {
                    card.column = f.column;
                    card.step = Some(f.step);
                    card.agent = f.agent;
                    card.cost_usd = f.cost_usd;
                    card.context_pct = f.context_pct;
                    card.gate = f.gate;
                    card.flow_note = f.note;
                    card.login = f.login;
                }
            }
        }
        let notice = self.inbox_notice(&state);
        let (cost_usd, tokens) = self
            .flow
            .as_ref()
            .map(|f| f.usage_since(&opw_quota::hours_ago_rfc3339(TODAY_HOURS)))
            .unwrap_or_default();
        BoardSnapshot {
            tasks,
            active_task_ids: active.into_iter().map(|r| r.display_id).collect(),
            quota: live_quota(&state),
            links: state.links(),
            today: TodayStats { tokens, cost_usd },
            mode: DataMode::Live,
            notice,
            tasks_dir_offer: self.tasks_dir_offer(),
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
        let config = self.config();
        let prefix = config.tracker.task_prefix.clone();
        let tasks = state.tasks.as_deref().unwrap_or_default();
        let active = self.active_rows();
        let done = self.done_ids();
        let mut systems = active.iter().map(|row| {
            let nx_status = tasks
                .iter()
                .find(|wp| wp.id == row.wp_id)
                .map(|wp| wp.status.clone())
                .unwrap_or_default();
            let flow = match (&self.flow, self.ctx_for(row, &config)) {
                (Some(flow), Some(ctx)) => {
                    flow.ensure(&ctx);
                    flow.system(&ctx)
                }
                _ => None,
            };
            let mut system = TaskSystem {
                task_id: row.display_id.clone(),
                title: row.title.clone(),
                repo: row.repo.as_deref().map(repo_name).unwrap_or_default(),
                branch: row.branch.clone().unwrap_or_default(),
                nx_status_color: state.status_color(&nx_status),
                nx_status,
                started_at: row.updated_at.clone(),
                steps: StepKind::ALL
                    .iter()
                    .map(|&kind| {
                        let intake = kind == StepKind::Intake;
                        StepView {
                            kind,
                            index: kind.index(),
                            state: if intake {
                                StepState::Done
                            } else {
                                StepState::Pending
                            },
                            started_at: intake.then(|| row.updated_at.clone()),
                            ended_at: intake.then(|| row.updated_at.clone()),
                            cost_usd: None,
                            round: None,
                        }
                    })
                    .collect(),
                agents: Vec::new(),
                findings: Vec::new(),
                new_comments: 0,
                gate: None,
            };
            if let Some(f) = flow {
                system.steps = f.steps;
                system.agents = f.agents;
                system.findings = f.findings;
                system.gate = f.gate;
            }
            system
        });
        OrbitSnapshot {
            active: systems.next(),
            others: systems.collect(),
            inbox: tasks
                .iter()
                .filter(|wp| active.iter().all(|a| a.wp_id != wp.id) && !done.contains(&wp.id))
                .map(|wp| InboxItem {
                    task_id: display_id(&prefix, wp.id),
                    title: wp.subject.clone(),
                    project: wp.project.clone(),
                })
                .collect(),
            distant: Vec::new(),
            quota: live_quota(&state),
            notice: self.inbox_notice(&state),
        }
    }

    /// Why the live inbox is empty or stale, shown above the board and in the empty map.
    fn inbox_notice(&self, state: &LiveState) -> Option<String> {
        if state.base_url.is_empty() {
            // a token in Credential Manager is shared by every build, the URL is per data dir
            return Some(NO_URL_NOTICE.to_owned());
        }
        match (&state.tasks_error, &state.tasks, &self.store_error) {
            (Some(e), _, _) => Some(format!("Không lấy được task từ OpenProject: {e}")),
            (None, None, _) => Some("Đang tải task từ OpenProject…".to_owned()),
            (None, Some(_), Some(e)) => Some(format!("Không mở được dữ liệu cục bộ: {e}")),
            (None, Some(_), None) => None,
        }
    }

    // ---------- start work ----------

    /// The task as OpenProject last returned it, or as stored when the user started it.
    fn find_task(&self, wp_id: u64) -> Option<WorkPackage> {
        let cached = self
            .live_state()
            .and_then(|s| s.tasks)
            .and_then(|tasks| tasks.into_iter().find(|wp| wp.id == wp_id));
        cached.or_else(|| {
            let row = self.stored_task(wp_id)?;
            Some(WorkPackage {
                id: row.wp_id,
                subject: row.title,
                lock_version: 0,
                status: String::new(),
                project: row.project,
                description: String::new(),
            })
        })
    }

    fn stored_task(&self, wp_id: u64) -> Option<TaskRow> {
        self.store.as_ref()?.lock().ok()?.task(wp_id).ok().flatten()
    }

    /// Dialog data for "Bắt đầu làm việc". Scans the workspace folders; run it off the UI thread.
    pub fn start_options(&self, wp_id: u64) -> Result<StartOptions, ApiError> {
        let wp = self.find_task(wp_id).ok_or_else(task_not_found)?;
        Ok(start::options(
            &self.config(),
            &wp,
            self.stored_task(wp_id).as_ref(),
        ))
    }

    /// Branches and working tree of one repo (runs git; off the UI thread).
    pub fn repo_state(&self, repo: &str, wp_id: Option<u64>) -> Result<RepoState, ApiError> {
        start::repo_state(&self.config(), repo, wp_id)
    }

    /// Delete a branch the user confirmed in the branch list (local, or on origin: network).
    pub fn delete_branch(&self, req: &DeleteBranchRequest) -> Result<RepoState, ApiError> {
        let state = start::delete_branch(&self.config(), &self.active_rows(), req)?;
        if let Some(store) = &self.store {
            if let Ok(store) = store.lock() {
                let where_ = if req.remote { "origin" } else { "local" };
                let detail = format!("{} ({where_}) in {}", req.branch, state.name);
                let _ = store.log_event(
                    &opw_quota::now_rfc3339(),
                    req.wp_id,
                    "branch.deleted",
                    Some(&detail),
                );
            }
        }
        Ok(state)
    }

    /// `git fetch --prune origin`, then the fresh state (network; off the UI thread).
    pub fn fetch_repo(&self, repo: &str, wp_id: Option<u64>) -> Result<RepoState, ApiError> {
        start::fetch(&self.config(), repo, wp_id)
    }

    /// Create the branch locally, remember the choices, write the brief and move the task to
    /// "Đang làm". Never pushes, never writes to OpenProject. Network + git: off the UI thread.
    pub fn start_task(&self, req: &StartRequest) -> Result<StartResult, ApiError> {
        let store = self.store.as_ref().ok_or_else(|| {
            ApiError::new(
                "storage",
                self.store_error
                    .clone()
                    .unwrap_or_else(|| "Chưa mở được dữ liệu cục bộ".into()),
            )
        })?;
        let wp = self.find_task(req.wp_id).ok_or_else(task_not_found)?;
        let state = self.live_state().unwrap_or_default();
        let details = match (&state.token, state.base_url.is_empty()) {
            (Some(token), false) => {
                let tracker = HttpTracker::new(&state.base_url, token);
                tracker
                    .work_package(req.wp_id)
                    .and_then(|fresh| Ok((fresh, tracker.comments(req.wp_id)?)))
                    .map_err(|e| e.to_string())
            }
            _ => Err("chưa có token".into()),
        };
        let config = self.config();
        let (mut result, row) = start::start(
            start::StartInput {
                config: &config,
                config_path: self.config_path.as_deref(),
                tasks_dir: self.tasks_root().as_deref(),
                wp: &wp,
                details,
                task_url: format!(
                    "{}/work_packages/{}",
                    state.base_url.trim_end_matches('/'),
                    req.wp_id
                ),
                now: opw_quota::now_rfc3339(),
            },
            req,
        )?;
        {
            let store = store
                .lock()
                .map_err(|_| ApiError::new("storage", "Dữ liệu cục bộ đang bị khoá"))?;
            store
                .upsert_task(&row)
                .map_err(|e| ApiError::new("storage", e.to_string()))?;
            let detail = format!(
                "{} from {}",
                result.branch,
                row.base.as_deref().unwrap_or_default()
            );
            let kind = if result.created {
                "branch.created"
            } else {
                "branch.switched"
            };
            let _ = store.log_event(&row.updated_at, Some(row.wp_id), kind, Some(&detail));
        }
        if let Ok(mut active) = self.active.lock() {
            result.warnings.extend(shared_repo_warnings(&active, &row));
            active.retain(|a| a.wp_id != row.wp_id);
            active.insert(0, row.clone());
        }
        self.reload_config();
        if let (Some(flow), Some(ctx)) = (&self.flow, self.ctx_for(&row, &self.config())) {
            flow.started(&ctx);
        }
        Ok(result)
    }

    /// Move the active task back to the Inbox. The branch and brief stay; the next start
    /// pre-fills them.
    pub fn pause_task(&self, wp_id: u64) -> Result<(), ApiError> {
        let store = self
            .store
            .as_ref()
            .ok_or_else(|| ApiError::new("storage", "Chưa mở được dữ liệu cục bộ"))?;
        let now = opw_quota::now_rfc3339();
        let found = store
            .lock()
            .map_err(|_| ApiError::new("storage", "Dữ liệu cục bộ đang bị khoá"))?
            .set_column(wp_id, COLUMN_INBOX, &now)
            .map_err(|e| ApiError::new("storage", e.to_string()))?;
        if !found {
            return Err(task_not_found());
        }
        if let Some(flow) = &self.flow {
            // the workflow keeps its place; a step that was running has to be run again
            flow.stop(wp_id);
        }
        if let Ok(mut active) = self.active.lock() {
            active.retain(|a| a.wp_id != wp_id);
        }
        Ok(())
    }

    // ---------- workflow (steps 2-8) ----------

    fn ctx_for(&self, row: &TaskRow, config: &Config) -> Option<TaskCtx> {
        Some(TaskCtx {
            row: row.clone(),
            task_dir: self.tasks_root()?.join(&row.display_id),
            config: config.clone(),
        })
    }

    /// The workflow context of a task in progress.
    fn task_ctx(&self, wp_id: u64) -> Result<(TaskCtx, &Flow), ApiError> {
        let flow = self.flow.as_ref().ok_or_else(|| {
            ApiError::new(
                "storage",
                "Chưa mở được dữ liệu cục bộ nên không chạy được các bước",
            )
        })?;
        let row = self
            .active_rows()
            .into_iter()
            .find(|r| r.wp_id == wp_id)
            .or_else(|| self.stored_task(wp_id))
            .ok_or_else(task_not_found)?;
        let ctx = self
            .ctx_for(&row, &self.config())
            .ok_or_else(|| ApiError::new("brief.missing", "Chưa có thư mục dữ liệu"))?;
        Ok((ctx, flow))
    }

    fn done_ids(&self) -> Vec<u64> {
        self.store
            .as_ref()
            .and_then(|s| s.lock().ok()?.task_ids_in(COLUMN_DONE).ok())
            .unwrap_or_default()
    }

    /// Everything the task window shows (reads the task files).
    pub fn task_detail(&self, wp_id: u64) -> Result<TaskDetail, ApiError> {
        let (ctx, flow) = self.task_ctx(wp_id)?;
        Ok(flow.detail(&ctx))
    }

    /// A workflow button: run, stop, retry, approve the plan, …
    pub fn flow_action(&self, wp_id: u64, action: FlowAction) -> Result<TaskDetail, ApiError> {
        let (ctx, flow) = self.task_ctx(wp_id)?;
        let finish = action == FlowAction::Finish;
        flow.act(ctx.clone(), action)?;
        if finish {
            if let Some(store) = &self.store {
                if let Ok(store) = store.lock() {
                    let _ = store.set_column(wp_id, COLUMN_DONE, &opw_quota::now_rfc3339());
                }
            }
            if let Ok(mut active) = self.active.lock() {
                active.retain(|a| a.wp_id != wp_id);
            }
        }
        Ok(flow.detail(&ctx))
    }

    /// Fix / accept / defer one review finding.
    pub fn decide_finding(&self, decision: &FindingDecision) -> Result<TaskDetail, ApiError> {
        let (ctx, flow) = self.task_ctx(decision.wp_id)?;
        flow.decide(decision)?;
        Ok(flow.detail(&ctx))
    }

    /// Save the brief the user edited in the task window (before the AI reads it).
    pub fn save_brief(&self, wp_id: u64, text: &str) -> Result<TaskDetail, ApiError> {
        let (ctx, flow) = self.task_ctx(wp_id)?;
        if text.trim().is_empty() {
            return Err(ApiError::new("brief.empty", "Brief trống"));
        }
        let file = ctx.task_dir.join(flow::BRIEF_FILE);
        std::fs::write(&file, text)
            .map_err(|e| ApiError::new("brief.write", format!("Không ghi được brief: {e}")))?;
        Ok(flow.detail(&ctx))
    }

    /// Show the task folder (brief, plan, push summary) in Explorer.
    pub fn open_task_folder(&self, wp_id: u64) -> Result<(), ApiError> {
        let (ctx, _) = self.task_ctx(wp_id)?;
        os::open_folder(&ctx.task_dir).map_err(|e| ApiError::new("path.missing", e.to_string()))
    }

    /// Folder of the task folders: `tasksDir` from config, else `<data dir>/tasks`.
    fn tasks_root(&self) -> Option<PathBuf> {
        match self.config().tasks_dir {
            Some(dir) => Some(opw_config::expand_home(&dir)),
            None => self.data_dir.as_ref().map(|d| d.join("tasks")),
        }
    }

    /// The Desktop folder to offer while the user has not chosen where briefs go.
    fn tasks_dir_offer(&self) -> Option<String> {
        if self.config().tasks_dir.is_some() || self.config_path.is_none() {
            return None;
        }
        folders::desktop_suggestion().map(|p| p.to_string_lossy().into_owned())
    }

    /// `brief.md` of a stored task.
    fn brief_file(&self, wp_id: u64) -> Result<PathBuf, ApiError> {
        let row = self.stored_task(wp_id).ok_or_else(task_not_found)?;
        let dir = self
            .tasks_root()
            .ok_or_else(|| ApiError::new("brief.missing", "Chưa có thư mục dữ liệu"))?;
        Ok(start::brief_path(&dir, &row.display_id))
    }

    /// Put the task folders in `dir` (`None` = back to `<data dir>/tasks`): check the app can
    /// write there, move the existing task folders, remember it in config.
    pub fn set_tasks_dir(&self, dir: Option<&str>) -> Result<TasksDirChange, ApiError> {
        let config_path = self
            .config_path
            .as_deref()
            .ok_or_else(|| ApiError::new("config.missing", "Không tìm thấy đường dẫn config"))?;
        let new_root = match dir.map(str::trim).filter(|d| !d.is_empty()) {
            Some(d) => opw_config::expand_home(d),
            None => self
                .data_dir
                .as_ref()
                .map(|d| d.join("tasks"))
                .ok_or_else(|| ApiError::new("tasksDir.invalid", "Chưa có thư mục dữ liệu"))?,
        };
        if !new_root.is_absolute() {
            return Err(ApiError::new(
                "tasksDir.invalid",
                "Cần đường dẫn đầy đủ (bắt đầu bằng ổ đĩa, ví dụ C:) hoặc ~/Desktop/...",
            ));
        }
        folders::ensure_writable(&new_root).map_err(|e| ApiError::new("tasksDir.write", e))?;
        let (moved, warnings) = match self.tasks_root() {
            Some(old) if !opw_config::same_path(&old.to_string_lossy(), &new_root.to_string_lossy()) => {
                folders::move_task_folders(&old, &new_root)
            }
            _ => (0, Vec::new()),
        };
        opw_config::save_tasks_dir(config_path, &opw_config::contract_home(&new_root))
            .map_err(|e| ApiError::new("config.write", e.to_string()))?;
        self.reload_config();
        Ok(TasksDirChange {
            path: new_root.to_string_lossy().into_owned(),
            moved,
            warnings,
        })
    }

    /// Show the task's `brief.md` in Explorer.
    pub fn open_brief(&self, wp_id: u64) -> Result<(), ApiError> {
        os::reveal(&self.brief_file(wp_id)?).map_err(|e| ApiError::new("brief.missing", e.to_string()))
    }

    /// "Lấy issue": read the task's GitHub issues through the user's own Chrome (Chrome asks
    /// Allow) and rewrite the brief's `## Từ issue`. Only reads; waits up to ~2 minutes for the
    /// user and the pages, so run it off the UI thread.
    pub fn fetch_issues(&self, wp_id: u64) -> Result<IssueFetch, ApiError> {
        let file = self.brief_file(wp_id)?;
        let missing = |_| ApiError::new("brief.missing", "Chưa có brief: bấm Bắt đầu cho task này trước");
        let brief = std::fs::read_to_string(&file).map_err(missing)?;
        let urls = issues::github_links(&brief);
        if urls.is_empty() {
            return Err(ApiError::new(
                "issue.noLinks",
                "Mô tả task không có link issue GitHub",
            ));
        }
        let reads = opw_browser::read_github_issues(&urls).map_err(issues::browser_error)?;
        let (section, read, warnings) = issues::section_from(reads)?;
        // the user may have edited the brief while Chrome was busy: rewrite on the latest text
        let latest = std::fs::read_to_string(&file).unwrap_or(brief);
        std::fs::write(&file, opw_context::replace_issue_section(&latest, &section))
            .map_err(|e| ApiError::new("brief.write", format!("Không ghi được brief: {e}")))?;
        if let Some(store) = &self.store {
            if let Ok(store) = store.lock() {
                let detail = format!("{read} issue");
                let _ = store.log_event(
                    &opw_quota::now_rfc3339(),
                    Some(wp_id),
                    "issue.read",
                    Some(&detail),
                );
            }
        }
        Ok(IssueFetch { read, warnings })
    }

    /// Open `chrome://inspect/#remote-debugging`, where the user allows "Lấy issue" to connect.
    pub fn open_browser_debug(&self) -> Result<(), ApiError> {
        opw_browser::open_debug_settings().map_err(|e| ApiError::new("browser.open", e))
    }

    /// "Kết nối lại" for one provider's quota source, or both.
    pub fn reconnect_quota(&self, provider: Option<Provider>) {
        if let Some(live) = &self.live {
            live.reconnect(provider);
        }
    }

    /// Open `claude auth login` / `codex login` in a console; reconnects when it closes.
    pub fn open_login(&self, provider: Provider) -> Result<(), ApiError> {
        let live = self
            .live
            .as_ref()
            .ok_or_else(|| ApiError::new("login.unavailable", "Bản demo không mở được đăng nhập"))?;
        live.open_login(provider)
            .map_err(|e| ApiError::new("login.failed", e))
    }

    fn reload_config(&self) {
        let Some(path) = &self.config_path else { return };
        if let Ok(fresh) = Config::load(path) {
            if let Ok(mut c) = self.config.lock() {
                *c = fresh;
            }
        }
    }

    // ---------- update check ----------

    pub fn update_status(&self) -> UpdateView {
        match &self.updater {
            Some(u) => u.view(),
            None => UpdateView {
                current: CORE_VERSION.to_owned(),
                enabled: false,
                latest: None,
                available: false,
                url: None,
                published_at: None,
                checked_at: None,
                error: None,
            },
        }
    }

    /// Ask GitHub now (network; off the UI thread).
    pub fn check_update(&self) -> UpdateView {
        match &self.updater {
            Some(u) => u.check_now(),
            None => self.update_status(),
        }
    }

    pub fn open_release_page(&self) -> Result<(), ApiError> {
        let url = self
            .updater
            .as_ref()
            .and_then(Updater::release_page)
            .ok_or_else(|| ApiError::new("update.url", "Không có link phát hành hợp lệ"))?;
        os::open_url(&url).map_err(|e| ApiError::new("open", e.to_string()))
    }

    // ---------- settings ----------

    pub fn settings(&self) -> SettingsView {
        let config = self.config();
        let state = self.live_state().unwrap_or_else(|| LiveState {
            base_url: config.tracker.base_url.clone(),
            ..LiveState::default()
        });
        let quota_error = state.quota_error();
        SettingsView {
            nx_base_url: state.base_url,
            nx_token_set: state.token.is_some(),
            nx_user: state.nx_user,
            nx_task_count: state.tasks.as_ref().map(Vec::len),
            nx_last_sync: state.last_sync,
            nx_error: state.tasks_error,
            claude_quota_at: state.claude.map(|q| q.observed_at),
            codex_quota_at: state.codex.map(|q| q.observed_at),
            quota_error,
            workspaces: config.workspaces.clone(),
            projects: config
                .projects
                .iter()
                .map(|(name, p)| ProjectMemory {
                    name: name.clone(),
                    repos: p.repos.clone(),
                    branch: p.branch.clone(),
                })
                .collect(),
            config_path: self
                .config_path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            data_dir: self.data_dir.as_ref().map(|p| p.to_string_lossy().into_owned()),
            browser_debug: opw_browser::find_debug_port().map(|p| p.browser.to_owned()),
            tasks_dir: self.tasks_root().map(|p| p.to_string_lossy().into_owned()),
            tasks_dir_chosen: config.tasks_dir.is_some(),
            tasks_dir_suggested: folders::desktop_suggestion().map(|p| p.to_string_lossy().into_owned()),
        }
    }

    /// Show a folder or file from Settings in Explorer. Only paths Settings itself lists are
    /// accepted (workspaces, remembered repos, data folder, config file).
    pub fn reveal_path(&self, path: &str) -> Result<(), ApiError> {
        let config = self.config();
        let mut known: Vec<String> = config.workspaces.clone();
        known.extend(config.projects.values().flat_map(|p| p.repos.iter().cloned()));
        known.extend(config.repos.keys().cloned());
        known.extend(
            [&self.data_dir, &self.config_path, &self.tasks_root()]
                .into_iter()
                .flatten()
                .map(|p| p.to_string_lossy().into_owned()),
        );
        if !known.iter().any(|k| opw_config::same_path(k, path)) {
            return Err(ApiError::new(
                "path.unknown",
                "Chỉ mở được đường dẫn đang hiện trong Cài đặt",
            ));
        }
        let full = opw_config::expand_home(path);
        let opened = if full.is_dir() {
            os::open_folder(&full)
        } else if full.exists() {
            os::reveal(&full)
        } else {
            // config.json before the first save: show its folder
            full.parent()
                .map_or_else(|| os::open_folder(&full), os::open_folder)
        };
        opened.map_err(|e| ApiError::new("path.missing", e.to_string()))
    }

    /// Show config.json in Explorer (created empty first if it does not exist yet).
    pub fn open_config(&self) -> Result<(), ApiError> {
        let path = self
            .config_path
            .as_deref()
            .ok_or_else(|| ApiError::new("config.missing", "Không tìm thấy đường dẫn config"))?;
        if !path.exists() {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| ApiError::new("config.write", e.to_string()))?;
            }
            std::fs::write(path, "{}\n").map_err(|e| ApiError::new("config.write", e.to_string()))?;
        }
        os::reveal(path).map_err(|e| ApiError::new("open", e.to_string()))
    }

    /// Check URL + token against OpenProject (`GET users/me`), then keep the token in Windows
    /// Credential Manager and the URL in config.json. Blocking network call: run it off the UI
    /// thread.
    pub fn save_nx_token(&self, base_url: &str, token: &str) -> Result<SettingsView, ApiError> {
        let base_url = opw_tracker::normalize_base_url(base_url)
            .ok_or_else(|| ApiError::new("url.invalid", "URL phải bắt đầu bằng https://"))?;
        // Empty token field = keep the stored one (e.g. only the URL changes).
        let stored = self.live_state().and_then(|s| s.token);
        let token = match (token.trim(), stored.as_deref()) {
            ("", Some(saved)) => saved.to_owned(),
            ("", None) => return Err(ApiError::new("token.empty", "Token trống")),
            (typed, _) => typed.to_owned(),
        };
        let token = token.as_str();
        let user = HttpTracker::new(&base_url, token)
            .me()
            .map_err(|e| ApiError::new("token.invalid", e.to_string()))?;
        let credential = self.config().tracker.token;
        secrets::set(&credential, token).map_err(|e| ApiError::new("token.store", e.to_string()))?;
        if let Some(path) = &self.config_path {
            opw_config::save_tracker_base_url(path, &base_url)
                .map_err(|e| ApiError::new("config.write", e.to_string()))?;
            self.reload_config();
        }
        if let Some(live) = &self.live {
            live.set_token(base_url, token.to_owned(), user);
        }
        Ok(self.settings())
    }

    pub fn clear_nx_token(&self) -> Result<SettingsView, ApiError> {
        let credential = self.config().tracker.token;
        secrets::delete(&credential).map_err(|e| ApiError::new("token.store", e.to_string()))?;
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

fn open_store(data_dir: Option<&std::path::Path>) -> Result<Store, String> {
    let dir = data_dir.ok_or("không tìm thấy thư mục home")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    Store::open(&dir.join(DB_FILE)).map_err(|e| e.to_string())
}

/// One repo has one checked-out branch: starting a second task in a repo another in-progress
/// task uses moves that repo to the new branch.
fn shared_repo_warnings(active: &[TaskRow], started: &TaskRow) -> Vec<String> {
    let Some(repo) = started.repo.as_deref() else {
        return Vec::new();
    };
    active
        .iter()
        .filter(|a| a.wp_id != started.wp_id)
        .filter(|a| a.repo.as_deref().is_some_and(|r| opw_config::same_path(r, repo)))
        .map(|a| {
            let branch = a.branch.as_deref().unwrap_or("?");
            format!(
                "{} cũng đang làm trong repo này. Repo giờ ở nhánh {}; khi quay lại {} thì switch về {}.",
                a.display_id,
                started.branch.as_deref().unwrap_or("?"),
                a.display_id,
                branch
            )
        })
        .collect()
}

fn task_not_found() -> ApiError {
    ApiError::new(
        "task.notFound",
        "Không thấy task này trong danh sách lấy từ OpenProject. Bấm Lấy lại ở Cài đặt rồi thử lại.",
    )
}

/// Folder name of a repo path from config (`~/Desktop/acme-web` → `acme-web`).
fn repo_name(path: &str) -> String {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
        .to_owned()
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
        assert_eq!(svc.start_options(1).unwrap_err().code, "task.notFound");
        assert_eq!(svc.pause_task(1).unwrap_err().code, "storage");
        // only paths Settings lists can be opened
        assert_eq!(svc.reveal_path("C:/Windows").unwrap_err().code, "path.unknown");
        let u = svc.update_status();
        assert_eq!(u.current, CORE_VERSION);
        assert!(!u.available);
    }

    struct NoAi;
    impl flow::Launcher for NoAi {
        fn launch(
            &self,
            _: &flow::Job,
            _: &std::sync::atomic::AtomicBool,
            _: &mut dyn FnMut(&str),
        ) -> Result<opw_agents::run::RunOutcome, flow::LaunchError> {
            Err(flow::LaunchError::Missing)
        }
    }

    #[test]
    fn task_window_reads_and_saves_the_brief() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        store
            .lock()
            .unwrap()
            .upsert_task(&TaskRow {
                wp_id: 7,
                display_id: "WP-7".into(),
                title: "t".into(),
                project: "p".into(),
                repo: None,
                branch: Some("f/WP-7".into()),
                base: Some("main".into()),
                column: COLUMN_ACTIVE.into(),
                updated_at: "2026-11-14T08:00:00Z".into(),
            })
            .unwrap();
        let flow = Flow::new(store.clone(), Arc::new(NoAi), None);
        let svc = AppService::with_flow(store, flow, Config::default(), tmp.path().to_path_buf());
        assert_eq!(svc.task_detail(8).unwrap_err().code, "task.notFound");
        let d = svc.task_detail(7).unwrap();
        assert_eq!(d.current, Some(StepKind::Analyze));
        assert!(d.brief.is_none());
        std::fs::create_dir_all(tmp.path().join("WP-7")).unwrap();
        assert_eq!(svc.save_brief(7, "  ").unwrap_err().code, "brief.empty");
        let d = svc.save_brief(7, "# [WP-7] t\n").unwrap();
        assert_eq!(d.brief.as_deref(), Some("# [WP-7] t\n"));
        assert_eq!(
            svc.flow_action(7, FlowAction::Finish).unwrap_err().code,
            "flow.notNow"
        );
    }

    #[test]
    fn repo_names_from_config_paths() {
        assert_eq!(repo_name("~/Desktop/acme-web"), "acme-web");
        assert_eq!(repo_name("C:\\code\\acme-api\\"), "acme-api");
        assert_eq!(repo_name("solo"), "solo");
    }

    /// Live, read-only: `cargo test -p opw-orchestrator -- --ignored live_status_colors --nocapture`.
    #[test]
    #[ignore = "calls the real OpenProject"]
    fn live_status_colors() {
        let config = Config::load(&opw_config::config_path().unwrap()).unwrap();
        let token = secrets::get(&config.tracker.token).unwrap().expect("token saved");
        let colors = HttpTracker::new(&config.tracker.base_url, &token)
            .status_colors()
            .unwrap();
        for (name, color) in &colors {
            println!("{color}  {name}");
        }
        assert!(!colors.is_empty());
    }

    #[test]
    fn warns_when_two_tasks_share_a_repo() {
        let row = |wp_id: u64, repo: &str| TaskRow {
            wp_id,
            display_id: format!("WP-{wp_id}"),
            title: "t".into(),
            project: "p".into(),
            repo: Some(repo.into()),
            branch: Some(format!("f/WP-{wp_id}")),
            base: Some("origin/main".into()),
            column: COLUMN_ACTIVE.into(),
            updated_at: "2026-11-13T08:00:00Z".into(),
        };
        let active = [
            row(1, "C:\\code\\web"),
            row(2, "C:\\code\\api"),
            row(3, "C:\\code\\web"),
        ];
        let started = row(3, "c:/code/web/");
        let w = shared_repo_warnings(&active, &started);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].starts_with("WP-1 ") && w[0].contains("f/WP-3") && w[0].ends_with("f/WP-1."));
        assert!(shared_repo_warnings(&active, &row(4, "C:\\code\\other")).is_empty());
    }
}
