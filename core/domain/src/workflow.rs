//! The twelve-step task workflow as a pure state machine (order chosen by the user, PLAN.md §12
//! "Luồng mới 12 bước").
//!
//! ```text
//! intake -> analyze -> <plan review gate> -> implement -> checks -> harness
//!   -> reviewLocal -> <findings gate> -> fix -> <"Review chéo lại?"> -> crossReview
//!   -> <findings gate> -> fixCross -> <"Review chéo lại?"> -> crossReview (again) …
//!   -> browserTest -> <findings gate> -> fixCross -> <"Review chéo lại?"> -> browserTest (again) …
//!   -> finalReview -> <findings gate> -> fixCross -> <"Review chéo lại?"> -> finalReview (again)
//!      (or browserTest again, then finalReview) …
//!   -> pushSummary
//! ```
//! Test trình duyệt runs before Review chốt (the user moved it on 2026-11-17), so the last
//! review also sees what was fixed after the browser test.
//! Every review that reports something stops at the findings gate: the user picks what the AI
//! fixes (or goes on without fixing). Every fix stops at the after-fix gate: the user decides
//! whether the other AI reviews it again. There is no cap on rounds or fixes.
//! Checks, harness and browserTest are optional: a task that does not use one marks it skipped
//! and moves on (nothing turns them on yet; later parts do).
//! At the plan gate the user can also code by hand (`SelfCode`): the workflow waits until they
//! say they are done, then reviews their change like an AI implementation.
//! The machine never commits or pushes; after `pushSummary` the task waits for the user.

use serde::{Deserialize, Serialize};

use crate::error::{DomainError, DomainResult};
use crate::view::GateKind;

/// Number of steps; the UI shows "x/12".
pub const STEP_COUNT: usize = 12;

/// Steps before the 12-step flow; tasks saved then are upgraded when read.
const LEGACY_STEP_COUNT: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StepKind {
    Intake,
    Analyze,
    Implement,
    /// Type check, lint and prettier, run by the app (no tokens). Optional.
    Checks,
    /// A scratch harness outside the repo. Optional: the AI proposes it, the user decides.
    Harness,
    /// First `/review-local`, right after implement.
    ReviewLocal,
    /// The AI that implemented (Claude by default) fixes what the first review found.
    Fix,
    /// The other AI (Codex by default) reviews the fixed code.
    CrossReview,
    /// Fixes what the cross review, the browser test or the final review found.
    FixCross,
    /// Chrome DevTools MCP test of the app. Optional.
    BrowserTest,
    /// `/review-local` again before the push summary: the last look at the code.
    FinalReview,
    PushSummary,
}

impl StepKind {
    pub const ALL: [StepKind; STEP_COUNT] = [
        StepKind::Intake,
        StepKind::Analyze,
        StepKind::Implement,
        StepKind::Checks,
        StepKind::Harness,
        StepKind::ReviewLocal,
        StepKind::Fix,
        StepKind::CrossReview,
        StepKind::FixCross,
        StepKind::BrowserTest,
        StepKind::FinalReview,
        StepKind::PushSummary,
    ];

    /// 1-based position, also the orbit number in the UI.
    pub fn index(self) -> u8 {
        match self {
            StepKind::Intake => 1,
            StepKind::Analyze => 2,
            StepKind::Implement => 3,
            StepKind::Checks => 4,
            StepKind::Harness => 5,
            StepKind::ReviewLocal => 6,
            StepKind::Fix => 7,
            StepKind::CrossReview => 8,
            StepKind::FixCross => 9,
            StepKind::BrowserTest => 10,
            StepKind::FinalReview => 11,
            StepKind::PushSummary => 12,
        }
    }

    /// Steps a task may leave out (skipped instead of run).
    pub fn is_optional(self) -> bool {
        matches!(self, StepKind::Checks | StepKind::Harness | StepKind::BrowserTest)
    }

    /// Either Sửa lỗi step.
    pub fn is_fix(self) -> bool {
        matches!(self, StepKind::Fix | StepKind::FixCross)
    }

    /// Tự review, Review chéo or Review chốt: steps that report findings.
    pub fn is_review(self) -> bool {
        matches!(
            self,
            StepKind::ReviewLocal | StepKind::CrossReview | StepKind::FinalReview
        )
    }

    /// Steps whose problems go to the findings gate: the three reviews, a harness that FAILs
    /// and a browser test with NG cases.
    pub fn reports_findings(self) -> bool {
        self.is_review() || matches!(self, StepKind::Harness | StepKind::BrowserTest)
    }

    /// The step after this one; `None` after the push summary.
    pub fn next(self) -> Option<StepKind> {
        StepKind::ALL.get(self.slot() + 1).copied()
    }

    fn slot(self) -> usize {
        usize::from(self.index() - 1)
    }
}

/// The Sửa lỗi step that fixes what `review` found: step 7 after Harness or Tự review, step 9
/// otherwise (Review chéo, Test trình duyệt, Review chốt).
pub fn fix_step_of(review: StepKind) -> StepKind {
    if matches!(review, StepKind::ReviewLocal | StepKind::Harness) {
        StepKind::Fix
    } else {
        StepKind::FixCross
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StepState {
    Pending,
    Running,
    WaitingUser,
    Done,
    Failed,
    Skipped,
}

/// Where the workflow as a whole stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    NotStarted,
    Running(StepKind),
    Gate(GateKind),
    Paused,
    /// push-summary is ready; the user reviews, commits and pushes by hand.
    AwaitingPush,
}

/// The user's answer at the after-fix gate ("Review chéo lại?").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AfterFix {
    /// Yes: the other AI reviews the change again (a new cross-review round).
    CrossReview,
    /// No: on to the next step (Test trình duyệt when the task uses it, else Review chốt);
    /// after a fix of what the browser test or the final review found, that step runs again.
    Next,
    /// Back to the findings of the review that was just fixed, to pick more of them.
    PickMore,
    /// After a fix of the final review's findings: try the fixed code in the browser again,
    /// then the final review runs again. Only when the task uses Test trình duyệt.
    BrowserTest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowEvent {
    Start,
    StepSucceeded(StepKind),
    /// Passed as "bỏ qua": an optional step with nothing to run (Kiểm tra without a command), or
    /// Review chốt when the code has not changed since Tự review (its result stands).
    StepSkipped(StepKind),
    StepFailed(StepKind),
    Retry(StepKind),
    PlanApproved,
    /// User gave feedback on the plan; analyze runs again.
    PlanRejected,
    /// At the plan gate: the user codes this one by hand.
    SelfCode,
    /// The user finished coding by hand; review it.
    SelfCodeDone,
    /// A review finished. `open` = findings it reported that the user has not accepted or
    /// deferred yet; any of them stops the flow at the findings gate.
    ReviewFinished {
        open: usize,
    },
    /// At the findings gate: `fix` findings are picked for the AI to fix; 0 = go on without
    /// fixing.
    FindingsPicked {
        fix: usize,
    },
    /// At the after-fix gate.
    AfterFix(AfterFix),
    /// Write the push summary again (another issue number, or the user changed the code).
    RedoPushSummary,
    Pause,
    Resume,
    /// "Làm tiếp" on a finished (or stopped) task: run again from this step, one the task already
    /// reached. Earlier steps keep their state, this one and the later ones start over. The
    /// caller refuses it while a step process runs.
    Reopen(StepKind),
    /// The AI of a running step stopped to ask the user something only they can do (log in to
    /// the app under test, give an account): the step waits at the question gate.
    AgentAsked(StepKind),
    /// The user answered (or did what was asked): the step runs again and goes on.
    AgentAnswered,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", try_from = "SavedWorkflow")]
pub struct Workflow {
    states: [StepState; STEP_COUNT],
    phase: Phase,
    /// Phase to return to after `Resume`.
    paused_from: Option<Phase>,
    round: u8,
    /// The review whose findings are being picked or fixed (findings gate, Sửa lỗi, after-fix
    /// gate). Saved tasks from before may not have it; see [`Workflow::gate_review`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fix_for: Option<StepKind>,
    /// Optional steps this task runs; the other optional steps are skipped.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    uses: Vec<StepKind>,
    /// Order of `states` on disk ([`LAYOUT`]).
    layout: u8,
}

/// Order of the saved `states`: 2 = Test trình duyệt (slot 10) before Review chốt (slot 11).
/// Saves without it (1) have Review chốt in slot 10 and Test trình duyệt in slot 11.
const LAYOUT: u8 = 2;

/// What a saved workflow looks like on disk: 12 states now, 8 before the 12-step flow. Older
/// saves also carry `maxRounds` (the cap on cross-review rounds is gone; serde ignores it).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedWorkflow {
    states: Vec<StepState>,
    phase: Phase,
    paused_from: Option<Phase>,
    round: u8,
    #[serde(default)]
    fix_for: Option<StepKind>,
    #[serde(default)]
    uses: Vec<StepKind>,
    #[serde(default)]
    layout: u8,
}

impl TryFrom<SavedWorkflow> for Workflow {
    type Error = String;

    fn try_from(s: SavedWorkflow) -> Result<Self, Self::Error> {
        let legacy = s.states.len() == LEGACY_STEP_COUNT;
        let mut states = if legacy {
            upgrade_states(&s.states, s.round)
        } else {
            s.states
                .try_into()
                .map_err(|v: Vec<StepState>| format!("{} step states, expected {STEP_COUNT}", v.len()))?
        };
        // saved (or upgraded) with Review chốt before Test trình duyệt
        if s.layout < LAYOUT {
            states.swap(9, 10);
        }
        // before the 12-step flow one "fix" served every review: after Codex had reviewed (round
        // > 0) it was the fix that is now step 9
        let phase = |p: Phase| match p {
            Phase::Running(StepKind::Fix) if legacy && s.round > 0 => Phase::Running(StepKind::FixCross),
            p => p,
        };
        Ok(Workflow {
            states,
            phase: phase(s.phase),
            paused_from: s.paused_from.map(phase),
            round: s.round,
            fix_for: s.fix_for,
            uses: s.uses,
            layout: LAYOUT,
        })
    }
}

/// The 8 states of a task saved before the 12-step flow, in the first 12-step layout (Review
/// chốt in slot 10, Test trình duyệt in slot 11; the caller swaps them). Steps it already
/// went past without them (Kiểm tra, Harness, Test trình duyệt) show as skipped, the ones ahead
/// stay pending. Its one "fix" becomes step 9 once Codex had reviewed (`round` > 0); step 7 then
/// shows as skipped (the old data cannot tell whether the first review needed a fix).
fn upgrade_states(old: &[StepState], round: u8) -> [StepState; STEP_COUNT] {
    use StepState::{Pending, Skipped};
    let at = |i: usize| old.get(i).copied().unwrap_or(Pending);
    let (intake, analyze, implement, review, fix, cross, fin, push) =
        (at(0), at(1), at(2), at(3), at(4), at(5), at(6), at(7));
    let reached = |s: StepState| s != Pending;
    let past_implement = [review, fix, cross, fin, push].into_iter().any(reached);
    let optional = |past: bool| if past { Skipped } else { Pending };
    let (fix7, fix9) = if round == 0 {
        (fix, Pending)
    } else {
        (optional(reached(review)), fix)
    };
    [
        intake,
        analyze,
        implement,
        optional(past_implement),
        optional(past_implement),
        review,
        fix7,
        cross,
        fix9,
        fin,
        optional(reached(push)),
        push,
    ]
}

impl Default for Workflow {
    fn default() -> Self {
        Self::new()
    }
}

impl Workflow {
    pub fn new() -> Self {
        Self {
            states: [StepState::Pending; STEP_COUNT],
            phase: Phase::NotStarted,
            paused_from: None,
            round: 0,
            fix_for: None,
            uses: Vec::new(),
            layout: LAYOUT,
        }
    }

    /// Whether the task runs this optional step (Kiểm tra, Harness, Test trình duyệt).
    pub fn uses(&self, step: StepKind) -> bool {
        self.uses.contains(&step)
    }

    /// Turn an optional step on or off for this task; it takes effect when the flow gets there.
    pub fn set_uses(&mut self, step: StepKind, on: bool) {
        self.uses.retain(|s| *s != step);
        if on && step.is_optional() {
            self.uses.push(step);
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn state_of(&self, step: StepKind) -> StepState {
        self.states[step.slot()]
    }

    /// Current cross-review round (1-based once review started, 0 before).
    pub fn round(&self) -> u8 {
        self.round
    }

    /// The step that failed, while waiting at the error gate.
    pub fn failed_step(&self) -> Option<StepKind> {
        match self.phase {
            Phase::Gate(GateKind::Error) => self.current_step(),
            _ => None,
        }
    }

    pub fn current_step(&self) -> Option<StepKind> {
        match self.phase {
            Phase::Running(step) => Some(step),
            Phase::Gate(GateKind::PlanReview) => Some(StepKind::Analyze),
            Phase::Gate(GateKind::Findings) => self.gate_review(),
            Phase::Gate(GateKind::AfterFix) => self.gate_review().map(fix_step_of),
            Phase::Gate(GateKind::SelfCode) => Some(StepKind::Implement),
            Phase::Gate(GateKind::Error) => StepKind::ALL
                .into_iter()
                .find(|s| self.state_of(*s) == StepState::Failed),
            Phase::Gate(GateKind::AgentQuestion) => self.asking(),
            _ => None,
        }
    }

    /// The step whose AI waits for the user's answer at the question gate.
    pub fn asking(&self) -> Option<StepKind> {
        match self.phase {
            Phase::Gate(GateKind::AgentQuestion) => StepKind::ALL
                .into_iter()
                .find(|s| self.state_of(*s) == StepState::WaitingUser),
            _ => None,
        }
    }

    /// The review whose findings the findings gate or the after-fix gate is about. A findings
    /// gate saved before every review had one was the final review's medium triage.
    pub fn gate_review(&self) -> Option<StepKind> {
        match self.phase {
            Phase::Gate(GateKind::Findings | GateKind::AfterFix) => {
                Some(self.fix_for.unwrap_or(StepKind::FinalReview))
            }
            _ => None,
        }
    }

    /// The step whose findings are being picked, fixed or checked again (`None` outside that
    /// loop). While Harness runs it is `Some(Harness)` right after a fix of what it found.
    pub fn fixing(&self) -> Option<StepKind> {
        self.fix_for
    }

    /// Where the gate's "go on" answer leads: past the findings gate without fixing, or "No" at
    /// the after-fix gate (the browser test or the final review again when it was the one
    /// fixed; otherwise the step after the cross review).
    pub fn skip_target(&self) -> Option<StepKind> {
        let review = self.gate_review()?;
        match self.phase {
            Phase::Gate(GateKind::Findings) => Some(self.after_review_target(review)),
            Phase::Gate(GateKind::AfterFix)
                if matches!(review, StepKind::BrowserTest | StepKind::FinalReview) =>
            {
                Some(review)
            }
            Phase::Gate(GateKind::AfterFix) => Some(self.after_review_target(StepKind::CrossReview)),
            _ => None,
        }
    }

    /// The after-fix gate may offer Test trình duyệt again: the final review's findings were
    /// fixed and the task uses the browser test.
    pub fn can_retest(&self) -> bool {
        self.phase == Phase::Gate(GateKind::AfterFix)
            && self.gate_review() == Some(StepKind::FinalReview)
            && self.uses(StepKind::BrowserTest)
    }

    pub fn apply(&mut self, event: WorkflowEvent) -> DomainResult<()> {
        use WorkflowEvent as E;
        match (self.phase, event) {
            (Phase::NotStarted, E::Start) => self.run(StepKind::Intake),

            (Phase::Running(cur), E::StepSucceeded(step)) => {
                self.ensure_current(cur, step)?;
                self.on_step_succeeded(step)
            }
            (Phase::Running(cur), E::StepSkipped(step))
                if step.is_optional() || step == StepKind::FinalReview =>
            {
                self.ensure_current(cur, step)?;
                self.set(step, StepState::Skipped);
                match step.next() {
                    Some(next) => self.run(next),
                    None => Ok(()),
                }
            }
            (Phase::Running(cur), E::StepFailed(step)) => {
                self.ensure_current(cur, step)?;
                self.set(step, StepState::Failed);
                self.phase = Phase::Gate(GateKind::Error);
                Ok(())
            }
            (Phase::Gate(GateKind::Error), E::Retry(step)) => {
                if self.state_of(step) != StepState::Failed {
                    return Err(DomainError::NotCurrentStep(step));
                }
                self.run(step)
            }
            (Phase::Running(cur), E::AgentAsked(step)) => {
                self.ensure_current(cur, step)?;
                self.set(step, StepState::WaitingUser);
                self.phase = Phase::Gate(GateKind::AgentQuestion);
                Ok(())
            }
            (Phase::Gate(GateKind::AgentQuestion), E::AgentAnswered) => {
                let step = self
                    .asking()
                    .ok_or(DomainError::NotCurrentStep(StepKind::Intake))?;
                self.run(step)
            }

            (Phase::Gate(GateKind::PlanReview), E::PlanApproved) => {
                self.set(StepKind::Analyze, StepState::Done);
                self.run(StepKind::Implement)
            }
            (Phase::Gate(GateKind::PlanReview), E::PlanRejected) => self.run(StepKind::Analyze),
            (Phase::Gate(GateKind::PlanReview), E::SelfCode) => {
                self.set(StepKind::Analyze, StepState::Done);
                self.set(StepKind::Implement, StepState::WaitingUser);
                self.phase = Phase::Gate(GateKind::SelfCode);
                Ok(())
            }
            (Phase::Gate(GateKind::SelfCode), E::SelfCodeDone) => {
                self.set(StepKind::Implement, StepState::Done);
                self.run(StepKind::Checks)
            }

            (Phase::Running(review), E::ReviewFinished { open }) if review.reports_findings() => {
                if open == 0 {
                    self.set(review, StepState::Done);
                    self.after_review(review)
                } else {
                    self.findings_gate(review);
                    Ok(())
                }
            }
            (Phase::Gate(GateKind::Findings), E::FindingsPicked { fix }) => {
                let review = self.fix_for.unwrap_or(StepKind::FinalReview);
                self.set(review, StepState::Done);
                if fix > 0 {
                    self.fix_for = Some(review);
                    self.run(fix_step_of(review))
                } else {
                    self.after_review(review)
                }
            }
            (Phase::Gate(GateKind::AfterFix), E::AfterFix(answer)) => {
                let review = self.fix_for.unwrap_or(StepKind::CrossReview);
                self.set(fix_step_of(review), StepState::Done);
                match answer {
                    AfterFix::CrossReview => {
                        self.fix_for = None;
                        self.start_cross_review()
                    }
                    AfterFix::PickMore => {
                        self.findings_gate(review);
                        Ok(())
                    }
                    // the step that found the problems runs again on the fixed code
                    AfterFix::Next if review == StepKind::BrowserTest => self.run(review),
                    AfterFix::Next if review == StepKind::FinalReview => {
                        self.fix_for = None;
                        self.run(review)
                    }
                    AfterFix::Next => {
                        self.fix_for = None;
                        // the cross review that was not wanted is "bỏ qua", not "chưa tới"
                        self.pass(StepKind::CrossReview);
                        self.pass(StepKind::FixCross);
                        self.run(StepKind::BrowserTest)
                    }
                    // the fixed code in the browser again; Review chốt follows it
                    AfterFix::BrowserTest if self.can_retest() => {
                        self.fix_for = None;
                        self.run(StepKind::BrowserTest)
                    }
                    AfterFix::BrowserTest => Err(DomainError::NotCurrentStep(StepKind::BrowserTest)),
                }
            }

            (Phase::AwaitingPush, E::RedoPushSummary) => self.run(StepKind::PushSummary),

            // `Running` here also means "ready to run": the caller checks no step process is
            // running. Only a step the task already reached can be run again.
            (phase, E::Reopen(step)) if phase != Phase::NotStarted => {
                // a Review chốt passed because the code had not changed can still be run by hand
                let skipped = self.state_of(step) == StepState::Skipped && step != StepKind::FinalReview;
                if step == StepKind::Intake || self.state_of(step) == StepState::Pending || skipped {
                    return Err(DomainError::NotCurrentStep(step));
                }
                for later in StepKind::ALL.into_iter().filter(|s| s.index() >= step.index()) {
                    self.set(later, StepState::Pending);
                }
                self.paused_from = None;
                // a Sửa lỗi run again fixes what its review found; Review chốt is pending again
                // now, so step 9 answers the cross review
                self.fix_for = match step {
                    StepKind::Fix => Some(StepKind::ReviewLocal),
                    StepKind::FixCross => Some(StepKind::CrossReview),
                    _ => None,
                };
                // review rounds count again from a step before the cross review; the cross
                // review itself starts a fresh first round
                if step.index() < StepKind::CrossReview.index() {
                    self.round = 0;
                } else if step == StepKind::CrossReview {
                    self.round = 1;
                }
                self.run(step)
            }

            (phase @ (Phase::Running(_) | Phase::Gate(_)), E::Pause) => {
                self.paused_from = Some(phase);
                self.phase = Phase::Paused;
                Ok(())
            }
            (Phase::Paused, E::Resume) => {
                self.phase = self.paused_from.take().unwrap_or(Phase::NotStarted);
                Ok(())
            }

            (phase, event) => Err(DomainError::InvalidTransition {
                state: format!("{phase:?}"),
                event: format!("{event:?}"),
            }),
        }
    }

    fn on_step_succeeded(&mut self, step: StepKind) -> DomainResult<()> {
        match step {
            StepKind::Intake => {
                self.set(step, StepState::Done);
                self.run(StepKind::Analyze)
            }
            StepKind::Analyze => {
                self.set(step, StepState::WaitingUser);
                self.phase = Phase::Gate(GateKind::PlanReview);
                Ok(())
            }
            StepKind::Implement | StepKind::Checks | StepKind::Harness | StepKind::BrowserTest => {
                self.set(step, StepState::Done);
                match step.next() {
                    Some(next) => self.run(next),
                    None => Ok(()),
                }
            }
            // Every fix waits for the user: review it again, or go on. A fix saved before the
            // findings gate existed answers Tự review (step 7) or Codex (step 9). A fix of what
            // the harness found runs the harness again (no review has looked at the code yet).
            StepKind::Fix | StepKind::FixCross => {
                let review = self.fix_for.unwrap_or(if step == StepKind::Fix {
                    StepKind::ReviewLocal
                } else {
                    StepKind::CrossReview
                });
                self.fix_for = Some(review);
                if review == StepKind::Harness {
                    self.set(step, StepState::Done);
                    return self.run(StepKind::Harness);
                }
                self.set(step, StepState::WaitingUser);
                self.phase = Phase::Gate(GateKind::AfterFix);
                Ok(())
            }
            StepKind::PushSummary => {
                self.set(step, StepState::Done);
                self.phase = Phase::AwaitingPush;
                Ok(())
            }
            // Reviews finish with their own event because the outcome matters.
            StepKind::ReviewLocal | StepKind::CrossReview | StepKind::FinalReview => {
                Err(DomainError::InvalidTransition {
                    state: format!("{:?}", self.phase),
                    event: format!("StepSucceeded({step:?}); use ReviewFinished"),
                })
            }
        }
    }

    /// Stop so the user picks which findings of `review` the AI fixes.
    fn findings_gate(&mut self, review: StepKind) {
        self.fix_for = Some(review);
        self.set(review, StepState::WaitingUser);
        self.phase = Phase::Gate(GateKind::Findings);
    }

    /// The step after a review (or harness, browser test) that leaves nothing to fix.
    fn after_review_target(&self, review: StepKind) -> StepKind {
        match review {
            StepKind::Harness => StepKind::ReviewLocal,
            StepKind::ReviewLocal => StepKind::CrossReview,
            StepKind::CrossReview if self.uses(StepKind::BrowserTest) => StepKind::BrowserTest,
            StepKind::CrossReview | StepKind::BrowserTest => StepKind::FinalReview,
            _ if self.browser_test_left() => StepKind::BrowserTest,
            _ => StepKind::PushSummary,
        }
    }

    /// A task saved in the old order may finish Review chốt with Test trình duyệt still ahead.
    fn browser_test_left(&self) -> bool {
        self.uses(StepKind::BrowserTest) && self.state_of(StepKind::BrowserTest) == StepState::Pending
    }

    /// Nothing (more) to fix after `review`: on to the next step of the flow.
    fn after_review(&mut self, review: StepKind) -> DomainResult<()> {
        self.fix_for = None;
        match review {
            // Sửa lỗi (step 7) may still be needed by Tự review
            StepKind::Harness => self.run(StepKind::ReviewLocal),
            StepKind::ReviewLocal => {
                self.pass(StepKind::Fix);
                self.start_cross_review()
            }
            // Test trình duyệt (skipped when the task does not use it), then Review chốt
            StepKind::CrossReview => {
                self.pass(StepKind::FixCross);
                self.run(StepKind::BrowserTest)
            }
            StepKind::BrowserTest => {
                self.pass(StepKind::FixCross);
                self.run(StepKind::FinalReview)
            }
            _ => {
                self.pass(StepKind::FixCross);
                let next = self.after_review_target(review);
                self.run(next)
            }
        }
    }

    /// The flow went past a step with nothing to do there: it shows "bỏ qua", not "chưa tới".
    fn pass(&mut self, step: StepKind) {
        if self.state_of(step) == StepState::Pending {
            self.set(step, StepState::Skipped);
        }
    }

    fn start_cross_review(&mut self) -> DomainResult<()> {
        self.round = self.round.saturating_add(1);
        self.run(StepKind::CrossReview)
    }

    fn ensure_current(&self, current: StepKind, step: StepKind) -> DomainResult<()> {
        if current == step {
            Ok(())
        } else {
            Err(DomainError::NotCurrentStep(step))
        }
    }

    /// Move to `step`; an optional step the task does not use is skipped on the way.
    fn run(&mut self, step: StepKind) -> DomainResult<()> {
        if step.is_optional() && !self.uses(step) {
            self.set(step, StepState::Skipped);
            return match step.next() {
                Some(next) => self.run(next),
                None => Ok(()),
            };
        }
        self.set(step, StepState::Running);
        self.phase = Phase::Running(step);
        Ok(())
    }

    fn set(&mut self, step: StepKind, state: StepState) {
        self.states[step.slot()] = state;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use WorkflowEvent as E;

    fn implemented() -> Workflow {
        let mut wf = Workflow::default();
        wf.apply(E::Start).unwrap();
        wf.apply(E::StepSucceeded(StepKind::Intake)).unwrap();
        wf.apply(E::StepSucceeded(StepKind::Analyze)).unwrap();
        wf.apply(E::PlanApproved).unwrap();
        wf.apply(E::StepSucceeded(StepKind::Implement)).unwrap();
        wf
    }

    /// Implemented, first `/review-local` clean, now in Codex round 1.
    fn started_to_review() -> Workflow {
        let mut wf = implemented();
        wf.apply(E::ReviewFinished { open: 0 }).unwrap();
        wf
    }

    const CLEAN: E = E::ReviewFinished { open: 0 };

    #[test]
    fn steps_are_numbered_in_order() {
        let indexes: Vec<u8> = StepKind::ALL.iter().map(|s| s.index()).collect();
        assert_eq!(indexes, (1..=12).collect::<Vec<u8>>());
        // Test trình duyệt (10) before Review chốt (11)
        assert_eq!(StepKind::FixCross.next(), Some(StepKind::BrowserTest));
        assert_eq!(StepKind::BrowserTest.next(), Some(StepKind::FinalReview));
        assert_eq!(StepKind::FinalReview.next(), Some(StepKind::PushSummary));
        assert_eq!(StepKind::PushSummary.next(), None);
        assert_eq!(fix_step_of(StepKind::ReviewLocal), StepKind::Fix);
        assert_eq!(fix_step_of(StepKind::FinalReview), StepKind::FixCross);
    }

    #[test]
    fn happy_path_ends_waiting_for_user_push() {
        let mut wf = implemented();
        assert_eq!(wf.phase(), Phase::Running(StepKind::ReviewLocal));
        wf.apply(CLEAN).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::CrossReview));
        assert_eq!(wf.round(), 1);
        wf.apply(CLEAN).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::FinalReview));
        wf.apply(CLEAN).unwrap();
        // Test trình duyệt is not used by this task: skipped on the way to the push summary
        assert_eq!(wf.phase(), Phase::Running(StepKind::PushSummary));
        wf.apply(E::StepSucceeded(StepKind::PushSummary)).unwrap();
        assert_eq!(wf.phase(), Phase::AwaitingPush);
        let mut again = wf.clone();
        again.apply(E::RedoPushSummary).unwrap();
        assert_eq!(again.phase(), Phase::Running(StepKind::PushSummary));
        // nothing to fix and the optional steps not used: all of them "bỏ qua"
        for s in StepKind::ALL {
            let expected = if s.is_optional() || s.is_fix() {
                StepState::Skipped
            } else {
                StepState::Done
            };
            assert_eq!(wf.state_of(s), expected, "{s:?}");
        }
        // a review does not succeed without saying what it found
        assert!(
            implemented()
                .apply(E::StepSucceeded(StepKind::ReviewLocal))
                .is_err()
        );
    }

    /// Every review that reports something waits for the user to pick what gets fixed.
    #[test]
    fn every_review_with_findings_stops_for_the_user_to_pick() {
        let mut wf = implemented();
        wf.apply(E::ReviewFinished { open: 2 }).unwrap();
        assert_eq!(wf.phase(), Phase::Gate(GateKind::Findings));
        assert_eq!(wf.current_step(), Some(StepKind::ReviewLocal));
        assert_eq!(wf.gate_review(), Some(StepKind::ReviewLocal));
        assert_eq!(wf.state_of(StepKind::ReviewLocal), StepState::WaitingUser);
        assert_eq!(wf.skip_target(), Some(StepKind::CrossReview));
        // nothing picked: on to Codex, step 7 "bỏ qua"
        let mut skip = wf.clone();
        skip.apply(E::FindingsPicked { fix: 0 }).unwrap();
        assert_eq!(skip.phase(), Phase::Running(StepKind::CrossReview));
        assert_eq!(skip.state_of(StepKind::Fix), StepState::Skipped);
        assert_eq!(skip.state_of(StepKind::ReviewLocal), StepState::Done);

        // Codex reports too: its gate, then Test trình duyệt when the task uses it, else the
        // final review
        skip.apply(E::ReviewFinished { open: 1 }).unwrap();
        assert_eq!(skip.gate_review(), Some(StepKind::CrossReview));
        assert_eq!(skip.skip_target(), Some(StepKind::FinalReview));
        let mut browser = skip.clone();
        browser.set_uses(StepKind::BrowserTest, true);
        assert_eq!(browser.skip_target(), Some(StepKind::BrowserTest));
        browser.apply(E::FindingsPicked { fix: 0 }).unwrap();
        assert_eq!(browser.phase(), Phase::Running(StepKind::BrowserTest));
        skip.apply(E::FindingsPicked { fix: 0 }).unwrap();
        assert_eq!(skip.phase(), Phase::Running(StepKind::FinalReview));
        assert_eq!(skip.state_of(StepKind::BrowserTest), StepState::Skipped);
        // the final review no longer fixes critical/high by itself
        skip.apply(E::ReviewFinished { open: 3 }).unwrap();
        assert_eq!(skip.phase(), Phase::Gate(GateKind::Findings));
        assert_eq!(skip.current_step(), Some(StepKind::FinalReview));
        assert_eq!(skip.skip_target(), Some(StepKind::PushSummary));
        skip.apply(E::FindingsPicked { fix: 0 }).unwrap();
        assert_eq!(skip.phase(), Phase::Running(StepKind::PushSummary));
    }

    /// After every fix the user is asked whether the other AI reviews again.
    #[test]
    fn every_fix_asks_whether_to_cross_review_again() {
        let mut wf = implemented();
        wf.apply(E::ReviewFinished { open: 2 }).unwrap();
        wf.apply(E::FindingsPicked { fix: 1 }).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::Fix));
        assert_eq!(wf.round(), 0);
        wf.apply(E::StepSucceeded(StepKind::Fix)).unwrap();
        assert_eq!(wf.phase(), Phase::Gate(GateKind::AfterFix));
        assert_eq!(wf.current_step(), Some(StepKind::Fix));
        assert_eq!(wf.state_of(StepKind::Fix), StepState::WaitingUser);
        assert_eq!(wf.skip_target(), Some(StepKind::FinalReview));

        // pick more of Tự review's findings, fix them, and get asked again
        let mut more = wf.clone();
        more.apply(E::AfterFix(AfterFix::PickMore)).unwrap();
        assert_eq!(more.phase(), Phase::Gate(GateKind::Findings));
        assert_eq!(more.gate_review(), Some(StepKind::ReviewLocal));
        assert_eq!(more.state_of(StepKind::Fix), StepState::Done);
        more.apply(E::FindingsPicked { fix: 1 }).unwrap();
        assert_eq!(more.phase(), Phase::Running(StepKind::Fix));

        // no cross review: Review chéo and step 9 "bỏ qua", the final review runs
        let mut no = wf.clone();
        no.apply(E::AfterFix(AfterFix::Next)).unwrap();
        assert_eq!(no.phase(), Phase::Running(StepKind::FinalReview));
        assert_eq!(no.state_of(StepKind::CrossReview), StepState::Skipped);
        assert_eq!(no.state_of(StepKind::FixCross), StepState::Skipped);
        assert_eq!(no.round(), 0);

        // yes: Codex round 1; its findings fixed in step 9; yes again: round 2, no cap
        wf.apply(E::AfterFix(AfterFix::CrossReview)).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::CrossReview));
        assert_eq!(wf.state_of(StepKind::Fix), StepState::Done);
        for round in 1..=5 {
            assert_eq!(wf.round(), round);
            wf.apply(E::ReviewFinished { open: 1 }).unwrap();
            wf.apply(E::FindingsPicked { fix: 1 }).unwrap();
            assert_eq!(wf.phase(), Phase::Running(StepKind::FixCross));
            wf.apply(E::StepSucceeded(StepKind::FixCross)).unwrap();
            assert_eq!(wf.phase(), Phase::Gate(GateKind::AfterFix));
            assert_eq!(wf.current_step(), Some(StepKind::FixCross));
            wf.apply(E::AfterFix(AfterFix::CrossReview)).unwrap();
        }
        assert_eq!(wf.round(), 6);
        wf.apply(CLEAN).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::FinalReview));
    }

    #[test]
    fn a_fix_of_the_final_review_runs_that_review_again_or_codex() {
        let mut wf = started_to_review();
        wf.apply(CLEAN).unwrap();
        wf.apply(E::ReviewFinished { open: 1 }).unwrap();
        wf.apply(E::FindingsPicked { fix: 1 }).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::FixCross));
        // survives a save while the fix runs
        let mut wf: Workflow = serde_json::from_str(&serde_json::to_string(&wf).unwrap()).unwrap();
        wf.apply(E::StepSucceeded(StepKind::FixCross)).unwrap();
        assert_eq!(wf.gate_review(), Some(StepKind::FinalReview));
        assert_eq!(wf.skip_target(), Some(StepKind::FinalReview));
        let mut again = wf.clone();
        again.apply(E::AfterFix(AfterFix::Next)).unwrap();
        assert_eq!(again.phase(), Phase::Running(StepKind::FinalReview));
        assert_eq!(again.round(), 1);
        wf.apply(E::AfterFix(AfterFix::CrossReview)).unwrap();
        assert_eq!(
            (wf.phase(), wf.round()),
            (Phase::Running(StepKind::CrossReview), 2)
        );
        wf.apply(CLEAN).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::FinalReview));
    }

    #[test]
    fn analyze_stops_at_plan_review_gate() {
        let mut wf = Workflow::default();
        wf.apply(E::Start).unwrap();
        wf.apply(E::StepSucceeded(StepKind::Intake)).unwrap();
        wf.apply(E::StepSucceeded(StepKind::Analyze)).unwrap();
        assert_eq!(wf.phase(), Phase::Gate(GateKind::PlanReview));
        assert_eq!(wf.current_step(), Some(StepKind::Analyze));
        assert_eq!(wf.gate_review(), None);
        wf.apply(E::PlanRejected).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::Analyze));
    }

    #[test]
    fn user_can_code_by_hand_then_reviews_run() {
        let mut wf = Workflow::default();
        wf.apply(E::Start).unwrap();
        wf.apply(E::StepSucceeded(StepKind::Intake)).unwrap();
        wf.apply(E::StepSucceeded(StepKind::Analyze)).unwrap();
        wf.apply(E::SelfCode).unwrap();
        assert_eq!(wf.phase(), Phase::Gate(GateKind::SelfCode));
        assert_eq!(wf.state_of(StepKind::Implement), StepState::WaitingUser);
        assert_eq!(wf.current_step(), Some(StepKind::Implement));
        assert!(wf.apply(E::SelfCodeDone).is_ok());
        assert_eq!(wf.phase(), Phase::Running(StepKind::ReviewLocal));
        assert_eq!(wf.state_of(StepKind::Implement), StepState::Done);
    }

    #[test]
    fn state_survives_a_round_trip_through_json() {
        let mut wf = started_to_review();
        wf.apply(E::ReviewFinished { open: 1 }).unwrap();
        wf.apply(E::FindingsPicked { fix: 1 }).unwrap();
        wf.apply(E::StepFailed(StepKind::FixCross)).unwrap();
        let back: Workflow = serde_json::from_str(&serde_json::to_string(&wf).unwrap()).unwrap();
        assert_eq!(back, wf);
        assert_eq!(back.failed_step(), Some(StepKind::FixCross));
    }

    #[test]
    fn an_ai_question_waits_for_the_answer_then_the_step_goes_on() {
        let mut wf = Workflow::new();
        wf.set_uses(StepKind::BrowserTest, true);
        wf.apply(E::Start).unwrap();
        // straight to the browser test for the test's sake
        wf.phase = Phase::Running(StepKind::BrowserTest);
        wf.set(StepKind::BrowserTest, StepState::Running);
        assert!(wf.apply(E::AgentAsked(StepKind::FinalReview)).is_err());
        wf.apply(E::AgentAsked(StepKind::BrowserTest)).unwrap();
        assert_eq!(wf.phase(), Phase::Gate(GateKind::AgentQuestion));
        assert_eq!(wf.asking(), Some(StepKind::BrowserTest));
        assert_eq!(wf.current_step(), Some(StepKind::BrowserTest));
        assert_eq!(wf.state_of(StepKind::BrowserTest), StepState::WaitingUser);
        // it survives a save and a pause
        let back: Workflow = serde_json::from_str(&serde_json::to_string(&wf).unwrap()).unwrap();
        assert_eq!(back.asking(), Some(StepKind::BrowserTest));
        wf.apply(E::Pause).unwrap();
        wf.apply(E::Resume).unwrap();
        assert!(wf.apply(E::Retry(StepKind::BrowserTest)).is_err());
        wf.apply(E::AgentAnswered).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::BrowserTest));
        assert_eq!(wf.state_of(StepKind::BrowserTest), StepState::Running);
        assert!(wf.apply(E::AgentAnswered).is_err());
    }

    #[test]
    fn failure_waits_for_retry_and_pause_resumes_same_phase() {
        let mut wf = Workflow::default();
        wf.apply(E::Start).unwrap();
        wf.apply(E::StepFailed(StepKind::Intake)).unwrap();
        assert_eq!(wf.phase(), Phase::Gate(GateKind::Error));
        wf.apply(E::Retry(StepKind::Intake)).unwrap();
        wf.apply(E::Pause).unwrap();
        assert_eq!(wf.phase(), Phase::Paused);
        wf.apply(E::Resume).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::Intake));
    }

    #[test]
    fn reopen_runs_again_from_a_step_and_resets_what_follows() {
        let mut wf = started_to_review();
        wf.apply(CLEAN).unwrap();
        wf.apply(CLEAN).unwrap();
        wf.apply(E::StepSucceeded(StepKind::PushSummary)).unwrap();
        assert_eq!(wf.phase(), Phase::AwaitingPush);
        assert_eq!(wf.round(), 1);

        let mut back = wf.clone();
        back.apply(E::Reopen(StepKind::Implement)).unwrap();
        assert_eq!(back.phase(), Phase::Running(StepKind::Implement));
        assert_eq!(back.state_of(StepKind::Analyze), StepState::Done);
        assert_eq!(back.state_of(StepKind::Implement), StepState::Running);
        for later in [
            StepKind::ReviewLocal,
            StepKind::CrossReview,
            StepKind::PushSummary,
        ] {
            assert_eq!(back.state_of(later), StepState::Pending, "{later:?}");
        }
        assert_eq!(back.round(), 0);
        // only steps already reached: the reviews after Viết code are pending again
        assert!(back.clone().apply(E::Reopen(StepKind::FinalReview)).is_err());
        assert!(back.clone().apply(E::Reopen(StepKind::Analyze)).is_ok());
        // a clean first review never needed Sửa lỗi
        assert!(wf.clone().apply(E::Reopen(StepKind::Fix)).is_err());

        let mut cross = wf.clone();
        cross.apply(E::Reopen(StepKind::CrossReview)).unwrap();
        assert_eq!(
            (cross.phase(), cross.round()),
            (Phase::Running(StepKind::CrossReview), 1)
        );
        let mut push = wf.clone();
        push.apply(E::Reopen(StepKind::PushSummary)).unwrap();
        assert_eq!(push.state_of(StepKind::FinalReview), StepState::Done);
        assert!(wf.clone().apply(E::Reopen(StepKind::Intake)).is_err());
        assert!(Workflow::default().apply(E::Reopen(StepKind::Analyze)).is_err());

        // step 9 run again answers Codex, then asks about another cross review
        let mut fixed = started_to_review();
        fixed.apply(E::ReviewFinished { open: 1 }).unwrap();
        fixed.apply(E::FindingsPicked { fix: 1 }).unwrap();
        fixed.apply(E::StepSucceeded(StepKind::FixCross)).unwrap();
        fixed.apply(E::AfterFix(AfterFix::Next)).unwrap();
        fixed.apply(CLEAN).unwrap();
        fixed.apply(E::Reopen(StepKind::FixCross)).unwrap();
        fixed.apply(E::StepSucceeded(StepKind::FixCross)).unwrap();
        assert_eq!(fixed.gate_review(), Some(StepKind::CrossReview));
    }

    #[test]
    fn optional_steps_are_skipped_unless_the_task_uses_them() {
        let wf = implemented();
        assert_eq!(wf.phase(), Phase::Running(StepKind::ReviewLocal));
        assert_eq!(wf.state_of(StepKind::Checks), StepState::Skipped);
        assert_eq!(wf.state_of(StepKind::Harness), StepState::Skipped);
        assert_eq!(wf.state_of(StepKind::BrowserTest), StepState::Pending);

        let mut wf = Workflow::default();
        wf.set_uses(StepKind::Harness, true);
        wf.set_uses(StepKind::Fix, true); // not optional: ignored
        wf.apply(E::Start).unwrap();
        wf.apply(E::StepSucceeded(StepKind::Intake)).unwrap();
        wf.apply(E::StepSucceeded(StepKind::Analyze)).unwrap();
        wf.apply(E::PlanApproved).unwrap();
        wf.apply(E::StepSucceeded(StepKind::Implement)).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::Harness));
        assert_eq!(wf.state_of(StepKind::Checks), StepState::Skipped);
        wf.apply(E::StepSucceeded(StepKind::Harness)).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::ReviewLocal));
        assert!(wf.uses(StepKind::Harness) && !wf.uses(StepKind::Fix));
        // only steps that ran can run again
        assert!(wf.clone().apply(E::Reopen(StepKind::Checks)).is_err());
        let mut again = wf.clone();
        again.apply(E::Reopen(StepKind::Harness)).unwrap();
        assert_eq!(again.phase(), Phase::Running(StepKind::Harness));
        // turned on but not runnable yet: passed as "bỏ qua"; only optional steps can be
        again.apply(E::StepSkipped(StepKind::Harness)).unwrap();
        assert_eq!(again.phase(), Phase::Running(StepKind::ReviewLocal));
        assert_eq!(again.state_of(StepKind::Harness), StepState::Skipped);
        assert!(again.apply(E::StepSkipped(StepKind::ReviewLocal)).is_err());
    }

    /// Review chốt is passed when the code has not changed since Tự review; the user can still
    /// run it by hand.
    #[test]
    fn final_review_can_be_passed_and_run_again_by_hand() {
        let mut wf = Workflow::default();
        wf.apply(E::Start).unwrap();
        wf.apply(E::StepSucceeded(StepKind::Intake)).unwrap();
        wf.apply(E::StepSucceeded(StepKind::Analyze)).unwrap();
        wf.apply(E::PlanApproved).unwrap();
        wf.apply(E::StepSucceeded(StepKind::Implement)).unwrap();
        wf.apply(E::ReviewFinished { open: 0 }).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::CrossReview));
        wf.apply(E::ReviewFinished { open: 0 }).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::FinalReview));
        wf.apply(E::StepSkipped(StepKind::FinalReview)).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::PushSummary));
        assert_eq!(wf.state_of(StepKind::FinalReview), StepState::Skipped);
        // other skipped steps stay closed, this one opens again
        assert!(wf.clone().apply(E::Reopen(StepKind::Checks)).is_err());
        wf.apply(E::Reopen(StepKind::FinalReview)).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::FinalReview));
        assert_eq!(wf.state_of(StepKind::PushSummary), StepState::Pending);
    }

    /// A harness that FAILs and a browser test with NG cases stop at the findings gate like a
    /// review; what they found is fixed in step 7 / step 9 and the same step runs again.
    #[test]
    fn harness_and_browser_test_failures_go_through_the_findings_gate() {
        let mut wf = Workflow::default();
        wf.set_uses(StepKind::Harness, true);
        wf.set_uses(StepKind::BrowserTest, true);
        wf.apply(E::Start).unwrap();
        wf.apply(E::StepSucceeded(StepKind::Intake)).unwrap();
        wf.apply(E::StepSucceeded(StepKind::Analyze)).unwrap();
        wf.apply(E::PlanApproved).unwrap();
        wf.apply(E::StepSucceeded(StepKind::Implement)).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::Harness));
        wf.apply(E::ReviewFinished { open: 2 }).unwrap();
        assert_eq!(wf.phase(), Phase::Gate(GateKind::Findings));
        assert_eq!(wf.current_step(), Some(StepKind::Harness));
        assert_eq!(wf.skip_target(), Some(StepKind::ReviewLocal));
        // left as they are: Tự review next, step 7 still free for it
        let mut skip = wf.clone();
        skip.apply(E::FindingsPicked { fix: 0 }).unwrap();
        assert_eq!(skip.phase(), Phase::Running(StepKind::ReviewLocal));
        assert_eq!(skip.state_of(StepKind::Fix), StepState::Pending);
        // fixed in step 7, then the harness runs again (no "Review chéo lại?")
        wf.apply(E::FindingsPicked { fix: 2 }).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::Fix));
        wf.apply(E::StepSucceeded(StepKind::Fix)).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::Harness));
        wf.apply(CLEAN).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::ReviewLocal));
        assert_eq!(wf.state_of(StepKind::Harness), StepState::Done);
        // Tự review and Review chéo pass; the browser test (before Review chốt) finds a
        // problem, fixed in step 9
        wf.apply(CLEAN).unwrap();
        wf.apply(CLEAN).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::BrowserTest));
        assert_eq!(wf.state_of(StepKind::FinalReview), StepState::Pending);
        wf.apply(E::ReviewFinished { open: 1 }).unwrap();
        assert_eq!(wf.skip_target(), Some(StepKind::FinalReview));
        wf.apply(E::FindingsPicked { fix: 1 }).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::FixCross));
        wf.apply(E::StepSucceeded(StepKind::FixCross)).unwrap();
        assert_eq!(wf.phase(), Phase::Gate(GateKind::AfterFix));
        assert_eq!(wf.skip_target(), Some(StepKind::BrowserTest));
        assert!(!wf.can_retest(), "only after a fix of Review chốt");
        // "No" runs the browser test again; clean → Review chốt reviews the fix too
        wf.apply(E::AfterFix(AfterFix::Next)).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::BrowserTest));
        wf.apply(CLEAN).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::FinalReview));
        // Review chốt finds something; after its fix the browser test can run again, and
        // Review chốt follows it
        wf.apply(E::ReviewFinished { open: 1 }).unwrap();
        wf.apply(E::FindingsPicked { fix: 1 }).unwrap();
        wf.apply(E::StepSucceeded(StepKind::FixCross)).unwrap();
        assert_eq!(wf.gate_review(), Some(StepKind::FinalReview));
        assert_eq!(wf.skip_target(), Some(StepKind::FinalReview));
        assert!(wf.can_retest());
        let mut again = wf.clone();
        again.apply(E::AfterFix(AfterFix::Next)).unwrap();
        assert_eq!(again.phase(), Phase::Running(StepKind::FinalReview));
        wf.apply(E::AfterFix(AfterFix::BrowserTest)).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::BrowserTest));
        assert_eq!(wf.state_of(StepKind::FixCross), StepState::Done);
        wf.apply(CLEAN).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::FinalReview));
        wf.apply(CLEAN).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::PushSummary));
        // a task without the browser test cannot ask for it
        let mut off = again.clone();
        off.apply(E::ReviewFinished { open: 1 }).unwrap();
        off.apply(E::FindingsPicked { fix: 1 }).unwrap();
        off.apply(E::StepSucceeded(StepKind::FixCross)).unwrap();
        off.set_uses(StepKind::BrowserTest, false);
        assert!(!off.can_retest());
        assert!(off.apply(E::AfterFix(AfterFix::BrowserTest)).is_err());
    }

    /// Tasks saved while Review chốt came before Test trình duyệt read in the new order; one that
    /// finished Review chốt with the browser test still ahead runs it, then Review chốt again.
    #[test]
    fn tasks_saved_in_the_old_order_are_read_in_the_new_one() {
        let old = |states: &str, phase: &str| {
            format!(
                r#"{{"states":[{states}],"phase":{phase},"pausedFrom":null,"round":1,"uses":["browserTest"]}}"#
            )
        };
        // old slots: … 9 fixCross, 10 finalReview (done), 11 browserTest (running), 12 push
        let mut wf: Workflow = serde_json::from_str(&old(
            r#""done","done","done","skipped","skipped","done","skipped","done","skipped","done","running","pending""#,
            r#"{"running":"browserTest"}"#,
        ))
        .unwrap();
        assert_eq!(wf.state_of(StepKind::FinalReview), StepState::Done);
        assert_eq!(wf.state_of(StepKind::BrowserTest), StepState::Running);
        wf.apply(CLEAN).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::FinalReview));
        // written back in the new layout, read back unchanged
        let json = serde_json::to_string(&wf).unwrap();
        assert!(json.contains(r#""layout":2"#), "{json}");
        let back: Workflow = serde_json::from_str(&json).unwrap();
        assert_eq!(back, wf);
        // old order, at Review chốt with the browser test not run yet: it runs after
        let mut fin: Workflow = serde_json::from_str(&old(
            r#""done","done","done","skipped","skipped","done","skipped","done","skipped","running","pending","pending""#,
            r#"{"running":"finalReview"}"#,
        ))
        .unwrap();
        assert_eq!(fin.state_of(StepKind::BrowserTest), StepState::Pending);
        fin.apply(CLEAN).unwrap();
        assert_eq!(fin.phase(), Phase::Running(StepKind::BrowserTest));
        fin.apply(CLEAN).unwrap();
        assert_eq!(fin.phase(), Phase::Running(StepKind::FinalReview));
        fin.apply(CLEAN).unwrap();
        assert_eq!(fin.phase(), Phase::Running(StepKind::PushSummary));
    }

    fn legacy(states: &str, phase: &str, round: u8) -> String {
        format!(r#"{{"states":[{states}],"phase":{phase},"pausedFrom":null,"round":{round},"maxRounds":3}}"#)
    }

    #[test]
    fn tasks_saved_with_eight_steps_are_upgraded() {
        // waiting for the push: the steps it went past without are skipped
        let push: Workflow = serde_json::from_str(&legacy(
            r#""done","done","done","done","pending","done","done","done""#,
            r#""awaitingPush""#,
            1,
        ))
        .unwrap();
        assert_eq!(push.phase(), Phase::AwaitingPush);
        for (s, want) in [
            (StepKind::Implement, StepState::Done),
            (StepKind::Checks, StepState::Skipped),
            (StepKind::Harness, StepState::Skipped),
            (StepKind::ReviewLocal, StepState::Done),
            (StepKind::Fix, StepState::Skipped),
            (StepKind::CrossReview, StepState::Done),
            (StepKind::FixCross, StepState::Pending),
            (StepKind::FinalReview, StepState::Done),
            (StepKind::BrowserTest, StepState::Skipped),
            (StepKind::PushSummary, StepState::Done),
        ] {
            assert_eq!(push.state_of(s), want, "{s:?}");
        }
        // fixing Codex's findings: that fix is step 9 now, and it asks about Codex afterwards
        let mut fixing: Workflow = serde_json::from_str(&legacy(
            r#""done","done","done","done","running","done","pending","pending""#,
            r#"{"running":"fix"}"#,
            2,
        ))
        .unwrap();
        assert_eq!(fixing.phase(), Phase::Running(StepKind::FixCross));
        assert_eq!(fixing.state_of(StepKind::FixCross), StepState::Running);
        assert_eq!(fixing.state_of(StepKind::BrowserTest), StepState::Pending);
        fixing.apply(E::StepSucceeded(StepKind::FixCross)).unwrap();
        assert_eq!(fixing.gate_review(), Some(StepKind::CrossReview));
        // the first review's fix (no Codex yet) stays step 7; Kiểm tra / Harness are behind it
        let mut first: Workflow = serde_json::from_str(&legacy(
            r#""done","done","done","done","running","pending","pending","pending""#,
            r#"{"running":"fix"}"#,
            0,
        ))
        .unwrap();
        assert_eq!(first.phase(), Phase::Running(StepKind::Fix));
        assert_eq!(first.state_of(StepKind::Checks), StepState::Skipped);
        first.apply(E::StepSucceeded(StepKind::Fix)).unwrap();
        assert_eq!(first.gate_review(), Some(StepKind::ReviewLocal));
        // still at the plan gate: nothing to skip yet
        let plan: Workflow = serde_json::from_str(&legacy(
            r#""done","waitingUser","pending","pending","pending","pending","pending","pending""#,
            r#"{"gate":"planReview"}"#,
            0,
        ))
        .unwrap();
        assert_eq!(plan.state_of(StepKind::Checks), StepState::Pending);
        // the old medium triage of the final review is the findings gate now
        let mut triage: Workflow = serde_json::from_str(&legacy(
            r#""done","done","done","done","pending","done","waitingUser","pending""#,
            r#"{"gate":"mediumTriage"}"#,
            1,
        ))
        .unwrap();
        assert_eq!(triage.phase(), Phase::Gate(GateKind::Findings));
        assert_eq!(triage.current_step(), Some(StepKind::FinalReview));
        triage.apply(E::FindingsPicked { fix: 0 }).unwrap();
        assert_eq!(triage.phase(), Phase::Running(StepKind::PushSummary));
        // saved again, it is a 12-step workflow
        let json = serde_json::to_string(&push).unwrap();
        assert!(!json.contains("maxRounds"));
        assert_eq!(serde_json::from_str::<Workflow>(&json).unwrap(), push);
        assert!(serde_json::from_str::<Workflow>(&legacy(r#""done""#, r#""notStarted""#, 0)).is_err());
    }

    #[test]
    fn invalid_events_are_rejected() {
        let mut wf = Workflow::default();
        assert!(wf.apply(E::PlanApproved).is_err());
        wf.apply(E::Start).unwrap();
        assert_eq!(
            wf.apply(E::StepSucceeded(StepKind::Analyze)),
            Err(DomainError::NotCurrentStep(StepKind::Analyze))
        );
        assert!(wf.apply(E::Start).is_err());
        // findings answers only at their gates
        assert!(wf.apply(E::FindingsPicked { fix: 1 }).is_err());
        assert!(wf.apply(E::AfterFix(AfterFix::CrossReview)).is_err());
        assert!(wf.apply(E::ReviewFinished { open: 0 }).is_err());
    }
}
