//! `fake-agent [--fixture <file.jsonl>] [--delay-ms <n>]`
//!
//! Stands in for `claude -p ... --output-format stream-json` in tests: prints each fixture line
//! to stdout with a delay, ignores stdin (the prompt), exits 0. Without `--fixture` it plays the
//! built-in sample captured from a real run on 2026-11-13 (content removed).

use std::io::Write;
use std::time::Duration;

const SAMPLE: &str = include_str!("../fixtures/claude-headless.jsonl");

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |flag: &str| args.windows(2).find(|w| w[0] == flag).map(|w| w[1].clone());
    let delay = Duration::from_millis(value("--delay-ms").and_then(|v| v.parse().ok()).unwrap_or(150));
    let text = match value("--fixture") {
        Some(path) => match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("fake-agent: cannot read {path}: {e}");
                std::process::exit(2);
            }
        },
        None => SAMPLE.to_owned(),
    };
    let mut out = std::io::stdout().lock();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        if writeln!(out, "{line}").and_then(|()| out.flush()).is_err() {
            return;
        }
        std::thread::sleep(delay);
    }
}
