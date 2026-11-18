//! Step 11, Test trình duyệt (PLAN.md §12 part 5f): the AI tries the change in Chrome through
//! the Chrome DevTools MCP server (and nothing else), following the scenario the analysis
//! proposed and the brief's acceptance criteria. It starts the app the way the repository runs
//! it locally; whatever it started is killed when the step ends (part 5d). Every NG case has a
//! screenshot in `<task>/browser-test/`; NG cases become findings the user picks for the AI to
//! fix in step 9, after which the test runs again.

use opw_agents::text::{self, RawFinding};
use opw_domain::finding::fingerprint;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Folder of the screenshots, in the task folder.
pub const DIR: &str = "browser-test";
/// Cases kept in the step's view.
const CASES_SHOWN: usize = 40;
/// Changes to real data kept in the step's view.
const CHANGES_SHOWN: usize = 20;
/// Title of the finding of an NG case: this, then the case's name.
pub const NG_TITLE: &str = "Test trình duyệt NG: ";

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
    /// The AI could not run it (no data, an action the page did not take, out of budget): not
    /// an NG, nothing to fix; `detail` says why.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub untested: bool,
}

/// What the AI said at the end of the step (its last ```json block).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Answer {
    pub summary: String,
    pub url: String,
    pub cases: Vec<Case>,
    /// Real data the AI changed while testing (where, and whether it undid it).
    pub changes: Vec<String>,
}

impl Answer {
    /// OK when every case ran and is OK; no case at all is not a test.
    pub fn passed(&self) -> bool {
        !self.cases.is_empty() && self.cases.iter().all(|c| c.ok && !c.untested)
    }

    /// Cases that ran and failed.
    pub fn ng(&self) -> usize {
        self.cases.iter().filter(|c| !c.ok && !c.untested).count()
    }

    /// Cases the AI could not run.
    pub fn untested(&self) -> Vec<&Case> {
        self.cases.iter().filter(|c| c.untested).collect()
    }
}

/// The number a case's name starts with ("6. Breadcrumb…", "6) …", "Ca 6: …"): the AI words a
/// case differently from one run to the next, but keeps the scenario's numbering.
pub fn case_number(name: &str) -> Option<u32> {
    let name = name.trim();
    let rest = ["Ca ", "ca ", "Case ", "case "]
        .iter()
        .find_map(|p| name.strip_prefix(p))
        .unwrap_or(name);
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let after = rest[digits.len()..].chars().next();
    if digits.is_empty() || !matches!(after, None | Some('.' | ')' | ':' | ' ' | '-')) {
        return None;
    }
    digits.parse().ok()
}

/// The fingerprint of a finding this step reported: by the case's number when it has one (the
/// same case worded differently next run is the same finding), else as any finding.
pub fn fingerprint_of(r: &RawFinding) -> String {
    match r.title.strip_prefix(NG_TITLE).and_then(case_number) {
        Some(n) => fingerprint("browser", &format!("ca {n}")),
        None => fingerprint(&r.file, &r.title),
    }
}

/// Fingerprints of the cases the AI could not run: their findings from an earlier run are
/// neither fixed nor confirmed.
pub fn untested_fingerprints(answer: &Answer) -> Vec<String> {
    answer
        .untested()
        .into_iter()
        .map(|c| {
            fingerprint_of(&RawFinding {
                severity: String::new(),
                file: c.file.clone().unwrap_or_else(|| "browser".into()),
                line: None,
                title: format!("{NG_TITLE}{}", c.name),
                detail: String::new(),
                suggestion: String::new(),
            })
        })
        .collect()
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
        .map(|c| {
            let untested = c.get("untested").and_then(Value::as_bool).unwrap_or(false);
            Case {
                name: string(c, "name"),
                ok: !untested && c.get("ok").and_then(Value::as_bool).unwrap_or(false),
                detail: string(c, "detail"),
                screenshot: Some(string(c, "screenshot"))
                    .filter(|s| !s.is_empty() && std::path::Path::new(s).is_file()),
                file: Some(string(c, "file")).filter(|f| !f.is_empty()),
                untested,
            }
        })
        .filter(|c| !c.name.is_empty())
        .collect();
    let changes = v
        .get("changes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    Some(Answer {
        summary: string(&v, "summary"),
        url: string(&v, "url"),
        cases,
        changes,
    })
}

/// At most [`CHANGES_SHOWN`] changes to real data.
pub fn shown_changes(answer: &Answer) -> Vec<String> {
    answer.changes.iter().take(CHANGES_SHOWN).cloned().collect()
}

/// NG cases as findings for the findings gate (a case not tested is not one).
pub fn findings(answer: &Answer) -> Vec<RawFinding> {
    answer
        .cases
        .iter()
        .filter(|c| !c.ok && !c.untested)
        .map(|c| {
            let mut detail = c.detail.clone();
            if let Some(shot) = &c.screenshot {
                detail.push_str(&format!("\nẢnh: {shot}"));
            }
            RawFinding {
                severity: "high".into(),
                file: c.file.clone().unwrap_or_else(|| "browser".into()),
                line: None,
                title: format!("{NG_TITLE}{}", c.name),
                detail: detail.trim().to_owned(),
                suggestion: String::new(),
            }
        })
        .collect()
}

/// "OK 4/4", "NG 1/4", "OK 4/4, 2 chưa test" (out of the cases that ran).
pub fn tally(answer: &Answer) -> String {
    let untested = answer.untested().len();
    let ran = answer.cases.len() - untested;
    if ran == 0 {
        return format!("chưa test được ca nào ({untested} ca)");
    }
    let ok = answer.cases.iter().filter(|c| c.ok).count();
    let word = if answer.ng() > 0 { "NG" } else { "OK" };
    let mut out = format!("{word} {ok}/{ran}");
    if untested > 0 {
        out.push_str(&format!(", {untested} chưa test"));
    }
    out
}

/// At most [`CASES_SHOWN`] cases: NG first, then the ones not tested, then OK.
pub fn shown(answer: &Answer) -> Vec<Case> {
    let mut cases = answer.cases.clone();
    cases.sort_by_key(|c| match (c.ok, c.untested) {
        (false, false) => 0,
        (_, true) => 1,
        (true, false) => 2,
    });
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
    fn a_case_not_tested_is_no_ng_and_the_same_case_keeps_its_finding() {
        let text = "```json\n{\"result\":\"ng\",\"cases\":[\
            {\"name\":\"1. Danh sách\",\"ok\":true},\
            {\"name\":\"4. Xuất Excel\",\"ok\":false,\"detail\":\"không bắt được file\",\"untested\":true},\
            {\"name\":\"6. Xoá hàng loạt\",\"ok\":false,\"untested\":true}],\
            \"changes\":[\"Thêm giangtest_bl5 vào người nhận (chưa bấm 登録)\", \" \"]}\n```";
        let a = answer(text).unwrap();
        assert_eq!((a.ng(), a.untested().len()), (0, 2));
        assert!(!a.passed());
        assert!(findings(&a).is_empty());
        assert_eq!(tally(&a), "OK 1/1, 2 chưa test");
        assert_eq!(a.changes, ["Thêm giangtest_bl5 vào người nhận (chưa bấm 登録)"]);
        // NG first, then not tested, then OK
        assert_eq!(shown(&a)[0].name, "4. Xuất Excel");
        assert_eq!(shown(&a)[2].name, "1. Danh sách");
        let none = answer("```json\n{\"cases\":[{\"name\":\"x\",\"untested\":true}]}\n```").unwrap();
        assert_eq!(tally(&none), "chưa test được ca nào (1 ca)");

        // the same case worded otherwise next run is the same finding
        let ng = |name: &str, file: &str| RawFinding {
            severity: "high".into(),
            file: file.into(),
            line: None,
            title: format!("{NG_TITLE}{name}"),
            detail: String::new(),
            suggestion: String::new(),
        };
        let a6 = fingerprint_of(&ng(
            "6. Breadcrumb, xoá hàng loạt, đổi role: hiển thị lại",
            "a.tsx",
        ));
        assert_eq!(
            a6,
            fingerprint_of(&ng("6. Breadcrumb/thư mục cha, xoá hàng loạt", "b.tsx"))
        );
        assert_eq!(untested_fingerprints(&a)[1], fingerprint_of(&ng("6) khác", "x")));
        assert_ne!(a6, fingerprint_of(&ng("4. Xuất Excel", "a.tsx")));
        // without a number: the name tells them apart, as before
        assert_eq!(
            fingerprint_of(&ng("chưa có org", "a.ts")),
            fingerprint("a.ts", &format!("{NG_TITLE}chưa có org"))
        );
        assert_eq!(case_number("Ca 12: abc"), Some(12));
        assert_eq!(case_number("3a. x"), None);
        assert_eq!(case_number("abc"), None);
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
