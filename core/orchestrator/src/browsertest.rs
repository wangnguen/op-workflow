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

/// Claude Code's memory of `repo`: `<projects>/<repo path, every other character than a letter
/// or digit as '->/memory`, when it has one. The drive letter's case may differ from Claude's.
pub fn repo_memory(projects: &std::path::Path, repo: &std::path::Path) -> Option<std::path::PathBuf> {
    let full = repo.to_string_lossy();
    let name: String = full
        .trim_end_matches(['/', '\\'])
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let dir = std::fs::read_dir(projects)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(&name))
        })?;
    let memory = dir.join("memory");
    memory.is_dir().then_some(memory)
}

/// Where Claude Code keeps its projects: `$CLAUDE_CONFIG_DIR/projects` or `~/.claude/projects`.
pub fn claude_projects() -> Option<std::path::PathBuf> {
    match std::env::var_os("CLAUDE_CONFIG_DIR") {
        Some(dir) => Some(std::path::PathBuf::from(dir).join("projects")),
        None => dirs::home_dir().map(|h| h.join(".claude").join("projects")),
    }
}

/// The AI stopped to ask the user (`{"result": "ask", "question": …, "url": …}`, the last json
/// block): (question, page).
pub fn question(text: &str) -> Option<(String, String)> {
    text::fenced_blocks(text).into_iter().rev().find_map(|b| {
        let v: Value = serde_json::from_str(b.body.trim()).ok()?;
        let v = v.get("browserTest").cloned().unwrap_or(v);
        let asks = v.get("result").and_then(Value::as_str) == Some("ask");
        let q = string(&v, "question");
        (asks && !q.is_empty()).then(|| (q, string(&v, "url")))
    })
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

    #[test]
    fn the_repo_memory_is_found_whatever_the_drive_letter_case() {
        let tmp = tempfile::tempdir().unwrap();
        let memory = tmp.path().join("c--Users-me-Desktop-op-workflow").join("memory");
        std::fs::create_dir_all(&memory).unwrap();
        let repo = std::path::Path::new("C:\\Users\\me\\Desktop\\op-workflow\\");
        assert_eq!(repo_memory(tmp.path(), repo), Some(memory));
        assert_eq!(
            repo_memory(tmp.path(), std::path::Path::new("C:/Users/me/other")),
            None
        );
        // a project folder without notes
        std::fs::create_dir_all(tmp.path().join("C--x")).unwrap();
        assert_eq!(repo_memory(tmp.path(), std::path::Path::new("C:/x")), None);
    }

    #[test]
    fn the_ai_may_stop_to_ask_for_a_login() {
        let text = "Trang yêu cầu đăng nhập.\n```json\n{\"result\":\"ask\",\"question\":\"Đăng nhập tài khoản test trong cửa sổ Chrome\",\"url\":\"http://localhost:5173/login\"}\n```";
        assert_eq!(
            question(text),
            Some((
                "Đăng nhập tài khoản test trong cửa sổ Chrome".to_owned(),
                "http://localhost:5173/login".to_owned()
            ))
        );
        assert!(answer(text).is_none());
        assert_eq!(
            question("```json\n{\"result\":\"ask\",\"question\":\" \"}\n```"),
            None
        );
        assert_eq!(question("```json\n{\"result\":\"ok\",\"cases\":[]}\n```"), None);
    }
}
