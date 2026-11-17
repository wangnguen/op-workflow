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
fn last_findings_value(text: &str) -> Option<Value> {
    let has_findings = |v: &Value| v.get("findings").is_some_and(Value::is_array);
    for block in fenced_blocks(text).iter().rev() {
        if let Ok(v) = serde_json::from_str::<Value>(block.body.trim()) {
            if has_findings(&v) {
                return Some(v);
            }
        }
    }
    static BARE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"\{\s*"findings"\s*:"#).expect("valid regex"));
    let starts: Vec<usize> = BARE.find_iter(text).map(|m| m.start()).collect();
    starts.iter().rev().find_map(|&at| {
        let mut stream = serde_json::Deserializer::from_str(&text[at..]).into_iter::<Value>();
        stream.next()?.ok().filter(has_findings)
    })
}

fn last_findings_json(text: &str) -> Option<Vec<RawFinding>> {
    findings_from_json(&last_findings_value(text)?)
}

/// Longest docs/test note kept from a review.
const NOTES_MAX_CHARS: usize = 3000;

/// Headings of the "do docs or tests need updating?" part of a review (`/review-local` writes
/// it in Japanese, e.g. `ドキュメント・テスト更新判定`).
static NOTES_HEADING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)ドキュメント|テスト更新|\bdocs?\b|documentation|tài liệu|cập nhật test")
        .expect("valid regex")
});

/// The reviewer's judgement on documentation and tests: the `notes` field of the json block,
/// else a markdown section whose heading names docs or tests. `None` when there is none.
pub fn review_notes(text: &str) -> Option<String> {
    let from_json = last_findings_value(text).and_then(|v| match v.get("notes")? {
        Value::String(s) => Some(s.trim().to_owned()),
        Value::Array(items) => Some(
            items
                .iter()
                .filter_map(Value::as_str)
                .map(|s| format!("- {}", s.trim()))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        _ => None,
    });
    let notes = from_json
        .filter(|n| !n.is_empty())
        .or_else(|| notes_section(text))?;
    Some(if notes.chars().count() > NOTES_MAX_CHARS {
        let cut: String = notes.chars().take(NOTES_MAX_CHARS).collect();
        format!("{cut}\n[…]")
    } else {
        notes
    })
}

/// Body of the first markdown heading that names docs/tests, up to the next heading of the same
/// or a higher level (fenced blocks are skipped: the json block is not part of it).
fn notes_section(text: &str) -> Option<String> {
    let level = |line: &str| {
        let t = line.trim_start();
        let n = t.chars().take_while(|c| *c == '#').count();
        (n > 0 && t[n..].starts_with(' ')).then_some(n)
    };
    let lines: Vec<&str> = text.lines().collect();
    let start = lines
        .iter()
        .position(|l| level(l).is_some() && NOTES_HEADING.is_match(l))?;
    let depth = level(lines[start]).unwrap_or(1);
    let mut body = Vec::new();
    let mut in_fence = false;
    for line in &lines[start + 1..] {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if level(line).is_some_and(|l| l <= depth) {
            break;
        }
        body.push(*line);
    }
    let body = body.join("\n").trim().to_owned();
    (!body.is_empty()).then_some(body)
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

/// A question the analysis asks the user, as the AI wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawQuestion {
    pub text: String,
    /// Answers the AI suggests.
    pub options: Vec<String>,
}

/// What the analysis advises for an optional step: `step` is `harness` or `browserTest`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawProposal {
    pub step: &'static str,
    pub recommend: bool,
    pub reason: String,
    /// What a harness would check, or the browser test scenario.
    pub checks: String,
}

/// An analysis answer split into the plan the user reads and the trailing json block.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlanAnswer {
    /// The markdown plan without the json block.
    pub plan: String,
    pub questions: Vec<RawQuestion>,
    pub proposals: Vec<RawProposal>,
}

/// Keys of the analysis json block.
const PLAN_KEYS: [&str; 3] = ["questions", "harness", "browserTest"];

/// Fenced blocks with the line numbers of their opening and closing fences (closing = the line
/// count when the block is unclosed).
fn fenced_spans(lines: &[&str]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim_start();
        let fence = if t.starts_with("```") {
            "```"
        } else if t.starts_with("~~~") {
            "~~~"
        } else {
            i += 1;
            continue;
        };
        let mut end = i + 1;
        while end < lines.len() && !lines[end].trim_start().starts_with(fence) {
            end += 1;
        }
        out.push((i, end));
        i = end + 1;
    }
    out
}

fn proposal(v: &Value, key: &'static str) -> Option<RawProposal> {
    let p = v.get(key)?;
    let recommend = match p {
        Value::Bool(b) => *b,
        Value::Object(_) => p.get("recommend").and_then(Value::as_bool).unwrap_or(false),
        _ => return None,
    };
    let checks = ["checks", "scenario"]
        .iter()
        .map(|k| match p.get(*k) {
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(Value::as_str)
                .map(|s| format!("- {}", s.trim()))
                .collect::<Vec<_>>()
                .join("\n"),
            _ => string_at(p, k),
        })
        .find(|s| !s.is_empty())
        .unwrap_or_default();
    Some(RawProposal {
        step: key,
        recommend,
        reason: string_at(p, "reason"),
        checks,
    })
}

fn questions_from_json(v: &Value) -> Vec<RawQuestion> {
    let Some(items) = v.get("questions").and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|q| {
            let (text, options) = match q {
                Value::String(s) => (s.trim().to_owned(), Vec::new()),
                Value::Object(_) => (
                    ["text", "question"]
                        .iter()
                        .map(|k| string_at(q, k))
                        .find(|s| !s.is_empty())
                        .unwrap_or_default(),
                    q.get("options")
                        .and_then(Value::as_array)
                        .map(|o| {
                            o.iter()
                                .filter_map(Value::as_str)
                                .map(|s| s.trim().to_owned())
                                .filter(|s| !s.is_empty())
                                .collect()
                        })
                        .unwrap_or_default(),
                ),
                _ => return None,
            };
            (!text.is_empty()).then_some(RawQuestion { text, options })
        })
        .collect()
}

static QUESTION_HEADING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*#{1,6}\s.*(câu hỏi|questions?)").expect("valid regex"));
/// `- `, `* `, `> `, `2. `, `3) ` at the start of a line.
static LIST_MARK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:[-*>]\s+|\d+[.)]\s+)+").expect("valid regex"));

/// Fallback when there is no json block: lines ending in `?` under a heading that names
/// questions ("## Rủi ro và câu hỏi"), up to the next heading.
fn questions_from_markdown(plan: &str) -> Vec<RawQuestion> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in plan.lines() {
        let t = line.trim();
        if t.starts_with('#') {
            inside = QUESTION_HEADING.is_match(line);
            continue;
        }
        if !inside {
            continue;
        }
        let text = LIST_MARK.replace(t, "");
        let text = text.trim().trim_matches('*').trim();
        if text.ends_with('?') || text.ends_with('？') {
            out.push(RawQuestion {
                text: text.to_owned(),
                options: Vec::new(),
            });
        }
    }
    out
}

/// Split an analysis answer: the last fenced json block with `questions` / `harness` /
/// `browserTest` is taken out of the plan and read; without one, questions come from the
/// plan's question section and there are no proposals.
pub fn plan_answer(text: &str) -> PlanAnswer {
    let lines: Vec<&str> = text.lines().collect();
    let block = fenced_spans(&lines).into_iter().rev().find_map(|(open, close)| {
        let body = lines[open + 1..close.min(lines.len())].join("\n");
        let v: Value = serde_json::from_str(body.trim()).ok()?;
        PLAN_KEYS
            .iter()
            .any(|k| v.get(k).is_some())
            .then_some((open, close, v))
    });
    let Some((open, close, v)) = block else {
        let plan = text.trim().to_owned();
        return PlanAnswer {
            questions: questions_from_markdown(&plan),
            plan,
            proposals: Vec::new(),
        };
    };
    let mut kept: Vec<&str> = lines[..open].to_vec();
    kept.extend_from_slice(&lines[(close + 1).min(lines.len())..]);
    while kept.last().is_some_and(|l| l.trim().is_empty()) {
        kept.pop();
    }
    let plan = kept.join("\n").trim().to_owned();
    let questions = if v.get("questions").is_some() {
        questions_from_json(&v)
    } else {
        questions_from_markdown(&plan)
    };
    PlanAnswer {
        questions,
        proposals: ["harness", "browserTest"]
            .into_iter()
            .filter_map(|k| proposal(&v, k))
            .collect(),
        plan,
    }
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
    fn docs_and_test_notes_come_from_json_or_their_section() {
        let json = "x\n```json\n{\"findings\":[],\"notes\":\"Cần cập nhật README\"}\n```";
        assert_eq!(review_notes(json).as_deref(), Some("Cần cập nhật README"));
        let list = "{\"findings\":[],\"notes\":[\"README\",\"test e2e\"]}";
        assert_eq!(review_notes(list).as_deref(), Some("- README\n- test e2e"));
        let ja = "## レビュー結果\n- 🟠 a.ts:1 x\n## ドキュメント・テスト更新判定\n- README: 不要\n- テスト: 追加推奨\n### 補足\nok\n## まとめ\nend\n```json\n{\"findings\":[]}\n```";
        assert_eq!(
            review_notes(ja).as_deref(),
            Some("- README: 不要\n- テスト: 追加推奨\n### 補足\nok")
        );
        assert_eq!(review_notes("## Kết quả\nKhông có lỗi nào."), None);
        assert_eq!(review_notes("{\"findings\":[],\"notes\":\"  \"}"), None);
    }

    #[test]
    fn plan_json_block_gives_questions_and_proposals_and_leaves_the_plan() {
        let text = "## Nguyên nhân\nx\n\n## Rủi ro\n- r\n\n```json\n{\"questions\":[{\"text\":\"Giữ API cũ?\",\"options\":[\"Có\",\" \",\"Không\"]},\"Đổi tên?\",{\"text\":\"\"}],\
                    \"harness\":{\"recommend\":true,\"reason\":\"logic thuần\",\"checks\":[\"tính tổng\",\"rỗng\"]},\
                    \"browserTest\":{\"recommend\":false,\"reason\":\"không có UI\",\"scenario\":\"\"}}\n```\n";
        let a = plan_answer(text);
        assert_eq!(a.plan, "## Nguyên nhân\nx\n\n## Rủi ro\n- r");
        assert_eq!(a.questions.len(), 2);
        assert_eq!(a.questions[0].options, vec!["Có", "Không"]);
        assert_eq!(a.questions[1].text, "Đổi tên?");
        assert_eq!(a.proposals.len(), 2);
        assert_eq!(a.proposals[0].step, "harness");
        assert!(a.proposals[0].recommend);
        assert_eq!(a.proposals[0].checks, "- tính tổng\n- rỗng");
        assert_eq!(a.proposals[1].step, "browserTest");
        assert!(!a.proposals[1].recommend);
        assert_eq!(a.proposals[1].reason, "không có UI");
        // other code blocks stay in the plan
        let code = "## Đề xuất\n```ts\nconst a = 1;\n```";
        assert_eq!(plan_answer(code).plan, code);
    }

    #[test]
    fn plan_questions_fall_back_to_the_question_section() {
        let text = "## Nguyên nhân\nVì sao?\n## Rủi ro và câu hỏi\n- Rủi ro: chậm.\n- **Có cần giữ API cũ?**\n2. Dùng lib nào?\n## Khác\nx?";
        let a = plan_answer(text);
        assert_eq!(a.plan, text);
        let qs: Vec<&str> = a.questions.iter().map(|q| q.text.as_str()).collect();
        assert_eq!(qs, vec!["Có cần giữ API cũ?", "Dùng lib nào?"]);
        assert!(a.proposals.is_empty());
        // a json block without questions: the markdown ones count
        let mixed = format!("{text}\n```json\n{{\"harness\": false}}\n```");
        let a = plan_answer(&mixed);
        assert_eq!(a.questions.len(), 2);
        assert_eq!(a.proposals.len(), 1);
        assert!(!a.proposals[0].recommend);
        assert_eq!(plan_answer("## Rủi ro\nkhông").questions, vec![]);
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
