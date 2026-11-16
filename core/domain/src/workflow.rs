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
//! At the plan gate the user can also code by hand (`SelfCode`): the workflow waits until they
//! say they are done, then reviews their change like an AI implementation.
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
#[serde(rename_all = "camelCase")]
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
                self.run(StepKind::ReviewLocal)
            }

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
            (Phase::Gate(GateKind::MediumTriage), E::MediumTriaged { fix }) => {
                self.set(StepKind::FinalReview, StepState::Done);
                if fix > 0 {
                    self.run(StepKind::Fix)
                } else {
                    self.run(StepKind::PushSummary)
                }
            }

            (Phase::AwaitingPush, E::RedoPushSummary) => self.run(StepKind::PushSummary),

            // `Running` here also means "ready to run": the caller checks no step process is
            // running. Only a step the task already reached can be run again.
            (phase, E::Reopen(step)) if phase != Phase::NotStarted => {
                if step == StepKind::Intake || self.state_of(step) == StepState::Pending {
                    return Err(DomainError::NotCurrentStep(step));
                }
                for later in StepKind::ALL.into_iter().filter(|s| s.index() >= step.index()) {
                    self.set(later, StepState::Pending);
                }
                self.paused_from = None;
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
        let mut again = wf.clone();
        again.apply(E::RedoPushSummary).unwrap();
        assert_eq!(again.phase(), Phase::Running(StepKind::PushSummary));
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
        assert_eq!(wf.phase(), Phase::Running(StepKind::Fix));
        wf.apply(E::StepSucceeded(StepKind::Fix)).unwrap();
        assert_eq!(wf.phase(), Phase::Running(StepKind::CrossReview));
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
        wf.apply(E::StepFailed(StepKind::Fix)).unwrap();
        let back: Workflow = serde_json::from_str(&serde_json::to_string(&wf).unwrap()).unwrap();
        assert_eq!(back, wf);
        assert_eq!(back.failed_step(), Some(StepKind::Fix));
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
