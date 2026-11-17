//! Step 11, Test trình duyệt (PLAN.md §12 part 5f): the AI tries the change in Chrome through
//! the Chrome DevTools MCP server (and nothing else), following the scenario the analysis
//! proposed and the brief's acceptance criteria. It starts the app the way the repository runs
//! it locally; whatever it started is killed when the step ends (part 5d). Every NG case has a
//! screenshot in `<task>/browser-test/`; NG cases become findings the user picks for the AI to
//! fix in step 9, after which the test runs again.

use opw_agents::text::{self, RawFinding};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Folder of the screenshots, in the task folder.
pub const DIR: &str = "browser-test";
/// Cases kept in the step's view.
const CASES_SHOWN: usize = 40;

/// One case as the AI reported it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Case {
    pub name: String,
    pub ok: bool,
    pub detail: String,
    /// Absolute path of its screenshot, when there is one on disk.
    pub screenshot: Option<String>,
    /// Repository file the case is about (for the finding).
    pub file: Option<String>,
}

/// What the AI said at the end of the step (its last ```json block).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Answer {
    pub summary: String,
    pub url: String,
    pub cases: Vec<Case>,
}

impl Answer {
    /// OK when every case is; no case at all is not a test.
    pub fn passed(&self) -> bool {
        !self.cases.is_empty() && self.cases.iter().all(|c| c.ok)
    }
}

fn string(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned()
}

/// Read the last ```json block with `cases`. A screenshot the AI named but that is not on disk
/// is dropped (it never got written).
pub fn answer(text: &str) -> Option<Answer> {
    let v = text::fenced_blocks(text).into_iter().rev().find_map(|b| {
        let v: Value = serde_json::from_str(b.body.trim()).ok()?;
        let v = v.get("browserTest").cloned().unwrap_or(v);
        v.get("cases").is_some_and(Value::is_array).then_some(v)
    })?;
    let cases = v
        .get("cases")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|c| Case {
            name: string(c, "name"),
            ok: c.get("ok").and_then(Value::as_bool).unwrap_or(false),
            detail: string(c, "detail"),
            screenshot: Some(string(c, "screenshot"))
                .filter(|s| !s.is_empty() && std::path::Path::new(s).is_file()),
            file: Some(string(c, "file")).filter(|f| !f.is_empty()),
        })
        .filter(|c| !c.name.is_empty())
        .collect();
    Some(Answer {
        summary: string(&v, "summary"),
        url: string(&v, "url"),
        cases,
    })
}

/// NG cases as findings for the findings gate.
pub fn findings(answer: &Answer) -> Vec<RawFinding> {
    answer
        .cases
        .iter()
        .filter(|c| !c.ok)
        .map(|c| {
            let mut detail = c.detail.clone();
            if let Some(shot) = &c.screenshot {
                detail.push_str(&format!("\nẢnh: {shot}"));
            }
            RawFinding {
                severity: "high".into(),
                file: c.file.clone().unwrap_or_else(|| "browser".into()),
                line: None,
                title: format!("Test trình duyệt NG: {}", c.name),
                detail: detail.trim().to_owned(),
                suggestion: String::new(),
            }
        })
        .collect()
}

/// "OK 4/4", "NG 1/4".
pub fn tally(answer: &Answer) -> String {
    let ok = answer.cases.iter().filter(|c| c.ok).count();
    let word = if answer.passed() { "OK" } else { "NG" };
    format!("{word} {ok}/{}", answer.cases.len())
}

/// At most [`CASES_SHOWN`] cases, NG ones first.
pub fn shown(answer: &Answer) -> Vec<Case> {
    let mut cases = answer.cases.clone();
    cases.sort_by_key(|c| c.ok);
    cases.truncate(CASES_SHOWN);
    cases
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_cases_and_keeps_only_screenshots_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let shot = tmp.path().join("1.png");
        std::fs::write(&shot, b"png").unwrap();
        let shot = shot.to_string_lossy().replace('\\', "\\\\");
        let text = format!(
            "Đã thử.\n```json\n{{\"result\":\"ng\",\"summary\":\"1 ca NG\",\"url\":\"http://localhost:5173/files\",\
             \"cases\":[{{\"name\":\"user chưa có org\",\"ok\":false,\"detail\":\"trang trắng\",\"screenshot\":\"{shot}\",\"file\":\"web/src/Files.tsx\"}},\
             {{\"name\":\"org có quyền\",\"ok\":true,\"screenshot\":\"C:/nope/2.png\"}},{{\"ok\":true}}]}}\n```"
        );
        let a = answer(&text).unwrap();
        assert_eq!(a.url, "http://localhost:5173/files");
        assert_eq!(a.cases.len(), 2);
        assert!(a.cases[0].screenshot.is_some() && a.cases[1].screenshot.is_none());
        assert!(!a.passed());
        assert_eq!(tally(&a), "NG 1/2");
        let f = findings(&a);
        assert_eq!(f.len(), 1);
        assert_eq!(
            (f[0].file.as_str(), f[0].title.as_str()),
            ("web/src/Files.tsx", "Test trình duyệt NG: user chưa có org")
        );
        assert!(f[0].detail.starts_with("trang trắng\nẢnh: "));
        assert_eq!(shown(&a)[0].name, "user chưa có org");
        assert!(answer("```json\n{\"result\":\"ok\"}\n```").is_none());
        assert!(!Answer::default().passed());
    }
}
