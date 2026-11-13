use crate::workflow::StepKind;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DomainError {
    #[error("event `{event}` is not valid while the workflow is {state}")]
    InvalidTransition { state: String, event: String },

    #[error("step {0:?} is not the current step")]
    NotCurrentStep(StepKind),

    #[error("invalid branch name `{0}`")]
    InvalidBranchName(String),
}

pub type DomainResult<T> = Result<T, DomainError>;
