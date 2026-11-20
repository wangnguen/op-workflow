//! Quota sources verified on 2026-11-13 (PLAN.md §7), all credential-free:
//!
//! | Source | Shape | Unit |
//! |---|---|---|
//! | Claude `rate_limit_event` (stream-json) | `rate_limit_info.unifiedWindows.{five_hour,seven_day}.{utilization,resetsAt}` | 0..1, unix s |
//! | Claude statusline JSON | `rate_limits.{five_hour,seven_day}.{used_percentage,resets_at}` | 0..100, unix s |
//! | Codex `app-server` `account/rateLimits/read` | `rateLimits.{primary,secondary}.{usedPercent,windowDurationMins,resetsAt}` | 0..100, unix s |
//! | Codex rollout `token_count.rate_limits` | `{primary,secondary}.{used_percent,window_minutes,resets_at}` | 0..100, unix s |
//! | Claude Code's own cache, `~/.claude.json` `cachedUsageUtilization` (checked 2026-11-17, CLI 2.1.x) | `utilization.{five_hour,seven_day}.{utilization,resets_at}`, `fetchedAtMs` | 0..100, RFC 3339 |
//!
//! The last row is what Claude Code's own usage panel shows: every Claude Code process (CLI, VS
//! Code) writes it after asking the server, so reading it costs no tokens and no credentials.
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
pub const SOURCE_CLAUDE_CODE: &str = "claude-code";

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
        // a request the limit refused says "rejected", often without a utilization
        let u = match num(info.get("utilization")) {
            Some(u) => u,
            None if info.get("status").and_then(Value::as_str) == Some("rejected") => 1.0,
            None => return None,
        };
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

/// Claude Code's config file (`~/.claude.json`, parsed whole): the usage it last fetched for the
/// signed-in account, timed by `fetchedAtMs`. `None` when the cache is missing or belongs to
/// another account than `oauthAccount` (someone switched accounts since). Only these two keys
/// are read; the file holds no token.
pub fn from_claude_code_cache(config: &Value) -> Option<QuotaSnapshot> {
    let cache = config.get("cachedUsageUtilization")?;
    let owner = cache.get("accountUuid").and_then(Value::as_str)?;
    let signed_in = config
        .pointer("/oauthAccount/accountUuid")
        .and_then(Value::as_str)?;
    if owner != signed_in {
        return None;
    }
    let fetched = num(cache.get("fetchedAtMs"))?;
    let usage = cache.get("utilization")?;
    let mut windows: Vec<QuotaWindow> = [
        ("five_hour", QuotaWindowKind::FiveHour),
        ("seven_day", QuotaWindowKind::SevenDay),
    ]
    .into_iter()
    .filter_map(|(key, kind)| {
        let w = usage.get(key)?;
        Some(window_at(kind, num(w.get("utilization"))?, w.get("resets_at")))
    })
    .collect();
    if windows.is_empty() {
        // newer shape: `limits: [{kind: "session" | "weekly_all", percent, resets_at}]`
        windows = usage
            .get("limits")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|l| {
                let kind = match l.get("kind").and_then(Value::as_str)? {
                    "session" => QuotaWindowKind::FiveHour,
                    "weekly_all" => QuotaWindowKind::SevenDay,
                    _ => return None,
                };
                Some(window_at(kind, num(l.get("percent"))?, l.get("resets_at")))
            })
            .collect();
    }
    let observed_at = unix_to_rfc3339(fetched / 1000.0)?;
    (!windows.is_empty()).then(|| snapshot(Provider::Claude, windows, SOURCE_CLAUDE_CODE, &observed_at, None))
}

/// `a` was observed after `b` (both RFC 3339; a string that does not parse is never newer).
pub fn is_newer(a: &str, b: &str) -> bool {
    seconds_between(b, a).is_some_and(|s| s > 0.0)
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

/// Seconds from `from` to `to` (both RFC 3339); `None` when either does not parse.
pub fn seconds_between(from: &str, to: &str) -> Option<f64> {
    let a = OffsetDateTime::parse(from, &Rfc3339).ok()?;
    let b = OffsetDateTime::parse(to, &Rfc3339).ok()?;
    Some((b - a).as_seconds_f64())
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

/// Like [`window`], with the reset time given as RFC 3339 text (stored in UTC like the others).
fn window_at(kind: QuotaWindowKind, used_pct: f64, resets_at: Option<&Value>) -> QuotaWindow {
    let reset = resets_at
        .and_then(Value::as_str)
        .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
        .map(|t| t.unix_timestamp() as f64);
    window(kind, used_pct, reset)
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

    #[test]
    fn seconds_between_rfc3339_stamps() {
        let s = seconds_between("2026-11-14T08:00:00Z", "2026-11-14T08:01:35.5Z").unwrap();
        assert!((s - 95.5).abs() < 1e-6);
        assert_eq!(seconds_between("x", "2026-11-14T08:00:00Z"), None);
    }

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
    fn a_refused_request_counts_as_a_full_window() {
        let ev = json!({"type":"rate_limit_event","rate_limit_info":{"status":"rejected",
            "resetsAt":1794568800,"rateLimitType":"five_hour"}});
        let s = from_claude_rate_limit_event(&ev, AT).unwrap();
        assert_eq!(used(&s, QuotaWindowKind::FiveHour), 100.0);
        assert!(s.windows[0].resets_at.is_some());
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

    /// Shape of `~/.claude.json` written by Claude Code on 2026-11-17 (ids replaced, other keys cut).
    fn claude_code_config(owner: &str) -> Value {
        json!({
            "oauthAccount": {"accountUuid": "acc-1", "emailAddress": "x"},
            "cachedUsageUtilization": {"fetchedAtMs": 1_794_886_482_855_u64, "accountUuid": owner,
                "utilization": {
                    "five_hour": {"utilization": 27, "resets_at": "2026-11-17T07:19:59.656530+00:00", "limit_dollars": null},
                    "seven_day": {"utilization": 43, "resets_at": "2026-11-22T11:59:59.656547+00:00"},
                    "seven_day_opus": null,
                    "limits": [{"kind": "session", "percent": 27}, {"kind": "weekly_all", "percent": 43}]}}
        })
    }

    #[test]
    fn claude_code_cache() {
        let s = from_claude_code_cache(&claude_code_config("acc-1")).unwrap();
        assert_eq!(
            (s.provider, s.source.as_str()),
            (Provider::Claude, SOURCE_CLAUDE_CODE)
        );
        assert_eq!(s.observed_at, "2026-11-17T03:34:42Z");
        assert!((used(&s, QuotaWindowKind::FiveHour) - 27.0).abs() < 1e-9);
        assert!((used(&s, QuotaWindowKind::SevenDay) - 43.0).abs() < 1e-9);
        assert_eq!(s.windows[0].resets_at.as_deref(), Some("2026-11-17T07:19:59Z"));
        // another account's numbers, or nobody signed in: nothing
        assert!(from_claude_code_cache(&claude_code_config("acc-2")).is_none());
        let mut signed_out = claude_code_config("acc-1");
        signed_out.as_object_mut().unwrap().remove("oauthAccount");
        assert!(from_claude_code_cache(&signed_out).is_none());
        assert!(from_claude_code_cache(&json!({})).is_none());
    }

    #[test]
    fn claude_code_cache_falls_back_to_limits() {
        let mut c = claude_code_config("acc-1");
        let u = c.pointer_mut("/cachedUsageUtilization/utilization").unwrap();
        u["five_hour"] = Value::Null;
        u["seven_day"] = Value::Null;
        let s = from_claude_code_cache(&c).unwrap();
        assert_eq!(s.windows.len(), 2);
        assert!((used(&s, QuotaWindowKind::SevenDay) - 43.0).abs() < 1e-9);
        assert!(s.windows[0].resets_at.is_none());
    }

    #[test]
    fn newer_compares_times_not_text() {
        // as text "…00Z" > "…00.5Z", but 00.5 is later
        assert!(is_newer("2026-11-17T03:00:00.5Z", "2026-11-17T03:00:00Z"));
        assert!(!is_newer("2026-11-17T03:00:00Z", "2026-11-17T03:00:00Z"));
        assert!(!is_newer("x", "2026-11-17T03:00:00Z"));
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
