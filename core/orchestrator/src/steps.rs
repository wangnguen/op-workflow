//! Settings → Các bước AI: the agent, model, effort and permission mode of each AI step, and
//! what past runs of that step cost. Models, effort levels and permission modes are picked from
//! the lists the CLIs on the machine give (`models.rs`); the app never picks one by itself
//! (PLAN.md §12, "Model và effort theo bước", part 6b).

use std::collections::HashMap;

use opw_config::Config;
use opw_domain::view::{AgentKind, ApiError, StepChoiceRequest, StepModels, StepSetting};
use opw_domain::workflow::StepKind;
use opw_storage::StepRunRow;

use crate::flow::{
    agent_default, agent_for, agent_switchable, default_effort, permission_default, permission_mode,
    step_config, step_key,
};
use crate::models::check_effort;

/// The AI steps with a setting of their own, in workflow order. Nhận task and Kiểm tra run no
/// AI; the second Sửa lỗi shares the first one's setting.
pub fn ai_steps() -> impl Iterator<Item = StepKind> {
    StepKind::ALL.into_iter().filter(|s| has_setting(*s))
}

fn has_setting(step: StepKind) -> bool {
    step != StepKind::FixCross && step_config(step, &opw_config::StepsConfig::default()).is_some()
}

/// One row per AI step: what is picked, and the cost and time of its finished runs.
pub fn settings(config: &Config, done_runs: &[StepRunRow]) -> Vec<StepSetting> {
    #[derive(Default)]
    struct Acc {
        runs: u32,
        cost: f64,
        costed: u32,
        seconds: f64,
        timed: u32,
        last_model: Option<String>,
    }
    let mut by_step: HashMap<&str, Acc> = HashMap::new();
    for r in done_runs {
        let acc = by_step.entry(r.step.as_str()).or_default();
        acc.runs += 1;
        if let Some(c) = r.cost_usd.filter(|c| *c > 0.0) {
            acc.cost += c;
            acc.costed += 1;
        }
        if let Some(s) = r
            .ended_at
            .as_deref()
            .and_then(|end| opw_quota::seconds_between(&r.started_at, end))
            .filter(|s| *s >= 0.0)
        {
            acc.seconds += s;
            acc.timed += 1;
        }
        if r.model.is_some() {
            acc.last_model.clone_from(&r.model);
        }
    }
    ai_steps()
        .map(|step| {
            let cfg = step_config(step, &config.steps).cloned().unwrap_or_default();
            let acc = by_step.remove(step_key(step)).unwrap_or_default();
            let agent = agent_for(step, config);
            StepSetting {
                step,
                agent,
                agent_choice: agent_switchable(step),
                agent_default: agent_default(step, config),
                permission_mode: permission_mode(step, config),
                permission_default: permission_default(step, agent).map(str::to_owned),
                model: cfg.model.filter(|m| !m.trim().is_empty()),
                effort: cfg.effort.filter(|e| !e.trim().is_empty()),
                effort_default: default_effort(step, config).map(str::to_owned),
                runs: acc.runs,
                max_budget_usd: cfg.max_budget_usd,
                timeout_min: cfg.timeout_min,
                default_budget_usd: crate::flow::limits(step, &Config::default()).0,
                default_timeout_min: (crate::flow::limits(step, &Config::default()).1.as_secs() / 60) as u32,
                avg_cost_usd: (acc.costed > 0).then(|| acc.cost / f64::from(acc.costed)),
                avg_seconds: (acc.timed > 0).then(|| acc.seconds / f64::from(acc.timed)),
                last_model: acc.last_model,
            }
        })
        .collect()
}

/// A model name goes into the CLI's arguments: letters, digits and `._-:/[]` only, so it can
/// never be read as another flag.
fn valid_model(m: &str) -> bool {
    !m.starts_with('-')
        && m.len() <= 80
        && m.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':' | '/' | '[' | ']'))
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::new("step.choice", message)
}

/// Check a Settings pick before it goes into config.json. Returns the agent the step will use.
/// `models` are the lists last read from the CLIs (Settings reads them before showing the
/// page); the effort must be one the picked model takes there.
pub fn validate(
    req: &StepChoiceRequest,
    config: &Config,
    models: Option<&StepModels>,
) -> Result<AgentKind, ApiError> {
    if !has_setting(req.step) {
        return Err(invalid("Bước này không có model / effort riêng để chọn"));
    }
    let agent = match req.agent {
        Some(AgentKind::Shell) => return Err(invalid("Agent phải là Claude hoặc Codex")),
        Some(a) if !agent_switchable(req.step) && a != agent_for(req.step, config) => {
            return Err(invalid(
                "Test trình duyệt chạy bằng Claude cho tới khi Codex qua một lần chạy thử thật",
            ));
        }
        Some(a) => a,
        None => agent_for(req.step, config),
    };
    let catalog = models.map(|m| {
        if agent == AgentKind::Codex {
            &m.codex
        } else {
            &m.claude
        }
    });
    let model = req.model.as_deref().map(str::trim).filter(|m| !m.is_empty());
    if let Some(m) = model {
        if !valid_model(m) {
            return Err(invalid(format!("Tên model không hợp lệ: {m}")));
        }
        // only models the CLI lists; a saved one it no longer lists may stay (to change the effort)
        let saved = step_config(req.step, &config.steps)
            .and_then(|c| c.model.as_deref())
            .map(str::trim);
        if let Some(c) = catalog.filter(|c| c.from_cli) {
            if saved != Some(m) && !c.models.iter().any(|x| x.id == m) {
                let cli = if agent == AgentKind::Codex {
                    "Codex"
                } else {
                    "Claude Code"
                };
                return Err(invalid(format!("{cli} trên máy không có model {m}")));
            }
        }
    }
    if let Some(e) = req.effort.as_deref().map(str::trim).filter(|e| !e.is_empty()) {
        check_effort(catalog, model, e).map_err(invalid)?;
    }
    if let Some(m) = req
        .permission_mode
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
    {
        check_mode(req.step, agent, m, config, catalog)?;
    }
    Ok(agent)
}

/// A permission mode goes into the CLI's arguments: letters, digits and `-` only. With the
/// CLI's list read, only its modes (or the one already saved) pass.
fn check_mode(
    step: StepKind,
    agent: AgentKind,
    mode: &str,
    config: &Config,
    catalog: Option<&opw_domain::view::AgentCatalog>,
) -> Result<(), ApiError> {
    if mode.starts_with('-')
        || mode.len() > 60
        || !mode.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err(invalid(format!("Chế độ quyền không hợp lệ: {mode}")));
    }
    let saved = (agent == agent_for(step, config))
        .then(|| permission_mode(step, config))
        .flatten();
    if let Some(c) = catalog.filter(|c| c.from_cli && !c.modes.is_empty()) {
        if saved.as_deref() != Some(mode) && !c.modes.iter().any(|m| m == mode) {
            let cli = if agent == AgentKind::Codex {
                "Codex"
            } else {
                "Claude Code"
            };
            return Err(invalid(format!("{cli} trên máy không có chế độ {mode}")));
        }
    }
    // Phân tích runs Claude with --restricted, which refuses to bypass permissions
    if step == StepKind::Analyze && agent == AgentKind::Claude && mode == "bypassPermissions" {
        return Err(invalid(
            "Phân tích chạy Claude ở chế độ chỉ đọc (--restricted), không nhận bypassPermissions",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn run(step: &str, cost: Option<f64>, start: &str, end: &str, model: Option<&str>) -> StepRunRow {
        StepRunRow {
            id: 0,
            wp_id: 1,
            step: step.into(),
            agent: Some("claude".into()),
            session_id: None,
            state: "done".into(),
            round: None,
            cost_usd: cost,
            tokens_in: None,
            tokens_out: None,
            started_at: start.into(),
            ended_at: Some(end.into()),
            model: model.map(str::to_owned),
            effort: None,
            note: None,
            log_file: None,
            denials: None,
        }
    }

    #[test]
    fn rows_show_picks_defaults_and_history() {
        let config = Config::from_value(&json!({
            "steps": {
                "fix": {"model": "claude-sonnet-5-5", "effort": "high"},
                "crossReview": {"agent": "claude"}
            }
        }));
        let runs = [
            run(
                "fix",
                Some(0.5),
                "2026-11-14T08:00:00Z",
                "2026-11-14T08:01:00Z",
                Some("claude-opus-5-5"),
            ),
            run(
                "fix",
                Some(0.3),
                "2026-11-14T09:00:00Z",
                "2026-11-14T09:02:00Z",
                None,
            ),
            run("analyze", None, "2026-11-14T07:00:00Z", "bad", None),
        ];
        let rows = settings(&config, &runs);
        // Harness and Test trình duyệt have their own row; Nhận task, Kiểm tra and step 9 not
        let steps: Vec<StepKind> = rows.iter().map(|r| r.step).collect();
        assert_eq!(steps.len(), 9);
        assert!(steps.contains(&StepKind::Harness) && steps.contains(&StepKind::BrowserTest));
        assert_eq!(rows[0].step, StepKind::Analyze);
        let fix = rows.iter().find(|r| r.step == StepKind::Fix).unwrap();
        assert_eq!(fix.model.as_deref(), Some("claude-sonnet-5-5"));
        assert_eq!(fix.effort.as_deref(), Some("high"));
        assert_eq!(fix.runs, 2);
        assert!((fix.avg_cost_usd.unwrap() - 0.4).abs() < 1e-9);
        assert!((fix.avg_seconds.unwrap() - 90.0).abs() < 1e-9);
        assert_eq!(fix.last_model.as_deref(), Some("claude-opus-5-5"));
        let analyze = &rows[0];
        assert_eq!(
            (analyze.runs, analyze.avg_cost_usd, analyze.avg_seconds),
            (1, None, None)
        );
        let cross = rows.iter().find(|r| r.step == StepKind::CrossReview).unwrap();
        assert!(cross.agent_choice);
        assert_eq!(cross.agent, AgentKind::Claude);
        // Viết code is Claude, so a cross review nobody picked would be Codex
        assert_eq!(cross.agent_default, AgentKind::Codex);
        assert_eq!(fix.permission_default.as_deref(), Some("acceptEdits"));
        assert_eq!(rows[0].permission_default, None);
        assert!(
            rows.iter()
                .all(|r| r.agent_choice == (r.step != StepKind::BrowserTest))
        );
        let push = rows.iter().find(|r| r.step == StepKind::PushSummary).unwrap();
        assert_eq!(push.effort, None);
        assert_eq!(push.effort_default.as_deref(), Some("low"));
    }

    #[test]
    fn picks_are_checked_before_they_reach_the_cli() {
        let config = Config::default();
        let req = |step, agent, model: Option<&str>, effort: Option<&str>| StepChoiceRequest {
            step,
            agent,
            model: model.map(str::to_owned),
            effort: effort.map(str::to_owned),
            permission_mode: None,
        };
        // the lists as the CLIs gave them
        let models = StepModels {
            claude: crate::models::claude_catalog(
                &json!({"models": [
                    {"value": "sonnet", "resolvedModel": "claude-sonnet-5-5", "supportsEffort": true,
                     "supportedEffortLevels": ["low", "medium", "high", "xhigh", "max"]},
                    {"value": "claude-haiku-4-5", "resolvedModel": "claude-haiku-4-5"}
                ]}),
                &serde_json::Value::Null,
                None,
                "t",
            ),
            codex: crate::models::codex_catalog(
                &[
                    json!({"model": "gpt-6.1-sol", "isDefault": true, "supportedReasoningEfforts": [
                        {"reasoningEffort": "low"}, {"reasoningEffort": "xhigh"}, {"reasoningEffort": "ultra"}
                    ]}),
                ],
                &serde_json::Value::Null,
                None,
                "t",
            ),
        };
        let ok = |r: StepChoiceRequest| validate(&r, &config, Some(&models)).unwrap();
        assert_eq!(
            ok(req(StepKind::Fix, None, Some("claude-sonnet-5-5"), Some("max"))),
            AgentKind::Claude
        );
        assert_eq!(
            ok(req(
                StepKind::CrossReview,
                Some(AgentKind::Codex),
                Some("gpt-6.1-sol"),
                Some("ultra")
            )),
            AgentKind::Codex
        );
        // "Theo máy" model: the effort of Codex's default model
        assert_eq!(
            ok(req(
                StepKind::CrossReview,
                Some(AgentKind::Codex),
                None,
                Some("xhigh")
            )),
            AgentKind::Codex
        );
        for bad in [
            req(StepKind::Intake, None, None, None),
            req(StepKind::Fix, None, Some("--dangerously-skip-permissions"), None),
            req(StepKind::Fix, None, Some("opus 5"), None),
            req(StepKind::Fix, None, None, Some("ultra\"")),
            req(StepKind::CrossReview, Some(AgentKind::Codex), None, Some("max")),
            req(
                StepKind::CrossReview,
                Some(AgentKind::Codex),
                None,
                Some("minimal"),
            ),
            req(StepKind::Fix, None, Some("claude-haiku-4-5"), Some("low")),
            req(StepKind::BrowserTest, Some(AgentKind::Codex), None, None),
            req(StepKind::Fix, Some(AgentKind::Shell), None, None),
            // a Claude model for a step switched to Codex
            req(
                StepKind::Fix,
                Some(AgentKind::Codex),
                Some("claude-sonnet-5-5"),
                None,
            ),
            // not in the list the CLI gave
            req(StepKind::Fix, None, Some("claude-opus-9"), None),
            req(StepKind::CrossReview, None, Some("claude-sonnet-5-5"), None),
        ] {
            assert_eq!(
                validate(&bad, &config, Some(&models)).unwrap_err().code,
                "step.choice",
                "{bad:?}"
            );
        }
        // a model saved earlier that the CLI no longer lists may stay, e.g. to change the effort
        let saved = Config::from_value(&json!({"steps": {"fix": {"model": "claude-old-1"}}}));
        let keep = req(StepKind::Fix, None, Some("claude-old-1"), Some("max"));
        assert!(validate(&keep, &saved, Some(&models)).is_ok());
        assert!(validate(&keep, &config, Some(&models)).is_err());
        // lists not read yet: plain words pass, the CLI has the last say
        assert!(validate(&req(StepKind::Fix, None, None, Some("max")), &config, None).is_ok());
        assert!(validate(&req(StepKind::Fix, None, None, Some("-x")), &config, None).is_err());
        // every step but Test trình duyệt can run Codex
        assert_eq!(
            ok(req(
                StepKind::Fix,
                Some(AgentKind::Codex),
                Some("gpt-6.1-sol"),
                None
            )),
            AgentKind::Codex
        );
    }

    #[test]
    fn permission_modes_come_from_the_cli_list() {
        let config = Config::default();
        let mut models = StepModels {
            claude: crate::models::claude_unread(String::new(), &serde_json::Value::Null, None),
            codex: crate::models::codex_unread(String::new(), None),
        };
        models.claude.from_cli = true;
        models.claude.modes = ["acceptEdits", "auto", "bypassPermissions", "plan"]
            .map(str::to_owned)
            .to_vec();
        models.codex.from_cli = true;
        models.codex.modes = ["read-only", "workspace-write", "danger-full-access"]
            .map(str::to_owned)
            .to_vec();
        let req = |step, agent, mode: &str| StepChoiceRequest {
            step,
            agent,
            model: None,
            effort: None,
            permission_mode: Some(mode.to_owned()),
        };
        let check = |r: StepChoiceRequest| validate(&r, &config, Some(&models));
        assert!(check(req(StepKind::Implement, None, "bypassPermissions")).is_ok());
        assert!(
            check(req(
                StepKind::Implement,
                Some(AgentKind::Codex),
                "danger-full-access"
            ))
            .is_ok()
        );
        assert!(check(req(StepKind::CrossReview, None, "read-only")).is_ok());
        for bad in [
            // Claude's modes are not Codex's, and the other way round
            req(StepKind::Implement, None, "workspace-write"),
            req(StepKind::Implement, Some(AgentKind::Codex), "acceptEdits"),
            req(StepKind::Implement, None, "--dangerously-skip-permissions"),
            req(StepKind::Implement, None, "plan mode"),
            // --restricted refuses it
            req(StepKind::Analyze, None, "bypassPermissions"),
        ] {
            assert_eq!(check(bad.clone()).unwrap_err().code, "step.choice", "{bad:?}");
        }
        // a mode saved earlier that the CLI no longer lists may stay
        let saved = Config::from_value(&json!({"steps": {"fix": {"permissionMode": "dontAsk"}}}));
        let keep = req(StepKind::Fix, None, "dontAsk");
        assert!(validate(&keep, &saved, Some(&models)).is_ok());
        assert!(validate(&keep, &config, Some(&models)).is_err());
    }

    #[test]
    fn steps_follow_the_picked_agent_and_cross_review_the_other_one() {
        let codex_code = Config::from_value(&json!({
            "steps": {"implement": {"agent": "codex"}, "fix": {"agent": "codex", "permissionMode": "danger-full-access"}}
        }));
        let rows = settings(&codex_code, &[]);
        let row = |step| rows.iter().find(|r| r.step == step).unwrap();
        assert_eq!(row(StepKind::Implement).agent, AgentKind::Codex);
        assert_eq!(
            row(StepKind::Implement).permission_default.as_deref(),
            Some("workspace-write")
        );
        assert_eq!(
            row(StepKind::Fix).permission_mode.as_deref(),
            Some("danger-full-access")
        );
        assert_eq!(row(StepKind::Analyze).agent, AgentKind::Claude);
        // nobody picked the cross review's AI: the other one than Viết code's
        assert_eq!(row(StepKind::CrossReview).agent, AgentKind::Claude);
        assert_eq!(row(StepKind::CrossReview).agent_default, AgentKind::Claude);
        assert_eq!(
            row(StepKind::PushSummary).permission_default,
            None,
            "Claude push summary keeps its own flags"
        );
        // Test trình duyệt stays on Claude even when config.json names Codex
        let browser = Config::from_value(&json!({"steps": {"browserTest": {"agent": "codex"}}}));
        assert_eq!(agent_for(StepKind::BrowserTest, &browser), AgentKind::Claude);
    }
}
