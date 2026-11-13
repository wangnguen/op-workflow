//! Adapters for the AI CLIs. Commands are built as argument arrays (never a shell string) and
//! prompts go through stdin. Every coding process also gets the git guard from `opw-vcs`.

pub mod claude;
pub mod cli;
pub mod codex;
pub mod env;

#[derive(Debug, thiserror::Error)]
pub enum AgentsError {
    #[error("invalid stream-json line: {0}")]
    BadLine(String),
}

pub type AgentsResult<T> = Result<T, AgentsError>;
