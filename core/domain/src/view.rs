//! IPC view DTOs. The JSON shape is the contract with `apps/desktop/ui/src/ipc/types.ts`:
//! camelCase keys, ISO 8601 timestamps as strings, `None` fields omitted.

use serde::{Deserialize, Serialize};

use crate::finding::{FindingState, Severity};
use crate::workflow::{StepKind, StepState};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    Claude,
    Codex,
    Shell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Running,
    Waiting,
    Idle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskColumn {
    Inbox,
    Active,
    AwaitingPush,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GateKind {
    PlanReview,
    MediumTriage,
    AgentQuestion,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Claude,
    Codex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum QuotaWindowKind {
    FiveHour,
    SevenDay,
}

/// Stable error shape at the core ↔ UI boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    pub code: String,
    pub message: String,
}

impl ApiError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
    pub core: String,
}

// ---------- quota ----------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWindow {
    pub kind: QuotaWindowKind,
    /// 0..100
    pub used_pct: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaSnapshot {
    pub provider: Provider,
    pub windows: Vec<QuotaWindow>,
    pub source: String,
    pub observed_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
}

// ---------- board ----------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepRef {
    pub kind: StepKind,
    pub index: u8,
    pub state: StepState,
    pub since: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskCard {
    pub id: String,
    pub wp_id: u64,
    pub title: String,
    pub project: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub nx_status: String,
    pub column: TaskColumn,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<StepRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_pct: Option<f64>,
    pub new_comments: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TodayStats {
    pub tokens: u64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardSnapshot {
    pub tasks: Vec<TaskCard>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_task_id: Option<String>,
    pub quota: Vec<QuotaSnapshot>,
    pub today: TodayStats,
    /// `demo` until an OpenProject token is saved, then `live`.
    pub mode: DataMode,
    /// Problem to show above the board (e.g. OpenProject unreachable).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DataMode {
    Demo,
    Live,
}

// ---------- settings ----------

/// What the Settings dialog shows. Never contains the token itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    pub nx_base_url: String,
    pub nx_token_set: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nx_user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nx_task_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nx_last_sync: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nx_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claude_quota_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codex_quota_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota_error: Option<String>,
}

// ---------- orbit ----------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepView {
    pub kind: StepKind,
    pub index: u8,
    pub state: StepState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub round: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentView {
    pub name: String,
    /// "running" | "done"
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolEvent {
    pub at: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentView {
    pub id: String,
    pub kind: AgentKind,
    pub status: SessionStatus,
    pub step: StepKind,
    pub context_tokens: u64,
    pub context_window: u64,
    pub handoff_at: u64,
    pub compactions: u32,
    pub subagents: Vec<SubagentView>,
    pub children: Vec<AgentView>,
    pub recent_tools: Vec<ToolEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FindingView {
    pub fingerprint: String,
    pub severity: Severity,
    pub title: String,
    pub file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    pub source: String,
    pub state: FindingState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GateView {
    pub kind: GateKind,
    pub since: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSystem {
    pub task_id: String,
    pub title: String,
    pub repo: String,
    pub branch: String,
    pub nx_status: String,
    pub started_at: String,
    pub steps: Vec<StepView>,
    pub agents: Vec<AgentView>,
    pub findings: Vec<FindingView>,
    pub new_comments: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gate: Option<GateView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxItem {
    pub task_id: String,
    pub title: String,
    pub project: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DistantSession {
    pub id: String,
    pub agent: AgentKind,
    pub label: String,
    pub status: SessionStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrbitSnapshot {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<TaskSystem>,
    pub inbox: Vec<InboxItem>,
    pub distant: Vec<DistantSession>,
    pub quota: Vec<QuotaSnapshot>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    /// Locks the JSON contract with `types.ts`.
    #[test]
    fn task_card_serializes_camel_case_and_omits_none() {
        let card = TaskCard {
            id: "WP-187716".into(),
            wp_id: 187716,
            title: "t".into(),
            project: "Web Platform".into(),
            repo: None,
            branch: Some("feature/187716".into()),
            nx_status: "In progress".into(),
            column: TaskColumn::AwaitingPush,
            step: Some(StepRef {
                kind: StepKind::CrossReview,
                index: 6,
                state: StepState::WaitingUser,
                since: "2026-11-13T08:00:00Z".into(),
            }),
            agent: Some(AgentKind::Codex),
            cost_usd: None,
            context_pct: Some(24.0),
            new_comments: 2,
        };
        let v = serde_json::to_value(&card).unwrap();
        assert_eq!(
            v,
            json!({
                "id": "WP-187716", "wpId": 187716, "title": "t", "project": "Web Platform",
                "branch": "feature/187716", "nxStatus": "In progress", "column": "awaitingPush",
                "step": {"kind": "crossReview", "index": 6, "state": "waitingUser", "since": "2026-11-13T08:00:00Z"},
                "agent": "codex", "contextPct": 24.0, "newComments": 2
            })
        );
    }

    #[test]
    fn enums_match_ts_literals() {
        let s = |v: Value| v.as_str().unwrap().to_owned();
        assert_eq!(
            s(serde_json::to_value(GateKind::MediumTriage).unwrap()),
            "mediumTriage"
        );
        assert_eq!(
            s(serde_json::to_value(QuotaWindowKind::FiveHour).unwrap()),
            "fiveHour"
        );
        assert_eq!(
            s(serde_json::to_value(StepKind::PushSummary).unwrap()),
            "pushSummary"
        );
        assert_eq!(
            s(serde_json::to_value(StepKind::FinalReview).unwrap()),
            "finalReview"
        );
        assert_eq!(s(serde_json::to_value(Severity::Critical).unwrap()), "critical");
        assert_eq!(
            s(serde_json::to_value(SessionStatus::Waiting).unwrap()),
            "waiting"
        );
    }
}
