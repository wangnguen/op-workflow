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
            stdin: None,
            env: vec![
                ("FORCE_COLOR".into(), "0".into()),
                ("NO_COLOR".into(), "1".into()),
            ],
            account: Account::Subscription,
            timeout,
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
                    args: args.to_vec(),
                    kind: kind_of(&words),
                    dir: String::new(),
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
        let npx = |tool: &str, args: Vec<String>, kind| Check {
            label: format!("{tool} · {name}"),
            program: "npx".into(),
            args: ["--no".to_owned(), tool.to_owned()]
                .into_iter()
                .chain(args)
                .collect(),
            kind,
            dir: pkg.clone(),
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

static TSC_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(.+?)\(\d+,\d+\): error TS\d+").expect("valid regex"));
static ESLINT_ERROR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s+\d+:\d+\s+error\s").expect("valid regex"));
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

/// Read what a check printed: errors in changed files, errors elsewhere (ignored).
pub fn read(repo: &Path, check: &Check, code: Option<i32>, output: &str, changed: &[String]) -> CheckResult {
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
                } else if ESLINT_ERROR.is_match(line) {
                    found += 1;
                    if touched(&file) {
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
                        r.errors.push(format!("{file}: chưa đúng định dạng prettier"));
                    } else {
                        r.old += 1;
                    }
                }
            }
        }
        CheckKind::Other => {
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
        text.push_str(&format!(", bỏ qua {old} lỗi cũ ở file khác"));
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
        assert_eq!(plan[0].args, vec!["eslint", "web/a.ts"]);
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
            vec!["--no", "tsc", "--noEmit", "-p", "tsconfig.json"]
        );
        assert_eq!(plan[1].args, vec!["--no", "eslint", "src/a.ts"]);
        assert_eq!(
            plan[2].args,
            vec!["--no", "prettier", "--check", "src/a.ts", "src/b.css"]
        );
        assert!(plan.iter().all(|c| c.dir == "web"));
        // no package.json: nothing to guess
        assert!(super::plan(tmp.path(), &[], &["docs/x.ts".into()]).is_empty());
    }

    #[test]
    fn only_errors_in_changed_files_count() {
        let repo = Path::new("C:/r");
        let changed = vec!["web/src/a.ts".to_owned()];
        let tsc = "src/a.ts(3,5): error TS2322: Type 'x'.\nsrc/old.ts(1,1): error TS1: y\n";
        let r = read(repo, &check(CheckKind::Tsc, "web"), Some(2), tsc, &changed);
        assert_eq!(r.errors, vec!["src/a.ts(3,5): error TS2322: Type 'x'."]);
        assert_eq!(r.old, 1);
        let only_old = read(
            repo,
            &check(CheckKind::Tsc, "web"),
            Some(2),
            "src/old.ts(1,1): error TS1: y",
            &changed,
        );
        assert!(only_old.ok() && only_old.old == 1);

        let eslint = "\nC:\\r\\web\\src\\a.ts\n  3:1  error  'x' is unused  no-unused-vars\n  4:1  warning  w  r\n\nC:\\r\\web\\src\\z.ts\n  1:1  error  e  r\n\n✖ 3 problems\n";
        let r = read(repo, &check(CheckKind::Eslint, "web"), Some(1), eslint, &changed);
        assert_eq!(r.errors.len(), 1, "{:?}", r.errors);
        assert_eq!(r.old, 1);

        let prettier = "Checking formatting...\n[warn] src/a.ts\n[warn] Code style issues found in the above file. Run Prettier with --write to fix.\n";
        let r = read(
            repo,
            &check(CheckKind::Prettier, "web"),
            Some(1),
            prettier,
            &changed,
        );
        assert_eq!(r.errors, vec!["src/a.ts: chưa đúng định dạng prettier"]);

        // failing without a readable error: the command itself is broken
        let r = read(
            repo,
            &check(CheckKind::Tsc, "web"),
            Some(1),
            "npm ERR! could not determine executable to run",
            &changed,
        );
        assert!(r.broken.as_deref().unwrap().contains("could not determine"));
        let r = read(repo, &check(CheckKind::Other, ""), Some(1), "boom", &changed);
        assert_eq!(r.errors, vec!["boom"]);
        assert!(read(repo, &check(CheckKind::Other, ""), Some(0), "fine", &changed).ok());
        assert!(
            read(repo, &check(CheckKind::Tsc, ""), None, "", &changed)
                .broken
                .is_some()
        );
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
            "1 lỗi (tsc, eslint), bỏ qua 2 lỗi cũ ở file khác"
        );
        assert_eq!(report(&results), "### eslint · web\na.ts: 1:1 error x");
    }
}
