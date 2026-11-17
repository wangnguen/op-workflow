//! The bell: alerts from the workflow (a step ended and waits), OpenProject (new comments, a task
//! moved to Test failed / Reopened) and quota levels. The last [`KEEP`] are kept in
//! `<data>/alerts.json` with their read/deleted marks, so a restart keeps the bell; the shell shows the
//! new ones as Windows notifications when that kind is on (`notify` in config). Alerts read back
//! from the file are never shown as notifications again.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use opw_domain::view::{AlertKind, AlertView, NotifySettings, Provider, QuotaSnapshot, QuotaWindowKind};
use serde::{Deserialize, Serialize};

/// Active alerts kept for the bell (Deleted uses its separate 30-day retention).
pub const KEEP: usize = 100;
/// The bell's file in the data folder.
pub const FILE: &str = "alerts.json";
pub const DELETED_RETENTION_SECS: f64 = 30.0 * 24.0 * 60.0 * 60.0;
/// At most this many alerts in Deleted (overflow from the active list lands there too); the
/// oldest deletions go first, before their 30 days.
pub const DELETED_KEEP: usize = 500;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedBell {
    next_id: u64,
    items: Vec<AlertView>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum SavedFile {
    Current(SavedBell),
    Legacy(Vec<AlertView>),
}

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
    /// Where the bell is saved; `None` keeps it in memory only (tests, browser previews).
    file: Option<PathBuf>,
}

impl Alerts {
    /// The bell saved in `file` (empty when the file is missing or unreadable).
    pub fn open(file: PathBuf) -> Self {
        let saved: Option<SavedFile> = std::fs::read_to_string(&file)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok());
        let (next_id, mut items) = match saved {
            Some(SavedFile::Current(s)) => (s.next_id, s.items),
            Some(SavedFile::Legacy(items)) => (0, items),
            None => (0, Vec::new()),
        };
        items.sort_by_key(|a| std::cmp::Reverse(a.id));
        let inner = Inner {
            next_id: next_id.max(items.iter().map(|a| a.id).max().unwrap_or(0)),
            items: items.into(),
            fresh: Vec::new(),
        };
        let alerts = Self {
            inner: Mutex::new(inner),
            file: Some(file),
        };
        alerts.purge_expired(&opw_quota::now_rfc3339());
        alerts
    }

    /// Write the bell to its file (a temp file renamed over it, so a crash leaves the old one).
    fn save(&self, inner: &Inner) {
        let Some(file) = &self.file else { return };
        let saved = SavedBell {
            next_id: inner.next_id,
            items: inner.items.iter().cloned().collect(),
        };
        if let Ok(json) = serde_json::to_string(&saved) {
            let _ = write_replacing(file, &json);
        }
    }

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
            deleted_at: None,
        };
        inner.items.push_front(view.clone());
        trim_active(&mut inner, at);
        inner.fresh.push(view);
        self.save(&inner);
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
            .map(|i| {
                u32::try_from(
                    i.items
                        .iter()
                        .filter(|a| !a.read && a.deleted_at.is_none())
                        .count(),
                )
                .unwrap_or(u32::MAX)
            })
            .unwrap_or_default()
    }

    /// Mark these alerts read; an empty list marks all.
    pub fn mark_read(&self, ids: &[u64]) {
        self.mark(|a| ids.is_empty() || ids.contains(&a.id));
    }

    /// The user looked at what these alerts were about (e.g. opened the Comment tab).
    pub fn mark_task_read(&self, wp_id: u64, kinds: &[AlertKind]) {
        self.mark(|a| a.wp_id == Some(wp_id) && kinds.contains(&a.kind));
    }

    /// Mark the unread alerts that `which` picks as read; save only when one changed.
    fn mark(&self, which: impl Fn(&AlertView) -> bool) {
        let Ok(mut inner) = self.inner.lock() else { return };
        let mut changed = false;
        for a in inner
            .items
            .iter_mut()
            .filter(|a| !a.read && a.deleted_at.is_none() && which(a))
        {
            a.read = true;
            changed = true;
        }
        if changed {
            self.save(&inner);
        }
    }

    /// Move selected active alerts to Deleted; empty selects all active alerts.
    /// They become read and cannot be handed to the shell as fresh toasts afterwards.
    pub fn delete(&self, ids: &[u64], now: &str) {
        let Ok(mut inner) = self.inner.lock() else { return };
        let mut removed = Vec::new();
        for a in inner
            .items
            .iter_mut()
            .filter(|a| a.deleted_at.is_none() && (ids.is_empty() || ids.contains(&a.id)))
        {
            a.deleted_at = Some(now.to_owned());
            a.read = true;
            removed.push(a.id);
        }
        if !removed.is_empty() {
            inner.fresh.retain(|a| !removed.contains(&a.id));
            cap_deleted(&mut inner);
            self.save(&inner);
        }
    }

    /// Restore selected alerts (empty = all deleted), keeping them read and never re-toasting.
    pub fn restore(&self, ids: &[u64]) {
        let Ok(mut inner) = self.inner.lock() else { return };
        let mut restored = VecDeque::new();
        inner.items.retain_mut(|a| {
            if a.deleted_at.is_none() || (!ids.is_empty() && !ids.contains(&a.id)) {
                return true;
            }
            a.deleted_at = None;
            a.read = true;
            restored.push_back(a.clone());
            false
        });
        if !restored.is_empty() {
            // Restoring an old item into a full bell must keep that restored item.
            restored.append(&mut inner.items);
            inner.items = restored;
            trim_active(&mut inner, &opw_quota::now_rfc3339());
            self.save(&inner);
        }
    }

    /// Called at startup and while the app runs. The 30 days start at deletion, not creation.
    pub fn purge_expired(&self, now: &str) {
        let Ok(mut inner) = self.inner.lock() else { return };
        let before = inner.items.len();
        inner.items.retain(|a| {
            !a.deleted_at.as_deref().is_some_and(|deleted| {
                opw_quota::seconds_between(deleted, now).is_some_and(|age| age >= DELETED_RETENTION_SECS)
            })
        });
        let archived = trim_active(&mut inner, now);
        if before != inner.items.len() || archived {
            self.save(&inner);
        }
    }
}

/// Keep overflow recoverable instead of erasing other alerts when an old one is restored.
fn trim_active(inner: &mut Inner, now: &str) -> bool {
    let mut active = 0;
    let mut archived = Vec::new();
    for a in &mut inner.items {
        if a.deleted_at.is_some() {
            continue;
        }
        active += 1;
        if active > KEEP {
            a.deleted_at = Some(now.to_owned());
            a.read = true;
            archived.push(a.id);
        }
    }
    inner.fresh.retain(|a| !archived.contains(&a.id));
    let capped = cap_deleted(inner);
    !archived.is_empty() || capped
}

/// Drop the oldest deletions past [`DELETED_KEEP`], so the file and every bell poll stay small.
fn cap_deleted(inner: &mut Inner) -> bool {
    let deleted = inner.items.iter().filter(|a| a.deleted_at.is_some()).count();
    if deleted <= DELETED_KEEP {
        return false;
    }
    let since_epoch = |at: &str| opw_quota::seconds_between("1970-01-01T00:00:00Z", at).unwrap_or(0.0);
    let mut by_age: Vec<(f64, u64)> = inner
        .items
        .iter()
        .filter_map(|a| a.deleted_at.as_deref().map(|at| (since_epoch(at), a.id)))
        .collect();
    by_age.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let drop: std::collections::HashSet<u64> = by_age
        .iter()
        .take(deleted - DELETED_KEEP)
        .map(|(_, id)| *id)
        .collect();
    inner.items.retain(|a| !drop.contains(&a.id));
    true
}

fn write_replacing(file: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, file)
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
pub fn quota_crossings(quota: &[QuotaSnapshot], levels: &[u8], now: &str) -> Vec<QuotaCross> {
    let mut out = Vec::new();
    for snap in quota {
        for w in &snap.windows {
            if !w.used_pct.is_finite()
                || w.resets_at
                    .as_deref()
                    .is_some_and(|r| opw_quota::seconds_between(r, now).is_some_and(|s| s >= 0.0))
            {
                continue;
            }
            let Some(level) = levels
                .iter()
                .copied()
                .filter(|l| (1..=100).contains(l) && w.used_pct >= f64::from(*l))
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
pub fn quota_mark(c: &QuotaCross, remembered: Option<&str>, now: &str) -> String {
    let previous = remembered.and_then(|m| m.split_once('@'));
    let same_period = previous.is_some_and(|(_, reset)| !reset_passed(reset, now));
    let level = if same_period {
        c.level.max(
            previous
                .and_then(|(level, _)| level.parse::<u8>().ok())
                .unwrap_or(0),
        )
    } else {
        c.level
    };
    let old_reset = previous.map(|(_, r)| r).filter(|r| !r.is_empty());
    let reset = match (same_period.then_some(old_reset).flatten(), c.resets_at.as_deref()) {
        (Some(old), Some(new)) if opw_quota::seconds_between(new, old).is_some_and(|s| s >= 0.0) => old,
        (_, Some(new)) => new,
        (Some(old), None) => old,
        _ => "",
    };
    format!("{level}@{reset}")
}

/// Announce `c` unless the same window already got this level (or a higher one) in the same
/// reset period.
pub fn quota_is_new(c: &QuotaCross, remembered: Option<&str>, now: &str) -> bool {
    let Some((level, resets)) = remembered.and_then(|m| m.split_once('@')) else {
        return true;
    };
    let level: u8 = level.parse().unwrap_or(0);
    reset_passed(resets, now) || c.level > level
}

fn reset_passed(reset: &str, now: &str) -> bool {
    opw_quota::seconds_between(reset, now).is_some_and(|s| s >= 0.0)
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
        assert_eq!(a.list().iter().filter(|a| a.deleted_at.is_none()).count(), KEEP);
    }

    #[test]
    fn the_bell_survives_a_restart_without_notifying_again() {
        let dir = std::env::temp_dir().join(format!("opw-alerts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join(FILE);
        let a = Alerts::open(file.clone());
        assert!(a.list().is_empty(), "no file yet");
        a.push(new(AlertKind::Comment, 1), "2026-11-14T01:00:00Z");
        a.push(new(AlertKind::Gate, 2), "2026-11-14T02:00:00Z");
        a.mark_task_read(2, &[AlertKind::Gate]);
        assert_eq!(a.take_fresh().len(), 2);

        let b = Alerts::open(file.clone());
        let list = b.list();
        assert_eq!(list.iter().map(|x| x.id).collect::<Vec<_>>(), [2, 1]);
        assert_eq!((list[0].read, list[1].read), (true, false));
        assert_eq!(b.unread(), 1);
        assert!(b.take_fresh().is_empty(), "read back, not new");
        b.push(new(AlertKind::Error, 3), "2026-11-14T03:00:00Z");
        assert_eq!(b.list()[0].id, 3, "ids go on after the saved ones");
        b.mark_read(&[]);
        assert_eq!(Alerts::open(file.clone()).unread(), 0);

        std::fs::write(&file, "not json").unwrap();
        assert!(Alerts::open(file).list().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
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
        let now = "2026-11-17T08:00:00Z";
        let reset = "2026-11-20T00:00:00Z";
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
        assert!(quota_crossings(&[snap(79.0, reset)], &[80, 95], now).is_empty());
        let c = &quota_crossings(&[snap(82.0, reset)], &[80, 95], now)[0];
        assert_eq!((c.key.as_str(), c.level), ("claude.sevenDay", 80));
        assert!(quota_is_new(c, None, now));
        let mark = quota_mark(c, None, now);
        assert!(!quota_is_new(c, Some(&mark), now));
        let higher = &quota_crossings(&[snap(96.0, reset)], &[80, 95], now)[0];
        assert!(quota_is_new(higher, Some(&mark), now));
        let later = "2026-11-21T00:00:00Z";
        let next_week = &quota_crossings(&[snap(81.0, "2026-11-27T00:00:00Z")], &[80, 95], later)[0];
        assert!(quota_is_new(next_week, Some(&mark), later));
        assert!(quota_crossings(&[snap(100.0, reset)], &[80, 95], later).is_empty());
        assert_eq!(quota_title(higher), "Claude · tuần đã dùng 96%");
    }

    #[test]
    fn changing_sources_and_reset_formats_do_not_repeat_exhausted_quota() {
        let now = "2026-11-17T09:00:00Z";
        let mut c = QuotaCross {
            key: "claude.fiveHour".into(),
            provider: Provider::Claude,
            window: QuotaWindowKind::FiveHour,
            level: 80,
            used_pct: 91.0,
            resets_at: Some("2026-11-17T12:19:59Z".into()),
        };
        let mut mark = quota_mark(&c, None, now);
        c.resets_at = Some("2026-11-17T19:20:00+07:00".into());
        assert!(!quota_is_new(&c, Some(&mark), now));
        c.level = 95;
        c.used_pct = 100.0;
        assert!(quota_is_new(&c, Some(&mark), now), "escalate once");
        mark = quota_mark(&c, Some(&mark), now);
        for i in 0..12 {
            c.level = if i % 2 == 0 { 80 } else { 95 };
            c.used_pct = if i % 2 == 0 { 91.0 } else { 100.0 };
            c.resets_at = match i % 3 {
                0 => None,
                1 => Some("2026-11-17T12:19:59.999Z".into()),
                _ => Some("2026-11-17T19:20:00+07:00".into()),
            };
            assert!(!quota_is_new(&c, Some(&mark), now));
            mark = quota_mark(&c, Some(&mark), now);
            assert!(mark.starts_with("95@"));
        }
    }

    #[test]
    fn deleted_keeps_the_latest_deletions_up_to_its_cap() {
        let a = Alerts::default();
        for i in 0..(KEEP + DELETED_KEEP + 10) as u64 {
            a.push(new(AlertKind::Comment, i), "2099-01-01T00:00:00Z");
        }
        let list = a.list();
        assert_eq!(list.iter().filter(|x| x.deleted_at.is_none()).count(), KEEP);
        assert_eq!(
            list.iter().filter(|x| x.deleted_at.is_some()).count(),
            DELETED_KEEP
        );
        // the first alerts overflowed first, so they were dropped first
        assert!(
            list.iter().all(|x| x.id > 10),
            "{:?}",
            list.iter().map(|x| x.id).min()
        );
        a.delete(&[], "2099-01-02T00:00:00Z");
        let list = a.list();
        assert_eq!(list.len(), DELETED_KEEP);
        assert!(list.iter().all(|x| x.deleted_at.is_some()));
        // the alerts deleted just now stay, older overflow goes
        assert_eq!(
            list.iter()
                .filter(|x| x.deleted_at.as_deref() == Some("2099-01-02T00:00:00Z"))
                .count(),
            KEEP
        );
    }

    #[test]
    fn deleted_alerts_are_read_restorable_and_never_toasted_again() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(FILE);
        let a = Alerts::open(file.clone());
        a.push(new(AlertKind::Quota, 1), "2000-01-01T00:00:00Z");
        a.push(new(AlertKind::Comment, 2), "2099-01-01T00:00:00Z");
        a.delete(&[1], "2099-01-01T00:00:00Z");
        a.delete(&[1], "2099-01-02T00:00:00Z"); // deleting again does not extend retention
        assert_eq!(a.unread(), 1);
        assert_eq!(a.take_fresh().iter().map(|a| a.id).collect::<Vec<_>>(), [2]);
        let b = Alerts::open(file);
        let deleted = b.list().into_iter().find(|a| a.id == 1).unwrap();
        assert!(deleted.read);
        assert_eq!(deleted.deleted_at.as_deref(), Some("2099-01-01T00:00:00Z"));
        b.restore(&[1]);
        assert_eq!(b.unread(), 1, "restoring keeps it read");
        assert!(b.list().iter().all(|a| a.deleted_at.is_none()));
        assert!(b.take_fresh().is_empty());
        b.delete(&[], "2099-01-01T00:00:00Z");
        assert_eq!(b.unread(), 0);
    }

    #[test]
    fn only_deleted_alerts_expire_after_30_days_and_ids_are_not_reused() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(FILE);
        let a = Alerts::open(file.clone());
        a.push(new(AlertKind::Quota, 1), "2000-01-01T00:00:00Z");
        a.delete(&[1], "2099-01-01T00:00:00Z");
        a.purge_expired("2099-01-30T23:59:59Z");
        assert_eq!(a.list().len(), 1, "creation time does not count");
        a.purge_expired("2099-01-31T07:00:00+07:00");
        assert!(a.list().is_empty());
        let b = Alerts::open(file);
        b.push(new(AlertKind::Comment, 2), "2000-01-01T00:00:00Z");
        b.purge_expired("2099-02-02T00:00:00Z");
        assert_eq!(b.list().len(), 1, "active alerts have no 30-day expiry");
        assert_eq!(b.list()[0].id, 2);
    }

    #[test]
    fn legacy_bells_load_and_new_active_alerts_cannot_evict_deleted_items() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(FILE);
        std::fs::write(&file, r#"[{"id":41,"at":"2026-11-17T00:00:00Z","kind":"quota","title":"quota","body":"","read":false}]"#).unwrap();
        let a = Alerts::open(file.clone());
        assert_eq!(a.unread(), 1);
        a.delete(&[41], "2099-01-01T00:00:00Z");
        for i in 0..KEEP + 5 {
            a.push(new(AlertKind::Comment, i as u64), "2099-01-01T00:00:00Z");
        }
        assert_eq!(a.list().iter().filter(|a| a.deleted_at.is_none()).count(), KEEP);
        assert!(a.list().iter().any(|a| a.id == 41 && a.deleted_at.is_some()));
        a.restore(&[41]);
        assert!(a.list().iter().any(|a| a.id == 41 && a.deleted_at.is_none()));
        assert_eq!(
            a.list().len(),
            KEEP + 6,
            "restoring must not erase other active alerts"
        );
        let b = Alerts::open(file);
        assert!(b.list().iter().any(|a| a.id == 41));
        assert!(b.take_fresh().is_empty());
    }
}
