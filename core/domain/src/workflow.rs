//! The eight-step task workflow as a pure state machine (review order chosen by the user,
//! PLAN.md §4).
//!
//! ```text
//! intake -> analyze -> <plan review gate> -> implement
//!   -> reviewLocal (first pass) -> fix if it found anything
//!   -> crossReview (Codex) <-> fix (max rounds)
//!   -> finalReview (/review-local again) -> <critical/high: fix, then crossReview again
//!                                           | medium: triage gate> -> pushSummary -> done
//! ```
//! The machine never commits or pushes; after `pushSummary` the task waits for the user.

use serde::{Deserialize, Serialize};

use crate::error::{DomainError, DomainResult};
use crate::view::GateKind;

/// Default number of cross-review/fix rounds before the workflow moves on to the final
/// `/review-local`.
pub const DEFAULT_MAX_REVIEW_ROUNDS: u8 = 3;

/// Number of steps; the UI shows "x/8".
pub const STEP_COUNT: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StepKind {
    Intake,
    Analyze,
    Implement,
    /// First `/review-local`, right after implement.
    ReviewLocal,
    /// The AI that implemented (Claude by default) fixes findings from either review.
    Fix,
    /// The other AI (Codex by default) reviews the fixed code.
    CrossReview,
    /// `/review-local` again once Codex is clean; its result goes through gate G2.
    FinalReview,
    PushSummary,
}

impl StepKind {
    pub const ALL: [StepKind; STEP_COUNT] = [
        StepKind::Intake,
        StepKind::Analyze,
        StepKind::Implement,
        StepKind::ReviewLocal,
        StepKind::Fix,
        StepKind::CrossReview,
        StepKind::FinalReview,
        StepKind::PushSummary,
    ];

    /// 1-based position, also the orbit number in the UI.
    pub fn index(self) -> u8 {
        match self {
            StepKind::Intake => 1,
            StepKind::Analyze => 2,
            StepKind::Implement => 3,
            StepKind::ReviewLocal => 4,
            StepKind::Fix => 5,
            StepKind::CrossReview => 6,
            StepKind::FinalReview => 7,
            StepKind::PushSummary => 8,
        }
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    MediumTriaged,
    Pause,
    Resume,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workflow {
    states: [StepState; STEP_COUNT],
    phase: Phase,
    /// Phase to return to after `Resume`.
    paused_from: Option<Phase>,
    round: u8,
    max_rounds: u8,
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

    pub fn current_step(&self) -> Option<StepKind> {
        match self.phase {
            Phase::Running(step) => Some(step),
            Phase::Gate(GateKind::PlanReview) => Some(StepKind::Analyze),
            Phase::Gate(GateKind::MediumTriage) => Some(StepKind::FinalReview),
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

            (Phase::Running(StepKind::ReviewLocal), E::ReviewLocalFinished { blocking }) => {
                self.set(StepKind::ReviewLocal, StepState::Done);
                if blocking > 0 {
                    self.run(StepKind::Fix)
                } else {
                    self.start_cross_review()
                }
            }

            (Phase::Running(StepKind::CrossReview), E::CrossReviewFinished { blocking }) => {
                self.set(StepKind::CrossReview, StepState::Done);
                if blocking > 0 && self.round < self.max_rounds {
                    self.run(StepKind::Fix)
                } else {
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
                    self.run(StepKind::Fix)
                } else if medium_undecided > 0 {
                    self.set(StepKind::FinalReview, StepState::WaitingUser);
                    self.phase = Phase::Gate(GateKind::MediumTriage);
                    Ok(())
                } else {
                    self.set(StepKind::FinalReview, StepState::Done);
                    self.run(StepKind::PushSummary)
                }
            }
            (Phase::Gate(GateKind::MediumTriage), E::MediumTriaged) => {
                self.set(StepKind::FinalReview, StepState::Done);
                self.run(StepKind::PushSummary)
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
            StepKind::Implement => {
                self.set(step, StepState::Done);
                self.run(StepKind::ReviewLocal)
            }
            // A fix, whichever review it answered, is always checked by Codex next.
            StepKind::Fix => {
                self.set(step, StepState::Done);
                self.start_cross_review()
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

    fn run(&mut self, step: StepKind) -> DomainResult<()> {
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
        assert_eq!(indexes, (1..=8).collect::<Vec<u8>>());
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
        wf.apply(E::StepSucceeded(StepKind::PushSummary)).unwrap();
        assert_eq!(wf.phase(), Phase::AwaitingPush);
        assert!(
            StepKind::ALL
                .iter()
                .all(|s| matches!(wf.state_of(*s), StepState::Done | StepState::Pending))
        );
        assert_eq!(wf.state_of(StepKind::Fix), StepState::Pending);
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
        wf.apply(E::StepSucceeded(StepKind::Fix)).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::CrossReview));
        assert_eq!(wf.round(), 2);
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
            assert_eq!(wf.phase(), Phase::Running(StepKind::Fix));
            wf.apply(E::StepSucceeded(StepKind::Fix)).unwrap();
        }
        assert_eq!(wf.round(), DEFAULT_MAX_REVIEW_ROUNDS);
        wf.apply(E::CrossReviewFinished { blocking: 1 }).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::FinalReview));
    }

    #[test]
    fn final_review_blocking_goes_back_through_codex_and_medium_waits_for_user() {
        let mut wf = started_to_review();
        wf.apply(E::CrossReviewFinished { blocking: 0 }).unwrap();
        wf.apply(E::FinalReviewFinished {
            critical_or_high: 1,
            medium_undecided: 3,
        })
        .unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::Fix));
        wf.apply(E::StepSucceeded(StepKind::Fix)).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::CrossReview));
        wf.apply(E::CrossReviewFinished { blocking: 0 }).unwrap();
        wf.apply(E::FinalReviewFinished {
            critical_or_high: 0,
            medium_undecided: 2,
        })
        .unwrap();
        assert_eq!(wf.phase(), Phase::Gate(GateKind::MediumTriage));
        assert_eq!(wf.current_step(), Some(StepKind::FinalReview));
        wf.apply(E::MediumTriaged).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::PushSummary));
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
