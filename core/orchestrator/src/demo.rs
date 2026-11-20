//! Demo data driven by the real workflow state machine. One tick = 4 s; the active task walks
//! through the steps (plan gate, first /review-local + its findings gate + fix + "Review chéo
//! lại?", two Codex rounds, a compaction, the final /review-local's findings gate) and then
//! loops. Replaced by live data in Phase 1/3.

use std::time::{SystemTime, UNIX_EPOCH};

use opw_domain::finding::{Finding, FindingState, Severity};
use opw_domain::view::{
    AgentKind, AgentView, BoardSnapshot, DistantSession, FindingView, GateKind, GateView, InboxItem,
    OrbitSnapshot, PriorityView, Provider, QuotaSnapshot, QuotaWindow, QuotaWindowKind, SessionStatus,
    StepRef, StepView, SubagentView, TaskCard, TaskColumn, TaskSystem, TodayStats, ToolEvent,
};
use opw_domain::workflow::{AfterFix, Phase, StepKind, StepState, Workflow, WorkflowEvent as E};

const TICK_SECS: u64 = 4;
const CLAUDE_WINDOW: u64 = 1_000_000;
const CLAUDE_HANDOFF: u64 = 500_000;
const CODEX_WINDOW: u64 = 500_000;
const CODEX_HANDOFF: u64 = 400_000;
const DEMO_TIME: &str = "2026-11-13T08:00:00Z";

fn script() -> Vec<E> {
    vec![
        E::Start,
        E::StepSucceeded(StepKind::Intake),
        E::StepSucceeded(StepKind::Analyze),
        E::PlanApproved,
        E::StepSucceeded(StepKind::Implement),
        // frame 6: Tự review's findings gate
        E::ReviewFinished { open: 1 },
        E::FindingsPicked { fix: 1 },
        // frame 8: "Review chéo lại?"
        E::StepSucceeded(StepKind::Fix),
        E::AfterFix(AfterFix::CrossReview),
        // frame 10: Codex's findings gate
        E::ReviewFinished { open: 3 },
        E::FindingsPicked { fix: 1 },
        E::StepSucceeded(StepKind::FixCross),
        E::AfterFix(AfterFix::CrossReview),
        E::ReviewFinished { open: 0 },
        // frame 15: the final review's findings gate
        E::ReviewFinished { open: 1 },
        E::FindingsPicked { fix: 0 },
        E::StepSucceeded(StepKind::PushSummary),
    ]
}

/// Number of distinct demo frames (not started + one per scripted event).
pub fn frames() -> u64 {
    script().len() as u64 + 1
}

pub fn tick_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() / TICK_SECS)
        .unwrap_or_default()
}

fn workflow_at(frame: u64) -> Workflow {
    let mut wf = Workflow::default();
    for event in script()
        .into_iter()
        .take(usize::try_from(frame).unwrap_or(usize::MAX))
    {
        wf.apply(event).expect("demo script follows the state machine");
    }
    wf
}

fn quota() -> Vec<QuotaSnapshot> {
    let w = |kind, used_pct| QuotaWindow {
        kind,
        used_pct,
        resets_at: None,
    };
    vec![
        QuotaSnapshot {
            provider: Provider::Claude,
            windows: vec![
                w(QuotaWindowKind::FiveHour, 17.0),
                w(QuotaWindowKind::SevenDay, 74.0),
            ],
            source: opw_quota::SOURCE_RATE_LIMIT_EVENT.into(),
            observed_at: opw_quota::now_rfc3339(),
            plan: Some("team".into()),
        },
        QuotaSnapshot {
            provider: Provider::Codex,
            windows: vec![
                w(QuotaWindowKind::FiveHour, 3.0),
                w(QuotaWindowKind::SevenDay, 0.0),
            ],
            source: opw_quota::SOURCE_APP_SERVER.into(),
            observed_at: opw_quota::now_rfc3339(),
            plan: Some("plus".into()),
        },
    ]
}

fn findings(frame: u64) -> Vec<FindingView> {
    if frame < 6 {
        return Vec::new();
    }
    // First /review-local finds one issue; Claude fixes it before Codex looks.
    let mut list = vec![Finding::new(
        Severity::High,
        "api/src/services/box.ts",
        Some(133),
        "Chưa kiểm tra quyền khi đọc thư mục chia sẻ",
        "review-local",
    )];
    if (7..10).contains(&frame) {
        list[0].state = FindingState::Fixing;
    } else if frame >= 10 {
        list[0].state = FindingState::Fixed;
    }
    if frame < 10 {
        return list.into_iter().map(to_view).collect();
    }
    let mut codex = vec![
        Finding::new(
            Severity::High,
            "api/src/order.ts",
            Some(42),
            "Null khi user không có org",
            "codex",
        ),
        Finding::new(
            Severity::Medium,
            "web/src/cart.tsx",
            Some(88),
            "Thiếu trạng thái loading",
            "codex",
        ),
        Finding::new(
            Severity::Low,
            "api/src/util.ts",
            Some(10),
            "Tên biến khó hiểu",
            "codex",
        ),
    ];
    if (11..14).contains(&frame) {
        codex[0].state = FindingState::Fixing;
    } else if frame >= 14 {
        codex[0].state = FindingState::Fixed;
    }
    if frame >= 11 {
        codex[1].state = FindingState::Accepted;
    }
    if frame >= 16 {
        codex[2].state = FindingState::Deferred;
    }
    list.extend(codex);
    list.into_iter().map(to_view).collect()
}

fn to_view(f: Finding) -> FindingView {
    FindingView {
        fingerprint: f.fingerprint,
        severity: f.severity,
        title: f.title,
        file: f.file,
        line: f.line,
        source: f.source,
        state: f.state,
        detail: None,
        suggestion: None,
        reason: None,
        chosen: false,
        fix_harness: false,
        round: None,
        step: None,
    }
}

fn agent_for(step: StepKind, frame: u64, waiting: bool) -> AgentView {
    let status = if waiting {
        SessionStatus::Waiting
    } else {
        SessionStatus::Running
    };
    let tools = |names: &[&str]| {
        names
            .iter()
            .map(|n| ToolEvent {
                at: DEMO_TIME.into(),
                name: (*n).into(),
            })
            .collect()
    };
    if step == StepKind::CrossReview {
        return AgentView {
            id: "codex-review".into(),
            kind: AgentKind::Codex,
            status,
            step,
            context_tokens: 60_000 + frame * 9_000,
            context_window: CODEX_WINDOW,
            handoff_at: CODEX_HANDOFF,
            compactions: 0,
            subagents: Vec::new(),
            children: Vec::new(),
            recent_tools: tools(&["git diff", "Read"]),
        };
    }
    // context grows, then the handoff + compact at frame 11 (Claude fixing Codex findings) drops it
    let context_tokens = if frame >= 11 {
        140_000 + frame * 12_000
    } else {
        90_000 + frame * 60_000
    };
    let subagents = if step == StepKind::Implement {
        vec![
            SubagentView {
                name: "Explore".into(),
                status: "running".into(),
            },
            SubagentView {
                name: "test-runner".into(),
                status: "running".into(),
            },
        ]
    } else {
        Vec::new()
    };
    AgentView {
        id: "claude-main".into(),
        kind: AgentKind::Claude,
        status,
        step,
        context_tokens,
        context_window: CLAUDE_WINDOW,
        handoff_at: CLAUDE_HANDOFF,
        compactions: u32::from(frame >= 11),
        subagents,
        children: Vec::new(),
        recent_tools: tools(&["Read", "Edit", "Bash(npx tsc)"]),
    }
}

fn active_system(frame: u64) -> Option<TaskSystem> {
    let wf = workflow_at(frame);
    let (step, gate) = match wf.phase() {
        Phase::NotStarted => return None,
        Phase::Running(s) => (Some(s), None),
        Phase::Gate(g) => (wf.current_step(), Some(g)),
        Phase::Paused | Phase::AwaitingPush => (None, None),
    };
    let steps = StepKind::ALL
        .iter()
        .map(|&kind| StepView {
            kind,
            index: kind.index(),
            state: wf.state_of(kind),
            started_at: (wf.state_of(kind) != StepState::Pending).then(|| DEMO_TIME.into()),
            ended_at: None,
            cost_usd: (wf.state_of(kind) == StepState::Done).then_some(0.12),
            round: (kind == StepKind::CrossReview).then_some(wf.round().max(1)),
            model: None,
            effort: None,
        })
        .collect();
    let gate_view = gate.map(|kind| GateView {
        kind,
        since: DEMO_TIME.into(),
        message: match kind {
            GateKind::PlanReview => "Plan đã sẵn sàng, chờ bạn duyệt".into(),
            GateKind::Findings => "Review báo lỗi: chọn lỗi cho AI sửa".into(),
            GateKind::AfterFix => "Sửa lỗi xong: Review chéo lại?".into(),
            GateKind::AgentQuestion => "AI đang hỏi bạn".into(),
            GateKind::Error => "Bước bị lỗi".into(),
            GateKind::SelfCode => "Bạn đang tự code".into(),
        },
        review: wf.gate_review(),
        next: wf.skip_target(),
    });
    Some(TaskSystem {
        task_id: "WP-187716".into(),
        title: "Sửa lỗi lưu đơn khi user chưa có org".into(),
        repo: "acme-web".into(),
        branch: "feature/187716".into(),
        nx_status: "In progress".into(),
        nx_status_color: Some("#cc5de8".into()),
        started_at: DEMO_TIME.into(),
        steps,
        agents: step
            .map(|s| vec![agent_for(s, frame, gate.is_some())])
            .unwrap_or_default(),
        findings: findings(frame),
        new_comments: u32::from(frame % 5 == 3),
        gate: gate_view,
    })
}

pub fn orbit(tick: u64) -> OrbitSnapshot {
    let frame = tick % frames();
    OrbitSnapshot {
        active: active_system(frame),
        others: Vec::new(),
        inbox: vec![
            InboxItem {
                task_id: "WP-188001".into(),
                wp_id: 188001,
                title: "Thêm bộ lọc danh sách user".into(),
                project: "Web Platform".into(),
                nx_status: "Open".into(),
                nx_status_color: None,
                priority: Some(PriorityView {
                    name: "High".into(),
                    color: Some("#ff922b".into()),
                    rank: Some(3),
                    ranks: Some(4),
                }),
            },
            InboxItem {
                task_id: "WP-188002".into(),
                wp_id: 188002,
                title: "Lỗi upload file lớn".into(),
                project: "Mobile App".into(),
                nx_status: "Open".into(),
                nx_status_color: None,
                priority: Some(PriorityView {
                    name: "Normal".into(),
                    color: None,
                    rank: Some(2),
                    ranks: Some(4),
                }),
            },
        ],
        distant: vec![DistantSession {
            id: "ext-1".into(),
            agent: AgentKind::Claude,
            label: "acme-desktop".into(),
            status: SessionStatus::Idle,
        }],
        quota: quota(),
        notice: None,
    }
}

pub fn board(tick: u64) -> BoardSnapshot {
    let frame = tick % frames();
    let system = active_system(frame);
    let inbox = |id: u64, title: &str, project: &str, comments: u32| TaskCard {
        id: format!("WP-{id}"),
        wp_id: id,
        title: title.into(),
        project: project.into(),
        repo: None,
        branch: None,
        nx_status: "Open".into(),
        nx_status_color: Some("#1a67a3".into()),
        column: TaskColumn::Inbox,
        step: None,
        agent: None,
        cost_usd: None,
        context_pct: None,
        new_comments: comments,
        gate: None,
        flow_note: None,
        login: None,
        issues: None,
        finished_at: None,
        push_check: None,
        nx_open: false,
        priority: None,
    };
    let mut tasks = vec![
        TaskCard {
            priority: Some(PriorityView {
                name: "High".into(),
                color: Some("#ff922b".into()),
                rank: Some(3),
                ranks: Some(4),
            }),
            ..inbox(188001, "Thêm bộ lọc danh sách user", "Web Platform", 2)
        },
        TaskCard {
            priority: Some(PriorityView {
                name: "Normal".into(),
                color: None,
                rank: Some(2),
                ranks: Some(4),
            }),
            ..inbox(188002, "Lỗi upload file lớn", "Mobile App", 0)
        },
        TaskCard {
            id: "WP-186196".into(),
            wp_id: 186196,
            title: "Chuẩn hoá thông báo lỗi".into(),
            project: "Web Platform".into(),
            repo: Some("acme-api".into()),
            branch: Some("feature/186196".into()),
            nx_status: "In progress".into(),
            nx_status_color: Some("#cc5de8".into()),
            column: TaskColumn::AwaitingPush,
            step: Some(StepRef {
                kind: StepKind::PushSummary,
                index: StepKind::PushSummary.index(),
                state: StepState::Done,
                since: DEMO_TIME.into(),
            }),
            agent: None,
            cost_usd: Some(1.84),
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
        },
    ];
    if let Some(sys) = &system {
        let current = sys
            .steps
            .iter()
            .find(|s| matches!(s.state, StepState::Running | StepState::WaitingUser));
        let agent = sys.agents.first();
        tasks.push(TaskCard {
            id: sys.task_id.clone(),
            wp_id: 187716,
            title: sys.title.clone(),
            project: "Web Platform".into(),
            repo: Some(sys.repo.clone()),
            branch: Some(sys.branch.clone()),
            nx_status: sys.nx_status.clone(),
            nx_status_color: sys.nx_status_color.clone(),
            column: if current.is_some() {
                TaskColumn::Active
            } else {
                TaskColumn::AwaitingPush
            },
            step: current.map(|s| StepRef {
                kind: s.kind,
                index: s.index,
                state: s.state,
                since: DEMO_TIME.into(),
            }),
            agent: agent.map(|a| a.kind),
            cost_usd: Some(0.21 + 0.1 * frame as f64),
            context_pct: agent.map(|a| (a.context_tokens as f64 / a.handoff_at as f64 * 100.0).min(100.0)),
            new_comments: sys.new_comments,
            gate: None,
            flow_note: None,
            login: None,
            issues: None,
            finished_at: None,
            push_check: None,
            nx_open: false,
            priority: None,
        });
    }
    BoardSnapshot {
        mode: opw_domain::view::DataMode::Demo,
        notice: None,
        tasks_dir_offer: None,
        unread_alerts: 0,
        active_task_ids: system
            .filter(|s| s.gate.is_some() || !s.agents.is_empty())
            .map(|s| s.task_id)
            .into_iter()
            .collect(),
        tasks,
        quota: quota(),
        links: Vec::new(),
        today: TodayStats {
            tokens: 1_200_000 + tick % 1000 * 1_000,
            cost_usd: 0.84,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_frame_is_valid_and_loop_covers_all_steps() {
        let mut seen_gates = Vec::new();
        for frame in 0..frames() {
            let orbit = orbit(frame);
            let board = board(frame);
            assert!(
                board
                    .tasks
                    .iter()
                    .filter(|t| t.column == TaskColumn::Active)
                    .count()
                    <= 1
            );
            if let Some(sys) = orbit.active {
                assert_eq!(sys.steps.len(), opw_domain::workflow::STEP_COUNT);
                if let Some(g) = sys.gate {
                    seen_gates.push(g.kind);
                }
            }
            serde_json::to_string(&board).unwrap();
        }
        use GateKind::{AfterFix, Findings, PlanReview};
        assert_eq!(
            seen_gates,
            vec![PlanReview, Findings, AfterFix, Findings, AfterFix, Findings]
        );
    }

    #[test]
    fn last_frame_is_awaiting_push_with_no_active_card() {
        let last = frames() - 1;
        let board = board(last);
        assert!(board.active_task_ids.is_empty());
        assert!(board.tasks.iter().all(|t| t.column != TaskColumn::Active));
    }
}
