//! Adapters for the AI CLIs. Commands are built as argument arrays (never a shell string) and
//! prompts go through stdin. Every coding process also gets the git guard from `opw-vcs`.

pub mod claude;
pub mod cli;
pub mod codex;
pub mod env;
pub mod run;
pub mod text;
pub mod transcript;

#[derive(Debug, thiserror::Error)]
pub enum AgentsError {
    #[error("invalid stream-json line: {0}")]
    BadLine(String),
}

pub type AgentsResult<T> = Result<T, AgentsError>;

/// Model and effort the user picked for a step. `None` = the CLI's own default on this machine:
/// no flag is passed, so nothing changes from how the CLI runs by hand.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelChoice {
    pub model: Option<String>,
    pub effort: Option<String>,
}

impl ModelChoice {
    pub fn new(model: Option<&str>, effort: Option<&str>) -> Self {
        let clean = |v: Option<&str>| v.map(str::trim).filter(|v| !v.is_empty()).map(str::to_owned);
        Self {
            model: clean(model),
            effort: clean(effort),
        }
    }
}
