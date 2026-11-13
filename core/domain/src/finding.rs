//! Review findings shared by Codex, Claude and `/review-local`, plus the gate policy.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FindingState {
    Open,
    Fixing,
    Fixed,
    Accepted,
    Deferred,
}

impl FindingState {
    /// Accepted and deferred findings are never asked about again.
    pub fn is_settled(self) -> bool {
        matches!(
            self,
            FindingState::Fixed | FindingState::Accepted | FindingState::Deferred
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub fingerprint: String,
    pub severity: Severity,
    pub title: String,
    pub file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    pub source: String,
    pub state: FindingState,
}

impl Finding {
    pub fn new(severity: Severity, file: &str, line: Option<u32>, title: &str, source: &str) -> Self {
        Self {
            fingerprint: fingerprint(file, title),
            severity,
            title: title.trim().to_owned(),
            file: file.trim().to_owned(),
            line,
            source: source.to_owned(),
            state: FindingState::Open,
        }
    }
}

/// Stable id for de-duplicating the same finding across rounds and reviewers.
/// FNV-1a 64 over the normalized `file` + `title` (line numbers move, so they are excluded).
pub fn fingerprint(file: &str, title: &str) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let normalized = format!("{}\u{1f}{}", normalize(file), normalize(title));
    let hash = normalized
        .bytes()
        .fold(OFFSET, |h, b| (h ^ u64::from(b)).wrapping_mul(PRIME));
    format!("{hash:016x}")
}

fn normalize(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
        .replace('\\', "/")
}

/// Two findings this many lines apart in the same file are shown as one "maybe the same" group.
pub const NEAR_LINES: u32 = 5;

/// Same fingerprint, or same file with lines at most [`NEAR_LINES`] apart. Needed because two
/// reviewers word the same bug differently (2026-11-13: `/review-local` and Codex both flagged
/// `job.repository.ts:316` with unrelated titles).
pub fn likely_same(a: &Finding, b: &Finding) -> bool {
    if a.fingerprint == b.fingerprint {
        return true;
    }
    match (a.line, b.line) {
        (Some(x), Some(y)) => normalize(&a.file) == normalize(&b.file) && x.abs_diff(y) <= NEAR_LINES,
        _ => false,
    }
}

/// Groups of indices into `findings` that are [`likely_same`], in first-seen order. Every
/// finding appears in exactly one group; most groups have a single member.
pub fn group_likely_same(findings: &[Finding]) -> Vec<Vec<usize>> {
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (i, f) in findings.iter().enumerate() {
        match groups
            .iter_mut()
            .find(|g| g.iter().any(|&j| likely_same(&findings[j], f)))
        {
            Some(g) => g.push(i),
            None => groups.push(vec![i]),
        }
    }
    groups
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateAction {
    /// Must be fixed before moving on (default: critical, high).
    Block,
    /// The user decides: fix, accept with a reason, or defer (default: medium).
    Ask,
    /// Shown only (default: low).
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatePolicy {
    pub block: Vec<Severity>,
    pub ask: Vec<Severity>,
    pub info: Vec<Severity>,
}

impl Default for GatePolicy {
    fn default() -> Self {
        Self {
            block: vec![Severity::Critical, Severity::High],
            ask: vec![Severity::Medium],
            info: vec![Severity::Low],
        }
    }
}

impl GatePolicy {
    pub fn action(&self, severity: Severity) -> GateAction {
        if self.block.contains(&severity) {
            GateAction::Block
        } else if self.ask.contains(&severity) {
            GateAction::Ask
        } else {
            GateAction::Info
        }
    }

    /// (blocking, undecided) counts among findings that are not settled yet.
    pub fn tally<'a>(&self, findings: impl IntoIterator<Item = &'a Finding>) -> (usize, usize) {
        findings
            .into_iter()
            .filter(|f| !f.state.is_settled())
            .fold((0, 0), |(b, a), f| match self.action(f.severity) {
                GateAction::Block => (b + 1, a),
                GateAction::Ask => (b, a + 1),
                GateAction::Info => (b, a),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_ignores_case_spacing_slashes_and_line() {
        let a = Finding::new(
            Severity::High,
            "api\\src\\order.ts",
            Some(42),
            "Null  when user has no org",
            "codex",
        );
        let b = Finding::new(
            Severity::Medium,
            "api/src/order.ts",
            Some(57),
            "null when user has NO org",
            "claude",
        );
        assert_eq!(a.fingerprint, b.fingerprint);
        assert_ne!(a.fingerprint, fingerprint("api/src/order.ts", "another title"));
        assert_eq!(a.fingerprint.len(), 16);
    }

    #[test]
    fn groups_the_same_bug_reported_with_different_titles() {
        let path = "BE/src/shared/repositories/job.repository.ts";
        let findings = [
            Finding::new(
                Severity::High,
                path,
                Some(316),
                "CAS blocks terminal-to-terminal recompute",
                "review-local",
            ),
            Finding::new(
                Severity::High,
                path,
                Some(316),
                "Preserve reconciliation for terminal jobs",
                "codex",
            ),
            Finding::new(
                Severity::Medium,
                path,
                Some(330),
                "Stale snapshot passes the CAS",
                "review-local",
            ),
            Finding::new(
                Severity::Low,
                ".claude/rules/known-false-positives.md",
                Some(75),
                "Docs out of date",
                "review-local",
            ),
            Finding::new(
                Severity::Low,
                "BE\\src\\shared\\repositories\\job.repository.ts",
                Some(319),
                "Nit",
                "codex",
            ),
        ];
        assert_eq!(
            group_likely_same(&findings),
            vec![vec![0, 1, 4], vec![2], vec![3]]
        );
        assert!(!likely_same(
            &Finding::new(Severity::Low, "a.ts", None, "x", "codex"),
            &Finding::new(Severity::Low, "a.ts", None, "y", "codex"),
        ));
    }

    #[test]
    fn default_gate_blocks_high_asks_medium_and_skips_settled() {
        let policy = GatePolicy::default();
        assert_eq!(policy.action(Severity::Critical), GateAction::Block);
        assert_eq!(policy.action(Severity::Medium), GateAction::Ask);
        assert_eq!(policy.action(Severity::Low), GateAction::Info);

        let mut accepted = Finding::new(Severity::Medium, "a.ts", None, "x", "codex");
        accepted.state = FindingState::Accepted;
        let findings = [
            Finding::new(Severity::High, "a.ts", None, "h", "codex"),
            Finding::new(Severity::Medium, "b.ts", None, "m", "codex"),
            Finding::new(Severity::Low, "c.ts", None, "l", "codex"),
            accepted,
        ];
        assert_eq!(policy.tally(&findings), (1, 1));
    }
}
