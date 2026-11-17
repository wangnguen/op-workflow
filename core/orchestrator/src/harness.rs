//! Step 5, Harness (PLAN.md §12 part 5e): a throwaway test harness for the task, built and run
//! outside the repository. The AI builds it once in the task's scratch folder
//! (`<data dir>/scratch/<task>`, never `%TEMP%`: the EDR locks new exes run from there), reusing
//! the repository's own harness library when something fits; after that the app runs it again by
//! itself (no tokens) after every Sửa lỗi. The harness prints one line per case,
//! `PASS: <case>` or `FAIL: <case> — <why>`, and exits non-zero when a case fails; a FAIL becomes
//! a finding the user picks for the AI to fix, like a review's.
//!
//! Each repository has its own library (`repos.<repo>.harnessDir`, default
//! `C:\tmp\<repo name>-harness`) with a `README.md` index; a harness worth keeping is copied
//! there with one index line. Another repository's library is never read.

use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

use opw_agents::text::{self, RawFinding};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Index of a harness library.
pub const README: &str = "README.md";
/// Cases kept in the step's view (the log has them all).
const CASES_SHOWN: usize = 40;

/// What the AI said at the end of the Harness step (its last ```json block).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Answer {
    /// `pass` or `fail` as the AI saw it; the app's own run decides.
    pub passed: bool,
    pub summary: String,
    /// Command (program + arguments) that runs the harness from the scratch folder.
    pub run: Vec<String>,
    /// The broken copy made the harness FAIL (`None` = not reported).
    pub negative: Option<(bool, String)>,
    /// Worth keeping in the repository's library.
    pub library: Option<LibraryPick>,
    /// Files the cases are about (`case name` → repo path), for the findings.
    pub files: Vec<(String, String)>,
}

/// A harness the AI found worth reusing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LibraryPick {
    /// Folder name in the library (`org-permissions`).
    pub name: String,
    /// One line for the README.
    pub summary: String,
    /// Files to copy, relative to the scratch folder.
    pub files: Vec<String>,
}

/// One case as the harness printed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Case {
    pub name: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

/// A run of the harness command, read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    pub cases: Vec<Case>,
    /// Exit code 0 and no FAIL line.
    pub passed: bool,
    /// The command printed no PASS / FAIL line at all.
    pub silent: bool,
}

/// The library of `repo`: `repos.<repo>.harnessDir` when set, else the default.
pub fn library_dir(configured: Option<&str>, repo: &Path) -> PathBuf {
    match configured.map(str::trim).filter(|d| !d.is_empty()) {
        Some(dir) => opw_config::expand_home(dir),
        None => default_library(repo),
    }
}

/// `C:\tmp\<repo name>-harness` (`~/tmp/…` off Windows): outside the repo and outside `%TEMP%`.
pub fn default_library(repo: &Path) -> PathBuf {
    let name = repo
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".into());
    let root = if cfg!(windows) {
        PathBuf::from("C:\\tmp")
    } else {
        dirs::home_dir().unwrap_or_default().join("tmp")
    };
    root.join(format!("{name}-harness"))
}

/// Create the library with its README the first time a task of the repo uses one.
pub fn ensure_library(dir: &Path, repo_name: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let readme = dir.join(README);
    if !readme.exists() {
        std::fs::write(
            &readme,
            format!(
                "# Harness library of {repo_name}\n\n\
                 Test harnesses built by op-workflow for tasks of this repository, kept to be \
                 reused. One folder per harness; each line below says what it checks.\n\n"
            ),
        )?;
    }
    Ok(())
}

fn string(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned()
}

/// Read the last ```json block holding `result` and `run`.
pub fn answer(text: &str) -> Option<Answer> {
    let v = text::fenced_blocks(text).into_iter().rev().find_map(|b| {
        let v: Value = serde_json::from_str(b.body.trim()).ok()?;
        let v = v.get("harness").cloned().unwrap_or(v);
        (v.get("result").is_some() && v.get("run").is_some()).then_some(v)
    })?;
    let run = match v.get("run") {
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        Some(Value::String(s)) => crate::checks::split_command(s),
        _ => Vec::new(),
    };
    let files = v
        .get("cases")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|c| {
            let file = string(c, "file");
            (!file.is_empty()).then(|| (string(c, "name"), file))
        })
        .collect();
    let negative = v.get("negativeControl").filter(|n| n.is_object()).map(|n| {
        (
            n.get("ok").and_then(Value::as_bool).unwrap_or(false),
            string(n, "detail"),
        )
    });
    let library = v
        .get("library")
        .filter(|l| l.get("save").and_then(Value::as_bool).unwrap_or(false));
    let library = library.and_then(|l| {
        let name = string(l, "name");
        let files: Vec<String> = l
            .get("files")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        (!name.is_empty() && !files.is_empty()).then(|| LibraryPick {
            name,
            summary: string(l, "summary"),
            files,
        })
    });
    Some(Answer {
        passed: string(&v, "result").eq_ignore_ascii_case("pass"),
        summary: string(&v, "summary"),
        run,
        negative,
        library,
        files,
    })
}

/// `PASS: name`, `FAIL: name — why` (also `PASS name`, `[FAIL] name: why`).
static CASE_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*\[?(PASS|FAIL)\]?[:\s]\s*(.+?)\s*$").expect("valid regex"));

/// Read what the harness printed and how it exited.
pub fn read(code: Option<i32>, output: &str) -> Outcome {
    let mut cases = Vec::new();
    for line in output.lines() {
        let Some(c) = CASE_LINE.captures(line) else {
            continue;
        };
        let ok = &c[1] == "PASS";
        let rest = c[2].trim();
        let (name, detail) = [" — ", " -- ", " - ", ": "]
            .iter()
            .find_map(|sep| rest.split_once(sep))
            .filter(|_| !ok)
            .map_or((rest, ""), |(n, d)| (n.trim(), d.trim()));
        cases.push(Case {
            name: name.to_owned(),
            ok,
            detail: detail.to_owned(),
        });
    }
    let silent = cases.is_empty();
    let passed = code == Some(0) && cases.iter().all(|c| c.ok);
    Outcome {
        cases,
        passed,
        silent,
    }
}

/// Failed cases as findings for the findings gate. `files` = case name → repo path from the AI.
pub fn findings(outcome: &Outcome, files: &[(String, String)], output: &str) -> Vec<RawFinding> {
    let file_of = |name: &str| {
        files
            .iter()
            .find(|(n, _)| n == name)
            .map_or_else(|| "harness".to_owned(), |(_, f)| f.clone())
    };
    let mut out: Vec<RawFinding> = outcome
        .cases
        .iter()
        .filter(|c| !c.ok)
        .map(|c| RawFinding {
            severity: "high".into(),
            file: file_of(&c.name),
            line: None,
            title: format!("Harness FAIL: {}", c.name),
            detail: c.detail.clone(),
            suggestion: String::new(),
        })
        .collect();
    // failed without saying which case: one finding with the end of the output
    if out.is_empty() && !outcome.passed {
        let tail: Vec<&str> = output.lines().rev().take(15).collect();
        let tail: Vec<&str> = tail.into_iter().rev().collect();
        out.push(RawFinding {
            severity: "high".into(),
            file: "harness".into(),
            line: None,
            title: "Harness FAIL".into(),
            detail: tail.join("\n"),
            suggestion: String::new(),
        });
    }
    out
}

/// "PASS 5/5", "FAIL 2/5".
pub fn tally(outcome: &Outcome) -> String {
    let ok = outcome.cases.iter().filter(|c| c.ok).count();
    let word = if outcome.passed { "PASS" } else { "FAIL" };
    if outcome.cases.is_empty() {
        word.to_owned()
    } else {
        format!("{word} {ok}/{}", outcome.cases.len())
    }
}

/// At most [`CASES_SHOWN`] cases, failed ones first.
pub fn shown(outcome: &Outcome) -> Vec<Case> {
    let mut cases = outcome.cases.clone();
    cases.sort_by_key(|c| c.ok);
    cases.truncate(CASES_SHOWN);
    cases
}

/// A plain relative path inside the scratch folder (no `..`, no drive, no root).
fn safe_relative(file: &str) -> Option<PathBuf> {
    let p = Path::new(file.trim());
    let ok = !file.trim().is_empty() && p.components().all(|c| matches!(c, Component::Normal(_)));
    ok.then(|| p.to_path_buf())
}

/// `org-permissions` from whatever the AI wrote (`Org Permissions!`).
fn folder_name(name: &str) -> String {
    let mut out = String::new();
    for ch in name.trim().to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').chars().take(60).collect()
}

/// Copy a harness from the scratch folder into the library and add its README line. Returns the
/// folder it went to (`<name>`, or `<name>-<task>` when that name is taken).
pub fn save(
    library: &Path,
    scratch: &Path,
    pick: &LibraryPick,
    task_id: &str,
    date: &str,
) -> Result<PathBuf, String> {
    let base = folder_name(&pick.name);
    if base.is_empty() {
        return Err("tên harness không hợp lệ".into());
    }
    let mut name = base.clone();
    if library.join(&name).exists() {
        name = format!("{base}-{}", folder_name(task_id));
    }
    let dest = library.join(&name);
    let mut copied = 0;
    for file in &pick.files {
        let Some(rel) = safe_relative(file) else { continue };
        let from = scratch.join(&rel);
        if !from.is_file() {
            continue;
        }
        let to = dest.join(&rel);
        if let Some(dir) = to.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::copy(&from, &to).map_err(|e| e.to_string())?;
        copied += 1;
    }
    if copied == 0 {
        return Err("không có file nào của harness để chép".into());
    }
    let readme = library.join(README);
    let mut text = std::fs::read_to_string(&readme).unwrap_or_default();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    let summary = pick.summary.replace('\n', " ");
    text.push_str(&format!(
        "- [{name}]({name}/) — {} ({task_id}, {date})\n",
        summary.trim()
    ));
    std::fs::write(&readme, text).map_err(|e| e.to_string())?;
    Ok(dest)
}

/// "Bỏ khỏi thư viện": delete the folder a task saved and its README line.
pub fn drop_saved(library: &Path, saved: &Path) -> Result<(), String> {
    let name = saved
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|_| saved.parent() == Some(library))
        .ok_or_else(|| "thư mục không nằm trong thư viện harness".to_owned())?;
    if saved.is_dir() {
        std::fs::remove_dir_all(saved).map_err(|e| e.to_string())?;
    }
    let readme = library.join(README);
    if let Ok(text) = std::fs::read_to_string(&readme) {
        let marker = format!("]({name}/)");
        let kept: Vec<&str> = text.lines().filter(|l| !l.contains(&marker)).collect();
        let mut out = kept.join("\n");
        out.push('\n');
        std::fs::write(&readme, out).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_answer_and_the_printed_cases() {
        let text = "Đã dựng harness.\n```json\n{\"result\": \"fail\", \"summary\": \"2 ca\", \"run\": \"node run.js --all\", \
                    \"cases\": [{\"name\": \"empty org\", \"ok\": false, \"file\": \"src/org.ts\"}], \
                    \"negativeControl\": {\"ok\": true, \"detail\": \"bản hỏng FAIL\"}, \
                    \"library\": {\"save\": true, \"name\": \"Org Permissions\", \"summary\": \"quyền org\", \"files\": [\"run.js\"]}}\n```";
        let a = answer(text).unwrap();
        assert!(!a.passed);
        assert_eq!(a.run, ["node", "run.js", "--all"]);
        assert_eq!(a.negative, Some((true, "bản hỏng FAIL".into())));
        assert_eq!(a.files, [("empty org".to_owned(), "src/org.ts".to_owned())]);
        assert_eq!(
            a.library.as_ref().map(|l| l.name.as_str()),
            Some("Org Permissions")
        );
        assert!(answer("no json").is_none());
        assert!(answer("```json\n{\"result\":\"pass\"}\n```").is_none());

        let out = "build ok\nPASS: owner keeps access\nFAIL: empty org — threw TypeError\n[PASS] guest\n";
        let o = read(Some(1), out);
        assert!(!o.passed && !o.silent);
        assert_eq!(o.cases.len(), 3);
        assert_eq!(o.cases[1].name, "empty org");
        assert_eq!(o.cases[1].detail, "threw TypeError");
        assert_eq!(tally(&o), "FAIL 2/3");
        let f = findings(&o, &a.files, out);
        assert_eq!(f.len(), 1);
        assert_eq!(
            (f[0].file.as_str(), f[0].title.as_str()),
            ("src/org.ts", "Harness FAIL: empty org")
        );
        assert_eq!(shown(&o)[0].name, "empty org");
        // exit code alone decides when nothing is printed
        let quiet = read(Some(2), "Error: cannot find module\n");
        assert!(quiet.silent && !quiet.passed);
        assert_eq!(
            findings(&quiet, &[], "Error: cannot find module\n")[0].title,
            "Harness FAIL"
        );
        assert!(read(Some(0), "PASS: a\n").passed);
    }

    #[test]
    fn saves_to_the_library_and_drops_it_again() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = tmp.path().join("web-harness");
        let scratch = tmp.path().join("scratch");
        std::fs::create_dir_all(scratch.join("lib")).unwrap();
        std::fs::write(scratch.join("run.js"), "console.log('PASS: a')").unwrap();
        std::fs::write(scratch.join("lib").join("h.js"), "x").unwrap();
        ensure_library(&lib, "web").unwrap();
        let pick = LibraryPick {
            name: "Org Permissions".into(),
            summary: "quyền org\nnhiều dòng".into(),
            files: vec![
                "run.js".into(),
                "lib/h.js".into(),
                "../secret".into(),
                "C:/x".into(),
            ],
        };
        let saved = save(&lib, &scratch, &pick, "WP-1", "2026-11-17").unwrap();
        assert_eq!(saved, lib.join("org-permissions"));
        assert!(saved.join("lib").join("h.js").is_file());
        let readme = std::fs::read_to_string(lib.join(README)).unwrap();
        assert!(readme.starts_with("# Harness library of web"));
        assert!(
            readme
                .contains("- [org-permissions](org-permissions/) — quyền org nhiều dòng (WP-1, 2026-11-17)")
        );
        // the name is taken: the task id is added
        let again = save(&lib, &scratch, &pick, "WP-2", "2026-11-18").unwrap();
        assert_eq!(again, lib.join("org-permissions-wp-2"));
        drop_saved(&lib, &saved).unwrap();
        assert!(!saved.exists());
        let readme = std::fs::read_to_string(lib.join(README)).unwrap();
        assert!(!readme.contains("(org-permissions/)") && readme.contains("(org-permissions-wp-2/)"));
        assert!(drop_saved(&lib, tmp.path()).is_err());
        assert_eq!(
            library_dir(Some("  "), Path::new("C:/code/web")),
            default_library(Path::new("C:/code/web"))
        );
        assert!(default_library(Path::new("/x/web")).ends_with("web-harness"));
    }
}
