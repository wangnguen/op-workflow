//! `brief.md`: the only thing the AI knows about a task (PLAN.md §4). Built by code from the
//! task description and comments, redacted, no LLM involved.

use crate::redact;

/// Keep the brief small: only the newest comments go in.
pub const MAX_COMMENTS: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BriefComment {
    pub created_at: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BriefInput<'a> {
    /// e.g. `WP-187716`
    pub task_id: &'a str,
    pub title: &'a str,
    /// Link to the task on OpenProject.
    pub task_url: &'a str,
    pub repo: &'a str,
    pub branch: &'a str,
    pub base: &'a str,
    pub description: &'a str,
    /// Any order; the brief lists the newest first.
    pub comments: &'a [BriefComment],
    pub issue_links: &'a [String],
    /// User-declared words to mask (customer names, secret names).
    pub redact_terms: &'a [String],
}

pub fn render_brief(b: &BriefInput<'_>) -> String {
    let clean = |s: &str| redact(s.trim(), b.redact_terms);
    let mut out = format!("# [{}] {}\n", b.task_id, clean(b.title));
    let issues = if b.issue_links.is_empty() {
        "chưa có".to_owned()
    } else {
        b.issue_links.join(" · ")
    };
    out.push_str(&format!(
        "Task: {} · Issue: {} · Repo: {}\nBranch: {} (base: {})\n\n",
        b.task_url, issues, b.repo, b.branch, b.base
    ));

    out.push_str("## Yêu cầu\n");
    let description = clean(b.description);
    out.push_str(if description.is_empty() {
        "(task không có mô tả)"
    } else {
        &description
    });
    out.push_str("\n\n## Comment trên OpenProject\n");
    let mut comments: Vec<&BriefComment> = b.comments.iter().collect();
    comments.sort_by(|a, c| c.created_at.cmp(&a.created_at));
    if comments.is_empty() {
        out.push_str("(chưa có comment)\n");
    }
    for c in comments.iter().take(MAX_COMMENTS) {
        out.push_str(&format!("### {}\n{}\n\n", c.created_at, clean(&c.text)));
    }
    if comments.len() > MAX_COMMENTS {
        out.push_str(&format!(
            "({} comment cũ hơn không đưa vào)\n",
            comments.len() - MAX_COMMENTS
        ));
    }
    out.push('\n');
    out.push_str(&render_issue_section(&[], b.redact_terms));
    out
}

/// Heading of the last brief section; [`replace_issue_section`] rewrites from here to the end.
pub const ISSUE_HEADING: &str = "## Từ issue";
/// Keep the brief small: the newest issue comments only.
pub const MAX_ISSUE_COMMENTS: usize = 15;

/// A comment's date and text. Authors are left out on purpose: the brief needs what was said,
/// not who (client staff names stay out of the files the AI reads).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BriefIssueComment {
    pub created_at: String,
    pub body: String,
}

/// One issue as read from its page (title, markdown body, comments).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BriefIssue {
    pub url: String,
    pub title: String,
    pub body: String,
    pub comments: Vec<BriefIssueComment>,
    /// Comments GitHub counts that the page did not give (hidden, minimized, not loaded).
    pub unread: u32,
}

/// `## Từ issue` with each issue's body and newest comments, redacted like the rest.
pub fn render_issue_section(issues: &[BriefIssue], redact_terms: &[String]) -> String {
    let clean = |s: &str| redact(s.trim(), redact_terms);
    let mut out = format!("{ISSUE_HEADING}\n");
    if issues.is_empty() {
        out.push_str("(chưa đọc issue: bấm \"Lấy issue\" trên thẻ task, hoặc dán nội dung vào đây)\n");
        return out;
    }
    for issue in issues {
        out.push_str(&format!("### {}\n{}\n\n", clean(&issue.title), issue.url));
        let body = clean(&issue.body);
        out.push_str(if body.is_empty() {
            "(issue không có nội dung)"
        } else {
            &body
        });
        out.push_str("\n\n");
        let mut comments: Vec<&BriefIssueComment> = issue.comments.iter().collect();
        comments.sort_by(|a, c| c.created_at.cmp(&a.created_at));
        if !comments.is_empty() {
            out.push_str("#### Comment trên issue (mới nhất trước)\n");
        }
        for c in comments.iter().take(MAX_ISSUE_COMMENTS) {
            out.push_str(&format!("- {}\n\n{}\n\n", c.created_at, clean(&c.body)));
        }
        if comments.len() > MAX_ISSUE_COMMENTS {
            out.push_str(&format!(
                "({} comment cũ hơn không đưa vào)\n\n",
                comments.len() - MAX_ISSUE_COMMENTS
            ));
        }
        if issue.unread > 0 {
            out.push_str(&format!(
                "({} comment trên GitHub không lấy được: bị ẩn hoặc trang chưa tải hết, mở issue để xem)\n\n",
                issue.unread
            ));
        }
    }
    out
}

/// Swap the `## Từ issue` section (always last) for `section`; append it when missing.
pub fn replace_issue_section(brief: &str, section: &str) -> String {
    let at = brief
        .match_indices(ISSUE_HEADING)
        .find(|(i, _)| *i == 0 || brief[..*i].ends_with('\n'))
        .map(|(i, _)| i);
    match at {
        Some(i) => format!("{}{}", &brief[..i], section),
        None => format!("{}\n\n{}", brief.trim_end(), section),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brief_has_header_newest_comment_first_and_is_redacted() {
        let comments = [
            BriefComment {
                created_at: "2026-11-10T01:00:00Z".into(),
                text: "cũ".into(),
            },
            BriefComment {
                created_at: "2026-11-12T01:00:00Z".into(),
                text: "Lỗi khi lưu, liên hệ a.b@example.com".into(),
            },
        ];
        let links = ["https://github.com/acme/web/issues/456".to_owned()];
        let out = render_brief(&BriefInput {
            task_id: "WP-187716",
            title: "Không lưu được đơn",
            task_url: "https://op.example.com/work_packages/187716",
            repo: "acme-web",
            branch: "f/WP-187716",
            base: "origin/development",
            description: "Bấm Lưu thì lỗi 500 cho AcmeCorp",
            comments: &comments,
            issue_links: &links,
            redact_terms: &["AcmeCorp".into()],
        });
        assert!(out.starts_with("# [WP-187716] Không lưu được đơn\n"));
        assert!(out.contains("Branch: f/WP-187716 (base: origin/development)"));
        assert!(out.contains("Issue: https://github.com/acme/web/issues/456"));
        let new = out.find("Lỗi khi lưu").unwrap();
        let old = out.find("\ncũ\n").unwrap();
        assert!(new < old, "newest comment first");
        assert!(!out.contains("a.b@example.com"));
        assert!(!out.contains("AcmeCorp"));
    }

    #[test]
    fn issue_section_is_rendered_redacted_and_replaced_in_place() {
        let issue = BriefIssue {
            url: "https://github.com/acme/web/issues/456".into(),
            title: "Lỗi lưu đơn của AcmeCorp".into(),
            body: "Bước tái hiện: bấm Lưu".into(),
            comments: vec![
                BriefIssueComment {
                    created_at: "2026-11-01T00:00:00Z".into(),
                    body: "cũ".into(),
                },
                BriefIssueComment {
                    created_at: "2026-11-05T00:00:00Z".into(),
                    body: "đã thử, vẫn lỗi".into(),
                },
            ],
            unread: 1,
        };
        let section = render_issue_section(&[issue], &["AcmeCorp".into()]);
        assert!(section.starts_with("## Từ issue\n### Lỗi lưu đơn của "));
        assert!(!section.contains("AcmeCorp"));
        assert!(section.find("vẫn lỗi").unwrap() < section.find("\n\ncũ\n").unwrap());
        assert!(section.contains("- 2026-11-05T00:00:00Z\n\nđã thử, vẫn lỗi\n"));
        assert!(section.contains("(1 comment trên GitHub không lấy được"));

        let brief = "# [WP-1] t\n\n## Yêu cầu\nx\n\n## Từ issue\n(chưa đọc issue)\n";
        let out = replace_issue_section(brief, &section);
        assert!(out.starts_with("# [WP-1] t\n\n## Yêu cầu\nx\n\n## Từ issue\n### "));
        assert!(!out.contains("(chưa đọc issue)"));
        // a brief without the heading gets the section appended
        assert!(replace_issue_section("# t\n", "## Từ issue\nz\n").ends_with("# t\n\n## Từ issue\nz\n"));
    }

    #[test]
    fn empty_task_still_renders() {
        let out = render_brief(&BriefInput {
            task_id: "WP-1",
            title: "t",
            task_url: "u",
            repo: "r",
            branch: "b",
            base: "main",
            description: "  ",
            comments: &[],
            issue_links: &[],
            redact_terms: &[],
        });
        assert!(out.contains("(task không có mô tả)"));
        assert!(out.contains("(chưa có comment)"));
        assert!(out.contains("Issue: chưa có"));
    }
}
