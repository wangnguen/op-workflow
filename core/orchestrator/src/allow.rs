//! "Cho phép lệnh này" in the Tiến trình tab: turn a shell command the CLI refused into
//! allow-list entries for `repos.<repo>.allowBash`, and check what the user wants to add before
//! it goes into config.json.

use opw_domain::view::{DeniedCall, RuleOption};

/// Longest rule accepted.
const RULE_MAX: usize = 200;

/// Tools whose calls are shell commands (Claude's `Bash`, Codex's `Shell`).
fn is_shell(tool: &str) -> bool {
    matches!(tool, "Bash" | "Shell")
}

/// Split a command line on `&&`, `||`, `;`, `|` and newlines outside quotes.
fn parts(command: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                cur.push(c);
                if c == q {
                    quote = None;
                }
            }
            None => match c {
                '\'' | '"' => {
                    quote = Some(c);
                    cur.push(c);
                }
                '&' if chars.peek() == Some(&'&') => {
                    chars.next();
                    out.push(std::mem::take(&mut cur));
                }
                '|' => {
                    if chars.peek() == Some(&'|') {
                        chars.next();
                    }
                    out.push(std::mem::take(&mut cur));
                }
                ';' | '\n' => out.push(std::mem::take(&mut cur)),
                _ => cur.push(c),
            },
        }
    }
    out.push(cur);
    out.into_iter()
        .map(|p| p.trim().to_owned())
        .filter(|p| !p.is_empty())
        .collect()
}

/// A word that names a subcommand or script (`run`, `lint`, `tsc`, `type-check`, `test:unit`).
fn is_word(token: &str) -> bool {
    let mut chars = token.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | ':'))
}

/// How many words after the program belong to the rule: `npm run lint`, `npx tsc`, `git diff`.
fn sub_words(program: &str) -> usize {
    match program {
        "npm" | "pnpm" | "yarn" | "bun" => 2,
        "npx" | "pnpx" | "bunx" | "git" | "cargo" | "go" | "dotnet" | "docker" | "make" => 1,
        _ => 0,
    }
}

/// `npx tsc --noEmit -p api` → `npx tsc *`; `cd api` gives nothing (the prompt asks the AI not
/// to `cd`). Leading `VAR=value` assignments are skipped.
fn rule_for(part: &str) -> Option<String> {
    let mut tokens = part
        .split_whitespace()
        .skip_while(|t| t.contains('=') && !t.starts_with('-'));
    let program = tokens.next()?;
    if program == "cd" || program == "pushd" || program == "popd" {
        return None;
    }
    let mut words = vec![program.to_owned()];
    for t in tokens.take(sub_words(program)) {
        if !is_word(t) {
            break;
        }
        words.push(t.to_owned());
    }
    Some(format!("{} *", words.join(" ")))
}

/// The allow-list entries that would let `command` run, and whether it chains several commands.
pub fn suggest(command: &str) -> (Vec<String>, bool) {
    let parts = parts(command);
    let mut rules: Vec<String> = Vec::new();
    for part in &parts {
        if let Some(rule) = rule_for(part) {
            if !rules.contains(&rule) {
                rules.push(rule);
            }
        }
    }
    (rules, parts.len() > 1)
}

/// `Bash(npx tsc *)` and `npx tsc *` are the same entry.
fn bare(rule: &str) -> &str {
    let r = rule.trim();
    r.strip_prefix("Bash(")
        .and_then(|r| r.strip_suffix(')'))
        .map_or(r, str::trim)
}

/// A refused call as the Tiến trình tab shows it, with the suggested rules marked when they are
/// already in the repo's allow list.
pub fn denied_call(tool: &str, input: &str, allowed: &[String]) -> DeniedCall {
    let (rules, chained) = if is_shell(tool) {
        suggest(input)
    } else {
        (Vec::new(), false)
    };
    DeniedCall {
        tool: tool.to_owned(),
        input: input.to_owned(),
        chained,
        rules: rules
            .into_iter()
            .map(|rule| RuleOption {
                allowed: allowed.iter().any(|a| bare(a) == rule),
                rule,
            })
            .collect(),
    }
}

/// git subcommands that write; an allow rule may never cover them (the git guard blocks them
/// anyway, this keeps config.json honest).
const GIT_WRITES: &[&str] = &[
    "push",
    "commit",
    "merge",
    "rebase",
    "reset",
    "clean",
    "tag",
    "checkout",
    "switch",
    "stash",
    "branch",
    "restore",
    "rm",
    "mv",
    "cherry-pick",
    "revert",
    "am",
    "apply",
    "pull",
    "fetch",
];

/// Check and tidy a rule the user wants to add: one command pattern, no shell operators, nothing
/// that would cover every command or a git write.
pub fn clean_rule(input: &str) -> Result<String, String> {
    let rule = bare(input).split_whitespace().collect::<Vec<_>>().join(" ");
    if rule.is_empty() {
        return Err("Luật trống".into());
    }
    if rule.chars().count() > RULE_MAX {
        return Err(format!("Luật dài quá {RULE_MAX} ký tự"));
    }
    if ["&&", "||", ";", "|", "`", "$(", ">", "<", "\n"]
        .iter()
        .any(|op| rule.contains(op))
    {
        return Err("Mỗi luật chỉ cho một lệnh: bỏ &&, ;, |, chuyển hướng hay $(…)".into());
    }
    let mut words = rule.split(' ');
    let program = words.next().unwrap_or_default();
    if program.starts_with('*') {
        return Err("Luật phải bắt đầu bằng tên lệnh (ví dụ `npx tsc *`), không phải `*`".into());
    }
    if program == "git" {
        let sub = words.next().unwrap_or("*");
        if sub == "*" || sub.starts_with('-') || GIT_WRITES.contains(&sub) {
            return Err("App không cho AI chạy lệnh git ghi (commit, push, checkout, …); chỉ thêm được lệnh git chỉ đọc như `git diff *`".into());
        }
    }
    if program == "gh" {
        return Err("App không cho AI dùng gh".into());
    }
    Ok(rule)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggests_program_and_subcommand() {
        assert_eq!(
            suggest("npx tsc --noEmit -p api/tsconfig.json"),
            (vec!["npx tsc *".into()], false)
        );
        assert_eq!(
            suggest("npm run lint -- --fix"),
            (vec!["npm run lint *".into()], false)
        );
        assert_eq!(suggest("npm test"), (vec!["npm test *".into()], false));
        assert_eq!(suggest("pnpm --filter web lint").0, ["pnpm *"]);
        assert_eq!(suggest("grep -rn \"a|b\" src").0, ["grep *"]);
        assert_eq!(suggest("ls apps").0, ["ls *"]);
        assert_eq!(suggest("node scripts/x.js").0, ["node *"]);
        assert_eq!(suggest("NODE_ENV=test npx jest src").0, ["npx jest *"]);
        assert_eq!(suggest("git diff origin/main").0, ["git diff *"]);
    }

    #[test]
    fn chained_commands_give_a_rule_per_part_and_skip_cd() {
        let (rules, chained) = suggest("cd apps/web && grep -rn foo src && ls src | head -5");
        assert!(chained);
        assert_eq!(rules, ["grep *", "ls *", "head *"]);
        let (rules, chained) = suggest("cd api; npx tsc --noEmit\nnpx tsc -p x");
        assert!(chained);
        assert_eq!(rules, ["npx tsc *"]);
        assert_eq!(suggest("cd api"), (vec![], false));
    }

    #[test]
    fn denied_calls_mark_rules_already_allowed() {
        let call = denied_call("Bash", "cd api && npx tsc --noEmit", &["Bash(npx tsc *)".into()]);
        assert!(call.chained);
        assert_eq!(call.rules.len(), 1);
        assert!(call.rules[0].allowed);
        let edit = denied_call("Edit", "/etc/hosts", &[]);
        assert!(edit.rules.is_empty() && !edit.chained);
    }

    #[test]
    fn rules_are_checked_before_they_reach_config() {
        assert_eq!(clean_rule("  Bash( npx   tsc * ) ").as_deref(), Ok("npx tsc *"));
        assert_eq!(clean_rule("git diff *").as_deref(), Ok("git diff *"));
        for bad in [
            "",
            "*",
            "* --help",
            "npm test && rm -rf /",
            "ls | sh",
            "echo $(whoami)",
            "git *",
            "git push *",
            "git -C x commit",
            "gh pr create",
        ] {
            assert!(clean_rule(bad).is_err(), "{bad}");
        }
        assert!(clean_rule(&"a".repeat(201)).is_err());
    }
}
