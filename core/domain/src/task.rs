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

/// Turn a branch name the user typed back into a template, so the next task of the same project
/// gets the same shape: `f/WP-187716-fix` with id 187716 → `f/WP-{taskId}-fix`.
/// `None` when the name does not contain the id.
pub fn template_from_branch(name: &str, wp_id: u64) -> Option<String> {
    let id = wp_id.to_string();
    let at = name.rfind(&id)?;
    // the id must not be part of a longer number
    let before = name[..at].chars().next_back();
    let after = name[at + id.len()..].chars().next();
    if before.is_some_and(|c| c.is_ascii_digit()) || after.is_some_and(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(format!("{}{{taskId}}{}", &name[..at], &name[at + id.len()..]))
}

/// Minimum digits for a run to count as a task id when guessing a repo's branch convention.
const MIN_ID_DIGITS: usize = 4;

/// Guess the branch convention of a repo from its existing branch names: each name with exactly
/// one run of 4+ digits becomes a template (`f/WP-187641` → `f/WP-{taskId}`), `origin/` dropped,
/// and the most common template wins (ties: the one seen first, i.e. the newest branch).
/// Needs at least two branches with the same shape, so one odd branch does not set the rule.
pub fn infer_branch_template<'a>(names: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for name in names {
        let name = name.strip_prefix("origin/").unwrap_or(name);
        let Some(template) = template_from_digit_run(name) else {
            continue;
        };
        match counts.iter_mut().find(|(t, _)| *t == template) {
            Some((_, n)) => *n += 1,
            None => counts.push((template, 1)),
        }
    }
    let best = counts.iter().map(|(_, n)| *n).max()?;
    counts
        .into_iter()
        .find(|(_, n)| *n == best && best >= 2)
        .map(|(t, _)| t)
}

fn template_from_digit_run(name: &str) -> Option<String> {
    let bytes = name.as_bytes();
    let mut runs = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i - start >= MIN_ID_DIGITS {
                runs.push((start, i));
            }
        } else {
            i += 1;
        }
    }
    match runs.as_slice() {
        [(start, end)] => Some(format!("{}{{taskId}}{}", &name[..*start], &name[*end..])),
        _ => None,
    }
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

    #[test]
    fn template_from_a_typed_branch_name() {
        assert_eq!(
            template_from_branch("f/WP-187716", 187716).as_deref(),
            Some("f/WP-{taskId}")
        );
        assert_eq!(
            template_from_branch("fix/187716-login", 187716).as_deref(),
            Some("fix/{taskId}-login")
        );
        assert_eq!(template_from_branch("f/1877160", 187716), None);
        assert_eq!(template_from_branch("f/login", 187716), None);
    }

    #[test]
    fn infers_the_repo_convention_from_existing_branches() {
        let names = [
            "origin/development",
            "f/WP-187641",
            "origin/f/WP-186196",
            "fix/issue-12",
            "origin/release-2026.11",
            "feature/185609",
            "f/WP-185000",
        ];
        assert_eq!(infer_branch_template(names).as_deref(), Some("f/WP-{taskId}"));
        // a single odd branch is not a convention
        assert_eq!(infer_branch_template(["feature/185609", "development"]), None);
        // two id-like numbers in one name: ambiguous, ignored
        assert_eq!(infer_branch_template(["f/1234-5678", "f/2345-6789"]), None);
        assert_eq!(
            render_branch_name("f/WP-{taskId}", 188001).unwrap(),
            "f/WP-188001"
        );
    }
}
