//! Settings → Các bước AI: the agent, model and effort of each AI step, and what past runs of
//! that step cost. The app shows prices and history for reference only; it never picks a model
//! or an effort by itself (PLAN.md §12, "Model và effort theo bước").

use std::collections::HashMap;
use std::path::PathBuf;

use opw_config::Config;
use opw_domain::view::{AgentKind, ApiError, StepChoiceRequest, StepSetting};
use opw_domain::workflow::StepKind;
use opw_storage::StepRunRow;

use crate::flow::{agent_for, default_effort, step_config, step_key};

/// `--effort` values of Claude Code (`claude --help`, 2.1.323).
pub const CLAUDE_EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
/// `model_reasoning_effort` values of Codex.
pub const CODEX_EFFORTS: [&str; 5] = ["minimal", "low", "medium", "high", "xhigh"];

/// The AI steps, in workflow order (intake runs no AI).
pub fn ai_steps() -> impl Iterator<Item = StepKind> {
    StepKind::ALL.into_iter().filter(|s| *s != StepKind::Intake)
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
            StepSetting {
                step,
                agent: agent_for(step, config),
                agent_choice: step == StepKind::CrossReview,
                model: cfg.model.filter(|m| !m.trim().is_empty()),
                effort: cfg.effort.filter(|e| !e.trim().is_empty()),
                effort_default: default_effort(step, config).map(str::to_owned),
                runs: acc.runs,
                avg_cost_usd: (acc.costed > 0).then(|| acc.cost / f64::from(acc.costed)),
                avg_seconds: (acc.timed > 0).then(|| acc.seconds / f64::from(acc.timed)),
                last_model: acc.last_model,
            }
        })
        .collect()
}

/// `~/.codex/config.toml` (or `$CODEX_HOME/config.toml`).
fn codex_config() -> Option<PathBuf> {
    let home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".codex")))?;
    Some(home.join("config.toml"))
}

/// Models named in the Codex config, for the Settings list (empty when there is none).
pub fn codex_models() -> Vec<String> {
    codex_config()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| opw_agents::codex::configured_models(&t))
        .unwrap_or_default()
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
pub fn validate(req: &StepChoiceRequest, config: &Config) -> Result<AgentKind, ApiError> {
    if req.step == StepKind::Intake {
        return Err(invalid("Bước Nhận task không chạy AI"));
    }
    let agent = match req.agent {
        Some(a) if req.step != StepKind::CrossReview && a != agent_for(req.step, config) => {
            return Err(invalid("Bước này chưa đổi được agent (mới có Review chéo)"));
        }
        Some(AgentKind::Shell) => return Err(invalid("Agent phải là Claude hoặc Codex")),
        Some(a) => a,
        None => agent_for(req.step, config),
    };
    if let Some(m) = req.model.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
        if !valid_model(m) {
            return Err(invalid(format!("Tên model không hợp lệ: {m}")));
        }
    }
    if let Some(e) = req.effort.as_deref().map(str::trim).filter(|e| !e.is_empty()) {
        let allowed: &[&str] = if agent == AgentKind::Codex {
            &CODEX_EFFORTS
        } else {
            &CLAUDE_EFFORTS
        };
        if !allowed.contains(&e) {
            return Err(invalid(format!("Effort không hợp lệ cho bước này: {e}")));
        }
    }
    Ok(agent)
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
        assert_eq!(rows.len(), 7);
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
        };
        assert_eq!(
            validate(
                &req(StepKind::Fix, None, Some("claude-sonnet-5-5"), Some("max")),
                &config
            )
            .unwrap(),
            AgentKind::Claude
        );
        assert_eq!(
            validate(
                &req(
                    StepKind::CrossReview,
                    Some(AgentKind::Codex),
                    Some("gpt-6.1-sol"),
                    Some("xhigh")
                ),
                &config
            )
            .unwrap(),
            AgentKind::Codex
        );
        for bad in [
            req(StepKind::Intake, None, None, None),
            req(StepKind::Fix, None, Some("--dangerously-skip-permissions"), None),
            req(StepKind::Fix, None, Some("opus 5"), None),
            req(StepKind::Fix, None, None, Some("ultra")),
            req(StepKind::CrossReview, Some(AgentKind::Codex), None, Some("max")),
            req(StepKind::Fix, Some(AgentKind::Codex), None, None),
        ] {
            assert_eq!(
                validate(&bad, &config).unwrap_err().code,
                "step.choice",
                "{bad:?}"
            );
        }
        assert_eq!(
            opw_agents::codex::configured_models("model = \"gpt-a\"\n"),
            ["gpt-a"]
        );
    }
}
