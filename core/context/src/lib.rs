//! What the AI is allowed to read. Code builds it; nothing here calls an LLM.
//!
//! Base version: redaction. Brief and handoff templates come in Phase 1/3.

use std::sync::LazyLock;

use regex::Regex;

const MASK: &str = "[redacted]";

static PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        // JWT
        r"eyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}",
        // GitHub tokens
        r"\b(?:ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{20,}\b",
        r"\bgithub_pat_[A-Za-z0-9_]{20,}\b",
        // OpenAI / Anthropic style keys
        r"\bsk-[A-Za-z0-9_-]{16,}\b",
        // GitLab personal access tokens
        r"\bglpat-[A-Za-z0-9_-]{16,}\b",
        // Authorization headers
        r"(?i)\bbearer\s+[A-Za-z0-9._~+/=-]{12,}",
        // e-mail addresses
        r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}",
        // phone numbers (VN mobile and generic international)
        r"(?:\+84|\b0)(?:\d[ .-]?){9}\b",
    ]
    .iter()
    .map(|p| Regex::new(p).expect("valid regex"))
    .collect()
});

/// Mask secrets and personal data before text reaches an AI.
/// `extra_terms` are user-declared words (customer names, secret names such as `JWT_SECRET`).
pub fn redact(text: &str, extra_terms: &[String]) -> String {
    let mut out = text.to_owned();
    for re in PATTERNS.iter() {
        out = re.replace_all(&out, MASK).into_owned();
    }
    for term in extra_terms.iter().map(|t| t.trim()).filter(|t| !t.is_empty()) {
        let re = Regex::new(&format!("(?i){}", regex::escape(term))).expect("escaped term is a valid regex");
        out = re.replace_all(&out, MASK).into_owned();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_tokens_emails_phones_and_terms() {
        let text = "mail a.b@example.com, call 0912 345 678, key sk-abcdefghijklmnopqrstu, \
                    gh ghp_abcdefghijklmnopqrstuvwxyz0123, Authorization: Bearer abcdef.123456789xyz, \
                    jwt eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTYifQ.SflKxwRJSMeKKF2QT4fwpM, \
                    secret JWT_SECRET for AcmeCorp";
        let out = redact(text, &["JWT_SECRET".into(), "acmecorp".into()]);
        for leaked in [
            "a.b@",
            "0912",
            "sk-abc",
            "ghp_",
            "abcdef.123",
            "eyJhbGci",
            "JWT_SECRET",
            "AcmeCorp",
        ] {
            assert!(!out.contains(leaked), "{leaked} leaked: {out}");
        }
        assert!(out.contains("mail [redacted]"));
    }

    #[test]
    fn keeps_normal_code_and_stack_traces() {
        let text = "TypeError: Cannot read properties of undefined (reading 'org') at order.ts:42:7";
        assert_eq!(redact(text, &[]), text);
    }
}
