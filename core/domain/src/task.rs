//! Task identity and branch naming. The app never changes OpenProject status; the user does.

use crate::error::{DomainError, DomainResult};

/// Default branch template; `{taskId}` is the OpenProject work package id.
pub const DEFAULT_BRANCH_TEMPLATE: &str = "feature/{taskId}";

/// Display id used on cards and in the brief, e.g. `WP-187716`.
pub fn display_id(prefix: &str, wp_id: u64) -> String {
    format!("{prefix}-{wp_id}")
}

/// Render a branch name from a per-project template and check it is a valid git ref name.
pub fn render_branch_name(template: &str, wp_id: u64) -> DomainResult<String> {
    let name = template.replace("{taskId}", &wp_id.to_string());
    validate_branch_name(&name)?;
    Ok(name)
}

/// Subset of `git check-ref-format --branch` rules, enough to reject typos in the dialog.
pub fn validate_branch_name(name: &str) -> DomainResult<()> {
    let bad = name.is_empty()
        || name.starts_with(['-', '/', '.'])
        || name.ends_with(['/', '.'])
        || name.ends_with(".lock")
        || name.contains("..")
        || name.contains("//")
        || name.contains("@{")
        || name
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "~^:?*[\\".contains(c));
    if bad {
        Err(DomainError::InvalidBranchName(name.to_owned()))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_default_template() {
        assert_eq!(
            render_branch_name(DEFAULT_BRANCH_TEMPLATE, 187716).unwrap(),
            "feature/187716"
        );
        assert_eq!(display_id("WP", 187716), "WP-187716");
    }

    #[test]
    fn rejects_invalid_branch_names() {
        for bad in [
            "", "f/WP 1", "f//x", "f/x.lock", "-x", "a..b", "f/x/", "a~b", "a:b",
        ] {
            assert!(validate_branch_name(bad).is_err(), "{bad:?} should be rejected");
        }
        assert!(validate_branch_name("feature/187716-fix_login").is_ok());
    }
}
