//! Quota and the flow (PLAN.md §12 part 5g): a warning before a step that costs tokens when a
//! window passed `quota.warnAt`, "Tự chạy" stopping at the highest level, and a usage limit hit in
//! the middle of a step recognised as such (with the reset time) instead of a plain failure.

use std::sync::LazyLock;

use opw_domain::view::{
    FlowStatus, GateKind, Provider, QuotaSnapshot, QuotaWindow, QuotaWindowKind, TaskDetail,
};
use opw_domain::workflow::StepKind;
use regex::Regex;

/// The warning and the stop level from `quota.warnAt` (default 80 / 95).
pub fn levels(warn_at: &[u8]) -> (f64, f64) {
    let mut v: Vec<u8> = warn_at
        .iter()
        .copied()
        .filter(|l| (1..=100).contains(l))
        .collect();
    v.sort_unstable();
    match (v.first(), v.last()) {
        (Some(lo), Some(hi)) => (f64::from(*lo), f64::from(*hi)),
        _ => (80.0, 95.0),
    }
}

/// The AI action offered now, including approving a plan or starting a selected fix.
pub fn note_step(d: &TaskDetail) -> Option<StepKind> {
    let step = match d.gate.as_ref().map(|g| g.kind) {
        Some(GateKind::PlanReview) => Some(StepKind::Implement),
        Some(GateKind::Findings) if d.findings.iter().any(|f| f.chosen) => {
            Some(match d.gate.as_ref()?.review? {
                StepKind::ReviewLocal | StepKind::Harness => StepKind::Fix,
                _ => StepKind::FixCross,
            })
        }
        Some(GateKind::Findings) => d.gate.as_ref()?.next,
        Some(GateKind::AfterFix) => Some(StepKind::CrossReview),
        Some(GateKind::Error) => d.current,
        _ if d.status == FlowStatus::Ready => d.current,
        _ => None,
    }?;
    if matches!(step, StepKind::Intake | StepKind::Checks)
        || (step == StepKind::Harness && d.tests.iter().any(|t| t.step == step && t.command.is_some()))
    {
        return None;
    }
    Some(step)
}

/// The fullest window of a snapshot.
pub fn worst(snap: &QuotaSnapshot) -> Option<&QuotaWindow> {
    snap.windows
        .iter()
        .filter(|w| w.used_pct.is_finite())
        .max_by(|a, b| a.used_pct.total_cmp(&b.used_pct))
}

/// The gauges already treat a passed reset as empty; flow decisions use the same rule.
pub fn current(mut snap: QuotaSnapshot, now: &str) -> QuotaSnapshot {
    for w in &mut snap.windows {
        if w.resets_at
            .as_deref()
            .is_some_and(|r| opw_quota::seconds_between(r, now).is_some_and(|s| s >= 0.0))
        {
            w.used_pct = 0.0;
        }
    }
    snap
}

/// Every window over the stop level must reset before auto-run can go on.
/// An unknown reset keeps the wait open until a fresh gauge reading clears it.
pub fn stop_reset(snap: &QuotaSnapshot, stop: f64) -> Option<String> {
    let mut latest: Option<&str> = None;
    for w in snap.windows.iter().filter(|w| w.used_pct >= stop) {
        let reset = w.resets_at.as_deref()?;
        opw_quota::seconds_between(reset, reset)?;
        if latest.is_none_or(|r| opw_quota::seconds_between(r, reset).is_some_and(|s| s > 0.0)) {
            latest = Some(reset);
        }
    }
    latest.map(str::to_owned)
}

pub fn provider_name(p: Provider) -> &'static str {
    match p {
        Provider::Claude => "Claude",
        Provider::Codex => "Codex",
    }
}

pub fn window_name(kind: QuotaWindowKind) -> &'static str {
    match kind {
        QuotaWindowKind::FiveHour => "5 giờ",
        QuotaWindowKind::SevenDay => "tuần",
    }
}

/// "Claude 5 giờ đã dùng 96%".
pub fn describe(p: Provider, w: &QuotaWindow) -> String {
    format!(
        "{} {} đã dùng {:.0}%",
        provider_name(p),
        window_name(w.kind),
        w.used_pct
    )
}

/// What a CLI says when the plan's usage limit ends a run ("Claude AI usage limit reached",
/// "You've hit your limit · resets 3pm", Codex "You've hit your usage limit", HTTP 429).
static QUOTA_HIT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)usage[ _-]?limit|hit your (usage )?limit|rate[ _-]?limit|quota[ _ ]exceeded|insufficient_quota|too many requests|\b429\b")
        .expect("valid regex")
});

/// The failed run's text says the usage limit ended it.
pub fn looks_quota_hit(text: &str) -> bool {
    QUOTA_HIT.is_match(text)
}

/// Claude's "Claude AI usage limit reached|1763380800": the reset time it carries.
pub fn reset_in_text(text: &str) -> Option<String> {
    static EPOCH: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)limit[^|\n]*\|\s*(\d{10})").expect("valid regex"));
    let secs: f64 = EPOCH.captures(text)?.get(1)?.as_str().parse().ok()?;
    opw_quota::unix_to_rfc3339(secs)
}

/// When the run can go on: the reset time the CLI gave, else the reset of the fullest window.
pub fn reset_time(text: &str, snap: Option<&QuotaSnapshot>) -> Option<String> {
    reset_in_text(text).or_else(|| snap.and_then(worst).and_then(|w| w.resets_at.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(five: f64, week: f64) -> QuotaSnapshot {
        QuotaSnapshot {
            provider: Provider::Claude,
            windows: vec![
                QuotaWindow {
                    kind: QuotaWindowKind::FiveHour,
                    used_pct: five,
                    resets_at: Some("2026-11-17T10:00:00Z".into()),
                },
                QuotaWindow {
                    kind: QuotaWindowKind::SevenDay,
                    used_pct: week,
                    resets_at: Some("2026-11-20T00:00:00Z".into()),
                },
            ],
            source: "probe".into(),
            observed_at: "2026-11-17T08:00:00Z".into(),
            plan: None,
        }
    }

    #[test]
    fn levels_worst_window_and_limit_messages() {
        assert_eq!(levels(&[95, 80]), (80.0, 95.0));
        assert_eq!(levels(&[]), (80.0, 95.0));
        assert_eq!(levels(&[90]), (90.0, 90.0));
        let s = snap(40.0, 97.0);
        let w = worst(&s).unwrap();
        assert_eq!(w.kind, QuotaWindowKind::SevenDay);
        assert_eq!(describe(Provider::Claude, w), "Claude tuần đã dùng 97%");
        for hit in [
            "Claude AI usage limit reached|1763380800",
            "You've hit your limit · resets 3pm (Asia/Saigon)",
            "You've hit your usage limit. Upgrade to Pro",
            "stream error: 429 Too Many Requests",
            "rate_limit_exceeded",
        ] {
            assert!(looks_quota_hit(hit), "{hit}");
        }
        assert!(!looks_quota_hit("Claude báo lỗi: cannot find module"));
        assert!(!looks_quota_hit("max turns limit reached"));
        assert!(!looks_quota_hit("maximum budget limit reached"));
        assert_eq!(
            reset_in_text("Claude AI usage limit reached|1763380800").as_deref(),
            Some("2025-11-17T12:00:00Z")
        );
        assert_eq!(
            reset_time("hit your limit", Some(&s)).as_deref(),
            Some("2026-11-20T00:00:00Z")
        );
        assert_eq!(reset_time("hit your limit", None), None);
    }

    #[test]
    fn expired_windows_and_multiple_blocking_windows_follow_the_gauges() {
        let s = snap(100.0, 96.0);
        assert_eq!(stop_reset(&s, 95.0).as_deref(), Some("2026-11-20T00:00:00Z"));
        let after_five = current(s.clone(), "2026-11-17T10:00:00Z");
        assert_eq!(worst(&after_five).unwrap().kind, QuotaWindowKind::SevenDay);
        let after_both = current(s.clone(), "2026-11-20T07:00:00+07:00");
        assert_eq!(worst(&after_both).unwrap().used_pct, 0.0);
        let mut unknown = s;
        unknown.windows[1].resets_at = None;
        assert_eq!(stop_reset(&unknown, 95.0), None);
    }
}
