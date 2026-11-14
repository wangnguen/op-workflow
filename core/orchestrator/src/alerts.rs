//! The bell: alerts from the workflow (a step ended and waits), OpenProject (new comments, a task
//! moved to Test failed / Reopened) and quota levels. The last [`KEEP`] stay in memory; the
//! shell shows the new ones as Windows notifications when that kind is on (`notify` in config).

use std::collections::VecDeque;
use std::sync::Mutex;

use opw_domain::view::{AlertKind, AlertView, NotifySettings, Provider, QuotaSnapshot, QuotaWindowKind};

/// Alerts kept for the bell.
pub const KEEP: usize = 100;

/// An alert before it gets its id and time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAlert {
    pub kind: AlertKind,
    pub wp_id: Option<u64>,
    pub task_id: Option<String>,
    pub title: String,
    pub body: String,
    pub tab: Option<String>,
}

#[derive(Debug, Default)]
struct Inner {
    next_id: u64,
    /// Newest first.
    items: VecDeque<AlertView>,
    /// Not handed to the shell yet.
    fresh: Vec<AlertView>,
}

#[derive(Debug, Default)]
pub struct Alerts {
    inner: Mutex<Inner>,
}

impl Alerts {
    pub fn push(&self, alert: NewAlert, at: &str) {
        let Ok(mut inner) = self.inner.lock() else { return };
        inner.next_id += 1;
        let view = AlertView {
            id: inner.next_id,
            at: at.to_owned(),
            kind: alert.kind,
            wp_id: alert.wp_id,
            task_id: alert.task_id,
            title: alert.title,
            body: alert.body,
            tab: alert.tab,
            read: false,
        };
        inner.items.push_front(view.clone());
        inner.items.truncate(KEEP);
        inner.fresh.push(view);
    }

    /// Alerts added since the last call (for Windows notifications), oldest first.
    pub fn take_fresh(&self) -> Vec<AlertView> {
        self.inner
            .lock()
            .map(|mut i| std::mem::take(&mut i.fresh))
            .unwrap_or_default()
    }

    /// Newest first.
    pub fn list(&self) -> Vec<AlertView> {
        self.inner
            .lock()
            .map(|i| i.items.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn unread(&self) -> u32 {
        self.inner
            .lock()
            .map(|i| u32::try_from(i.items.iter().filter(|a| !a.read).count()).unwrap_or(u32::MAX))
            .unwrap_or_default()
    }

    /// Mark these alerts read; an empty list marks all.
    pub fn mark_read(&self, ids: &[u64]) {
        if let Ok(mut inner) = self.inner.lock() {
            for a in &mut inner.items {
                if ids.is_empty() || ids.contains(&a.id) {
                    a.read = true;
                }
            }
        }
    }

    /// The user looked at what these alerts were about (e.g. opened the Comment tab).
    pub fn mark_task_read(&self, wp_id: u64, kinds: &[AlertKind]) {
        if let Ok(mut inner) = self.inner.lock() {
            for a in &mut inner.items {
                if a.wp_id == Some(wp_id) && kinds.contains(&a.kind) {
                    a.read = true;
                }
            }
        }
    }
}

/// Whether a Windows notification shows for this kind (the bell lists every alert anyway).
pub fn toast_wanted(kind: AlertKind, notify: &NotifySettings) -> bool {
    match kind {
        AlertKind::Comment | AlertKind::Status => notify.comments,
        AlertKind::Gate | AlertKind::StepDone | AlertKind::Error => notify.steps,
        AlertKind::Quota => notify.quota,
    }
}

/// Statuses that mean "the fix did not hold": Test failed, Reopened, Rejected (any case and
/// spacing, e.g. `Test Failed`, `test-failed`).
pub fn alarming_status(status: &str) -> bool {
    let s: String = status
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase();
    s.contains("failed") || s.contains("reopen") || s.contains("rejected")
}

/// One line of at most `max` characters, whitespace collapsed, "…" when cut.
pub fn excerpt(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let cut: String = flat.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

/// The highest warning level one quota window has reached.
#[derive(Debug, Clone, PartialEq)]
pub struct QuotaCross {
    /// `claude.fiveHour`: the key the app remembers it under.
    pub key: String,
    pub provider: Provider,
    pub window: QuotaWindowKind,
    pub level: u8,
    pub used_pct: f64,
    pub resets_at: Option<String>,
}

/// Windows at or above one of `levels` (e.g. `[80, 95]`), with the highest level reached.
pub fn quota_crossings(quota: &[QuotaSnapshot], levels: &[u8]) -> Vec<QuotaCross> {
    let mut out = Vec::new();
    for snap in quota {
        for w in &snap.windows {
            let Some(level) = levels
                .iter()
                .copied()
                .filter(|l| w.used_pct >= f64::from(*l))
                .max()
            else {
                continue;
            };
            let provider = match snap.provider {
                Provider::Claude => "claude",
                Provider::Codex => "codex",
            };
            let window = match w.kind {
                QuotaWindowKind::FiveHour => "fiveHour",
                QuotaWindowKind::SevenDay => "sevenDay",
            };
            out.push(QuotaCross {
                key: format!("{provider}.{window}"),
                provider: snap.provider,
                window: w.kind,
                level,
                used_pct: w.used_pct,
                resets_at: w.resets_at.clone(),
            });
        }
    }
    out
}

/// What the app remembers about an announced level: `95@<resets_at>`.
pub fn quota_mark(c: &QuotaCross) -> String {
    format!("{}@{}", c.level, c.resets_at.as_deref().unwrap_or_default())
}

/// Announce `c` unless the same window already got this level (or a higher one) in the same
/// reset period.
pub fn quota_is_new(c: &QuotaCross, remembered: Option<&str>) -> bool {
    let Some((level, resets)) = remembered.and_then(|m| m.split_once('@')) else {
        return true;
    };
    let level: u8 = level.parse().unwrap_or(0);
    resets != c.resets_at.as_deref().unwrap_or_default() || c.level > level
}

/// "Claude · 5 giờ đã dùng 82%".
pub fn quota_title(c: &QuotaCross) -> String {
    let who = match c.provider {
        Provider::Claude => "Claude",
        Provider::Codex => "Codex",
    };
    let window = match c.window {
        QuotaWindowKind::FiveHour => "5 giờ",
        QuotaWindowKind::SevenDay => "tuần",
    };
    format!("{who} · {window} đã dùng {:.0}%", c.used_pct)
}

#[cfg(test)]
mod tests {
    use super::*;
    use opw_domain::view::QuotaWindow;

    fn new(kind: AlertKind, wp: u64) -> NewAlert {
        NewAlert {
            kind,
            wp_id: Some(wp),
            task_id: Some(format!("WP-{wp}")),
            title: "t".into(),
            body: "b".into(),
            tab: None,
        }
    }

    #[test]
    fn bell_keeps_newest_first_and_hands_out_fresh_ones_once() {
        let a = Alerts::default();
        a.push(new(AlertKind::Comment, 1), "2026-11-14T01:00:00Z");
        a.push(new(AlertKind::Gate, 2), "2026-11-14T02:00:00Z");
        assert_eq!(a.unread(), 2);
        let list = a.list();
        assert_eq!(list[0].kind, AlertKind::Gate);
        assert_eq!(a.take_fresh().len(), 2);
        assert!(a.take_fresh().is_empty());
        a.mark_task_read(1, &[AlertKind::Comment]);
        assert_eq!(a.unread(), 1);
        a.mark_read(&[]);
        assert_eq!(a.unread(), 0);
        for i in 0..(KEEP as u64 + 5) {
            a.push(new(AlertKind::Error, i), "x");
        }
        assert_eq!(a.list().len(), KEEP);
    }

    #[test]
    fn statuses_toasts_and_excerpts() {
        for s in ["Test failed", "Test-Failed", "Reopened", "Rejected"] {
            assert!(alarming_status(s), "{s}");
        }
        for s in ["Open", "In progress", "Resolved", "Closed"] {
            assert!(!alarming_status(s), "{s}");
        }
        let off = NotifySettings {
            comments: false,
            ..NotifySettings::default()
        };
        assert!(!toast_wanted(AlertKind::Status, &off));
        assert!(toast_wanted(AlertKind::Gate, &off));
        assert_eq!(excerpt("  Vẫn lỗi\n\nkhi   lưu ", 50), "Vẫn lỗi khi lưu");
        assert_eq!(excerpt("abcdefghij", 5), "abcd…");
    }

    #[test]
    fn quota_levels_are_announced_once_per_reset() {
        let snap = |used: f64, resets: &str| QuotaSnapshot {
            provider: Provider::Claude,
            windows: vec![QuotaWindow {
                kind: QuotaWindowKind::SevenDay,
                used_pct: used,
                resets_at: Some(resets.into()),
            }],
            source: "probe".into(),
            observed_at: "x".into(),
            plan: None,
        };
        assert!(quota_crossings(&[snap(79.0, "r1")], &[80, 95]).is_empty());
        let c = &quota_crossings(&[snap(82.0, "r1")], &[80, 95])[0];
        assert_eq!((c.key.as_str(), c.level), ("claude.sevenDay", 80));
        assert!(quota_is_new(c, None));
        let mark = quota_mark(c);
        assert!(!quota_is_new(c, Some(&mark)));
        let higher = &quota_crossings(&[snap(96.0, "r1")], &[80, 95])[0];
        assert!(quota_is_new(higher, Some(&mark)));
        let next_week = &quota_crossings(&[snap(81.0, "r2")], &[80, 95])[0];
        assert!(quota_is_new(next_week, Some(&mark)));
        assert_eq!(quota_title(higher), "Claude · tuần đã dùng 96%");
    }
}
