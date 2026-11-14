//! Quota sources verified on 2026-11-13 (PLAN.md §7), all credential-free:
//!
//! | Source | Shape | Unit |
//! |---|---|---|
//! | Claude `rate_limit_event` (stream-json) | `rate_limit_info.unifiedWindows.{five_hour,seven_day}.{utilization,resetsAt}` | 0..1, unix s |
//! | Claude statusline JSON | `rate_limits.{five_hour,seven_day}.{used_percentage,resets_at}` | 0..100, unix s |
//! | Codex `app-server` `account/rateLimits/read` | `rateLimits.{primary,secondary}.{usedPercent,windowDurationMins,resetsAt}` | 0..100, unix s |
//! | Codex rollout `token_count.rate_limits` | `{primary,secondary}.{used_percent,window_minutes,resets_at}` | 0..100, unix s |
//!
//! Parsers are pure: callers pass `observed_at`. Missing fields give `None`, never a panic.

use opw_domain::view::{Provider, QuotaSnapshot, QuotaWindow, QuotaWindowKind};
use serde_json::Value;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

const FIVE_HOURS_MIN: f64 = 300.0;
const SEVEN_DAYS_MIN: f64 = 10_080.0;

pub const SOURCE_RATE_LIMIT_EVENT: &str = "rate_limit_event";
pub const SOURCE_STATUSLINE: &str = "statusline";
pub const SOURCE_APP_SERVER: &str = "app-server";
pub const SOURCE_ROLLOUT: &str = "rollout";

/// Claude `{"type":"rate_limit_event","rate_limit_info":{...}}` from `claude -p --output-format stream-json`.
/// Uses `unifiedWindows` (observed, not yet in the SDK types) and falls back to the declared
/// `rateLimitType` + `utilization` pair, which carries a single window per event.
pub fn from_claude_rate_limit_event(event: &Value, observed_at: &str) -> Option<QuotaSnapshot> {
    let info = event.get("rate_limit_info")?;
    let mut windows = Vec::new();
    if let Some(unified) = info.get("unifiedWindows") {
        for (key, kind) in [
            ("five_hour", QuotaWindowKind::FiveHour),
            ("seven_day", QuotaWindowKind::SevenDay),
        ] {
            if let Some(w) = unified.get(key) {
                if let Some(u) = num(w.get("utilization")) {
                    windows.push(window(kind, u * 100.0, num(w.get("resetsAt"))));
                }
            }
        }
    }
    if windows.is_empty() {
        let kind = match info.get("rateLimitType")?.as_str()? {
            "five_hour" => QuotaWindowKind::FiveHour,
            "seven_day" | "seven_day_opus" | "seven_day_sonnet" => QuotaWindowKind::SevenDay,
            _ => return None,
        };
        let u = num(info.get("utilization"))?;
        windows.push(window(kind, u * 100.0, num(info.get("resetsAt"))));
    }
    Some(snapshot(
        Provider::Claude,
        windows,
        SOURCE_RATE_LIMIT_EVENT,
        observed_at,
        None,
    ))
}

/// Claude Code statusline stdin JSON (Pro/Max/Team, after the first API response).
pub fn from_claude_statusline(status: &Value, observed_at: &str) -> Option<QuotaSnapshot> {
    let limits = status.get("rate_limits")?;
    let windows: Vec<_> = [
        ("five_hour", QuotaWindowKind::FiveHour),
        ("seven_day", QuotaWindowKind::SevenDay),
    ]
    .into_iter()
    .filter_map(|(key, kind)| {
        let w = limits.get(key)?;
        Some(window(
            kind,
            num(w.get("used_percentage"))?,
            num(w.get("resets_at")),
        ))
    })
    .collect();
    (!windows.is_empty()).then(|| snapshot(Provider::Claude, windows, SOURCE_STATUSLINE, observed_at, None))
}

/// Codex app-server `account/rateLimits/read` result (or the `rateLimits` of an
/// `account/rateLimits/updated` notification). Accepts the whole result or just the snapshot.
pub fn from_codex_app_server(result: &Value, observed_at: &str) -> Option<QuotaSnapshot> {
    let snap = result.get("rateLimits").unwrap_or(result);
    let windows = codex_windows(snap, "usedPercent", "windowDurationMins", "resetsAt");
    let plan = snap.get("planType").and_then(Value::as_str).map(str::to_owned);
    (!windows.is_empty()).then(|| snapshot(Provider::Codex, windows, SOURCE_APP_SERVER, observed_at, plan))
}

/// Codex rollout `token_count` payload: `{"type":"token_count","rate_limits":{...}}`.
pub fn from_codex_rollout(payload: &Value, observed_at: &str) -> Option<QuotaSnapshot> {
    let limits = payload.get("rate_limits").unwrap_or(payload);
    let windows = codex_windows(limits, "used_percent", "window_minutes", "resets_at");
    let plan = limits.get("plan_type").and_then(Value::as_str).map(str::to_owned);
    (!windows.is_empty()).then(|| snapshot(Provider::Codex, windows, SOURCE_ROLLOUT, observed_at, plan))
}

/// A window whose reset time has passed is shown as 0% until a fresh reading arrives
/// (the rollout file kept "17%" after the weekly reset in our test).
pub fn effective_used_pct(w: &QuotaWindow, now: OffsetDateTime) -> f64 {
    let expired = w
        .resets_at
        .as_deref()
        .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
        .is_some_and(|reset| reset <= now);
    if expired { 0.0 } else { w.used_pct }
}

/// Percent left right now (0..100), with a passed reset counting as full.
pub fn remaining_pct_now(w: &QuotaWindow) -> f64 {
    (100.0 - effective_used_pct(w, OffsetDateTime::now_utc())).clamp(0.0, 100.0)
}

pub fn unix_to_rfc3339(secs: f64) -> Option<String> {
    #[allow(clippy::cast_possible_truncation)]
    let dt = OffsetDateTime::from_unix_timestamp(secs as i64).ok()?;
    dt.format(&Rfc3339).ok()
}

pub fn now_rfc3339() -> String {
    OffsetDateTime::now_utc().format(&Rfc3339).unwrap_or_default()
}

/// `hours` ago, in the same UTC form as [`now_rfc3339`] (so the strings compare in time order).
pub fn hours_ago_rfc3339(hours: u32) -> String {
    (OffsetDateTime::now_utc() - time::Duration::hours(i64::from(hours)))
        .format(&Rfc3339)
        .unwrap_or_default()
}

fn codex_windows(v: &Value, used_key: &str, mins_key: &str, reset_key: &str) -> Vec<QuotaWindow> {
    ["primary", "secondary"]
        .iter()
        .filter_map(|slot| {
            let w = v.get(slot).filter(|w| !w.is_null())?;
            let used = num(w.get(used_key))?;
            let kind = match num(w.get(mins_key)) {
                Some(m) if (m - SEVEN_DAYS_MIN).abs() < 1.0 => QuotaWindowKind::SevenDay,
                Some(m) if (m - FIVE_HOURS_MIN).abs() < 1.0 => QuotaWindowKind::FiveHour,
                // unknown duration: primary is the short window, secondary the weekly one
                _ if *slot == "primary" => QuotaWindowKind::FiveHour,
                _ => QuotaWindowKind::SevenDay,
            };
            Some(window(kind, used, num(w.get(reset_key))))
        })
        .collect()
}

fn window(kind: QuotaWindowKind, used_pct: f64, resets_at: Option<f64>) -> QuotaWindow {
    QuotaWindow {
        kind,
        used_pct: used_pct.clamp(0.0, 100.0),
        resets_at: resets_at.filter(|r| *r > 0.0).and_then(unix_to_rfc3339),
    }
}

fn snapshot(
    provider: Provider,
    windows: Vec<QuotaWindow>,
    source: &str,
    observed_at: &str,
    plan: Option<String>,
) -> QuotaSnapshot {
    QuotaSnapshot {
        provider,
        windows,
        source: source.into(),
        observed_at: observed_at.into(),
        plan,
    }
}

fn num(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const AT: &str = "2026-11-13T08:00:00Z";

    fn used(s: &QuotaSnapshot, kind: QuotaWindowKind) -> f64 {
        s.windows
            .iter()
            .find(|w| w.kind == kind)
            .map(|w| w.used_pct)
            .unwrap()
    }

    /// Captured from `claude -p ... --output-format stream-json` on 2026-11-13.
    #[test]
    fn claude_rate_limit_event_unified_windows() {
        let ev = json!({"type":"rate_limit_event","rate_limit_info":{"status":"allowed","resetsAt":1794568800,
            "rateLimitType":"five_hour","overageStatus":"rejected","overageDisabledReason":"org_level_disabled",
            "isUsingOverage":false,"unifiedWindows":{"five_hour":{"utilization":0.16,"resetsAt":1794568800},
            "seven_day":{"utilization":0.74,"resetsAt":1794744000}}}});
        let s = from_claude_rate_limit_event(&ev, AT).unwrap();
        assert_eq!(s.provider, Provider::Claude);
        assert!((used(&s, QuotaWindowKind::FiveHour) - 16.0).abs() < 1e-9);
        assert!((used(&s, QuotaWindowKind::SevenDay) - 74.0).abs() < 1e-9);
        assert_eq!(s.windows[0].resets_at.as_deref(), Some("2026-11-13T11:20:00Z"));
    }

    #[test]
    fn claude_rate_limit_event_falls_back_to_declared_fields() {
        let ev = json!({"type":"rate_limit_event","rate_limit_info":{"status":"allowed_warning",
            "rateLimitType":"seven_day","utilization":0.9,"resetsAt":1794744000}});
        let s = from_claude_rate_limit_event(&ev, AT).unwrap();
        assert_eq!(s.windows.len(), 1);
        assert!((used(&s, QuotaWindowKind::SevenDay) - 90.0).abs() < 1e-9);
        assert!(from_claude_rate_limit_event(&json!({"type":"rate_limit_event"}), AT).is_none());
    }

    #[test]
    fn claude_statusline() {
        let st = json!({"rate_limits":{"five_hour":{"used_percentage":42.5,"resets_at":1794568800},
            "seven_day":{"used_percentage":74}}});
        let s = from_claude_statusline(&st, AT).unwrap();
        assert_eq!(s.source, SOURCE_STATUSLINE);
        assert!((used(&s, QuotaWindowKind::FiveHour) - 42.5).abs() < 1e-9);
        assert!(s.windows[1].resets_at.is_none());
        assert!(from_claude_statusline(&json!({"model":{}}), AT).is_none());
    }

    /// Captured from `codex app-server` `account/rateLimits/read` on 2026-11-13 (account id removed).
    #[test]
    fn codex_app_server() {
        let res = json!({"ordinaryUsageAllowed":true,"rateLimits":{"limitId":"codex","limitName":null,
            "primary":{"usedPercent":3,"windowDurationMins":300,"resetsAt":1794561106},
            "secondary":{"usedPercent":0,"windowDurationMins":10080,"resetsAt":1795150440},
            "credits":{"hasCredits":false,"unlimited":false,"balance":"0"},"planType":"plus"}});
        let s = from_codex_app_server(&res, AT).unwrap();
        assert_eq!(s.plan.as_deref(), Some("plus"));
        assert!((used(&s, QuotaWindowKind::FiveHour) - 3.0).abs() < 1e-9);
        assert!((used(&s, QuotaWindowKind::SevenDay)).abs() < 1e-9);
    }

    /// Captured from `~/.codex/sessions/**/rollout-*.jsonl` `token_count` on 2026-11-13.
    #[test]
    fn codex_rollout() {
        let p = json!({"type":"token_count","rate_limits":{"limit_id":"codex",
            "primary":{"used_percent":3.0,"window_minutes":300,"resets_at":1794561106},
            "secondary":{"used_percent":17.0,"window_minutes":10080,"resets_at":1794544784},
            "plan_type":"plus"}});
        let s = from_codex_rollout(&p, AT).unwrap();
        assert_eq!(s.source, SOURCE_ROLLOUT);
        assert!((used(&s, QuotaWindowKind::SevenDay) - 17.0).abs() < 1e-9);
    }

    #[test]
    fn expired_window_counts_as_zero() {
        let w = QuotaWindow {
            kind: QuotaWindowKind::SevenDay,
            used_pct: 17.0,
            resets_at: unix_to_rfc3339(1_794_544_784.0),
        };
        let before = OffsetDateTime::from_unix_timestamp(1_794_544_000).unwrap();
        let after = OffsetDateTime::from_unix_timestamp(1_794_545_000).unwrap();
        assert!((effective_used_pct(&w, before) - 17.0).abs() < 1e-9);
        assert!(effective_used_pct(&w, after).abs() < 1e-9);
    }
}
