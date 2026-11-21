//! Orchestrator: the one place that combines tracker, vcs, agents, quota and storage.
//!
//! [`AppService`] is the facade used by the Tauri commands. Without an OpenProject token it serves
//! the [`demo`] timeline; once a token is saved it serves the real task inbox from [`live`], plus
//! the task the user started ([`start`]) and its workflow steps 2-8 ([`flow`]). Quota and the
//! update check need no token.

pub mod alerts;
pub mod allow;
pub mod browsertest;
pub mod checks;
pub mod comments;
pub mod demo;
pub mod devserver;
pub mod diffview;
pub mod flow;
mod flow_mcp;
pub mod folders;
pub mod harness;
pub mod issues;
pub mod live;
pub mod models;
pub mod os;
pub mod pushcheck;
pub mod quotagate;
pub mod selftest;
pub mod session_probe;
pub mod sessions;
pub mod start;
pub mod steps;
mod task_repo;
pub mod terminals;
pub mod tray;
pub mod updater;
pub(crate) mod workspace_tools;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use opw_config::{Config, secrets};
use opw_context::BriefComment;
use opw_domain::session::{
    AskAnswer, NewSession, OutsideChat, OutsideSession, SessionChat, SessionKind, SessionState, SessionView,
    WorkspaceSnapshot, WorkspaceTask,
};
use opw_domain::task::display_id;
use opw_domain::view::{
    AgentKind, AlertKind, AlertView, ApiError, AppInfo, BoardSnapshot, BrowserState, BrowserStatus, ChatView,
    CloseChoice, CommentsView, DataMode, DeleteBranchRequest, DiffView, DistantSession, FindingDecision,
    FlowAction, FlowStatus, HarnessLib, InboxItem, IssueFetch, IssueStatus, NotifyKind, OrbitSnapshot,
    ProjectMemory, Provider, PushCheck, QuotaNote, QuotaSnapshot, RepoChecks, RepoState, SessionStatus,
    SettingsView, StartOptions, StartRequest, StartResult, StepChoiceRequest, StepRef, StepView, TaskCard,
    TaskColumn, TaskDetail, TaskSystem, TaskText, TasksDirChange, TodayStats, UpdateView,
};
use opw_domain::workflow::{StepKind, StepState};
use opw_storage::{Store, TaskRow};
use opw_tracker::{Comment, HttpTracker, TrackerApi, WorkPackage};

use crate::alerts::{Alerts, NewAlert};
use crate::flow::{CliLauncher, Flow, TaskCtx};
use crate::live::{ClaudeProbe, CommentsSink, Live, LiveSettings, LiveState};
use crate::sessions::Sessions;
use crate::updater::Updater;

pub const CORE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The harness library of every repo config knows (remembered by "Bắt đầu làm việc" or written
/// under `repos`), each repo once.
/// Every repo the config knows (remembered for a project, or with settings of its own).
fn known_repos(config: &Config) -> Vec<String> {
    let mut repos: Vec<String> = Vec::new();
    for r in config
        .projects
        .values()
        .flat_map(|p| p.repos.iter())
        .chain(config.repos.keys())
    {
        if !r.trim().is_empty() && !repos.iter().any(|k| opw_config::same_path(k, r)) {
            repos.push(r.clone());
        }
    }
    repos
}

/// Settings → Repo → Lệnh kiểm tra: the Kiểm tra commands of every known repo.
fn repo_checks(config: &Config) -> Vec<RepoChecks> {
    known_repos(config)
        .into_iter()
        .map(|repo| RepoChecks {
            commands: config
                .repos
                .iter()
                .find(|(k, _)| opw_config::same_path(k, &repo))
                .map(|(_, c)| c.checks.clone())
                .unwrap_or_default(),
            repo,
        })
        .collect()
}

fn harness_libs(config: &Config) -> Vec<HarnessLib> {
    known_repos(config)
        .into_iter()
        .map(|repo| {
            let configured = config
                .repos
                .iter()
                .find(|(k, _)| opw_config::same_path(k, &repo))
                .and_then(|(_, c)| c.harness_dir.clone())
                .filter(|d| !d.trim().is_empty());
            let dir = harness::library_dir(configured.as_deref(), &opw_config::expand_home(&repo));
            HarnessLib {
                exists: dir.is_dir(),
                dir: dir.to_string_lossy().into_owned(),
                custom: configured.is_some(),
                repo,
            }
        })
        .collect()
}

const DB_FILE: &str = "opw.db";
const NO_TOKEN_NOTICE: &str = "Chưa có token OpenProject nên chưa có task giao cho bạn. Workspace và Phiên vẫn dùng bình thường; muốn nhận task thì mở Cài đặt → OpenProject.";
const NO_URL_NOTICE: &str = "Chưa có URL OpenProject cho bản này. Mở Cài đặt → OpenProject, nhập URL rồi bấm Kiểm tra & lưu (token đã lưu thì không cần dán lại).";
pub(crate) const COLUMN_ACTIVE: &str = "active";
const COLUMN_INBOX: &str = "inbox";
/// The user pushed: the task leaves the board.
const COLUMN_DONE: &str = "done";
/// Finished tasks shown in the "Đã xong" column, most recent first.
const DONE_SHOWN: usize = 40;
/// "Hôm nay" on the bottom bar: the last 24 hours.
const TODAY_HOURS: u32 = 24;
/// `task_kv` id for values that belong to no task (quota levels already announced).
const NO_TASK: u64 = 0;
const QUOTA_NOTIFIED: &str = "quota.notified";

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
    /// The bell (and what the shell shows as Windows notifications).
    alerts: Arc<Alerts>,
    /// Models and effort levels of the CLIs on this machine (Settings → Các bước AI).
    models: models::Catalogs,
    /// The Workspace: Claude Code sessions the app runs (PLAN.md §12 B).
    sessions: Option<Sessions>,
    /// Why the briefs could not move to the Desktop folder on start (Settings shows it).
    tasks_dir_error: Mutex<Option<String>>,
}

impl AppService {
    pub fn workspace_sessions(&self) -> Result<&Sessions, ApiError> {
        self.sessions()
    }
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
            claude_probe: ClaudeProbe::from_config(&config.quota),
            claude_probe_model: config.quota.claude_probe_model.clone(),
            probe_dir,
        };
        let (store, store_error) = match open_store(data_dir.as_deref()) {
            Ok(s) => (Some(s), None),
            Err(e) => (None, Some(e)),
        };
        let active = store
            .as_ref()
            .and_then(|s| s.active_tasks().ok())
            .unwrap_or_default();
        // comments read before the restart show at once; every new read is kept for next time
        let saved_comments = store.as_ref().map(comments::all_saved_op).unwrap_or_default();
        let store = store.map(|s| Arc::new(Mutex::new(s)));
        let keep_comments = store.clone().map(|store| -> CommentsSink {
            Box::new(move |wp_id, list| {
                if let Ok(store) = store.lock() {
                    comments::save_op(&store, wp_id, list, &opw_quota::now_rfc3339());
                }
            })
        });
        let live = Arc::new(Live::start(
            settings,
            config.tracker.base_url.clone(),
            token,
            saved_comments,
            keep_comments,
        ));
        let alerts = Arc::new(
            data_dir
                .as_ref()
                .map(|d| Alerts::open(d.join(alerts::FILE)))
                .unwrap_or_default(),
        );
        let flow = store.as_ref().and_then(|store| {
            let dir = data_dir.as_ref()?;
            let launcher = CliLauncher::new(dir.join("guard-hooks")).ok()?;
            let flow = Flow::new(store.clone(), Arc::new(launcher), Some(dir.join("runs")));
            if let Some(path) = &config_path {
                flow.set_config_path(path.clone());
            }
            let quota = live.clone();
            flow.set_quota_sink(move |snap| quota.observe_claude(snap));
            let gauges = live.clone();
            flow.set_quota_source(move |provider| {
                let state = gauges.snapshot();
                match provider {
                    Provider::Claude => state.claude,
                    Provider::Codex => state.codex,
                }
            });
            let bell = alerts.clone();
            flow.set_alert_sink(move |a| {
                bell.push(
                    NewAlert {
                        kind: a.kind,
                        wp_id: Some(a.wp_id),
                        task_id: Some(a.task_id),
                        title: a.title,
                        body: a.body,
                        tab: Some(a.tab.to_owned()),
                    },
                    &opw_quota::now_rfc3339(),
                );
            });
            Some(flow)
        });
        let sessions = store.as_ref().zip(data_dir.as_ref()).map(|(store, dir)| {
            let sessions = Sessions::new(store.clone(), dir.join("guard-hooks"));
            let quota = live.clone();
            sessions.set_quota_sink(move |snap| quota.observe_claude(snap));
            sessions.start_janitor();
            sessions
        });
        if let (Some(flow), Some(sessions)) = (&flow, &sessions) {
            flow.set_sessions(sessions.clone());
        }
        let updater = Updater::start(config.update.clone(), CORE_VERSION, data_dir.clone());
        let service = Self {
            config: Mutex::new(config),
            config_path,
            data_dir,
            live: Some(live),
            store,
            store_error,
            active: Mutex::new(active),
            updater: Some(updater),
            flow,
            alerts,
            models: models::Catalogs::default(),
            sessions,
            tasks_dir_error: Mutex::new(None),
        };
        service.settle_tasks_dir();
        service
    }

    /// Briefs live in `Desktop\op-workflow-tasks` unless the user picked another folder: the
    /// first start creates it and moves the task folders kept in the data folder there. When
    /// that fails they stay in the data folder and Settings says why.
    fn settle_tasks_dir(&self) {
        if self.config().tasks_dir.is_some() || self.config_path.is_none() {
            return;
        }
        let failed = match folders::desktop_suggestion() {
            Some(desktop) => self
                .set_tasks_dir(Some(&desktop.to_string_lossy()))
                .err()
                .map(|e| e.message),
            None => Some("Không tìm thấy thư mục Desktop của Windows".to_owned()),
        };
        if let Ok(mut slot) = self.tasks_dir_error.lock() {
            *slot = failed
                .map(|why| format!("Brief vẫn ở thư mục dữ liệu vì không chuyển ra Desktop được: {why}"));
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
            issues: None,
            finished_at: None,
            push_check: None,
            nx_open: false,
            priority: state.priority(&wp.priority),
        }
    }

    pub fn board_snapshot(&self) -> BoardSnapshot {
        let Some(state) = self.live_state() else {
            return demo::board(demo::tick_now());
        };
        let config = self.config();
        let prefix = config.tracker.task_prefix.clone();
        let done = self.done_ids();
        // no token: no OpenProject list, but the tasks already started still show
        let mut tasks: Vec<TaskCard> = Self::open_tasks(&state)
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
                        issues: None,
                        finished_at: None,
                        push_check: None,
                        nx_open: false,
                        priority: None,
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
        let parked = self.parked_cards(&mut tasks, &config);
        tasks.extend(parked);
        tasks.extend(self.done_cards(&state, &config));
        self.fill_new_comments(&mut tasks, &state);
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
            mode: if state.token.is_some() {
                DataMode::Live
            } else {
                DataMode::NoToken
            },
            notice,
            unread_alerts: self.alerts.unread(),
        }
    }

    /// Tasks the user moved back to Inbox (paused, or dragged out of "Đã xong") that OpenProject
    /// no longer lists as open and assigned: they stay on the board instead of vanishing, and
    /// "Bắt đầu" works from the saved task.
    /// Their Inbox cards (and those of tasks OpenProject still lists) show where they stopped.
    fn parked_cards(&self, shown: &mut [TaskCard], config: &Config) -> Vec<TaskCard> {
        let Some(store) = &self.store else {
            return Vec::new();
        };
        let rows = store
            .lock()
            .ok()
            .and_then(|s| s.tasks_in(COLUMN_INBOX, DONE_SHOWN).ok())
            .unwrap_or_default();
        for card in shown.iter_mut().filter(|c| c.column == TaskColumn::Inbox) {
            if let Some(row) = rows.iter().find(|r| r.wp_id == card.wp_id) {
                self.fill_history(card, row, config);
            }
        }
        rows.into_iter()
            .filter(|row| !shown.iter().any(|c| c.wp_id == row.wp_id))
            .map(|row| {
                let mut card = Self::parked_card(&row);
                self.fill_history(&mut card, &row, config);
                card
            })
            .collect()
    }

    /// A task back in the Inbox keeps its history: branch, where it stopped, what it cost.
    fn fill_history(&self, card: &mut TaskCard, row: &TaskRow, config: &Config) {
        card.repo = row.repo.as_deref().map(repo_name);
        card.branch = row.branch.clone();
        if let (Some(flow), Some(ctx)) = (&self.flow, self.ctx_for(row, config)) {
            if let Some(f) = flow.card(&ctx) {
                card.step = Some(f.step);
                card.cost_usd = f.cost_usd;
                card.finished_at = f.finished_at;
            }
        }
    }

    fn parked_card(row: &TaskRow) -> TaskCard {
        TaskCard {
            id: row.display_id.clone(),
            wp_id: row.wp_id,
            title: row.title.clone(),
            project: row.project.clone(),
            repo: row.repo.as_deref().map(repo_name),
            branch: row.branch.clone(),
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
            issues: None,
            finished_at: None,
            push_check: None,
            nx_open: false,
            priority: None,
        }
    }

    /// The "Đã xong" column: tasks the user marked done, with the push check made then, and a
    /// reminder when OpenProject still lists the task as open.
    fn done_cards(&self, state: &LiveState, config: &Config) -> Vec<TaskCard> {
        let Some(store) = &self.store else {
            return Vec::new();
        };
        let rows = store
            .lock()
            .ok()
            .and_then(|s| s.tasks_in(COLUMN_DONE, DONE_SHOWN).ok())
            .unwrap_or_default();
        rows.iter()
            .map(|row| {
                let open = state
                    .tasks
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .find(|wp| wp.id == row.wp_id);
                let mut card = TaskCard {
                    id: row.display_id.clone(),
                    wp_id: row.wp_id,
                    title: open.map_or_else(|| row.title.clone(), |wp| wp.subject.clone()),
                    project: row.project.clone(),
                    repo: row.repo.as_deref().map(repo_name),
                    branch: row.branch.clone(),
                    nx_status: open.map_or_else(|| "—".into(), |wp| wp.status.clone()),
                    nx_status_color: open.and_then(|wp| state.status_color(&wp.status)),
                    column: TaskColumn::Done,
                    step: None,
                    agent: None,
                    cost_usd: None,
                    context_pct: None,
                    new_comments: 0,
                    gate: None,
                    flow_note: None,
                    login: None,
                    issues: None,
                    finished_at: Some(row.updated_at.clone()),
                    push_check: store.lock().ok().and_then(|s| pushcheck::last(&s, row.wp_id)),
                    nx_open: open.is_some(),
                    priority: open.and_then(|wp| state.priority(&wp.priority)),
                };
                if let (Some(flow), Some(ctx)) = (&self.flow, self.ctx_for(row, config)) {
                    if let Some(f) = flow.card(&ctx) {
                        card.step = Some(f.step);
                        card.cost_usd = f.cost_usd;
                        card.finished_at = f.finished_at.or(card.finished_at);
                    }
                }
                card
            })
            .collect()
    }

    /// "+N comment" on the cards: OpenProject comments newer than the last one seen, plus new
    /// comments of the last issue read.
    fn fill_new_comments(&self, cards: &mut [TaskCard], state: &LiveState) {
        let Some(store) = self.store.as_ref().and_then(|s| s.lock().ok()) else {
            return;
        };
        for card in cards {
            let cached = state.comments.get(&card.wp_id).map(Vec::as_slice);
            card.new_comments = comments::new_count(&store, card.wp_id, cached, state.nx_user_id);
            card.issues = issues::status(&store, card.wp_id);
        }
    }

    fn new_comments_of(&self, wp_id: u64) -> u32 {
        let state = self.live_state().unwrap_or_default();
        let cached = state.comments.get(&wp_id).map(Vec::as_slice);
        self.store
            .as_ref()
            .and_then(|s| s.lock().ok())
            .map_or(0, |store| {
                comments::new_count(&store, wp_id, cached, state.nx_user_id)
            })
    }

    pub fn orbit_snapshot(&self) -> OrbitSnapshot {
        let Some(state) = self.live_state() else {
            return demo::orbit(demo::tick_now());
        };
        let config = self.config();
        let prefix = config.tracker.task_prefix.clone();
        let tasks = Self::open_tasks(&state);
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
                            model: None,
                            effort: None,
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
            system.new_comments = self.new_comments_of(row.wp_id);
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
                    wp_id: wp.id,
                    title: wp.subject.clone(),
                    project: wp.project.clone(),
                    nx_status: wp.status.clone(),
                    nx_status_color: state.status_color(&wp.status),
                    priority: state.priority(&wp.priority),
                })
                .collect(),
            distant: self
                .own_free_sessions()
                .iter()
                .map(own_star)
                .chain(self.outside_sessions().iter().map(distant))
                .collect(),
            quota: live_quota(&state),
            notice: self.inbox_notice(&state),
        }
    }

    /// The tasks OpenProject lists as open and assigned; none without a token (a list fetched
    /// before the token was removed is not shown).
    fn open_tasks(state: &LiveState) -> &[WorkPackage] {
        if state.token.is_none() {
            return &[];
        }
        state.tasks.as_deref().unwrap_or_default()
    }

    /// Why the live inbox is empty or stale, shown above the board and in the empty map.
    fn inbox_notice(&self, state: &LiveState) -> Option<String> {
        if state.token.is_none() {
            return Some(NO_TOKEN_NOTICE.to_owned());
        }
        if state.base_url.is_empty() {
            // a token in Credential Manager is shared by every build, the URL is per data dir
            return Some(NO_URL_NOTICE.to_owned());
        }
        match (&state.tasks_error, &state.tasks, &self.store_error) {
            (Some(e), _, _) => Some(format!("Không lấy được task: {e}")),
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
                updated_at: String::new(),
                priority: String::new(),
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
        let in_brief = details.as_ref().ok().and_then(|(_, c)| comments::max_id(c));
        let config = self.config();
        let (mut result, row, binding) = start::start(
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
            let binding =
                serde_json::to_string(&binding).map_err(|e| ApiError::new("storage", e.to_string()))?;
            store
                .upsert_task_with_binding(&row, task_repo::KEY, &binding)
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
            if let Some(id) = in_brief {
                comments::raise(&store, row.wp_id, comments::SEEN, id, &row.updated_at);
                comments::raise(&store, row.wp_id, comments::NOTIFIED, id, &row.updated_at);
            }
        }
        if let Ok(mut active) = self.active.lock() {
            result.warnings.extend(shared_repo_warnings(&active, &row));
            active.retain(|a| a.wp_id != row.wp_id);
            active.insert(0, row.clone());
        }
        self.reload_config();
        if let (Some(flow), Some(ctx)) = (&self.flow, self.ctx_for(&row, &self.config())) {
            if let Err(e) = flow.started(&ctx, req.carry_on.as_ref()) {
                result.warnings.push(format!("Chưa làm tiếp được: {}", e.message));
            }
        }
        Ok(result)
    }

    /// Move a task back to the Inbox: an active one (Tạm dừng) or a finished one (dragged out
    /// of "Đã xong"). The branch, brief and history stay; the next start pre-fills them, reads
    /// the brief again and, for a finished task, runs a new round on the same branch.
    pub fn pause_task(&self, wp_id: u64) -> Result<(), ApiError> {
        let store = self
            .store
            .as_ref()
            .ok_or_else(|| ApiError::new("storage", "Chưa mở được dữ liệu cục bộ"))?;
        if let Some(step) = self.flow.as_ref().and_then(|f| f.running_step(wp_id)) {
            return Err(ApiError::new(
                "step.running",
                format!(
                    "{} đang chạy. Bấm Dừng trước rồi mới tạm dừng task.",
                    flow::step_label(step)
                ),
            ));
        }
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
        Ok(self.with_counts(flow.detail(&ctx)))
    }

    fn with_counts(&self, mut detail: TaskDetail) -> TaskDetail {
        detail.new_comments = self.new_comments_of(detail.wp_id);
        detail.quota_note = self.quota_note(&detail);
        detail
    }

    /// The step about to run costs tokens and its provider's quota passed `quota.warnAt`: say
    /// so next to Chạy, with what the step cost last time.
    fn quota_note(&self, d: &TaskDetail) -> Option<QuotaNote> {
        let step = quotagate::note_step(d)?;
        let config = self.config();
        let provider = match flow::agent_for(step, &config) {
            opw_domain::view::AgentKind::Codex => Provider::Codex,
            opw_domain::view::AgentKind::Shell => return None,
            opw_domain::view::AgentKind::Claude => Provider::Claude,
        };
        let state = self.live_state()?;
        let snap = match provider {
            Provider::Claude => state.claude,
            Provider::Codex => state.codex,
        }?;
        let snap = quotagate::current(snap, &opw_quota::now_rfc3339());
        let w = quotagate::worst(&snap)?;
        let (warn, stop) = quotagate::levels(&config.quota.warn_at);
        if w.used_pct < warn {
            return None;
        }
        let agent = match provider {
            Provider::Claude => "claude",
            Provider::Codex => "codex",
        };
        let last_cost_usd = self
            .done_step_runs()
            .iter()
            .rev()
            .find(|r| {
                r.step == flow::step_key(step) && r.agent.as_deref() == Some(agent) && r.cost_usd.is_some()
            })
            .and_then(|r| r.cost_usd);
        Some(QuotaNote {
            step,
            provider,
            window: w.kind,
            used_pct: w.used_pct,
            resets_at: w.resets_at.clone(),
            stop: w.used_pct >= stop,
            last_cost_usd,
        })
    }

    /// A workflow button: run, stop, retry, approve the plan, …
    pub fn flow_action(&self, wp_id: u64, action: FlowAction) -> Result<TaskDetail, ApiError> {
        let (ctx, flow) = self.task_ctx(wp_id)?;
        if ctx.row.column == COLUMN_INBOX && !matches!(action, FlowAction::Stop) {
            return Err(ApiError::new(
                "task.parked",
                "Task đang ở Inbox: bấm Bắt đầu làm việc trên thẻ để làm tiếp",
            ));
        }
        let forced = match action {
            FlowAction::Finish { forced } => Some(forced),
            _ => None,
        };
        let reopen = matches!(action, FlowAction::Reopen { .. });
        // the dev server the user confirms is remembered for the repo too
        let dev_server = match &action {
            FlowAction::SetDevServer {
                url,
                commands,
                how_to,
            } => Some(devserver::tidy(url, commands, how_to)),
            _ => None,
        };
        flow.act(ctx.clone(), action)?;
        if let (Some(server), Some(repo), Some(path)) =
            (dev_server, ctx.row.repo.as_deref(), self.config_path.as_deref())
        {
            let keep = Some(&server).filter(|s| !s.url.is_empty() || !s.how_to.is_empty());
            opw_config::save_dev_server(path, repo, keep)
                .map_err(|e| ApiError::new("config.write", e.to_string()))?;
            self.reload_config();
            // the detail reads the repo's dev server from the config just written
            let (fresh, flow) = self.task_ctx(wp_id)?;
            return Ok(self.with_counts(flow.detail(&fresh)));
        }
        let now = opw_quota::now_rfc3339();
        if let Some(forced) = forced {
            if let Some(store) = self.store.as_ref().and_then(|s| s.lock().ok()) {
                let _ = store.set_column(wp_id, COLUMN_DONE, &now);
                // the check shown on the "Đã xong" card says the user overrode it
                if let Some(mut check) = pushcheck::last(&store, wp_id).filter(|_| forced) {
                    check.forced = true;
                    pushcheck::save(&store, wp_id, &check);
                }
            }
            if let Ok(mut active) = self.active.lock() {
                active.retain(|a| a.wp_id != wp_id);
            }
        }
        if reopen {
            if let Some(store) = self.store.as_ref().and_then(|s| s.lock().ok()) {
                let _ = store.set_column(wp_id, COLUMN_ACTIVE, &now);
            }
            if let Ok(mut active) = self.active.lock() {
                if !active.iter().any(|a| a.wp_id == wp_id) {
                    let mut row = ctx.row.clone();
                    row.column = COLUMN_ACTIVE.into();
                    row.updated_at = now;
                    active.insert(0, row);
                }
            }
        }
        Ok(self.with_counts(flow.detail(&ctx)))
    }

    /// Fix / accept / defer one review finding.
    pub fn decide_finding(&self, decision: &FindingDecision) -> Result<TaskDetail, ApiError> {
        let (ctx, flow) = self.task_ctx(decision.wp_id)?;
        flow.decide(decision)?;
        Ok(self.with_counts(flow.detail(&ctx)))
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
        Ok(self.with_counts(flow.detail(&ctx)))
    }

    /// Open a page of this task in the default browser: the work package on OpenProject (no
    /// `url`: the task itself; else a comment on it) or one of its GitHub issues. Only links the
    /// app itself built for this task are opened.
    pub fn open_task_link(&self, wp_id: u64, url: Option<&str>) -> Result<(), ApiError> {
        let state = self.live_state().unwrap_or_default();
        let task_url = nx_task_url(&state, wp_id);
        let url = match (url, &task_url) {
            (Some(u), _) => u.to_owned(),
            (None, Some(t)) => t.clone(),
            (None, None) => {
                return Err(ApiError::new(
                    "link.noUrl",
                    "Chưa có URL OpenProject: nhập ở Cài đặt → OpenProject",
                ));
            }
        };
        let issues = self
            .store
            .as_ref()
            .and_then(|s| s.lock().ok())
            .map(|s| comments::stored_issues(&s, wp_id))
            .unwrap_or_default();
        if !comments::is_task_link(&url, task_url.as_deref(), &issues) {
            return Err(ApiError::new(
                "link.refused",
                "Link không thuộc task này nên app không mở",
            ));
        }
        os::open_url(&url).map_err(|e| ApiError::new("open", e.to_string()))
    }

    /// A link the user clicked in a brief or a chat, in the default browser (https only).
    pub fn open_link(&self, url: &str) -> Result<(), ApiError> {
        let url = url.trim();
        if !url.starts_with("https://") {
            return Err(ApiError::new("link.refused", "App chỉ mở link https://"));
        }
        os::open_url(url).map_err(|e| ApiError::new("open", e.to_string()))
    }

    /// Show the task folder (brief, plan, push summary) in Explorer.
    pub fn open_task_folder(&self, wp_id: u64) -> Result<(), ApiError> {
        let (ctx, _) = self.task_ctx(wp_id)?;
        os::open_folder(&ctx.task_dir).map_err(|e| ApiError::new("path.missing", e.to_string()))
    }

    /// "Đã push xong": `git fetch` the task branch and compare it with `origin/<branch>`. Read
    /// only; network, so off the UI thread. The result is kept for the "Đã xong" card.
    pub fn push_check(&self, wp_id: u64) -> Result<PushCheck, ApiError> {
        let (ctx, _) = self.task_ctx(wp_id)?;
        let repo = ctx
            .row
            .repo
            .as_deref()
            .map(opw_config::expand_home)
            .filter(|p| p.is_dir())
            .ok_or_else(|| {
                ApiError::new(
                    "repo.missing",
                    "Không thấy thư mục repo của task (đã xoá hoặc đổi chỗ?)",
                )
            })?;
        let branch = ctx
            .row
            .branch
            .clone()
            .ok_or_else(|| ApiError::new("task.branch", "Task chưa có nhánh"))?;
        let check = pushcheck::check(&repo, &branch, &opw_quota::now_rfc3339());
        if let Some(store) = self.store.as_ref().and_then(|s| s.lock().ok()) {
            pushcheck::save(&store, wp_id, &check);
        }
        Ok(check)
    }

    /// The Chat tab: the transcript of one step run (`None` = the latest). Reads its raw log.
    pub fn step_chat(&self, wp_id: u64, run_id: Option<i64>) -> Result<ChatView, ApiError> {
        let (ctx, flow) = self.task_ctx(wp_id)?;
        flow.chat(&ctx, run_id)
    }

    /// "Cho phép lệnh này": add `rule` to `repos.<repo>.allowBash` in config.json on this
    /// machine; the next Viết code / Sửa lỗi run may use it.
    pub fn allow_bash(&self, wp_id: u64, rule: &str) -> Result<TaskDetail, ApiError> {
        let path = self
            .config_path
            .as_deref()
            .ok_or_else(|| ApiError::new("config.missing", "Không tìm thấy đường dẫn config"))?;
        let (ctx, _) = self.task_ctx(wp_id)?;
        let repo = ctx
            .row
            .repo
            .clone()
            .ok_or_else(|| ApiError::new("repo.missing", "Task này chưa có repo"))?;
        let rule = allow::clean_rule(rule).map_err(|m| ApiError::new("rule.invalid", m))?;
        opw_config::add_allow_bash(path, &repo, &rule)
            .map_err(|e| ApiError::new("config.write", e.to_string()))?;
        self.reload_config();
        self.task_detail(wp_id)
    }

    /// Continue a finished run's AI session (`claude --resume`, `codex resume`) in a console
    /// window in the task's repo, with the same git guard as the app's own runs.
    pub fn open_resume(&self, wp_id: u64, run_id: i64) -> Result<(), ApiError> {
        let (ctx, flow) = self.task_ctx(wp_id)?;
        let (repo, provider, session) = flow.resume_target(&ctx, run_id)?;
        if provider == Provider::Claude
            && self
                .sessions
                .as_ref()
                .is_some_and(|s| s.task_session(wp_id).as_deref() == Some(session.as_str()))
        {
            return Err(ApiError::new(
                "session.workspace",
                "Mở phiên chính trong Workspace rồi chọn Terminal của phiên hoặc Mở trong VS Code",
            ));
        }
        let (exe, flag, name) = match provider {
            Provider::Claude => (opw_agents::cli::find_claude(), "--resume", "claude"),
            Provider::Codex => (opw_agents::cli::find_codex(), "resume", "codex"),
        };
        let exe =
            exe.ok_or_else(|| ApiError::new("agent.missing", format!("Không thấy lệnh `{name}` trên PATH")))?;
        let env = self
            .data_dir
            .as_ref()
            .map(|d| opw_vcs::guard_env(&d.join("guard-hooks")))
            .unwrap_or_default();
        os::open_console(&repo, &exe, &[flag, session.as_str()], &env)
            .map_err(|e| ApiError::new("open", e.to_string()))
    }

    /// Folder of the task folders: `tasksDir` from config, else `<data dir>/tasks`.
    fn tasks_root(&self) -> Option<PathBuf> {
        match self.config().tasks_dir {
            Some(dir) => Some(opw_config::expand_home(&dir)),
            None => self.data_dir.as_ref().map(|d| d.join("tasks")),
        }
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
        if let Ok(mut slot) = self.tasks_dir_error.lock() {
            *slot = None;
        }
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
        let result = self.read_issues(wp_id);
        if let Some(store) = self.store.as_ref().and_then(|s| s.lock().ok()) {
            let status = IssueStatus {
                at: opw_quota::now_rfc3339(),
                read: result.as_ref().map_or(0, |r| r.read),
                error: result.as_ref().err().map(|e| e.message.clone()),
            };
            issues::save_status(&store, wp_id, &status);
        }
        result
    }

    fn read_issues(&self, wp_id: u64) -> Result<IssueFetch, ApiError> {
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
        let (section, read_issues, warnings) = issues::section_from(reads)?;
        let read = u32::try_from(read_issues.len()).unwrap_or(u32::MAX);
        let now = opw_quota::now_rfc3339();
        let stored: Vec<comments::StoredIssue> = read_issues
            .iter()
            .map(|i| comments::StoredIssue::from_brief(i, &now))
            .collect();
        // the user may have edited the brief while Chrome was busy: rewrite on the latest text
        let latest = std::fs::read_to_string(&file).unwrap_or(brief);
        std::fs::write(&file, opw_context::replace_issue_section(&latest, &section))
            .map_err(|e| ApiError::new("brief.write", format!("Không ghi được brief: {e}")))?;
        let mut new_comments = 0;
        if let Some(store) = &self.store {
            if let Ok(store) = store.lock() {
                new_comments = comments::save_issue_read(&store, wp_id, &stored, &now);
                let detail = format!("{read} issue");
                let _ = store.log_event(&now, Some(wp_id), "issue.read", Some(&detail));
            }
        }
        Ok(IssueFetch {
            read,
            warnings,
            new_comments,
        })
    }

    // ---------- comments, diff ----------

    /// The Comment tab: OpenProject comments read now (network: off the UI thread) and the
    /// issues of the last "Lấy issue". `mark_seen`: the tab is open, so they count as seen.
    /// Works for an Inbox task too (the comment box of its card): no brief yet, so no
    /// "Cập nhật brief".
    pub fn task_comments(&self, wp_id: u64, mark_seen: bool) -> Result<CommentsView, ApiError> {
        let brief_file = match self.task_ctx(wp_id) {
            Ok((ctx, _)) => Some(ctx.task_dir.join(flow::BRIEF_FILE)),
            Err(_) if self.find_task(wp_id).is_some() => None,
            Err(e) => return Err(e),
        };
        let state = self.live_state().unwrap_or_default();
        let now = opw_quota::now_rfc3339();
        let (mut list, error) = match self.read_comments(wp_id, &state) {
            Ok(list) => (list, None),
            Err(e) => (Vec::new(), Some(e)),
        };
        let brief = brief_file.and_then(|f| std::fs::read_to_string(f).ok());
        let store = self.store.as_ref().and_then(|s| s.lock().ok());
        // OpenProject cannot be read now: show the last read, saying when it was
        let mut saved_at = None;
        if error.is_some() {
            match store.as_ref().and_then(|s| comments::saved_op(s, wp_id)) {
                Some(saved) => {
                    list = saved.comments;
                    saved_at = Some(saved.saved_at);
                }
                None => list = state.comments.get(&wp_id).cloned().unwrap_or_default(),
            }
        }
        let seen = store.as_ref().and_then(|s| comments::seen_id(s, wp_id));
        let (issues, issues_seen) = store.as_ref().map_or_else(Default::default, |s| {
            (comments::stored_issues(s, wp_id), comments::issues_seen(s, wp_id))
        });
        let me = state.nx_user_id;
        let task_url = nx_task_url(&state, wp_id);
        let view = CommentsView {
            comments: comments::op_views(&list, seen, me, task_url.as_deref()),
            task_url,
            issues: comments::issue_threads(&issues, &issues_seen),
            issues_read_at: issues.iter().map(|i| i.read_at.clone()).max(),
            brief_stale: error.is_none() && comments::brief_stale(brief.as_deref(), &list, me),
            error,
            saved_at,
            fetched_at: now.clone(),
        };
        if mark_seen {
            if let Some(s) = &store {
                if view.error.is_none() {
                    comments::mark_seen(s, wp_id, &list, &now);
                } else {
                    comments::mark_issues_seen(s, wp_id, &now);
                }
            }
            self.alerts
                .mark_task_read(wp_id, &[AlertKind::Comment, AlertKind::Status]);
        }
        Ok(view)
    }

    /// What the task asks: its brief once it was started, and OpenProject's description. Works
    /// for a task never started (the Inbox "Xem task" box). `read_now`: ask OpenProject now
    /// (network, off the UI thread), falling back to the last task list; else the list only.
    pub fn task_text(&self, wp_id: u64, read_now: bool) -> Result<TaskText, ApiError> {
        let brief = self
            .task_ctx(wp_id)
            .ok()
            .and_then(|(ctx, _)| std::fs::read_to_string(ctx.task_dir.join(flow::BRIEF_FILE)).ok());
        let state = self.live_state().unwrap_or_default();
        let listed = state
            .tasks
            .as_ref()
            .and_then(|tasks| tasks.iter().find(|wp| wp.id == wp_id))
            .map(|wp| wp.description.clone());
        if listed.is_none() && brief.is_none() && self.find_task(wp_id).is_none() {
            return Err(task_not_found());
        }
        let (description, description_error) = if read_now {
            match self.read_description(wp_id, &state) {
                Ok(text) => (text, None),
                Err(e) => (listed.unwrap_or_default(), Some(e)),
            }
        } else {
            (listed.unwrap_or_default(), None)
        };
        Ok(TaskText {
            description,
            description_error,
            brief,
            task_url: nx_task_url(&state, wp_id),
        })
    }

    fn read_description(&self, wp_id: u64, state: &LiveState) -> Result<String, String> {
        let Some(token) = state.token.as_deref().filter(|_| !state.base_url.is_empty()) else {
            return Err("Chưa có token OpenProject: nhập ở Cài đặt → OpenProject để đọc mô tả.".into());
        };
        HttpTracker::new(&state.base_url, token)
            .work_package(wp_id)
            .map(|wp| wp.description)
            .map_err(|e| format!("Không đọc được mô tả từ OpenProject: {e}"))
    }

    /// OpenProject comments of one task, read now; the cache (badges) and the saved copy follow.
    /// Callers must not hold the store lock.
    fn read_comments(&self, wp_id: u64, state: &LiveState) -> Result<Vec<Comment>, String> {
        let Some(token) = state.token.as_deref().filter(|_| !state.base_url.is_empty()) else {
            return Err("Chưa có token OpenProject: nhập ở Cài đặt → OpenProject để xem comment.".into());
        };
        let list = HttpTracker::new(&state.base_url, token)
            .comments(wp_id)
            .map_err(|e| format!("Không đọc được comment từ OpenProject: {e}"))?;
        if let Some(store) = self.store.as_ref().and_then(|s| s.lock().ok()) {
            comments::save_op(&store, wp_id, &list, &opw_quota::now_rfc3339());
        }
        if let Some(live) = &self.live {
            live.put_comments(wp_id, list.clone());
        }
        Ok(list)
    }

    /// Rewrite the brief's `## Comment trên OpenProject` with the comments on OpenProject now
    /// (redacted like at start). The rest of the brief, including the user's edits, stays.
    pub fn refresh_brief_comments(&self, wp_id: u64) -> Result<CommentsView, ApiError> {
        let (ctx, flow) = self.task_ctx(wp_id)?;
        if flow.detail(&ctx).status == FlowStatus::Running {
            return Err(ApiError::new(
                "brief.busy",
                "Một bước đang chạy và đang đọc brief. Đợi bước xong rồi cập nhật.",
            ));
        }
        let state = self.live_state().unwrap_or_default();
        let list = self
            .read_comments(wp_id, &state)
            .map_err(|e| ApiError::new("comments.read", e))?;
        let file = ctx.task_dir.join(flow::BRIEF_FILE);
        let brief = std::fs::read_to_string(&file)
            .map_err(|_| ApiError::new("brief.missing", "Chưa có brief: bấm Bắt đầu cho task này trước"))?;
        let items: Vec<BriefComment> = list
            .iter()
            .map(|c| BriefComment {
                created_at: c.created_at.clone(),
                text: c.raw.clone(),
            })
            .collect();
        let section = opw_context::render_comment_section(&items, &[]);
        std::fs::write(&file, opw_context::replace_comment_section(&brief, &section))
            .map_err(|e| ApiError::new("brief.write", format!("Không ghi được brief: {e}")))?;
        flow.brief_refreshed(&ctx);
        if let Some(store) = &self.store {
            if let Ok(store) = store.lock() {
                let detail = format!("{} comment", list.len());
                let _ = store.log_event(
                    &opw_quota::now_rfc3339(),
                    Some(wp_id),
                    "brief.comments",
                    Some(&detail),
                );
            }
        }
        self.task_comments(wp_id, true)
    }

    /// The Diff tab (git, read-only; off the UI thread). `mark_seen`: remember what was shown,
    /// for "N file đổi từ lần xem trước" next time.
    pub fn task_diff(&self, wp_id: u64, mark_seen: bool) -> Result<DiffView, ApiError> {
        let (ctx, _) = self.task_ctx(wp_id)?;
        diffview::task_diff(
            self.store.as_deref(),
            &ctx.row,
            mark_seen,
            &opw_quota::now_rfc3339(),
        )
    }

    // ---------- alerts ----------

    /// The bell, newest first.
    pub fn alerts(&self) -> Vec<AlertView> {
        self.alerts.purge_expired(&opw_quota::now_rfc3339());
        self.alerts.list()
    }

    /// Mark alerts read (empty = all).
    pub fn mark_alerts_read(&self, ids: &[u64]) {
        self.alerts.mark_read(ids);
    }

    pub fn delete_alerts(&self, ids: &[u64]) -> Vec<AlertView> {
        self.alerts.delete(ids, &opw_quota::now_rfc3339());
        self.alerts()
    }

    pub fn restore_alerts(&self, ids: &[u64]) -> Vec<AlertView> {
        self.alerts.purge_expired(&opw_quota::now_rfc3339());
        self.alerts.restore(ids);
        self.alerts.list()
    }

    /// Look for new comments, status changes and quota levels; return the alerts added since the
    /// last call whose kind has Windows notifications on. The shell calls this every few seconds.
    pub fn poll_alerts(&self) -> Vec<AlertView> {
        let now = opw_quota::now_rfc3339();
        self.alerts.purge_expired(&now);
        let config = self.config();
        if let (Some(state), Some(store)) = (self.live_state(), &self.store) {
            if let Ok(store) = store.lock() {
                if state.token.is_some() {
                    let started = self.started_ids(&store);
                    for wp in state.tasks.as_deref().unwrap_or_default() {
                        let task_id = display_id(&config.tracker.task_prefix, wp.id);
                        let tab = started.contains(&wp.id).then(|| "comments".to_owned());
                        self.check_status(&store, wp, &task_id, tab.clone(), &now);
                        // until `users/me` answers, the user's own comments cannot be told
                        // apart: wait (the marks stay, so a later poll announces them)
                        if let (Some(list), Some(me)) = (state.comments.get(&wp.id), state.nx_user_id) {
                            let seen = TaskComments { list, me };
                            self.check_comments(&store, wp, &seen, &task_id, tab, &now);
                        }
                    }
                }
                self.check_quota(&store, &live_quota(&state), &config.quota.warn_at, &now);
            }
        }
        self.wake_after_quota(&config, &now);
        self.alerts
            .take_fresh()
            .into_iter()
            .filter(|a| alerts::toast_wanted(a.kind, &config.notify))
            .collect()
    }

    /// Tasks the usage limit stopped whose limit reset: run them again with "Tự chạy", else
    /// ring the bell.
    fn wake_after_quota(&self, config: &Config, now: &str) {
        let Some(flow) = &self.flow else { return };
        let (_, stop) = quotagate::levels(&config.quota.warn_at);
        for wake in flow.quota_waits_due(now, stop) {
            let wp_id = wake.wp_id;
            let Ok((ctx, _)) = self.task_ctx(wp_id) else {
                continue;
            };
            let task_id = ctx.row.display_id.clone();
            let action = if wake.retry {
                FlowAction::Retry
            } else {
                FlowAction::Run
            };
            if wake.auto.unwrap_or(config.steps.auto_advance) && flow.act(ctx, action).is_ok() {
                continue;
            }
            self.alerts.push(
                NewAlert {
                    kind: AlertKind::Gate,
                    wp_id: Some(wp_id),
                    task_id: Some(task_id.clone()),
                    title: format!("{task_id} · hạn mức đã reset"),
                    body: "Bấm Chạy hoặc Thử lại để tiếp tục bước đang chờ hạn mức.".into(),
                    tab: Some("progress".into()),
                },
                now,
            );
        }
    }

    /// Tasks that have a task window (started at least once).
    fn started_ids(&self, store: &Store) -> Vec<u64> {
        let mut ids: Vec<u64> = self.active_rows().iter().map(|r| r.wp_id).collect();
        for column in [COLUMN_INBOX, COLUMN_DONE] {
            ids.extend(store.task_ids_in(column).unwrap_or_default());
        }
        ids
    }

    /// Ring the bell for other people's new comments; the user's own only move the mark.
    fn check_comments(
        &self,
        store: &Store,
        wp: &WorkPackage,
        seen: &TaskComments<'_>,
        task_id: &str,
        tab: Option<String>,
        now: &str,
    ) {
        let list = seen.list;
        let Some(max) = comments::max_id(list) else { return };
        let notified: Option<u64> = store
            .kv_get(wp.id, comments::NOTIFIED, "")
            .ok()
            .flatten()
            .and_then(|v| v.parse().ok());
        let Some(notified) = notified else {
            // first time the app sees this task: what is there now is the baseline
            comments::raise(store, wp.id, comments::NOTIFIED, max, now);
            if comments::seen_id(store, wp.id).is_none() {
                comments::raise(store, wp.id, comments::SEEN, max, now);
            }
            return;
        };
        if max <= notified {
            return;
        }
        let fresh = comments::to_announce(list, notified, Some(seen.me));
        if let Some(newest) = fresh.iter().max_by_key(|c| c.id) {
            let who = newest.author.as_deref().unwrap_or("OpenProject");
            self.alerts.push(
                NewAlert {
                    kind: AlertKind::Comment,
                    wp_id: Some(wp.id),
                    task_id: Some(task_id.to_owned()),
                    title: format!("{task_id} · {} comment mới", fresh.len()),
                    body: format!("{who}: {}", alerts::excerpt(&newest.raw, 140)),
                    tab,
                },
                now,
            );
        }
        comments::raise(store, wp.id, comments::NOTIFIED, max, now);
    }

    fn check_status(&self, store: &Store, wp: &WorkPackage, task_id: &str, tab: Option<String>, now: &str) {
        if wp.status.is_empty() {
            return;
        }
        let before = store.kv_get(wp.id, comments::STATUS, "").ok().flatten();
        if before.as_deref() == Some(wp.status.as_str()) {
            return;
        }
        let _ = store.kv_set(wp.id, comments::STATUS, "", &wp.status, now);
        if before.is_some() && alerts::alarming_status(&wp.status) {
            self.alerts.push(
                NewAlert {
                    kind: AlertKind::Status,
                    wp_id: Some(wp.id),
                    task_id: Some(task_id.to_owned()),
                    title: format!("{task_id} chuyển sang {}", wp.status),
                    body: wp.subject.clone(),
                    tab,
                },
                now,
            );
        }
    }

    fn check_quota(&self, store: &Store, quota: &[QuotaSnapshot], levels: &[u8], now: &str) {
        let mut announced = Vec::new();
        for c in alerts::quota_crossings(quota, levels, now) {
            let remembered = store.kv_get(NO_TASK, QUOTA_NOTIFIED, &c.key).ok().flatten();
            let is_new = alerts::quota_is_new(&c, remembered.as_deref(), now);
            let mark = alerts::quota_mark(&c, remembered.as_deref(), now);
            // Preserve the highest announced level while readings/sources fluctuate. Updating
            // an overlapping reset deadline must not turn it into a new notification period.
            if remembered.as_deref() != Some(mark.as_str()) {
                let _ = store.kv_set(NO_TASK, QUOTA_NOTIFIED, &c.key, &mark, now);
            }
            if is_new {
                announced.push(c);
            }
        }
        // Startup may discover both windows at once: one notification per provider.
        for provider in [Provider::Claude, Provider::Codex] {
            let crossed: Vec<_> = announced.iter().filter(|c| c.provider == provider).collect();
            let Some(first) = crossed.first() else { continue };
            let title = if crossed.len() == 1 {
                alerts::quota_title(first)
            } else if crossed.iter().any(|c| c.used_pct >= 100.0) {
                format!("{} · đã hết hạn mức", quotagate::provider_name(provider))
            } else {
                format!("{} · hạn mức sắp hết", quotagate::provider_name(provider))
            };
            let body = crossed
                .iter()
                .map(|c| {
                    format!(
                        "{}: còn {:.0}%",
                        quotagate::window_name(c.window),
                        (100.0 - c.used_pct).max(0.0)
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            self.alerts.push(
                NewAlert {
                    kind: AlertKind::Quota,
                    wp_id: None,
                    task_id: None,
                    title,
                    body: format!("{body}. Mở Hạn mức để xem giờ reset."),
                    tab: None,
                },
                now,
            );
        }
    }

    /// Icon and tooltip of the tray icon.
    pub fn tray_view(&self) -> tray::TrayView {
        match self.live_state() {
            Some(state) => tray::tray_view(&live_quota(&state), &state.links()),
            None => tray::tray_view(&demo::board(demo::tick_now()).quota, &[]),
        }
    }

    /// Turn one kind of Windows notification on or off.
    pub fn set_notify(&self, kind: NotifyKind, on: bool) -> Result<SettingsView, ApiError> {
        let path = self
            .config_path
            .as_deref()
            .ok_or_else(|| ApiError::new("config.missing", "Không tìm thấy đường dẫn config"))?;
        opw_config::save_notify(path, kind, on).map_err(|e| ApiError::new("config.write", e.to_string()))?;
        self.reload_config();
        Ok(self.settings())
    }

    /// Settings → Hạn mức: how often Claude is probed while Claude Code is in use outside the
    /// app (`quota.claudeActiveProbeSec`, 0 = off) and otherwise (`quota.claudeProbeMin`).
    /// Takes effect without a restart.
    pub fn set_claude_probe(&self, active_sec: u32, idle_min: u32) -> Result<SettingsView, ApiError> {
        let path = self
            .config_path
            .as_deref()
            .ok_or_else(|| ApiError::new("config.missing", "Không tìm thấy đường dẫn config"))?;
        opw_config::save_claude_probe(path, active_sec, idle_min)
            .map_err(|e| ApiError::new("config.write", e.to_string()))?;
        self.reload_config();
        if let Some(live) = &self.live {
            live.set_claude_probe(ClaudeProbe::from_config(&self.config().quota));
        }
        Ok(self.settings())
    }

    /// What the window's X does (`onClose`): ask, tray or quit.
    pub fn close_choice(&self) -> CloseChoice {
        self.config().on_close
    }

    /// Settings → Thông báo, or "Nhớ lựa chọn" in the close dialog.
    pub fn set_on_close(&self, choice: CloseChoice) -> Result<SettingsView, ApiError> {
        let path = self
            .config_path
            .as_deref()
            .ok_or_else(|| ApiError::new("config.missing", "Không tìm thấy đường dẫn config"))?;
        opw_config::save_on_close(path, choice).map_err(|e| ApiError::new("config.write", e.to_string()))?;
        self.reload_config();
        Ok(self.settings())
    }

    /// AI steps running right now (the close dialog says how many).
    pub fn running_steps(&self) -> usize {
        self.flow.as_ref().map_or(0, Flow::running_count)
            + self.sessions.as_ref().map_or(0, Sessions::running)
    }

    /// "Thoát hẳn": stop every running AI step (each process with what it started) and wait up
    /// to `wait` for them to end, so nothing keeps running after the app is gone. Returns how
    /// many were running.
    pub fn quit_flow(&self, wait: Duration) -> usize {
        let sessions = self.sessions.as_ref().map_or(0, Sessions::running);
        if let Some(s) = &self.sessions {
            s.shutdown(wait);
        }
        let Some(flow) = &self.flow else { return sessions };
        let stopped = flow.stop_all();
        let deadline = std::time::Instant::now() + wait;
        while flow.running_count() > 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        stopped + sessions
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

    /// Claude Code and Codex on this machine: where, which version, signed in, how "Cài"
    /// installs them. Starts each CLI two times (`--version`, the sign-in status), so call it
    /// off the UI thread.
    pub fn ai_clis(&self) -> Vec<opw_domain::view::AiCli> {
        [Provider::Claude, Provider::Codex]
            .into_iter()
            .map(|provider| {
                let exe = match provider {
                    Provider::Claude => opw_agents::cli::find_claude(),
                    Provider::Codex => opw_agents::cli::find_codex(),
                };
                let version = exe.as_deref().and_then(cli_version);
                let signed_in = exe.as_deref().and_then(|e| match provider {
                    Provider::Claude => live::claude_signed_in(e),
                    Provider::Codex => live::codex_signed_in(e),
                });
                let install_with = match provider {
                    Provider::Claude => Some(format!("PowerShell: {}", live::CLAUDE_INSTALL)),
                    Provider::Codex => opw_agents::cli::find_npm().map(|_| live::CODEX_INSTALL.to_owned()),
                };
                opw_domain::view::AiCli {
                    provider,
                    path: exe.map(|p| p.to_string_lossy().into_owned()),
                    version,
                    signed_in,
                    install_with,
                }
            })
            .collect()
    }

    /// "Cài": the CLI's installer in a console. Codex without npm opens its install page and
    /// says why.
    pub fn install_cli(&self, provider: Provider) -> Result<(), ApiError> {
        let live = self
            .live
            .as_ref()
            .ok_or_else(|| ApiError::new("install.unavailable", "Bản demo không cài được"))?;
        live.open_install(provider).map_err(|why| {
            if provider == Provider::Codex {
                let _ = os::open_url(live::CODEX_INSTALL_PAGE);
            }
            ApiError::new("install.failed", why)
        })
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
                can_install: false,
                auto_download: false,
                download: None,
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

    /// Download the newer version's installer now (or again after a failure), in the background.
    pub fn download_update(&self) -> UpdateView {
        match &self.updater {
            Some(u) => u.download_now(),
            None => self.update_status(),
        }
    }

    /// Settings → Giới thiệu → "Tự tải bản mới" (`update.autoDownload`). Turned on with a newer
    /// version known, its download starts now.
    pub fn set_update_auto_download(&self, on: bool) -> Result<UpdateView, ApiError> {
        let path = self
            .config_path
            .as_deref()
            .ok_or_else(|| ApiError::new("config.missing", "Không tìm thấy đường dẫn config"))?;
        opw_config::save_update_auto_download(path, on)
            .map_err(|e| ApiError::new("config.write", e.to_string()))?;
        self.reload_config();
        Ok(match &self.updater {
            Some(u) => u.set_auto_download(on),
            None => self.update_status(),
        })
    }

    /// "Cài bản mới": start the downloaded, checked installer. While AI work runs (sessions,
    /// steps) only with `force`; the caller quits the app next, which stops that work the way
    /// "Thoát hẳn" does.
    pub fn install_update(&self, force: bool) -> Result<(), ApiError> {
        let updater = self
            .updater
            .as_ref()
            .ok_or_else(|| ApiError::new("update.off", "Bản này không tự cập nhật"))?;
        let running = self.running_steps();
        if running > 0 && !force {
            return Err(ApiError::new(
                "update.running",
                format!(
                    "Đang có {running} việc AI chạy (phiên hoặc bước). Cài bản mới sẽ dừng chúng như khi thoát app; hội thoại vẫn còn."
                ),
            ));
        }
        updater.run_installer()
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
            browser: browser_status(),
            tasks_dir: self.tasks_root().map(|p| p.to_string_lossy().into_owned()),
            tasks_dir_chosen: config.tasks_dir.is_some(),
            tasks_dir_suggested: folders::desktop_suggestion().map(|p| p.to_string_lossy().into_owned()),
            tasks_dir_error: self.tasks_dir_error.lock().ok().and_then(|e| e.clone()),
            notify: config.notify,
            steps: steps::settings(&config, &self.done_step_runs()),
            verify_mcp: config.steps.verify_mcp,
            claude_active_probe_sec: config.quota.claude_active_probe_sec,
            claude_probe_min: config.quota.claude_probe_min,
            on_close: config.on_close,
            hide_outside_sessions: config.hide_outside_sessions,
            harness_libs: harness_libs(&config),
            repo_checks: repo_checks(&config),
        }
    }

    /// Settings → Các bước AI: the models and effort levels the Claude and Codex CLIs on this
    /// machine offer (no prompt, no tokens). Reused for 10 minutes unless `refresh`. Starts the
    /// CLIs, so call it off the UI thread.
    pub fn step_models(&self, refresh: bool) -> opw_domain::view::StepModels {
        let dir = self
            .data_dir
            .as_ref()
            .map_or_else(|| std::env::temp_dir().join("opw-probe"), |d| d.join("probe"));
        self.models.get(&dir, refresh)
    }

    fn done_step_runs(&self) -> Vec<opw_storage::StepRunRow> {
        self.store
            .as_ref()
            .and_then(|s| s.lock().ok()?.done_step_runs().ok())
            .unwrap_or_default()
    }

    /// Settings → Các bước AI: agent, model and effort of one step (`None` = the machine's).
    pub fn set_step_choice(&self, req: &StepChoiceRequest) -> Result<SettingsView, ApiError> {
        let path = self
            .config_path
            .as_deref()
            .ok_or_else(|| ApiError::new("config.missing", "Không tìm thấy đường dẫn config"))?;
        let config = self.config();
        let agent = steps::validate(req, &config, self.models.cached().as_ref())?;
        // only an agent other than the step's default is written, so Review chéo keeps taking
        // the other AI than Viết code's until the user picks one that differs
        let agent_key =
            (req.agent.is_some() && agent != flow::agent_default(req.step, &config)).then_some(match agent {
                opw_domain::view::AgentKind::Codex => "codex",
                _ => "claude",
            });
        opw_config::save_step_choice(
            path,
            &opw_config::StepChoice {
                step: flow::step_key(req.step),
                agent: agent_key,
                model: req.model.as_deref(),
                effort: req.effort.as_deref(),
                permission_mode: req.permission_mode.as_deref(),
            },
        )
        .map_err(|e| ApiError::new("config.write", e.to_string()))?;
        self.reload_config();
        Ok(self.settings())
    }

    pub fn set_verify_mcp(&self, on: bool) -> Result<SettingsView, ApiError> {
        let path = self
            .config_path
            .as_deref()
            .ok_or_else(|| ApiError::new("config.missing", "Không tìm thấy đường dẫn cài đặt"))?;
        opw_config::save_verify_mcp(path, on).map_err(|e| ApiError::new("config.write", e.to_string()))?;
        self.reload_config();
        Ok(self.settings())
    }

    pub fn set_step_limits(
        &self,
        req: &opw_domain::view::StepLimitsRequest,
    ) -> Result<SettingsView, ApiError> {
        if !steps::ai_steps().any(|s| s == req.step)
            || req.max_budget_usd.is_some_and(|v| !v.is_finite() || v <= 0.0)
            || req.timeout_min == Some(0)
        {
            return Err(ApiError::new(
                "step.limits",
                "Trần chi phí và thời gian phải lớn hơn 0; để trống để dùng mặc định.",
            ));
        }
        let path = self
            .config_path
            .as_deref()
            .ok_or_else(|| ApiError::new("config.missing", "Không tìm thấy đường dẫn cài đặt"))?;
        opw_config::save_step_limits(
            path,
            flow::step_key(req.step),
            req.max_budget_usd,
            req.timeout_min,
        )
        .map_err(|e| ApiError::new("config.write", e.to_string()))?;
        self.reload_config();
        Ok(self.settings())
    }

    /// Settings → Repo & nhánh → Thư viện harness: the library folder of one repo (`None` = back
    /// to `C:\tmp\<repo>-harness`). Not inside the repo, not in `%TEMP%` (the EDR locks new exes
    /// run from there).
    /// Settings → Repo → Lệnh kiểm tra: the commands Kiểm tra runs for `repo` (`{files}` = the
    /// changed files); an empty list = guessed per package again.
    pub fn set_repo_checks(&self, repo: &str, commands: &[String]) -> Result<SettingsView, ApiError> {
        let path = self
            .config_path
            .as_deref()
            .ok_or_else(|| ApiError::new("config.missing", "Không tìm thấy đường dẫn config"))?;
        if !known_repos(&self.config())
            .iter()
            .any(|r| opw_config::same_path(r, repo))
        {
            return Err(ApiError::new("checks.repo", "Repo này chưa có trong Cài đặt"));
        }
        opw_config::save_repo_checks(path, repo, commands)
            .map_err(|e| ApiError::new("config.write", e.to_string()))?;
        self.reload_config();
        Ok(self.settings())
    }

    pub fn set_harness_dir(&self, repo: &str, dir: Option<&str>) -> Result<SettingsView, ApiError> {
        let path = self
            .config_path
            .as_deref()
            .ok_or_else(|| ApiError::new("config.missing", "Không tìm thấy đường dẫn config"))?;
        let config = self.config();
        if !harness_libs(&config)
            .iter()
            .any(|l| opw_config::same_path(&l.repo, repo))
        {
            return Err(ApiError::new("harness.repo", "Repo này chưa có trong Cài đặt"));
        }
        let dir = dir.map(str::trim).filter(|d| !d.is_empty());
        if let Some(d) = dir {
            let full = opw_config::expand_home(d);
            let repo_dir = opw_config::expand_home(repo);
            if !full.is_absolute() {
                return Err(ApiError::new(
                    "harness.dir",
                    "Nhập đường dẫn đầy đủ, ví dụ C:\\tmp\\web-harness",
                ));
            }
            if full.starts_with(&repo_dir) {
                return Err(ApiError::new(
                    "harness.dir",
                    "Thư viện harness phải nằm ngoài repo",
                ));
            }
            if full.starts_with(std::env::temp_dir()) {
                return Err(ApiError::new(
                    "harness.dir",
                    "Không đặt trong %TEMP%: phần mềm bảo mật khoá file chạy từ đó",
                ));
            }
        }
        opw_config::save_harness_dir(path, repo, dir)
            .map_err(|e| ApiError::new("config.write", e.to_string()))?;
        self.reload_config();
        Ok(self.settings())
    }

    /// Open a file or folder a task's Harness / Test trình duyệt result shows (scratch folder,
    /// library folder, screenshot), or anything inside the task folder.
    pub fn open_task_path(&self, wp_id: u64, path: &str) -> Result<(), ApiError> {
        let (ctx, flow) = self.task_ctx(wp_id)?;
        let detail = flow.detail(&ctx);
        let mut known: Vec<String> = Vec::new();
        for t in &detail.tests {
            known.extend(t.dir.clone());
            known.extend(t.library.clone());
            known.extend(t.cases.iter().filter_map(|c| c.screenshot.clone()));
        }
        let full = PathBuf::from(path);
        let in_task = !path.contains("..") && full.starts_with(&ctx.task_dir);
        if !in_task && !known.iter().any(|k| opw_config::same_path(k, path)) {
            return Err(ApiError::new(
                "path.unknown",
                "Chỉ mở được đường dẫn của task này",
            ));
        }
        let opened = if full.is_dir() {
            os::open_folder(&full)
        } else {
            os::reveal(&full)
        };
        opened.map_err(|e| ApiError::new("path.missing", e.to_string()))
    }

    /// Show a folder or file from Settings in Explorer. Only paths Settings itself lists are
    /// accepted (workspaces, remembered repos, harness libraries, data folder, config file).
    pub fn reveal_path(&self, path: &str) -> Result<(), ApiError> {
        let config = self.config();
        let mut known: Vec<String> = config.workspaces.clone();
        known.extend(config.projects.values().flat_map(|p| p.repos.iter().cloned()));
        known.extend(config.repos.keys().cloned());
        known.extend(harness_libs(&config).into_iter().map(|l| l.dir));
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

/// The Workspace (PLAN.md §12 B): sessions, their chat, what they ask.
impl AppService {
    fn sessions(&self) -> Result<&Sessions, ApiError> {
        self.sessions.as_ref().ok_or_else(|| {
            ApiError::new(
                "storage",
                self.store_error
                    .clone()
                    .unwrap_or_else(|| "Chưa mở được dữ liệu cục bộ".into()),
            )
        })
    }

    /// Where session changes go as they happen (the shell emits them to the window).
    pub fn set_session_sink(&self, sink: sessions::PatchSink) {
        if let Some(s) = &self.sessions {
            s.set_sink(sink);
        }
    }

    /// The tree: tasks in progress (with or without a session) and every session.
    pub fn workspace_snapshot(&self) -> WorkspaceSnapshot {
        let tasks = self
            .active_rows()
            .into_iter()
            .map(|r| WorkspaceTask {
                wp_id: r.wp_id,
                task_id: r.display_id,
                title: r.title,
                project: r.project,
                repo: r.repo,
                branch: r.branch,
            })
            .collect();
        WorkspaceSnapshot {
            tasks,
            sessions: self.sessions.as_ref().map(Sessions::list).unwrap_or_default(),
            outside: self.outside_sessions(),
            archived: self.sessions.as_ref().map(Sessions::archived).unwrap_or_default(),
        }
    }

    /// The app's sessions without a task (Workspace → Phiên riêng), most recently used first,
    /// at most [`OWN_STARS_MAX`]: the map shows them beside the tasks.
    fn own_free_sessions(&self) -> Vec<SessionView> {
        self.sessions
            .as_ref()
            .map(Sessions::list)
            .unwrap_or_default()
            .into_iter()
            .filter(|s| s.kind == SessionKind::Free)
            .take(OWN_STARS_MAX)
            .collect()
    }

    /// Claude sessions open outside the app (VS Code, a terminal); none when the user hid them.
    fn outside_sessions(&self) -> Vec<OutsideSession> {
        match &self.sessions {
            Some(s) if !self.config().hide_outside_sessions => s.outside(),
            _ => Vec::new(),
        }
    }

    /// The conversation of a session opened outside the app (read-only).
    pub fn outside_chat(&self, id: &str) -> Result<OutsideChat, ApiError> {
        self.sessions()?.outside_chat(id)
    }

    /// Archive a session opened outside the app (it stays open where it is).
    pub fn archive_outside(&self, id: &str) -> Result<(), ApiError> {
        self.sessions()?.archive_outside(id)
    }

    /// Put an archived session (the app's or one opened outside) back on the list.
    pub fn restore_session(&self, id: &str) -> Result<(), ApiError> {
        self.sessions()?.restore(id)
    }

    /// "Tiếp tục trong app": a session opened outside the app joins the app's list.
    pub fn adopt_outside(&self, id: &str) -> Result<opw_domain::session::SessionView, ApiError> {
        self.sessions()?.adopt(id)
    }

    /// "Tách bản sao": a copy of a session opened outside the app, typed into here at once.
    pub fn fork_outside(&self, id: &str) -> Result<opw_domain::session::SessionView, ApiError> {
        self.sessions()?.fork(id)
    }

    /// Settings → Phiên: show or hide Claude sessions opened outside the app.
    pub fn set_hide_outside_sessions(&self, hide: bool) -> Result<SettingsView, ApiError> {
        let path = self
            .config_path
            .as_deref()
            .ok_or_else(|| ApiError::new("config.missing", "Không tìm thấy đường dẫn config"))?;
        opw_config::save_hide_outside_sessions(path, hide)
            .map_err(|e| ApiError::new("config.write", e.to_string()))?;
        self.reload_config();
        Ok(self.settings())
    }

    /// Repos "+ Phiên mới" offers: those of tasks in progress, those the config knows and the
    /// git repos in the workspace folders.
    pub fn workspace_repos(&self) -> Vec<String> {
        let config = self.config();
        let mut repos: Vec<String> = Vec::new();
        let mut add = |r: String| {
            if !r.trim().is_empty() && !repos.iter().any(|k| opw_config::same_path(k, &r)) {
                repos.push(r);
            }
        };
        for r in self.active_rows().into_iter().filter_map(|r| r.repo) {
            add(r);
        }
        for r in known_repos(&config) {
            add(r);
        }
        let roots: Vec<PathBuf> = config
            .workspaces
            .iter()
            .map(|w| opw_config::expand_home(w))
            .collect();
        for p in opw_vcs::discover_repos(&roots, start::WORKSPACE_DEPTH) {
            add(p.to_string_lossy().into_owned());
        }
        repos
    }

    /// A new session: the task's (`wpId`) in its repo, or one in `repo` without a task.
    pub fn session_new(&self, req: &NewSession) -> Result<SessionView, ApiError> {
        let sessions = self.sessions()?;
        if let Some(wp_id) = req.wp_id {
            let row = self
                .store
                .as_ref()
                .and_then(|s| s.lock().ok()?.task(wp_id).ok().flatten())
                .ok_or_else(task_not_found)?;
            let repo = row.repo.ok_or_else(|| {
                ApiError::new("session.repo", "Task chưa có repo: bấm Bắt đầu làm việc trước")
            })?;
            return sessions.ensure_task(wp_id, &repo);
        }
        let repo = req.repo.as_deref().map(str::trim).unwrap_or_default();
        if repo.is_empty() || !opw_config::expand_home(repo).is_dir() {
            return Err(ApiError::new(
                "session.repo",
                format!("Không thấy thư mục {repo}"),
            ));
        }
        sessions.create(None, repo)
    }

    pub fn session_chat(&self, id: &str) -> Result<SessionChat, ApiError> {
        self.sessions()?.chat(id)
    }

    pub fn workspace_flow(&self, wp_id: u64) -> Result<opw_domain::session::WorkspaceFlow, ApiError> {
        let (ctx, flow) = self.task_ctx(wp_id)?;
        Ok(flow.workspace(&ctx))
    }

    pub fn workspace_step_done(&self, wp_id: u64, step: StepKind) -> Result<(), ApiError> {
        let (ctx, flow) = self.task_ctx(wp_id)?;
        flow.complete_workspace_step(&ctx, step)
    }

    pub fn session_send(
        &self,
        id: &str,
        text: &str,
        force: bool,
        images: &[opw_domain::session::ImageIn],
    ) -> Result<SessionView, ApiError> {
        self.sessions()?.send(id, text, force, images)
    }

    /// What the session changed: a task's change since its base (like the Diff tab, without
    /// marking it seen), or a free session's uncommitted change.
    pub fn session_diff(&self, id: &str) -> Result<DiffView, ApiError> {
        let (repo, wp_id) = self.sessions()?.place(id)?;
        if let Some(wp_id) = wp_id {
            if let Ok((ctx, _)) = self.task_ctx(wp_id) {
                return diffview::task_diff(
                    self.store.as_deref(),
                    &ctx.row,
                    false,
                    &opw_quota::now_rfc3339(),
                );
            }
        }
        let diff = opw_vcs::Git::new(&repo)
            .task_diff("HEAD", None)
            .map_err(|e| ApiError::new("diff.git", e.to_string()))?;
        Ok(diffview::view(
            diff,
            "HEAD",
            Some("Phiên riêng: thay đổi chưa commit so với commit hiện tại (HEAD).".into()),
            None,
        ))
    }

    /// Open one file of the session's repo in VS Code (the repo's window), at `line`.
    pub fn session_open_file(&self, id: &str, path: &str, line: Option<u32>) -> Result<(), ApiError> {
        let (repo, _) = self.sessions()?.place(id)?;
        sessions::open_file_in_vscode(&repo, path, line)
    }

    /// The same for the task window's Diff tab.
    pub fn task_open_file(&self, wp_id: u64, path: &str, line: Option<u32>) -> Result<(), ApiError> {
        let (ctx, _) = self.task_ctx(wp_id)?;
        let repo = ctx
            .row
            .repo
            .as_deref()
            .map(opw_config::expand_home)
            .ok_or_else(|| ApiError::new("task.repo", "Task chưa có repo"))?;
        sessions::open_file_in_vscode(&repo, path, line)
    }

    /// A changed file of the session's repo next to its version at `rev` in VS Code's diff view.
    pub fn session_compare_file(
        &self,
        id: &str,
        rev: &str,
        path: &str,
        old_path: Option<&str>,
    ) -> Result<(), ApiError> {
        let (repo, _) = self.sessions()?.place(id)?;
        sessions::compare_in_vscode(&repo, rev, path, old_path)
    }

    /// The same for the task window's Diff tab.
    pub fn task_compare_file(
        &self,
        wp_id: u64,
        rev: &str,
        path: &str,
        old_path: Option<&str>,
    ) -> Result<(), ApiError> {
        let (ctx, _) = self.task_ctx(wp_id)?;
        let repo = ctx
            .row
            .repo
            .as_deref()
            .map(opw_config::expand_home)
            .ok_or_else(|| ApiError::new("task.repo", "Task chưa có repo"))?;
        sessions::compare_in_vscode(&repo, rev, path, old_path)
    }

    /// The session's repo folder in VS Code; the session stays in the app.
    pub fn session_open_repo(&self, id: &str) -> Result<(), ApiError> {
        let (repo, _) = self.sessions()?.place(id)?;
        sessions::open_repo_in_vscode(&repo)
    }

    pub fn session_files(&self, id: &str, query: &str) -> Result<Vec<String>, ApiError> {
        self.sessions()?.files(id, query)
    }

    pub fn session_commands(&self, id: &str) -> Result<Vec<opw_domain::session::SlashCommand>, ApiError> {
        self.sessions()?.commands(id)
    }

    pub fn session_scripts(&self, id: &str) -> Result<Vec<opw_domain::session::RepoScript>, ApiError> {
        self.sessions()?.scripts(id)
    }

    /// A notification for a session that waits, when the user turned those on.
    pub fn session_wait_note(&self, id: &str) -> Option<(String, String)> {
        if !self.config().notify.sessions {
            return None;
        }
        self.sessions().ok()?.wait_note(id)
    }

    pub fn session_answer(
        &self,
        id: &str,
        ask_id: &str,
        answer: &AskAnswer,
    ) -> Result<SessionView, ApiError> {
        self.sessions()?.answer(id, ask_id, answer)
    }

    pub fn session_interrupt(&self, id: &str) -> Result<(), ApiError> {
        self.sessions()?.interrupt(id)
    }

    pub fn session_set_mode(&self, id: &str, mode: &str) -> Result<SessionView, ApiError> {
        self.sessions()?.set_mode(id, mode)
    }

    pub fn session_stop(&self, id: &str) -> Result<SessionView, ApiError> {
        self.sessions()?.stop(id)
    }

    pub fn session_stop_task(&self, id: &str, task_id: &str) -> Result<SessionView, ApiError> {
        self.sessions()?.stop_task(id, task_id)
    }

    pub fn session_task_output(
        &self,
        id: &str,
        task_id: &str,
    ) -> Result<opw_domain::session::TaskOutput, ApiError> {
        self.sessions()?.task_output(id, task_id)
    }

    pub fn session_set_model(&self, id: &str, model: Option<&str>) -> Result<SessionView, ApiError> {
        self.sessions()?.set_model(id, model)
    }

    pub fn session_open_vscode(&self, id: &str, force: bool) -> Result<(), ApiError> {
        self.sessions()?.open_vscode(id, force)
    }

    pub fn session_hide(&self, id: &str) -> Result<(), ApiError> {
        self.sessions()?.hide(id)
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

/// The first line of `<cli> --version` ("2.1.329 (Claude Code)", "codex-cli 0.169.1"), waiting
/// at most a few seconds.
fn cli_version(exe: &std::path::Path) -> Option<String> {
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    live::hidden(&mut cmd);
    let mut child = cmd.spawn().ok()?;
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut out = String::new();
    std::io::Read::read_to_string(child.stdout.as_mut()?, &mut out).ok()?;
    out.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_owned)
}

/// Chrome for "Lấy issue", as Settings shows it.
fn browser_status() -> BrowserStatus {
    use opw_browser::ChromeState;
    let (state, name) = match opw_browser::chrome_state() {
        ChromeState::Ready(p) => (BrowserState::Ready, Some(p.browser.to_owned())),
        ChromeState::Closed => (BrowserState::Closed, None),
        ChromeState::NotEnabled => (BrowserState::NotEnabled, None),
        ChromeState::Missing => (BrowserState::Missing, None),
    };
    BrowserStatus { state, name }
}

/// Folder name of a repo path from config (`~/Desktop/acme-web` → `acme-web`).
fn repo_name(path: &str) -> String {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
        .to_owned()
}

/// `<OpenProject>/work_packages/<id>`, once the URL is set.
fn nx_task_url(state: &LiveState, wp_id: u64) -> Option<String> {
    (!state.base_url.is_empty())
        .then(|| format!("{}/work_packages/{wp_id}", state.base_url.trim_end_matches('/')))
}

/// How many of the app's own sessions without a task the map shows.
const OWN_STARS_MAX: usize = 8;

/// One of the app's sessions without a task as a far star, named after its folder.
fn own_star(s: &SessionView) -> DistantSession {
    let folder = std::path::Path::new(&s.repo)
        .file_name()
        .map_or_else(|| s.repo.clone(), |n| n.to_string_lossy().into_owned());
    DistantSession {
        id: s.id.clone(),
        agent: AgentKind::Claude,
        label: folder,
        status: match s.state {
            SessionState::Running => SessionStatus::Running,
            SessionState::Waiting => SessionStatus::Waiting,
            SessionState::Idle | SessionState::Outside => SessionStatus::Idle,
        },
        title: s.title.clone(),
        cwd: s.repo.clone(),
        open_in: s.outside.clone().unwrap_or_default(),
        own: true,
    }
}

/// A session opened outside the app as a far star on the map, named after its folder.
fn distant(s: &OutsideSession) -> DistantSession {
    let folder = std::path::Path::new(&s.cwd)
        .file_name()
        .map_or_else(|| s.cwd.clone(), |n| n.to_string_lossy().into_owned());
    DistantSession {
        id: s.id.clone(),
        agent: AgentKind::Claude,
        label: folder,
        status: match s.state {
            SessionState::Running => SessionStatus::Running,
            SessionState::Waiting => SessionStatus::Waiting,
            SessionState::Idle | SessionState::Outside => SessionStatus::Idle,
        },
        title: s.title.clone(),
        cwd: s.cwd.clone(),
        open_in: s.open_in.clone(),
        own: false,
    }
}

fn live_quota(state: &LiveState) -> Vec<QuotaSnapshot> {
    state.claude.iter().chain(state.codex.iter()).cloned().collect()
}

/// One task's cached OpenProject comments and the user's OpenProject id, for the bell.
struct TaskComments<'a> {
    list: &'a [Comment],
    me: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outside_sessions_are_far_stars_named_after_their_folder() {
        let star = distant(&OutsideSession {
            id: "89d2".into(),
            cwd: "C:/code/acme-web".into(),
            title: "Sửa lỗi build".into(),
            state: SessionState::Waiting,
            open_in: "VS Code".into(),
            updated_at: None,
        });
        assert_eq!(
            (star.label.as_str(), star.status, star.title.as_str(), star.agent),
            (
                "acme-web",
                SessionStatus::Waiting,
                "Sửa lỗi build",
                AgentKind::Claude
            )
        );
        // the setting hides them from the map and Workspace, and is kept in config.json
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.json");
        std::fs::write(&path, "{}").unwrap();
        let service = AppService {
            config_path: Some(path),
            ..AppService::default()
        };
        service.reload_config();
        assert!(!service.settings().hide_outside_sessions);
        assert!(
            service
                .set_hide_outside_sessions(true)
                .unwrap()
                .hide_outside_sessions
        );
        assert!(service.outside_sessions().is_empty());
        assert!(service.workspace_snapshot().outside.is_empty());
    }

    #[test]
    fn ai_step_settings_validate_and_save_limits_separately() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"steps":{"analyze":{"model":"chosen","effort":"high"}}}"#,
        )
        .unwrap();
        let service = AppService {
            config_path: Some(path),
            ..AppService::default()
        };
        service.reload_config();
        assert!(service.settings().verify_mcp);
        assert!(!service.set_verify_mcp(false).unwrap().verify_mcp);
        let mut req = opw_domain::view::StepLimitsRequest {
            step: StepKind::Analyze,
            max_budget_usd: Some(7.0),
            timeout_min: Some(32),
        };
        let view = service.set_step_limits(&req).unwrap();
        let row = view.steps.iter().find(|s| s.step == StepKind::Analyze).unwrap();
        assert_eq!(row.model.as_deref(), Some("chosen"));
        assert_eq!(row.effort.as_deref(), Some("high"));
        assert_eq!(row.max_budget_usd, Some(7.0));
        assert_eq!(row.timeout_min, Some(32));
        assert_eq!(row.default_budget_usd, 5.0);
        for invalid in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            req.max_budget_usd = Some(invalid);
            assert!(service.set_step_limits(&req).is_err());
        }
        req.max_budget_usd = None;
        req.timeout_min = Some(0);
        assert!(service.set_step_limits(&req).is_err());
        req.timeout_min = None;
        let view = service.set_step_limits(&req).unwrap();
        let row = view.steps.iter().find(|s| s.step == StepKind::Analyze).unwrap();
        assert_eq!((row.max_budget_usd, row.timeout_min), (None, None));
        req.step = StepKind::Checks;
        assert!(service.set_step_limits(&req).is_err());
    }

    #[test]
    fn quota_alerts_are_batched_and_fluctuating_sources_do_not_repeat_them() {
        let svc = AppService::new();
        let store = Store::open_in_memory().unwrap();
        let now = "2026-11-17T09:00:00Z";
        let make = |provider| QuotaSnapshot {
            provider,
            windows: vec![
                opw_domain::view::QuotaWindow {
                    kind: opw_domain::view::QuotaWindowKind::FiveHour,
                    used_pct: 100.0,
                    resets_at: Some("2026-11-17T12:20:00Z".into()),
                },
                opw_domain::view::QuotaWindow {
                    kind: opw_domain::view::QuotaWindowKind::SevenDay,
                    used_pct: 96.0,
                    resets_at: Some("2026-11-20T00:00:00Z".into()),
                },
            ],
            source: "probe".into(),
            observed_at: now.into(),
            plan: None,
        };
        let mut quota = vec![make(Provider::Claude), make(Provider::Codex)];
        svc.check_quota(&store, &quota, &[80, 95], now);
        let first = svc.alerts.take_fresh();
        assert_eq!(first.len(), 2, "one per provider, not per window");
        assert!(
            first.iter().any(|a| a.title == "Claude · đã hết hạn mức"),
            "{:?}",
            first.iter().map(|a| &a.title).collect::<Vec<_>>()
        );
        for i in 0..10 {
            for snap in &mut quota {
                snap.windows[0].used_pct = if i % 2 == 0 { 91.0 } else { 100.0 };
                snap.windows[0].resets_at = Some(
                    if i % 2 == 0 {
                        "2026-11-17T12:19:59Z"
                    } else {
                        "2026-11-17T19:20:00+07:00"
                    }
                    .into(),
                );
            }
            svc.check_quota(&store, &quota, &[80, 95], now);
        }
        assert!(svc.alerts.take_fresh().is_empty());
        assert_eq!(svc.alerts().len(), 2);
        let restarted = AppService::new();
        restarted.check_quota(&store, &quota, &[80, 95], now);
        assert!(
            restarted.alerts.take_fresh().is_empty(),
            "SQLite marks prevent a startup burst"
        );
        for snap in &mut quota {
            snap.windows[0].resets_at = Some("2026-11-21T12:20:00Z".into());
            snap.windows[1].resets_at = Some("2026-11-27T00:00:00Z".into());
        }
        restarted.check_quota(&store, &quota, &[80, 95], "2026-11-21T09:00:00Z");
        assert_eq!(
            restarted.alerts.take_fresh().len(),
            2,
            "the next real period can warn again"
        );
    }

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
        let flow = Flow::new_without_mcp(store.clone(), Arc::new(NoAi), None);
        let svc = AppService::with_flow(store, flow, Config::default(), tmp.path().to_path_buf());
        assert_eq!(svc.task_detail(8).unwrap_err().code, "task.notFound");
        assert_eq!(svc.task_text(8, false).unwrap_err().code, "task.notFound");
        let d = svc.task_detail(7).unwrap();
        assert_eq!(d.current, Some(StepKind::Analyze));
        assert!(d.brief.is_none());
        assert_eq!(svc.task_text(7, false).unwrap().brief, None);
        std::fs::create_dir_all(tmp.path().join("WP-7")).unwrap();
        assert_eq!(svc.save_brief(7, "  ").unwrap_err().code, "brief.empty");
        let d = svc.save_brief(7, "# [WP-7] t\n").unwrap();
        assert_eq!(d.brief.as_deref(), Some("# [WP-7] t\n"));
        // the Workspace Brief tab reads the same file; no token: the description says why
        let text = svc.task_text(7, true).unwrap();
        assert_eq!(text.brief.as_deref(), Some("# [WP-7] t\n"));
        assert_eq!(text.description, "");
        assert!(text.description_error.unwrap().contains("token"));
        assert_eq!(svc.task_text(7, false).unwrap().description_error, None);
        // done before the push summary: the remaining steps are skipped, the task leaves "Đang làm"
        // and the last push check remembers the user overrode it
        let check = PushCheck {
            branch: "f/WP-7".into(),
            pushed: false,
            remote: false,
            ahead: 2,
            behind: 0,
            uncommitted: 0,
            head: None,
            fetch_error: None,
            other_branch: None,
            checked_at: "2026-11-16T08:00:00Z".into(),
            forced: false,
        };
        pushcheck::save(&svc.store.as_ref().unwrap().lock().unwrap(), 7, &check);
        let d = svc.flow_action(7, FlowAction::Finish { forced: true }).unwrap();
        assert_eq!(d.status, FlowStatus::Done);
        assert!(svc.active.lock().unwrap().is_empty());
        let saved = pushcheck::last(&svc.store.as_ref().unwrap().lock().unwrap(), 7).unwrap();
        assert!(saved.forced && saved.ahead == 2);
        assert_eq!(
            svc.flow_action(7, FlowAction::Finish { forced: false })
                .unwrap_err()
                .code,
            "flow.notNow"
        );
        let d = svc
            .flow_action(
                7,
                FlowAction::Reopen {
                    step: None,
                    note: None,
                },
            )
            .unwrap();
        assert_eq!(
            (d.status, d.current),
            (FlowStatus::Ready, Some(StepKind::Analyze))
        );
        assert_eq!(svc.active.lock().unwrap()[0].wp_id, 7);
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

    /// Live, read-only: does the server give priorities a colour? Prints counts only.
    /// `cargo test -p opw-orchestrator -- --ignored live_priority_colors --nocapture`
    #[test]
    #[ignore = "calls the real OpenProject"]
    fn live_priority_colors() {
        let config = Config::load(&opw_config::config_path().unwrap()).unwrap();
        let token = secrets::get(&config.tracker.token).unwrap().expect("token saved");
        let tracker = HttpTracker::new(&config.tracker.base_url, &token);
        let p = tracker.priorities().unwrap();
        let tasks = tracker.inbox().unwrap();
        let named = tasks.iter().filter(|t| !t.priority.is_empty()).count();
        println!(
            "priorities: {} in order, {} with a colour; inbox tasks with a priority: {named}/{}",
            p.order.len(),
            p.colors.len(),
            tasks.len()
        );
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
