//! Reading what an AI answered: review findings (JSON block first, markdown markers as a
//! fallback, PLAN.md §9) and the fenced blocks of a push summary.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

/// One finding as a reviewer wrote it, before the app gives it a fingerprint and a state.
/// `severity` is always one of `critical`, `high`, `medium`, `low`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawFinding {
    pub severity: String,
    pub file: String,
    pub line: Option<u32>,
    pub title: String,
    pub detail: String,
    pub suggestion: String,
}

/// A fenced code block: info string (`json`, `text`, …) and body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fenced {
    pub lang: String,
    pub body: String,
    /// The nearest non-empty line above the opening fence (a heading such as
    /// `### Commit message`), cleaned of markdown; empty when there is none.
    pub heading: String,
}

/// Every fenced block (```` ``` ```` or `~~~`) in `text`, in order. An unclosed block runs to the end.
pub fn fenced_blocks(text: &str) -> Vec<Fenced> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim_start();
        let fence = if trimmed.starts_with("```") {
            "```"
        } else if trimmed.starts_with("~~~") {
            "~~~"
        } else {
            i += 1;
            continue;
        };
        let lang = trimmed.trim_start_matches(fence).trim().to_owned();
        let heading = lines[..i]
            .iter()
            .rev()
            .map(|l| l.trim())
            .find(|l| !l.is_empty())
            .map(clean_heading)
            .unwrap_or_default();
        let start = i + 1;
        let mut end = start;
        while end < lines.len() && !lines[end].trim_start().starts_with(fence) {
            end += 1;
        }
        out.push(Fenced {
            lang,
            body: lines[start..end.min(lines.len())].join("\n"),
            heading,
        });
        i = end + 1;
    }
    out
}

fn clean_heading(line: &str) -> String {
    line.trim_start_matches('#')
        .trim()
        .trim_matches(|c| c == '*' || c == '_' || c == '`')
        .trim()
        .trim_end_matches(':')
        .trim_matches(|c| c == '*' || c == '_')
        .trim()
        .to_owned()
}

/// `critical`/`high`/`medium`/`low` from the words reviewers use; unknown words count as medium.
pub fn normalize_severity(s: &str) -> String {
    match s.trim().to_ascii_lowercase().as_str() {
        "critical" | "blocker" | "severe" => "critical",
        "high" | "major" | "error" => "high",
        "low" | "minor" | "nit" | "nitpick" | "info" | "trivial" | "style" => "low",
        _ => "medium",
    }
    .to_owned()
}

fn string_at(v: &Value, key: &str) -> String {
    match v.get(key) {
        Some(Value::String(s)) => s.trim().to_owned(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

/// `{"findings": [...]}` → findings; `None` when the value has no `findings` array.
pub fn findings_from_json(v: &Value) -> Option<Vec<RawFinding>> {
    let items = v.get("findings")?.as_array()?;
    Some(
        items
            .iter()
            .filter(|f| f.is_object())
            .map(|f| RawFinding {
                severity: normalize_severity(&string_at(f, "severity")),
                file: string_at(f, "file"),
                line: f
                    .get("line")
                    .and_then(|l| l.as_u64().or_else(|| l.as_str()?.trim().parse().ok()))
                    .and_then(|l| u32::try_from(l).ok())
                    .filter(|l| *l > 0),
                title: string_at(f, "title"),
                detail: string_at(f, "detail"),
                suggestion: string_at(f, "suggestion"),
            })
            .filter(|f| !f.title.is_empty() || !f.detail.is_empty())
            .collect(),
    )
}

/// The last JSON object in `text` that has a `findings` array: fenced blocks first (last one
/// wins), then a bare `{"findings": …}` object.
fn last_findings_json(text: &str) -> Option<Vec<RawFinding>> {
    for block in fenced_blocks(text).iter().rev() {
        if let Ok(v) = serde_json::from_str::<Value>(block.body.trim()) {
            if let Some(found) = findings_from_json(&v) {
                return Some(found);
            }
        }
    }
    static BARE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"\{\s*"findings"\s*:"#).expect("valid regex"));
    let starts: Vec<usize> = BARE.find_iter(text).map(|m| m.start()).collect();
    starts.iter().rev().find_map(|&at| {
        let mut stream = serde_json::Deserializer::from_str(&text[at..]).into_iter::<Value>();
        stream.next()?.ok().as_ref().and_then(findings_from_json)
    })
}

static MARKER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(🔴|🟠|🟡|🔵|\[(critical|high|medium|low)\])").expect("valid regex"));
static FILE_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"`?([A-Za-z0-9_./\\-]+\.[A-Za-z0-9]+)(?::(\d+))?`?").expect("valid regex"));
static CLEAN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^[\s\-*#>\d.)]*(🔴|🟠|🟡|🔵)?\s*(\*\*)?\s*(\[(critical|high|medium|low)\])?\s*(\*\*)?[\s:—-]*",
    )
    .expect("valid regex")
});
static NO_FINDINGS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(no (issues|findings|problems|bugs)( found)?|lgtm|looks good)\b|không (có|tìm thấy|phát hiện) (lỗi|vấn đề)")
        .expect("valid regex")
});

fn marker_severity(marker: &str) -> &'static str {
    match marker.to_ascii_lowercase().as_str() {
        "🔴" | "[critical]" => "critical",
        "🟠" | "[high]" => "high",
        "🟡" | "[medium]" => "medium",
        _ => "low",
    }
}

/// Lines marked 🔴/🟠/🟡/🔵 or `[High]`…, as `/review-local` prints them without its JSON block.
fn findings_from_markdown(text: &str) -> Vec<RawFinding> {
    text.lines()
        .filter_map(|line| {
            let marker = MARKER.find(line)?;
            let rest = CLEAN.replace(line, "").trim().to_owned();
            let (file, line_no) = FILE_LINE
                .captures(&rest)
                .map(|c| {
                    (
                        c.get(1).map(|m| m.as_str().to_owned()).unwrap_or_default(),
                        c.get(2).and_then(|m| m.as_str().parse().ok()),
                    )
                })
                .unwrap_or_default();
            let title = rest.trim_matches(|c| c == '*' || c == ' ').to_owned();
            (!title.is_empty()).then(|| RawFinding {
                severity: marker_severity(marker.as_str()).to_owned(),
                file,
                line: line_no,
                title,
                detail: String::new(),
                suggestion: String::new(),
            })
        })
        .collect()
}

/// Findings of a review answer. `None` = the answer cannot be read as a review (no JSON block,
/// no severity markers, no "no issues" sentence); the step should fail rather than pass a
/// review nobody understood.
pub fn findings_from_text(text: &str) -> Option<Vec<RawFinding>> {
    if let Some(found) = last_findings_json(text) {
        return Some(found);
    }
    let marked = findings_from_markdown(text);
    if !marked.is_empty() {
        return Some(marked);
    }
    NO_FINDINGS.is_match(text).then(Vec::new)
}

/// GitHub issue number from a link such as `https://github.com/o/r/issues/279`.
pub fn issue_number(url: &str) -> Option<u64> {
    let (_, tail) = url.split_once("/issues/")?;
    tail.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_json_block_wins_and_severity_is_normalized() {
        let text = "Review\n```json\n{\"findings\":[{\"severity\":\"High\",\"file\":\"a.ts\",\"line\":3,\"title\":\"old\",\"detail\":\"\",\"suggestion\":\"\"}]}\n```\n\
                    later\n```json\n{\"findings\":[{\"severity\":\"major\",\"file\":\"b.ts\",\"line\":null,\"title\":\"Null org\",\"detail\":\"d\",\"suggestion\":\"s\"},{\"severity\":\"nit\",\"file\":\"c.ts\",\"line\":\"12\",\"title\":\"name\"}]}\n```";
        let found = findings_from_text(text).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].severity, "high");
        assert_eq!(found[0].file, "b.ts");
        assert_eq!(found[0].line, None);
        assert_eq!(found[1].severity, "low");
        assert_eq!(found[1].line, Some(12));
    }

    #[test]
    fn bare_json_and_empty_lists_count() {
        let text = "All good.\n{\"findings\": []}\n";
        assert_eq!(findings_from_text(text), Some(vec![]));
        let text =
            "x {\"findings\":[{\"severity\":\"critical\",\"file\":\"x.rs\",\"line\":1,\"title\":\"t\"}]} y";
        assert_eq!(findings_from_text(text).unwrap()[0].severity, "critical");
    }

    #[test]
    fn markdown_markers_are_the_fallback() {
        let text = "## Kết quả\n- 🟠 **[High]** `BE/src/job.repository.ts:316` CAS blocks recompute\n\
                    - 🟡 Missing loading state in web/src/cart.tsx:88\n- 🔵 Nit\nOther line";
        let found = findings_from_text(text).unwrap();
        assert_eq!(found.len(), 3);
        assert_eq!(found[0].severity, "high");
        assert_eq!(found[0].file, "BE/src/job.repository.ts");
        assert_eq!(found[0].line, Some(316));
        assert!(found[0].title.contains("CAS blocks"), "{}", found[0].title);
        assert_eq!(found[1].severity, "medium");
        assert_eq!(found[1].line, Some(88));
        assert_eq!(found[2].severity, "low");
    }

    #[test]
    fn unreadable_answers_are_none_but_no_issues_is_empty() {
        assert_eq!(findings_from_text("I could not finish."), None);
        assert_eq!(findings_from_text("No issues found."), Some(vec![]));
        assert_eq!(findings_from_text("Không có lỗi nào."), Some(vec![]));
    }

    #[test]
    fn fenced_blocks_keep_their_heading() {
        let text = "### Commit message\n\n```text\nfeat: x\n\nbody\n```\n**PR description:**\n```markdown\n## Summary\n- a\n```\n```\nunclosed";
        let blocks = fenced_blocks(text);
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].heading, "Commit message");
        assert_eq!(blocks[0].body, "feat: x\n\nbody");
        assert_eq!(blocks[1].heading, "PR description");
        assert_eq!(blocks[1].lang, "markdown");
        assert_eq!(blocks[2].body, "unclosed");
    }

    #[test]
    fn issue_numbers_from_links() {
        assert_eq!(issue_number("https://github.com/o/r/issues/279"), Some(279));
        assert_eq!(
            issue_number("https://github.com/o/r/issues/279#issuecomment-1"),
            Some(279)
        );
        assert_eq!(issue_number("https://github.com/o/r/pull/3"), None);
    }
}
