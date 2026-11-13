//! Codex CLI headless (`codex exec --json`). Flags verified with codex-cli 0.169.0 on
//! 2026-11-13 (PLAN.md §9), except the two `memories.*` overrides, which are not tried yet.

/// JSON Schema for `--output-schema`. OpenAI structured output needs every property listed in
/// `required` and `additionalProperties: false`; an optional value is typed as `[T, "null"]`.
pub const FINDINGS_SCHEMA: &str = r#"{
  "type": "object",
  "additionalProperties": false,
  "required": ["findings"],
  "properties": {
    "findings": {
      "type": "array",
      "items": {
        "type": "object",
        "additionalProperties": false,
        "required": ["severity", "file", "line", "title", "detail", "suggestion"],
        "properties": {
          "severity": { "type": "string", "enum": ["critical", "high", "medium", "low"] },
          "file": { "type": "string" },
          "line": { "type": ["integer", "null"] },
          "title": { "type": "string" },
          "detail": { "type": "string" },
          "suggestion": { "type": "string" }
        }
      }
    }
  }
}"#;

/// Read-only cross-review. The prompt goes through stdin (`-`); the final JSON lands in
/// `output_file`. Memories are off so Codex does not read `~/.codex/memories`.
pub fn cross_review_args(schema_file: &str, output_file: &str) -> Vec<String> {
    [
        "exec",
        "--sandbox",
        "read-only",
        "--ephemeral",
        "--json",
        "--output-schema",
        schema_file,
        "-c",
        "memories.use_memories=false",
        "-c",
        "memories.generate_memories=false",
        "-o",
        output_file,
        "-",
    ]
    .iter()
    .map(|a| (*a).to_owned())
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cross_review_is_read_only_ephemeral_and_reads_stdin() {
        let args = cross_review_args("s.json", "out.json");
        assert_eq!(args[0], "exec");
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--sandbox" && w[1] == "read-only")
        );
        assert!(args.contains(&"--ephemeral".to_owned()));
        assert!(
            args.windows(2)
                .any(|w| w[0] == "-c" && w[1] == "memories.use_memories=false")
        );
        assert_eq!(args.last().map(String::as_str), Some("-"));
    }

    #[test]
    fn schema_is_strict() {
        let v: serde_json::Value = serde_json::from_str(FINDINGS_SCHEMA).unwrap();
        let item = &v["properties"]["findings"]["items"];
        assert_eq!(item["additionalProperties"], false);
        assert_eq!(item["required"].as_array().unwrap().len(), 6);
    }
}
