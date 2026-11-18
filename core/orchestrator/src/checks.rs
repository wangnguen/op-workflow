//! Step 4, Kiểm tra (PLAN.md §12 part 5c): type check, lint and prettier on the files the task
//! changed, run by the app (no tokens). Commands come from `repos.<repo>.checks` in config.json;
//! without them the app guesses from each package that holds a changed file (`tsconfig.json`,
//! an eslint or prettier config). Errors count only in the files the task touched: old errors
//! elsewhere in the repo are reported as ignored, so a repo whose lint is not clean still passes.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use opw_agents::env::Account;
use opw_agents::run::{RunSpec, run};
use opw_vcs::Git;
use regex::Regex;

/// Which output format a check prints (how its errors are read).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckKind {
    Tsc,
    Eslint,
    Prettier,
    /// A configured command the app does not know: its exit code decides.
    Other,
    /// The task's harness (step 5), run from its scratch folder (`dir` is absolute).
    Harness,
}

/// One command to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    /// `tsc · web`, or the configured command.
    pub label: String,
    pub program: String,
    pub args: Vec<String>,
    pub kind: CheckKind,
    /// Folder it runs in, relative to the repo (`""` = the repo root, `web`).
    pub dir: String,
    /// Fed to the command (prettier `--stdin-filepath` on a file's old text).
    pub stdin: Option<String>,
}

/// What one check found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckResult {
    pub label: String,
    /// Error lines in files the task changed (or the output tail of an unknown failing command).
    pub errors: Vec<String>,
    /// Errors in files the task did not touch: ignored.
    pub old: usize,
    /// The command could not run or failed without a readable error.
    pub broken: Option<String>,
    /// Combined output, for the task's `checks/` log.
    pub output: String,
}

impl CheckResult {
    pub fn ok(&self) -> bool {
        self.errors.is_empty() && self.broken.is_none()
    }
}

/// Runs a check; tests swap in a fake.
pub trait CheckRunner: Send + Sync {
    /// Exit code (`None` = stopped / timed out) and everything the command printed.
    fn run(
        &self,
        repo: &Path,
        check: &Check,
        timeout: Duration,
        stop: &AtomicBool,
    ) -> Result<(Option<i32>, String), String>;
}

/// Runs the real command (no shell: program + argument array).
#[derive(Debug, Default)]
pub struct ProcessRunner;

impl CheckRunner for ProcessRunner {
    fn run(
        &self,
        repo: &Path,
        check: &Check,
        timeout: Duration,
        stop: &AtomicBool,
    ) -> Result<(Option<i32>, String), String> {
        let program =
            resolve(&check.program).ok_or_else(|| format!("không thấy `{}` trên PATH", check.program))?;
        let spec = RunSpec {
            program,
            args: check.args.clone(),
            cwd: if check.dir.is_empty() {
                repo.to_path_buf()
            } else {
                repo.join(&check.dir)
            },
            stdin: check.stdin.clone(),
            env: vec![
                ("FORCE_COLOR".into(), "0".into()),
                ("NO_COLOR".into(), "1".into()),
            ],
            account: Account::Subscription,
            timeout,
            // tsc / eslint / prettier start nothing that outlives them; a harness may
            sweep: check.kind == CheckKind::Harness,
        };
        let mut out = String::new();
        let outcome = run(&spec, stop, &mut |line| {
            out.push_str(line);
            out.push('\n');
        })
        .map_err(|e| e.to_string())?;
        if !outcome.stderr_tail.is_empty() {
            out.push_str(&outcome.stderr_tail);
            out.push('\n');
        }
        if outcome.timed_out {
            return Err(format!("quá {} phút", timeout.as_secs() / 60));
        }
        Ok((if outcome.stopped { None } else { outcome.code }, out))
    }
}

/// `npx` → `npx.cmd` on Windows; anything with a path is used as it is.
fn resolve(program: &str) -> Option<PathBuf> {
    if program.contains(['/', '\\']) {
        return Some(PathBuf::from(program));
    }
    let names: Vec<String> = if cfg!(windows) {
        ["exe", "cmd", "bat"]
            .iter()
            .map(|e| format!("{program}.{e}"))
            .chain([program.to_owned()])
            .collect()
    } else {
        vec![program.to_owned()]
    };
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    opw_agents::cli::find_in_path(&names, &std::env::var_os("PATH").unwrap_or_default())
}

/// Files the task changed (tracked against the fork point with `base`, plus untracked ones),
/// repo-relative with `/`.
pub fn changed_files(repo: &Path, base: &str) -> Vec<String> {
    let git = Git::new(repo);
    let fork = git.merge_base(base, "HEAD").unwrap_or_else(|_| "HEAD".into());
    let mut files: Vec<String> = git
        .run(&["diff", "--name-only", "--diff-filter=d", &fork])
        .unwrap_or_default()
        .lines()
        .chain(
            git.run(&["ls-files", "--others", "--exclude-standard"])
                .unwrap_or_default()
                .lines(),
        )
        .map(|l| l.trim().replace('\\', "/"))
        .filter(|l| !l.is_empty())
        .collect();
    files.sort();
    files.dedup();
    files
}

const CODE: [&str; 8] = ["ts", "tsx", "js", "jsx", "mjs", "cjs", "mts", "cts"];
const PRETTIER: [&str; 6] = ["css", "scss", "json", "vue", "html", "less"];

fn ext(file: &str) -> &str {
    file.rsplit_once('.').map_or("", |(_, e)| e)
}

fn is_code(file: &str) -> bool {
    CODE.contains(&ext(file))
}

/// Nearest folder (repo-relative, `""` = root) above `file` with a `package.json`.
fn package_of(repo: &Path, file: &str) -> Option<String> {
    let mut dir = Path::new(file).parent();
    while let Some(d) = dir {
        let rel = d.to_string_lossy().replace('\\', "/");
        if repo.join(&rel).join("package.json").is_file() {
            return Some(rel);
        }
        dir = d.parent();
    }
    None
}

fn has_any(dir: &Path, names: &[&str]) -> bool {
    names.iter().any(|n| dir.join(n).is_file())
}

/// `eslint` / `prettier` in the package's dependencies or its own `prettier` key.
fn package_mentions(dir: &Path, name: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(dir.join("package.json")) else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    ["dependencies", "devDependencies"]
        .iter()
        .any(|k| v.get(k).and_then(|d| d.get(name)).is_some())
        || v.get(name).is_some()
}

/// Split a configured command like a shell would for plain words and "quoted parts".
pub fn split_command(command: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    for c in command.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                started = false;
            }
            (None, c) => cur.push(c),
        }
    }
    if started || !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn kind_of(words: &[String]) -> CheckKind {
    let has = |w: &str| {
        words
            .iter()
            .any(|x| x == w || x.ends_with(&format!("/{w}")) || x.ends_with(&format!("{w}.cmd")))
    };
    if has("tsc") || has("vue-tsc") {
        CheckKind::Tsc
    } else if has("eslint") {
        CheckKind::Eslint
    } else if has("prettier") {
        CheckKind::Prettier
    } else {
        CheckKind::Other
    }
}

/// The checks for this change. `configured` (`repos.<repo>.checks`) wins; `{files}` in a
/// configured command becomes the changed files. Empty = nothing to check.
pub fn plan(repo: &Path, configured: &[String], changed: &[String]) -> Vec<Check> {
    if changed.is_empty() {
        return Vec::new();
    }
    let configured: Vec<&String> = configured.iter().filter(|c| !c.trim().is_empty()).collect();
    if !configured.is_empty() {
        return configured
            .into_iter()
            .filter_map(|command| {
                let mut words = Vec::new();
                for w in split_command(command) {
                    if w == "{files}" {
                        words.extend(changed.iter().cloned());
                    } else {
                        words.push(w);
                    }
                }
                let (program, args) = words.split_first()?;
                Some(Check {
                    label: command.trim().to_owned(),
                    program: program.clone(),
                    args: if is_npx(program) {
                        npx_args(args)
                    } else {
                        args.to_vec()
                    },
                    kind: kind_of(&words),
                    dir: String::new(),
                    stdin: None,
                })
            })
            .collect();
    }
    // guess per package
    let mut packages: Vec<(String, Vec<String>)> = Vec::new();
    for file in changed {
        if !is_code(file) && !PRETTIER.contains(&ext(file)) {
            continue;
        }
        let Some(pkg) = package_of(repo, file) else {
            continue;
        };
        let rel = match pkg.as_str() {
            "" => file.clone(),
            p => file.strip_prefix(&format!("{p}/")).unwrap_or(file).to_owned(),
        };
        match packages.iter_mut().find(|(p, _)| *p == pkg) {
            Some((_, files)) => files.push(rel),
            None => packages.push((pkg, vec![rel])),
        }
    }
    let mut out = Vec::new();
    for (pkg, files) in packages {
        let dir = repo.join(&pkg);
        let name = if pkg.is_empty() {
            "repo".to_owned()
        } else {
            pkg.clone()
        };
        // `--`: npm 11 reads options after the tool's name as its own (`-p` = `--package`,
        // `--check` dropped), so the tool's options go after it
        let npx = |tool: &str, args: Vec<String>, kind| Check {
            label: format!("{tool} · {name}"),
            program: "npx".into(),
            args: ["--no".to_owned(), "--".to_owned(), tool.to_owned()]
                .into_iter()
                .chain(args)
                .collect(),
            kind,
            dir: pkg.clone(),
            stdin: None,
        };
        let code: Vec<String> = files.iter().filter(|f| is_code(f)).cloned().collect();
        if dir.join("tsconfig.json").is_file() && code.iter().any(|f| f.contains(".ts")) {
            out.push(npx(
                "tsc",
                vec!["--noEmit".into(), "-p".into(), "tsconfig.json".into()],
                CheckKind::Tsc,
            ));
        }
        let eslint = has_any(
            &dir,
            &[
                "eslint.config.js",
                "eslint.config.mjs",
                "eslint.config.cjs",
                "eslint.config.ts",
                ".eslintrc",
                ".eslintrc.js",
                ".eslintrc.cjs",
                ".eslintrc.json",
                ".eslintrc.yml",
            ],
        ) || package_mentions(&dir, "eslint");
        if eslint && !code.is_empty() {
            out.push(npx("eslint", code.clone(), CheckKind::Eslint));
        }
        let prettier = has_any(
            &dir,
            &[
                ".prettierrc",
                ".prettierrc.json",
                ".prettierrc.js",
                ".prettierrc.cjs",
                ".prettierrc.yml",
                "prettier.config.js",
                "prettier.config.mjs",
                "prettier.config.cjs",
            ],
        ) || package_mentions(&dir, "prettier");
        if prettier {
            out.push(npx(
                "prettier",
                ["--check".to_owned()].into_iter().chain(files).collect(),
                CheckKind::Prettier,
            ));
        }
    }
    out
}

fn is_npx(program: &str) -> bool {
    let name = program
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(program)
        .to_ascii_lowercase();
    matches!(name.as_str(), "npx" | "npx.cmd" | "npx.exe")
}

/// A configured `npx …` command with `--` before the tool (see `plan`), unless it has one or
/// uses npx's own `-p` / `-c` (whose values are not the tool).
fn npx_args(args: &[String]) -> Vec<String> {
    let tool = args.iter().position(|a| !a.starts_with('-'));
    let own_values = args
        .iter()
        .take(tool.unwrap_or(args.len()))
        .any(|a| matches!(a.as_str(), "-p" | "--package" | "-c" | "--call"));
    match tool {
        Some(i) if !own_values && !args.iter().any(|a| a == "--") => {
            let mut out = args[..i].to_vec();
            out.push("--".into());
            out.extend_from_slice(&args[i..]);
            out
        }
        _ => args.to_vec(),
    }
}

/// The lines (new side) the task changed in each file, from `git diff -U0` against the fork
/// point. A file that is not here (untracked, or a diff the app could not read) counts as
/// changed on every line.
#[derive(Debug, Clone, Default)]
pub struct ChangedLines(std::collections::HashMap<String, Vec<(u32, u32)>>);

static HUNK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@").expect("valid regex"));

impl ChangedLines {
    pub fn of(repo: &Path, base: &str) -> Self {
        let git = Git::new(repo);
        let fork = git.merge_base(base, "HEAD").unwrap_or_else(|_| "HEAD".into());
        let diff = git
            .run(&["diff", "-U0", "--no-color", "--no-ext-diff", &fork])
            .unwrap_or_default();
        Self::parse(&diff)
    }

    /// `+++ b/<file>` then `@@ -a,b +c,d @@` hunks; a pure deletion (`d` = 0) marks the lines
    /// around it.
    pub fn parse(diff: &str) -> Self {
        let mut map: std::collections::HashMap<String, Vec<(u32, u32)>> = std::collections::HashMap::new();
        let mut file: Option<String> = None;
        for line in diff.lines() {
            if let Some(path) = line.strip_prefix("+++ ") {
                let path = path.trim().trim_matches('"');
                file = path.strip_prefix("b/").map(|p| norm(Path::new(""), "", p));
                if let Some(f) = &file {
                    map.entry(f.clone()).or_default();
                }
            } else if let (Some(f), Some(c)) = (&file, HUNK.captures(line)) {
                let start: u32 = c[1].parse().unwrap_or(0);
                let count: u32 = c.get(2).map_or(1, |m| m.as_str().parse().unwrap_or(1));
                let range = if count == 0 {
                    (start, start + 1)
                } else {
                    (start, start + count - 1)
                };
                map.entry(f.clone()).or_default().push(range);
            }
        }
        Self(map)
    }

    /// The task changed `line` of `file` (repo-relative, as `norm` gives it).
    fn touches(&self, file: &str, line: u32) -> bool {
        self.0
            .get(file)
            .is_none_or(|ranges| ranges.iter().any(|(a, b)| (*a..=*b).contains(&line)))
    }
}

static TSC_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(.+?)\(\d+,\d+\): error TS\d+").expect("valid regex"));
static ESLINT_ERROR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s+(\d+):\d+\s+error\s").expect("valid regex"));
static PRETTIER_WARN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[warn\]\s+(.+?)\s*$").expect("valid regex"));
static COLOR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[[0-9;]*m").expect("valid regex"));

/// `file` as a repo-relative path with `/`, lowercased on Windows (for comparing).
fn norm(repo: &Path, dir: &str, file: &str) -> String {
    let file = file.trim().replace('\\', "/");
    let root = repo.to_string_lossy().replace('\\', "/");
    let rel = if cfg!(windows) {
        let lower = file.to_lowercase();
        let root_l = format!("{}/", root.trim_end_matches('/').to_lowercase());
        if lower.starts_with(&root_l) {
            file[root_l.len()..].to_owned()
        } else if Path::new(&file).is_absolute() {
            return lower;
        } else if dir.is_empty() {
            file
        } else {
            format!("{dir}/{file}")
        }
    } else if let Some(r) = file.strip_prefix(&format!("{}/", root.trim_end_matches('/'))) {
        r.to_owned()
    } else if file.starts_with('/') || dir.is_empty() {
        file
    } else {
        format!("{dir}/{file}")
    };
    let rel = rel.trim_start_matches("./").to_owned();
    if cfg!(windows) { rel.to_lowercase() } else { rel }
}

/// Lines kept of an unreadable failing command.
const TAIL_LINES: usize = 25;

fn tail(output: &str) -> Vec<String> {
    let lines: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(TAIL_LINES)..]
        .iter()
        .map(|l| (*l).to_owned())
        .collect()
}

/// Read what a check printed: errors in changed files, errors elsewhere (ignored). Lint errors
/// count only on the lines the task changed (`lines`): a file's old lint errors are not the
/// task's to fix. Type errors count anywhere in a changed file (a change can break a line it
/// did not touch).
pub fn read(
    repo: &Path,
    check: &Check,
    code: Option<i32>,
    output: &str,
    changed: &[String],
    lines: &ChangedLines,
) -> CheckResult {
    let output = COLOR.replace_all(output, "").into_owned();
    let changed: Vec<String> = changed.iter().map(|f| norm(repo, "", f)).collect();
    let touched = |file: &str| changed.contains(&norm(repo, &check.dir, file));
    let mut r = CheckResult {
        label: check.label.clone(),
        output: output.clone(),
        ..CheckResult::default()
    };
    if code.is_none() {
        r.broken = Some("bị dừng".into());
        return r;
    }
    let mut found = 0_usize;
    match check.kind {
        CheckKind::Tsc => {
            for line in output.lines() {
                if let Some(c) = TSC_LINE.captures(line) {
                    found += 1;
                    if touched(&c[1]) {
                        r.errors.push(line.trim().to_owned());
                    } else {
                        r.old += 1;
                    }
                }
            }
        }
        CheckKind::Eslint => {
            let mut file = String::new();
            for line in output.lines() {
                if !line.starts_with(char::is_whitespace) && !line.trim().is_empty() && line.contains('.') {
                    line.trim().clone_into(&mut file);
                } else if let Some(c) = ESLINT_ERROR.captures(line) {
                    found += 1;
                    let at: u32 = c[1].parse().unwrap_or(0);
                    if touched(&file) && lines.touches(&norm(repo, &check.dir, &file), at) {
                        r.errors.push(format!("{file}: {}", line.trim()));
                    } else {
                        r.old += 1;
                    }
                }
            }
        }
        CheckKind::Prettier => {
            for line in output.lines() {
                if let Some(c) = PRETTIER_WARN.captures(line) {
                    let file = &c[1];
                    if file.contains(' ') && !touched(file) {
                        continue; // "Code style issues found in 2 files. …"
                    }
                    found += 1;
                    if touched(file) {
                        r.errors.push(format!("{file}{PRETTIER_SUFFIX}"));
                    } else {
                        r.old += 1;
                    }
                }
            }
        }
        CheckKind::Other | CheckKind::Harness => {
            if code != Some(0) {
                r.errors = tail(&output);
                if r.errors.is_empty() {
                    r.errors.push(format!("thoát với mã {}", code.unwrap_or(-1)));
                }
            }
            return r;
        }
    }
    if code != Some(0) && found == 0 {
        let why = tail(&output).join("\n");
        r.broken = Some(if why.is_empty() {
            format!("thoát với mã {} mà không in lỗi", code.unwrap_or(-1))
        } else {
            why
        });
    }
    r
}

/// What follows the file in a prettier error line.
const PRETTIER_SUFFIX: &str = ": chưa đúng định dạng prettier";

/// Prettier only says which files are off, not which lines. A file that was off prettier at
/// the fork point already is not the task's to reformat (the AI would rewrite lines it never
/// touched): its error moves to `old`. A file new in the task, or whose old text the app cannot
/// check, keeps its error.
pub fn forgive_old_format(
    repo: &Path,
    base: &str,
    check: &Check,
    r: &mut CheckResult,
    runner: &dyn CheckRunner,
    timeout: Duration,
    stop: &AtomicBool,
) {
    if check.kind != CheckKind::Prettier || r.errors.is_empty() {
        return;
    }
    let git = Git::new(repo);
    let fork = git.merge_base(base, "HEAD").unwrap_or_else(|_| "HEAD".into());
    let lf = |s: &str| s.replace("\r\n", "\n").trim_end().to_owned();
    let mut keep = Vec::new();
    for e in std::mem::take(&mut r.errors) {
        let Some(file) = e.strip_suffix(PRETTIER_SUFFIX) else {
            keep.push(e);
            continue;
        };
        let rel = file.trim().replace('\\', "/");
        let path = match check.dir.as_str() {
            "" => rel.clone(),
            d => format!("{d}/{rel}"),
        };
        let Ok(old) = git.run(&["show", &format!("{fork}:{path}")]) else {
            keep.push(e); // new in the task
            continue;
        };
        let probe = Check {
            label: format!("prettier (bản gốc) · {rel}"),
            program: "npx".into(),
            args: ["--no", "--", "prettier", "--stdin-filepath", &rel]
                .iter()
                .map(|a| (*a).to_owned())
                .collect(),
            kind: CheckKind::Other,
            dir: check.dir.clone(),
            stdin: Some(old.clone()),
        };
        match runner.run(repo, &probe, timeout, stop) {
            Ok((Some(0), formatted)) if lf(&formatted) != lf(&old) => r.old += 1,
            _ => keep.push(e),
        }
    }
    r.errors = keep;
}

/// "tsc · web: 0 lỗi; eslint · web: 2 lỗi" for the step note.
pub fn summary(results: &[CheckResult]) -> String {
    let errors: usize = results.iter().map(|r| r.errors.len()).sum();
    let old: usize = results.iter().map(|r| r.old).sum();
    let broken = results.iter().filter(|r| r.broken.is_some()).count();
    let mut text = if broken > 0 {
        format!("{broken} lệnh không chạy được")
    } else {
        format!("{errors} lỗi")
    };
    let names: Vec<&str> = results
        .iter()
        .map(|r| r.label.split(" · ").next().unwrap_or(&r.label))
        .collect();
    let mut names = names;
    names.dedup();
    if !names.is_empty() {
        text.push_str(&format!(" ({})", names.join(", ")));
    }
    if old > 0 {
        text.push_str(&format!(
            ", bỏ qua {old} lỗi cũ (ở file khác hoặc dòng task không sửa)"
        ));
    }
    text
}

/// What the AI gets to fix: every error line, grouped by check.
pub fn report(results: &[CheckResult]) -> String {
    let mut out = String::new();
    for r in results.iter().filter(|r| !r.ok()) {
        out.push_str(&format!("### {}\n", r.label));
        if let Some(b) = &r.broken {
            out.push_str(b);
            out.push('\n');
        }
        for e in &r.errors {
            out.push_str(e);
            out.push('\n');
        }
        out.push('\n');
    }
    out.trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(kind: CheckKind, dir: &str) -> Check {
        Check {
            label: "x".into(),
            program: "npx".into(),
            args: vec![],
            kind,
            dir: dir.into(),
            stdin: None,
        }
    }

    #[test]
    fn splits_commands_and_fills_in_the_changed_files() {
        assert_eq!(
            split_command(r#"npx eslint "a b.ts" --max-warnings 0 '' x"#),
            vec!["npx", "eslint", "a b.ts", "--max-warnings", "0", "", "x"]
        );
        let tmp = tempfile::tempdir().unwrap();
        let plan = plan(
            tmp.path(),
            &[
                "npx eslint {files}".into(),
                "  ".into(),
                "npm run typecheck".into(),
            ],
            &["web/a.ts".into()],
        );
        assert_eq!(plan.len(), 2);
        // npm 11 reads options after the tool as its own: `--` before the tool
        assert_eq!(plan[0].args, vec!["--", "eslint", "web/a.ts"]);
        let s = |v: &[&str]| v.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            npx_args(&s(&["--no", "tsc", "--noEmit", "-p", "x.json"])),
            s(&["--no", "--", "tsc", "--noEmit", "-p", "x.json"])
        );
        assert_eq!(npx_args(&s(&["--", "tsc"])), s(&["--", "tsc"]), "has one already");
        assert_eq!(
            npx_args(&s(&["-p", "typescript", "tsc"])),
            s(&["-p", "typescript", "tsc"]),
            "npx's own -p takes a value"
        );
        assert_eq!(plan[0].kind, CheckKind::Eslint);
        assert_eq!(plan[1].kind, CheckKind::Other);
        assert!(super::plan(tmp.path(), &["npx tsc".into()], &[]).is_empty());
    }

    #[test]
    fn guesses_per_package_from_its_configs() {
        let tmp = tempfile::tempdir().unwrap();
        let web = tmp.path().join("web");
        std::fs::create_dir_all(web.join("src")).unwrap();
        std::fs::write(web.join("package.json"), r#"{"devDependencies":{"eslint":"9"}}"#).unwrap();
        std::fs::write(web.join("tsconfig.json"), "{}").unwrap();
        std::fs::write(web.join(".prettierrc"), "{}").unwrap();
        std::fs::create_dir_all(tmp.path().join("docs")).unwrap();
        let changed = vec![
            "web/src/a.ts".into(),
            "web/src/b.css".into(),
            "docs/x.md".into(),
            "README.md".into(),
        ];
        let plan = plan(tmp.path(), &[], &changed);
        let labels: Vec<&str> = plan.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, vec!["tsc · web", "eslint · web", "prettier · web"]);
        assert_eq!(
            plan[0].args,
            vec!["--no", "--", "tsc", "--noEmit", "-p", "tsconfig.json"]
        );
        assert_eq!(plan[1].args, vec!["--no", "--", "eslint", "src/a.ts"]);
        assert_eq!(
            plan[2].args,
            vec!["--no", "--", "prettier", "--check", "src/a.ts", "src/b.css"]
        );
        assert!(plan.iter().all(|c| c.dir == "web"));
        // no package.json: nothing to guess
        assert!(super::plan(tmp.path(), &[], &["docs/x.ts".into()]).is_empty());
    }

    #[test]
    fn only_errors_in_changed_files_count() {
        let repo = Path::new("C:/r");
        let none = ChangedLines::default();
        let changed = vec!["web/src/a.ts".to_owned()];
        let tsc = "src/a.ts(3,5): error TS2322: Type 'x'.\nsrc/old.ts(1,1): error TS1: y\n";
        let r = read(repo, &check(CheckKind::Tsc, "web"), Some(2), tsc, &changed, &none);
        assert_eq!(r.errors, vec!["src/a.ts(3,5): error TS2322: Type 'x'."]);
        assert_eq!(r.old, 1);
        let only_old = read(
            repo,
            &check(CheckKind::Tsc, "web"),
            Some(2),
            "src/old.ts(1,1): error TS1: y",
            &changed,
            &none,
        );
        assert!(only_old.ok() && only_old.old == 1);

        let eslint = "\nC:\\r\\web\\src\\a.ts\n  3:1  error  'x' is unused  no-unused-vars\n  4:1  warning  w  r\n\nC:\\r\\web\\src\\z.ts\n  1:1  error  e  r\n\n✖ 3 problems\n";
        let r = read(
            repo,
            &check(CheckKind::Eslint, "web"),
            Some(1),
            eslint,
            &changed,
            &none,
        );
        assert_eq!(r.errors.len(), 1, "{:?}", r.errors);
        assert_eq!(r.old, 1);
        // a lint error on a line the task did not change is an old one, even in a changed file
        let diff = "diff --git a/web/src/a.ts b/web/src/a.ts\n--- a/web/src/a.ts\n+++ b/web/src/a.ts\n\
                    @@ -10,2 +10,3 @@ fn\n-x\n-y\n+x\n+y\n+z\n@@ -30 +31,0 @@\n-gone\n";
        let lines = ChangedLines::parse(diff);
        let eslint = "\nC:\\r\\web\\src\\a.ts\n  3:1  error  any  no-explicit-any\n  11:1  error  new  r\n  \
                      32:1  error  near a removed line  r\n  40:1  error  far  r\n";
        let r = read(
            repo,
            &check(CheckKind::Eslint, "web"),
            Some(1),
            eslint,
            &changed,
            &lines,
        );
        assert_eq!(r.errors.len(), 2, "{:?}", r.errors);
        assert!(r.errors[0].ends_with("11:1  error  new  r"), "{:?}", r.errors);
        assert_eq!(r.old, 2);
        // a file the diff does not list (untracked: all new) counts on every line
        let untracked = ChangedLines::parse("+++ b/web/src/other.ts\n@@ -1 +1 @@\n");
        let r = read(
            repo,
            &check(CheckKind::Eslint, "web"),
            Some(1),
            eslint,
            &changed,
            &untracked,
        );
        assert_eq!(r.errors.len(), 4);

        let prettier = "Checking formatting...\n[warn] src/a.ts\n[warn] Code style issues found in the above file. Run Prettier with --write to fix.\n";
        let r = read(
            repo,
            &check(CheckKind::Prettier, "web"),
            Some(1),
            prettier,
            &changed,
            &none,
        );
        assert_eq!(r.errors, vec!["src/a.ts: chưa đúng định dạng prettier"]);

        // failing without a readable error: the command itself is broken
        let r = read(
            repo,
            &check(CheckKind::Tsc, "web"),
            Some(1),
            "npm ERR! could not determine executable to run",
            &changed,
            &none,
        );
        assert!(r.broken.as_deref().unwrap().contains("could not determine"));
        let r = read(
            repo,
            &check(CheckKind::Other, ""),
            Some(1),
            "boom",
            &changed,
            &none,
        );
        assert_eq!(r.errors, vec!["boom"]);
        assert!(
            read(
                repo,
                &check(CheckKind::Other, ""),
                Some(0),
                "fine",
                &changed,
                &none
            )
            .ok()
        );
        assert!(
            read(repo, &check(CheckKind::Tsc, ""), None, "", &changed, &none)
                .broken
                .is_some()
        );
    }

    /// Prettier on stdin: upper-cases the text (= the old text was "off prettier") unless it is
    /// already upper case.
    struct Upper;
    impl CheckRunner for Upper {
        fn run(
            &self,
            _repo: &Path,
            check: &Check,
            _timeout: Duration,
            _stop: &AtomicBool,
        ) -> Result<(Option<i32>, String), String> {
            assert!(
                check.args.contains(&"--stdin-filepath".to_owned()),
                "{:?}",
                check.args
            );
            Ok((Some(0), check.stdin.clone().unwrap_or_default().to_uppercase()))
        }
    }

    #[test]
    fn a_file_off_prettier_before_the_task_is_not_the_tasks_to_reformat() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .arg("-C")
                .arg(repo)
                .args([
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        };
        git(&["init", "-q", "-b", "main"]);
        std::fs::create_dir_all(repo.join("web/src")).unwrap();
        std::fs::write(repo.join("web/src/messy.ts"), "messy\n").unwrap();
        std::fs::write(repo.join("web/src/clean.ts"), "CLEAN\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "base"]);
        git(&["switch", "-q", "-c", "task"]);
        std::fs::write(repo.join("web/src/new.ts"), "new\n").unwrap();
        let check = Check {
            label: "prettier · web".into(),
            program: "npx".into(),
            args: vec![],
            kind: CheckKind::Prettier,
            dir: "web".into(),
            stdin: None,
        };
        let mut r = CheckResult {
            errors: ["src\\messy.ts", "src/clean.ts", "src/new.ts"]
                .iter()
                .map(|f| format!("{f}{PRETTIER_SUFFIX}"))
                .collect(),
            ..CheckResult::default()
        };
        let go = AtomicBool::new(false);
        forgive_old_format(repo, "main", &check, &mut r, &Upper, Duration::from_secs(5), &go);
        assert_eq!(
            r.errors,
            vec![
                format!("src/clean.ts{PRETTIER_SUFFIX}"),
                format!("src/new.ts{PRETTIER_SUFFIX}")
            ],
            "messy was off before the task; clean was fine and new is the task's own"
        );
        assert_eq!(r.old, 1);
    }

    #[test]
    fn summary_and_report() {
        let results = vec![
            CheckResult {
                label: "tsc · web".into(),
                old: 2,
                ..CheckResult::default()
            },
            CheckResult {
                label: "eslint · web".into(),
                errors: vec!["a.ts: 1:1 error x".into()],
                ..CheckResult::default()
            },
        ];
        assert_eq!(
            summary(&results),
            "1 lỗi (tsc, eslint), bỏ qua 2 lỗi cũ (ở file khác hoặc dòng task không sửa)"
        );
        assert_eq!(report(&results), "### eslint · web\na.ts: 1:1 error x");
    }
}
