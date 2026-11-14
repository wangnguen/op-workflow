//! `git diff` output (unified, no colour) as files, hunks and numbered lines, for the Diff tab.
//!
//! Pure parsing: [`crate::Git::task_diff`] runs git, this module only reads its text.

/// Lines kept per file; the rest is cut (the tab says so).
pub const MAX_FILE_LINES: usize = 4000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChange {
    Added,
    Deleted,
    Modified,
    Renamed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    /// Line number on the base side (context and removed lines).
    pub old: Option<u32>,
    /// Line number on the working-tree side (context and added lines).
    pub new: Option<u32>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffHunk {
    /// `@@ -10,6 +10,8 @@ fn save` → `fn save` (may be empty).
    pub section: String,
    pub old_start: u32,
    pub new_start: u32,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    /// Path in the working tree (the new name of a rename), with `/`.
    pub path: String,
    /// Old name of a renamed file.
    pub old_path: Option<String>,
    pub change: FileChange,
    pub binary: bool,
    /// Not in git yet (`git diff` does not list these; the app adds them).
    pub untracked: bool,
    pub added: u32,
    pub removed: u32,
    pub hunks: Vec<DiffHunk>,
    /// Lines were left out after [`MAX_FILE_LINES`].
    pub truncated: bool,
    /// Hash of this file's diff text, to tell "changed since you last looked".
    pub fingerprint: String,
}

impl FileDiff {
    fn new(path: String) -> Self {
        Self {
            path,
            old_path: None,
            change: FileChange::Modified,
            binary: false,
            untracked: false,
            added: 0,
            removed: 0,
            hunks: Vec::new(),
            truncated: false,
            fingerprint: String::new(),
        }
    }

    fn line_count(&self) -> usize {
        self.hunks.iter().map(|h| h.lines.len()).sum()
    }
}

/// FNV-1a: stable across runs and Rust versions (unlike `DefaultHasher`).
fn fnv(text: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// Undo git's C-style quoting of unusual paths: `"a\tb"` → `a<TAB>b`. Octal escapes are UTF-8
/// bytes, so they are collected as bytes first.
fn unquote(s: &str) -> String {
    let Some(inner) = s.strip_prefix('"').and_then(|r| r.strip_suffix('"')) else {
        return s.to_owned();
    };
    let mut out: Vec<u8> = Vec::new();
    let bytes = inner.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b != b'\\' || i + 1 >= bytes.len() {
            out.push(b);
            i += 1;
            continue;
        }
        let next = bytes[i + 1];
        let octal = bytes.len() >= i + 4 && bytes[i + 1..i + 4].iter().all(|b| (b'0'..=b'7').contains(b));
        if octal {
            let value = bytes[i + 1..i + 4]
                .iter()
                .fold(0u32, |acc, b| acc * 8 + u32::from(b - b'0'));
            out.push(u8::try_from(value).unwrap_or(b'?'));
            i += 4;
            continue;
        }
        match next {
            b'n' => out.push(b'\n'),
            b't' => out.push(b'\t'),
            b'r' => out.push(b'\r'),
            b'"' => out.push(b'"'),
            b'\\' => out.push(b'\\'),
            other => {
                out.push(b'\\');
                out.push(other);
            }
        }
        i += 2;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `a/src/x.ts` → `src/x.ts`; `/dev/null` → `None`.
fn side_path(s: &str, prefix: &str) -> Option<String> {
    let s = unquote(s.trim_end_matches('\t').trim_end());
    if s == "/dev/null" {
        return None;
    }
    Some(s.strip_prefix(prefix).unwrap_or(&s).to_owned())
}

/// Path from `diff --git a/P b/P` when both sides are the same (not a rename).
fn header_path(rest: &str) -> Option<String> {
    if let Some(quoted) = rest.strip_prefix('"') {
        // "a/x y" "b/x y"
        let end = quoted.find('"')? + 2;
        return side_path(&rest[..end], "a/");
    }
    let n = rest.len().checked_sub(5)? / 2;
    let (a, b) = (rest.get(2..2 + n)?, rest.get(2 + n..)?);
    (rest.starts_with("a/") && b == format!(" b/{a}")).then(|| a.to_owned())
}

/// `@@ -10,6 +12,8 @@ section` → (10, 6, 12, 8, "section").
fn hunk_header(line: &str) -> Option<(u32, u32, u32, u32, String)> {
    let rest = line.strip_prefix("@@ -")?;
    let (ranges, section) = rest.split_once(" @@").unwrap_or((rest, ""));
    let (old, new) = ranges.split_once(" +")?;
    let range = |r: &str| -> Option<(u32, u32)> {
        match r.split_once(',') {
            Some((start, len)) => Some((start.parse().ok()?, len.parse().ok()?)),
            None => Some((r.parse().ok()?, 1)),
        }
    };
    let (os, ol) = range(old)?;
    let (ns, nl) = range(new)?;
    Some((os, ol, ns, nl, section.trim().to_owned()))
}

/// Parse the whole output of `git diff --no-color --no-ext-diff`.
pub fn parse_unified(text: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut raw = String::new();
    // lines still expected in the current hunk (old side, new side)
    let mut left = (0u32, 0u32);
    let mut pos = (0u32, 0u32);

    let close = |files: &mut Vec<FileDiff>, raw: &mut String| {
        if let Some(f) = files.last_mut() {
            if f.fingerprint.is_empty() {
                f.fingerprint = fnv(raw);
            }
        }
        raw.clear();
    };

    for line in text.lines() {
        // never a hunk line: those start with ' ', '+', '-' or '\'
        if let Some(rest) = line.strip_prefix("diff --git ") {
            close(&mut files, &mut raw);
            left = (0, 0);
            files.push(FileDiff::new(header_path(rest).unwrap_or_default()));
            raw.push_str(line);
            raw.push('\n');
            continue;
        }
        let Some(file) = files.last_mut() else { continue };
        raw.push_str(line);
        raw.push('\n');

        if (left.0 > 0 || left.1 > 0) && !file.hunks.is_empty() {
            let parsed = match line.as_bytes().first() {
                Some(b'+') => Some((LineKind::Added, &line[1..])),
                Some(b'-') => Some((LineKind::Removed, &line[1..])),
                Some(b' ') => Some((LineKind::Context, &line[1..])),
                // "\ No newline at end of file" does not count
                Some(b'\\') => continue,
                // some tools strip the space of an empty context line
                None => Some((LineKind::Context, "")),
                // not a hunk line after all (the counts were off): read it as a header
                Some(_) => None,
            };
            if let Some((kind, text)) = parsed {
                if file.line_count() >= MAX_FILE_LINES {
                    file.truncated = true;
                }
                let (old, new) = match kind {
                    LineKind::Context => {
                        left = (left.0.saturating_sub(1), left.1.saturating_sub(1));
                        pos = (pos.0 + 1, pos.1 + 1);
                        (Some(pos.0 - 1), Some(pos.1 - 1))
                    }
                    LineKind::Added => {
                        left.1 = left.1.saturating_sub(1);
                        file.added += 1;
                        pos.1 += 1;
                        (None, Some(pos.1 - 1))
                    }
                    LineKind::Removed => {
                        left.0 = left.0.saturating_sub(1);
                        file.removed += 1;
                        pos.0 += 1;
                        (Some(pos.0 - 1), None)
                    }
                };
                if let (false, Some(hunk)) = (file.truncated, file.hunks.last_mut()) {
                    hunk.lines.push(DiffLine {
                        kind,
                        old,
                        new,
                        text: text.to_owned(),
                    });
                }
                continue;
            }
            left = (0, 0);
        }

        if let Some((os, ol, ns, nl, section)) = hunk_header(line) {
            left = (ol, nl);
            pos = (os, ns);
            file.hunks.push(DiffHunk {
                section,
                old_start: os,
                new_start: ns,
                lines: Vec::new(),
            });
        } else if line.starts_with("new file mode") {
            file.change = FileChange::Added;
        } else if line.starts_with("deleted file mode") {
            file.change = FileChange::Deleted;
        } else if let Some(p) = line.strip_prefix("rename from ") {
            file.change = FileChange::Renamed;
            file.old_path = Some(unquote(p));
        } else if let Some(p) = line.strip_prefix("rename to ") {
            file.change = FileChange::Renamed;
            file.path = unquote(p);
        } else if line.starts_with("Binary files ") || line == "GIT binary patch" {
            file.binary = true;
        } else if let Some(p) = line.strip_prefix("--- ") {
            if file.path.is_empty() {
                if let Some(p) = side_path(p, "a/") {
                    file.path = p;
                }
            }
        } else if let Some(p) = line.strip_prefix("+++ ") {
            if let Some(p) = side_path(p, "b/") {
                if file.change != FileChange::Renamed {
                    file.path = p;
                }
            }
        }
    }
    close(&mut files, &mut raw);
    files.retain(|f| !f.path.is_empty());
    for f in &mut files {
        f.path = f.path.replace('\\', "/");
    }
    files
}

/// A file git does not track yet, shown as fully added. `None` content = binary or too big.
pub fn untracked_file(path: &str, content: Option<&str>) -> FileDiff {
    let mut f = FileDiff::new(path.replace('\\', "/"));
    f.change = FileChange::Added;
    f.untracked = true;
    match content {
        None => f.binary = true,
        Some(text) => {
            let mut lines = Vec::new();
            for (i, l) in text.lines().enumerate() {
                if lines.len() >= MAX_FILE_LINES {
                    f.truncated = true;
                    f.added += 1;
                    continue;
                }
                lines.push(DiffLine {
                    kind: LineKind::Added,
                    old: None,
                    new: u32::try_from(i + 1).ok(),
                    text: l.to_owned(),
                });
                f.added += 1;
            }
            if !lines.is_empty() {
                f.hunks.push(DiffHunk {
                    section: String::new(),
                    old_start: 0,
                    new_start: 1,
                    lines,
                });
            }
        }
    }
    f.fingerprint = fnv(&format!("untracked {path}\n{}", content.unwrap_or("<binary>")));
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
diff --git a/src/order.ts b/src/order.ts
index 1111111..2222222 100644
--- a/src/order.ts
+++ b/src/order.ts
@@ -10,4 +10,5 @@ export function save(o: Order) {
   const org = user.org;
-  return org.id;
+  if (!org) return null;
+  return org.id;
 }
-
@@ -40,1 +41,1 @@
--- not a header, a removed line
+++ not a header, an added line
diff --git a/docs/new file.md b/docs/new file.md
new file mode 100644
index 0000000..3333333
--- /dev/null
+++ b/docs/new file.md
@@ -0,0 +1,2 @@
+# Hi
+there
\\ No newline at end of file
diff --git a/old.txt b/renamed.txt
similarity index 90%
rename from old.txt
rename to renamed.txt
index 4444444..5555555 100644
--- a/old.txt
+++ b/renamed.txt
@@ -1 +1 @@
-a
+b
diff --git a/logo.png b/logo.png
index 6666666..7777777 100644
Binary files a/logo.png and b/logo.png differ
diff --git a/gone.rs b/gone.rs
deleted file mode 100644
index 8888888..0000000
--- a/gone.rs
+++ /dev/null
@@ -1,1 +0,0 @@
-fn main() {}
";

    #[test]
    fn parses_files_hunks_and_line_numbers() {
        let files = parse_unified(SAMPLE);
        let paths: Vec<_> = files.iter().map(|f| (f.path.as_str(), f.change)).collect();
        assert_eq!(
            paths,
            vec![
                ("src/order.ts", FileChange::Modified),
                ("docs/new file.md", FileChange::Added),
                ("renamed.txt", FileChange::Renamed),
                ("logo.png", FileChange::Modified),
                ("gone.rs", FileChange::Deleted),
            ]
        );
        let order = &files[0];
        assert_eq!((order.added, order.removed), (3, 3));
        assert_eq!(order.hunks.len(), 2);
        assert_eq!(order.hunks[0].section, "export function save(o: Order) {");
        let first = &order.hunks[0].lines;
        assert_eq!((first[0].old, first[0].new), (Some(10), Some(10)));
        assert_eq!(first[1].kind, LineKind::Removed);
        assert_eq!((first[1].old, first[1].new), (Some(11), None));
        assert_eq!((first[2].old, first[2].new), (None, Some(11)));
        assert_eq!((first[3].old, first[3].new), (None, Some(12)));
        assert_eq!((first[4].old, first[4].new), (Some(12), Some(13)));
        // a removed empty line
        assert_eq!((first[5].kind, first[5].text.as_str()), (LineKind::Removed, ""));
        // "--- "/"+++ " inside a hunk are content, not headers
        let second = &order.hunks[1].lines;
        assert_eq!(second[0].text, "-- not a header, a removed line");
        assert_eq!(second[1].text, "++ not a header, an added line");
        assert_eq!(second[1].new, Some(41));

        assert_eq!(files[1].added, 2);
        assert_eq!(files[1].hunks[0].lines[1].new, Some(2));
        assert_eq!(files[2].old_path.as_deref(), Some("old.txt"));
        assert!(files[3].binary && files[3].hunks.is_empty());
        assert_eq!(files[4].removed, 1);
    }

    #[test]
    fn fingerprints_change_with_the_content_only() {
        let a = parse_unified(SAMPLE);
        let b = parse_unified(SAMPLE);
        assert_eq!(a[0].fingerprint, b[0].fingerprint);
        assert_ne!(a[0].fingerprint, a[1].fingerprint);
        let edited = SAMPLE.replace("+  if (!org) return null;", "+  if (!org) return undefined;");
        let c = parse_unified(&edited);
        assert_ne!(a[0].fingerprint, c[0].fingerprint);
        assert_eq!(a[1].fingerprint, c[1].fingerprint);
    }

    #[test]
    fn quoted_paths_and_untracked_files() {
        let text = "diff --git \"a/t\\303\\251st.txt\" \"b/t\\303\\251st.txt\"\n\
                    --- \"a/t\\303\\251st.txt\"\n+++ \"b/t\\303\\251st.txt\"\n@@ -1 +1 @@\n-x\n+y\n";
        let files = parse_unified(text);
        assert_eq!(files[0].path, "tést.txt");

        let f = untracked_file("src\\new.ts", Some("a\nb\n"));
        assert_eq!((f.path.as_str(), f.added, f.untracked), ("src/new.ts", 2, true));
        assert_eq!(f.hunks[0].lines[1].new, Some(2));
        assert!(untracked_file("x.bin", None).binary);
    }

    #[test]
    fn long_files_are_cut_but_counted() {
        let body: String = (0..MAX_FILE_LINES + 10).map(|i| format!("+l{i}\n")).collect();
        let text = format!(
            "diff --git a/big.txt b/big.txt\nnew file mode 100644\n--- /dev/null\n+++ b/big.txt\n@@ -0,0 +1,{} @@\n{body}",
            MAX_FILE_LINES + 10
        );
        let f = &parse_unified(&text)[0];
        assert!(f.truncated);
        assert_eq!(f.line_count(), MAX_FILE_LINES);
        assert_eq!(f.added as usize, MAX_FILE_LINES + 10);
    }
}
