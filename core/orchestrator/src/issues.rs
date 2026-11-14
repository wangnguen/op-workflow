//! "Lấy issue": the task's GitHub issues, read through the user's own Chrome, go into the
//! brief's `## Từ issue` section. Read-only on GitHub: no token, no clicks, no comments.

use opw_browser::{BrowserError, IssueError, IssueRead};
use opw_context::{BriefIssue, BriefIssueComment, ISSUE_HEADING, render_issue_section};
use opw_domain::view::ApiError;
use opw_tracker::{IssueHost, extract_issue_links};

/// Each read is one tab in the user's Chrome; keep it to the issues the task is about.
pub const MAX_ISSUES: usize = 3;

/// GitHub issue links of the task: everything in the brief above `## Từ issue` (description,
/// OpenProject comments, the link list). Links inside issue text are not followed.
pub fn github_links(brief: &str) -> Vec<String> {
    let head = brief
        .find(&format!("\n{ISSUE_HEADING}"))
        .map_or(brief, |at| &brief[..at]);
    extract_issue_links(head)
        .into_iter()
        .filter(|l| l.host == IssueHost::GitHub)
        .map(|l| l.url)
        .take(MAX_ISSUES)
        .collect()
}

/// "Issue #383" — warnings name the issue number, not the client's org/repo.
fn label(url: &str) -> String {
    url.rsplit('/')
        .next()
        .filter(|n| n.chars().all(|c| c.is_ascii_digit()))
        .map_or_else(|| "Issue".to_owned(), |n| format!("Issue #{n}"))
}

/// The new `## Từ issue` section, the issues that made it in, and what failed. All failed →
/// an error (the brief keeps its old section).
pub fn section_from(reads: Vec<IssueRead>) -> Result<(String, Vec<BriefIssue>, Vec<String>), ApiError> {
    let mut issues = Vec::new();
    let mut warnings = Vec::new();
    let mut signed_out = false;
    for read in reads {
        match read.result {
            Ok(page) => {
                let loaded = u32::try_from(page.comments.len()).unwrap_or(u32::MAX);
                issues.push(BriefIssue {
                    url: page.url,
                    title: page.title,
                    body: page.body,
                    comments: page
                        .comments
                        .into_iter()
                        .map(|c| BriefIssueComment {
                            created_at: c.created_at,
                            body: c.body,
                        })
                        .collect(),
                    unread: page.total_comments.saturating_sub(loaded),
                });
            }
            Err(e) => {
                signed_out |= e == IssueError::SignedOut;
                warnings.push(format!("{}: {e}", label(&read.url)));
            }
        }
    }
    if issues.is_empty() {
        let code = if signed_out {
            "github.signedOut"
        } else {
            "issue.failed"
        };
        return Err(ApiError::new(code, warnings.join("; ")));
    }
    Ok((render_issue_section(&issues, &[]), issues, warnings))
}

pub fn browser_error(e: BrowserError) -> ApiError {
    let code = match e {
        BrowserError::NotEnabled => "browser.notEnabled",
        BrowserError::NotRunning => "browser.notRunning",
        BrowserError::Denied => "browser.denied",
        BrowserError::Protocol(_) => "browser.failed",
    };
    ApiError::new(code, e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use opw_browser::{IssueComment, IssuePage};

    #[test]
    fn links_come_from_above_the_issue_section_github_only() {
        let brief = "# [WP-1] t\n\nxem https://github.com/acme/web/issues/12 và \
                     https://gitlab.acme.io/g/p/-/issues/3\n\n## Từ issue\n\
                     ### cũ\nhttps://github.com/acme/web/issues/99\n";
        assert_eq!(github_links(brief), vec!["https://github.com/acme/web/issues/12"]);
        let many: String = (1..=5)
            .map(|n| format!("https://github.com/a/b/issues/{n} "))
            .collect();
        assert_eq!(github_links(&many).len(), MAX_ISSUES);
    }

    fn page(url: &str) -> IssuePage {
        IssuePage {
            url: url.into(),
            title: "保存できない".into(),
            body: "手順".into(),
            comments: vec![IssueComment {
                created_at: "2026-11-10T00:00:00Z".into(),
                body: "補足".into(),
            }],
            total_comments: 2,
        }
    }

    #[test]
    fn section_keeps_what_was_read_and_names_failures_by_number() {
        let reads = vec![
            IssueRead {
                url: "https://github.com/acme/web/issues/12".into(),
                result: Ok(page("https://github.com/acme/web/issues/12")),
            },
            IssueRead {
                url: "https://github.com/acme/web/issues/13".into(),
                result: Err(IssueError::NotFound),
            },
        ];
        let (section, read, warnings) = section_from(reads).unwrap();
        assert_eq!(read.len(), 1);
        assert!(section.contains("### 保存できない"));
        assert!(section.contains("(1 comment trên GitHub không lấy được"));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].starts_with("Issue #13: "));
        assert!(!warnings[0].contains("acme"));
    }

    #[test]
    fn all_failed_is_an_error_and_signed_out_is_called_out() {
        let failed = |e: IssueError| {
            vec![IssueRead {
                url: "https://github.com/a/b/issues/1".into(),
                result: Err(e),
            }]
        };
        assert_eq!(
            section_from(failed(IssueError::SignedOut)).unwrap_err().code,
            "github.signedOut"
        );
        assert_eq!(
            section_from(failed(IssueError::Timeout)).unwrap_err().code,
            "issue.failed"
        );
        assert_eq!(browser_error(BrowserError::NotEnabled).code, "browser.notEnabled");
    }
}
