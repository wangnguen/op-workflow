//! The twelve-step task workflow as a pure state machine (order chosen by the user, PLAN.md §12
//! "Luồng mới 12 bước").
//!
//! ```text
//! intake -> analyze -> <plan review gate> -> implement -> checks -> harness
//!   -> reviewLocal (first pass) -> fix if it found anything
//!   -> crossReview (Codex) <-> fixCross (max rounds)
//!   -> finalReview (/review-local again) -> <critical/high: fixCross, then finalReview again
//!                                           | medium: triage gate> -> browserTest -> pushSummary
//! ```
//! Checks, harness and browserTest are optional: a task that does not use one marks it skipped
//! and moves on (nothing turns them on yet; later parts do).
//! At the plan gate the user can also code by hand (`SelfCode`): the workflow waits until they
//! say they are done, then reviews their change like an AI implementation.
//! The machine never commits or pushes; after `pushSummary` the task waits for the user.

use serde::{Deserialize, Serialize};

use crate::error::{DomainError, DomainResult};
use crate::view::GateKind;

/// Default number of cross-review/fix rounds before the workflow moves on to the final
/// `/review-local`.
pub const DEFAULT_MAX_REVIEW_ROUNDS: u8 = 3;

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
    /// Fixes what the cross review or the final review found.
    FixCross,
    /// `/review-local` again once Codex is clean; its result goes through gate G2.
    FinalReview,
    /// Chrome DevTools MCP test of the app. Optional.
    BrowserTest,
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
        StepKind::FinalReview,
        StepKind::BrowserTest,
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
            StepKind::FinalReview => 10,
            StepKind::BrowserTest => 11,
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

    /// The step after this one; `None` after the push summary.
    pub fn next(self) -> Option<StepKind> {
        StepKind::ALL.get(self.slot() + 1).copied()
    }

    fn slot(self) -> usize {
        usize::from(self.index() - 1)
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowEvent {
    Start,
    StepSucceeded(StepKind),
    StepFailed(StepKind),
    Retry(StepKind),
    PlanApproved,
    /// User gave feedback on the plan; analyze runs again.
    PlanRejected,
    /// At the plan gate: the user codes this one by hand.
    SelfCode,
    /// The user finished coding by hand; review it.
    SelfCodeDone,
    /// First `/review-local` finished. `blocking` = findings to fix before Codex reviews.
    ReviewLocalFinished {
        blocking: usize,
    },
    /// Cross-review finished. `blocking` = findings the user (or gate policy) wants fixed.
    CrossReviewFinished {
        blocking: usize,
    },
    /// Final `/review-local` finished with these counts of still-open findings.
    FinalReviewFinished {
        critical_or_high: usize,
        medium_undecided: usize,
    },
    /// Every medium finding of the final review is decided; `fix` of them go back to a fix.
    MediumTriaged {
        fix: usize,
    },
    /// Write the push summary again (another issue number, or the user changed the code).
    RedoPushSummary,
    Pause,
    Resume,
    /// "Làm tiếp" on a finished (or stopped) task: run again from this step, one the task already
    /// reached. Earlier steps keep their state, this one and the later ones start over. The
    /// caller refuses it while a step process runs.
    Reopen(StepKind),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", try_from = "SavedWorkflow")]
pub struct Workflow {
    states: [StepState; STEP_COUNT],
    phase: Phase,
    /// Phase to return to after `Resume`.
    paused_from: Option<Phase>,
    round: u8,
    max_rounds: u8,
    /// The review a running `FixCross` answers: after a final-review fix the final review runs
    /// again; otherwise Codex reviews the fix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fix_for: Option<StepKind>,
    /// Optional steps this task runs; the other optional steps are skipped.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    uses: Vec<StepKind>,
}

/// What a saved workflow looks like on disk: 12 states now, 8 before the 12-step flow.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedWorkflow {
    states: Vec<StepState>,
    phase: Phase,
    paused_from: Option<Phase>,
    round: u8,
    max_rounds: u8,
    #[serde(default)]
    fix_for: Option<StepKind>,
    #[serde(default)]
    uses: Vec<StepKind>,
}

impl TryFrom<SavedWorkflow> for Workflow {
    type Error = String;

    fn try_from(s: SavedWorkflow) -> Result<Self, Self::Error> {
        let legacy = s.states.len() == LEGACY_STEP_COUNT;
        let states = if legacy {
            upgrade_states(&s.states, s.round)
        } else {
            s.states
                .try_into()
                .map_err(|v: Vec<StepState>| format!("{} step states, expected {STEP_COUNT}", v.len()))?
        };
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
            max_rounds: s.max_rounds.max(1),
            fix_for: s.fix_for,
            uses: s.uses,
        })
    }
}

/// The 8 states of a task saved before the 12-step flow, in the 12-step layout. Steps it already
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
        Self::new(DEFAULT_MAX_REVIEW_ROUNDS)
    }
}

impl Workflow {
    pub fn new(max_rounds: u8) -> Self {
        Self {
            states: [StepState::Pending; STEP_COUNT],
            phase: Phase::NotStarted,
            paused_from: None,
            round: 0,
            max_rounds: max_rounds.max(1),
            fix_for: None,
            uses: Vec::new(),
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

    pub fn max_rounds(&self) -> u8 {
        self.max_rounds
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
            Phase::Gate(GateKind::MediumTriage) => Some(StepKind::FinalReview),
            Phase::Gate(GateKind::SelfCode) => Some(StepKind::Implement),
            Phase::Gate(GateKind::Error) => StepKind::ALL
                .into_iter()
                .find(|s| self.state_of(*s) == StepState::Failed),
            _ => None,
        }
    }

    pub fn apply(&mut self, event: WorkflowEvent) -> DomainResult<()> {
        use WorkflowEvent as E;
        match (self.phase, event) {
            (Phase::NotStarted, E::Start) => self.run(StepKind::Intake),

            (Phase::Running(cur), E::StepSucceeded(step)) => {
                self.ensure_current(cur, step)?;
                self.on_step_succeeded(step)
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

            (Phase::Running(StepKind::ReviewLocal), E::ReviewLocalFinished { blocking }) => {
                self.set(StepKind::ReviewLocal, StepState::Done);
                if blocking > 0 {
                    self.run(StepKind::Fix)
                } else {
                    self.pass(StepKind::Fix);
                    self.start_cross_review()
                }
            }

            (Phase::Running(StepKind::CrossReview), E::CrossReviewFinished { blocking }) => {
                self.set(StepKind::CrossReview, StepState::Done);
                if blocking > 0 && self.round < self.max_rounds {
                    self.fix_for = Some(StepKind::CrossReview);
                    self.run(StepKind::FixCross)
                } else {
                    self.pass(StepKind::FixCross);
                    self.run(StepKind::FinalReview)
                }
            }

            (
                Phase::Running(StepKind::FinalReview),
                E::FinalReviewFinished {
                    critical_or_high,
                    medium_undecided,
                },
            ) => {
                if critical_or_high > 0 {
                    self.set(StepKind::FinalReview, StepState::Done);
                    self.fix_final_review()
                } else if medium_undecided > 0 {
                    self.set(StepKind::FinalReview, StepState::WaitingUser);
                    self.phase = Phase::Gate(GateKind::MediumTriage);
                    Ok(())
                } else {
                    self.set(StepKind::FinalReview, StepState::Done);
                    self.run(StepKind::BrowserTest)
                }
            }
            (Phase::Gate(GateKind::MediumTriage), E::MediumTriaged { fix }) => {
                self.set(StepKind::FinalReview, StepState::Done);
                if fix > 0 {
                    self.fix_final_review()
                } else {
                    self.run(StepKind::BrowserTest)
                }
            }

            (Phase::AwaitingPush, E::RedoPushSummary) => self.run(StepKind::PushSummary),

            // `Running` here also means "ready to run": the caller checks no step process is
            // running. Only a step the task already reached can be run again.
            (phase, E::Reopen(step)) if phase != Phase::NotStarted => {
                if step == StepKind::Intake
                    || matches!(self.state_of(step), StepState::Pending | StepState::Skipped)
                {
                    return Err(DomainError::NotCurrentStep(step));
                }
                for later in StepKind::ALL.into_iter().filter(|s| s.index() >= step.index()) {
                    self.set(later, StepState::Pending);
                }
                self.paused_from = None;
                if step != StepKind::FixCross {
                    self.fix_for = None;
                }
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
            // The first review's fix is checked by Codex next.
            StepKind::Fix => {
                self.set(step, StepState::Done);
                self.start_cross_review()
            }
            // A fix of the final review: that review runs again; of Codex's: Codex again.
            StepKind::FixCross => {
                self.set(step, StepState::Done);
                if self.fix_for.take() == Some(StepKind::FinalReview) {
                    self.run(StepKind::FinalReview)
                } else {
                    self.start_cross_review()
                }
            }
            StepKind::PushSummary => {
                self.set(step, StepState::Done);
                self.phase = Phase::AwaitingPush;
                Ok(())
            }
            // Reviews finish with their own events because the outcome matters.
            StepKind::ReviewLocal | StepKind::CrossReview | StepKind::FinalReview => {
                Err(DomainError::InvalidTransition {
                    state: format!("{:?}", self.phase),
                    event: format!("StepSucceeded({step:?}); use the review result event"),
                })
            }
        }
    }

    /// The flow went past a Sửa lỗi step with nothing to fix: it shows "bỏ qua", not "chưa tới".
    fn pass(&mut self, step: StepKind) {
        if self.state_of(step) == StepState::Pending {
            self.set(step, StepState::Skipped);
        }
    }

    fn fix_final_review(&mut self) -> DomainResult<()> {
        self.fix_for = Some(StepKind::FinalReview);
        self.run(StepKind::FixCross)
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
        wf.apply(E::ReviewLocalFinished { blocking: 0 }).unwrap();
        wf
    }

    fn clean_final() -> E {
        E::FinalReviewFinished {
            critical_or_high: 0,
            medium_undecided: 0,
        }
    }

    #[test]
    fn steps_are_numbered_in_order() {
        let indexes: Vec<u8> = StepKind::ALL.iter().map(|s| s.index()).collect();
        assert_eq!(indexes, (1..=12).collect::<Vec<u8>>());
        assert_eq!(StepKind::BrowserTest.next(), Some(StepKind::PushSummary));
        assert_eq!(StepKind::PushSummary.next(), None);
    }

    #[test]
    fn happy_path_ends_waiting_for_user_push() {
        let mut wf = implemented();
        assert_eq!(wf.phase(), Phase::Running(StepKind::ReviewLocal));
        wf.apply(E::ReviewLocalFinished { blocking: 0 }).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::CrossReview));
        assert_eq!(wf.round(), 1);
        wf.apply(E::CrossReviewFinished { blocking: 0 }).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::FinalReview));
        wf.apply(clean_final()).unwrap();
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
    }

    /// The order the user chose: review-local -> Claude fixes -> Codex ⇄ Claude -> review-local.
    #[test]
    fn review_local_findings_are_fixed_before_codex_reviews() {
        let mut wf = implemented();
        wf.apply(E::ReviewLocalFinished { blocking: 2 }).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::Fix));
        assert_eq!(wf.round(), 0);
        wf.apply(E::StepSucceeded(StepKind::Fix)).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::CrossReview));
        assert_eq!(wf.round(), 1);
        wf.apply(E::CrossReviewFinished { blocking: 1 }).unwrap();
        // Codex's findings are fixed in step 9, then Codex reviews again
        assert_eq!(wf.phase(), Phase::Running(StepKind::FixCross));
        wf.apply(E::StepSucceeded(StepKind::FixCross)).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::CrossReview));
        assert_eq!(wf.round(), 2);
        assert_eq!(wf.state_of(StepKind::Fix), StepState::Done);
        wf.apply(E::CrossReviewFinished { blocking: 0 }).unwrap();
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
        wf.apply(E::PlanRejected).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::Analyze));
    }

    #[test]
    fn review_fix_loop_stops_after_max_rounds() {
        let mut wf = started_to_review();
        for expected_round in 1..DEFAULT_MAX_REVIEW_ROUNDS {
            assert_eq!(wf.round(), expected_round);
            wf.apply(E::CrossReviewFinished { blocking: 2 }).unwrap();
            assert_eq!(wf.phase(), Phase::Running(StepKind::FixCross));
            wf.apply(E::StepSucceeded(StepKind::FixCross)).unwrap();
        }
        assert_eq!(wf.round(), DEFAULT_MAX_REVIEW_ROUNDS);
        wf.apply(E::CrossReviewFinished { blocking: 1 }).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::FinalReview));
    }

    #[test]
    fn final_review_blocking_is_fixed_then_reviewed_again_and_medium_waits_for_user() {
        let mut wf = started_to_review();
        wf.apply(E::CrossReviewFinished { blocking: 0 }).unwrap();
        wf.apply(E::FinalReviewFinished {
            critical_or_high: 1,
            medium_undecided: 3,
        })
        .unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::FixCross));
        wf.apply(E::StepSucceeded(StepKind::FixCross)).unwrap();
        // fixed in step 9, then the final review runs again (not Codex)
        assert_eq!(wf.phase(), Phase::Running(StepKind::FinalReview));
        assert_eq!(wf.round(), 1);
        wf.apply(E::FinalReviewFinished {
            critical_or_high: 0,
            medium_undecided: 2,
        })
        .unwrap();
        assert_eq!(wf.phase(), Phase::Gate(GateKind::MediumTriage));
        assert_eq!(wf.current_step(), Some(StepKind::FinalReview));
        wf.apply(E::MediumTriaged { fix: 0 }).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::PushSummary));
    }

    #[test]
    fn mediums_picked_for_fixing_go_through_fix_and_codex_again() {
        let mut wf = started_to_review();
        wf.apply(E::CrossReviewFinished { blocking: 0 }).unwrap();
        wf.apply(E::FinalReviewFinished {
            critical_or_high: 0,
            medium_undecided: 1,
        })
        .unwrap();
        wf.apply(E::MediumTriaged { fix: 1 }).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::FixCross));
        wf.apply(E::StepSucceeded(StepKind::FixCross)).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::FinalReview));
        // survives a save while the fix runs
        let mut wf = started_to_review();
        wf.apply(E::CrossReviewFinished { blocking: 0 }).unwrap();
        wf.apply(E::FinalReviewFinished {
            critical_or_high: 1,
            medium_undecided: 0,
        })
        .unwrap();
        let mut back: Workflow = serde_json::from_str(&serde_json::to_string(&wf).unwrap()).unwrap();
        back.apply(E::StepSucceeded(StepKind::FixCross)).unwrap();
        assert_eq!(back.phase(), Phase::Running(StepKind::FinalReview));
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
        wf.apply(E::CrossReviewFinished { blocking: 1 }).unwrap();
        wf.apply(E::StepFailed(StepKind::FixCross)).unwrap();
        let back: Workflow = serde_json::from_str(&serde_json::to_string(&wf).unwrap()).unwrap();
        assert_eq!(back, wf);
        assert_eq!(back.failed_step(), Some(StepKind::FixCross));
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
        wf.apply(E::CrossReviewFinished { blocking: 0 }).unwrap();
        wf.apply(clean_final()).unwrap();
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
        // fixing Codex's findings: that fix is step 9 now
        let fixing: Workflow = serde_json::from_str(&legacy(
            r#""done","done","done","done","running","done","pending","pending""#,
            r#"{"running":"fix"}"#,
            2,
        ))
        .unwrap();
        assert_eq!(fixing.phase(), Phase::Running(StepKind::FixCross));
        assert_eq!(fixing.state_of(StepKind::FixCross), StepState::Running);
        assert_eq!(fixing.state_of(StepKind::BrowserTest), StepState::Pending);
        // the first review's fix (no Codex yet) stays step 7; Kiểm tra / Harness are behind it
        let first: Workflow = serde_json::from_str(&legacy(
            r#""done","done","done","done","running","pending","pending","pending""#,
            r#"{"running":"fix"}"#,
            0,
        ))
        .unwrap();
        assert_eq!(first.phase(), Phase::Running(StepKind::Fix));
        assert_eq!(first.state_of(StepKind::Checks), StepState::Skipped);
        // still at the plan gate: nothing to skip yet
        let plan: Workflow = serde_json::from_str(&legacy(
            r#""done","waitingUser","pending","pending","pending","pending","pending","pending""#,
            r#"{"gate":"planReview"}"#,
            0,
        ))
        .unwrap();
        assert_eq!(plan.state_of(StepKind::Checks), StepState::Pending);
        // saved again, it is a 12-step workflow
        let json = serde_json::to_string(&push).unwrap();
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
    }
}
