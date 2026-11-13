//! Environment hygiene for the AI processes the app starts (headless and terminal panes).
//!
//! When op-workflow itself runs inside a Claude Code session (typical while developing it),
//! the environment carries that session's variables. A child `claude` that inherits them
//! skips writing its transcript (learned from Orrery) and may try to talk to the parent
//! session. Seen on 2.1.323: `CLAUDECODE`, `CLAUDE_CODE_SESSION_ID`,
//! `CLAUDE_CODE_MESSAGING_SOCKET`, `CLAUDE_CODE_MESSAGING_TOKEN`, `CLAUDE_PID`, …
//!
//! This is an explicit list on purpose: user settings such as `CLAUDE_CONFIG_DIR` or
//! `CLAUDE_CODE_GIT_BASH_PATH` must reach the child unchanged.

/// Variables that belong to a parent Claude Code session and are removed from children.
pub const SESSION_ENV: [&str; 17] = [
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_EMIT_STARTUP_TIMING",
    "CLAUDE_CODE_STARTUP_FAILURE_RESULTS",
    "CLAUDE_CODE_QUESTION_PREVIEW_FORMAT",
    "CLAUDE_CODE_ENABLE_SDK_FILE_CHECKPOINTING",
    "CLAUDE_CODE_ENABLE_TASKS",
    "CLAUDE_CODE_SSE_PORT",
    "CLAUDE_AGENT_SDK_VERSION",
    "CLAUDE_EFFORT",
    "CLAUDE_PID",
];

/// Which account a run uses (PLAN.md §10: chosen per run).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Account {
    /// The CLI's own login; an inherited API key must not silently take over.
    Subscription,
    /// The app sets the key for this process itself, after filtering.
    ApiKey,
}

/// Should this inherited variable be removed before starting an AI process?
pub fn is_stripped(name: &str, account: Account) -> bool {
    let upper = name.to_ascii_uppercase();
    let api_key = matches!(
        upper.as_str(),
        "ANTHROPIC_API_KEY" | "CODEX_API_KEY" | "OPENAI_API_KEY"
    );
    SESSION_ENV.contains(&upper.as_str()) || (api_key && account == Account::Subscription)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_parent_session_vars_but_keeps_user_settings() {
        for v in [
            "CLAUDECODE",
            "CLAUDE_CODE_SESSION_ID",
            "claude_pid",
            "CLAUDE_CODE_MESSAGING_TOKEN",
        ] {
            assert!(is_stripped(v, Account::ApiKey), "{v}");
        }
        for v in [
            "CLAUDE_CONFIG_DIR",
            "CLAUDE_CODE_GIT_BASH_PATH",
            "PATH",
            "ANTHROPIC_BASE_URL",
        ] {
            assert!(!is_stripped(v, Account::Subscription), "{v}");
        }
    }

    #[test]
    fn api_keys_only_pass_in_api_key_mode() {
        assert!(is_stripped("ANTHROPIC_API_KEY", Account::Subscription));
        assert!(is_stripped("CODEX_API_KEY", Account::Subscription));
        assert!(!is_stripped("ANTHROPIC_API_KEY", Account::ApiKey));
    }
}
