//! `opw-hook <kind>`: reads the hook/statusline JSON on stdin and appends one compact line to
//! `<data_dir>/events/<session_id>.jsonl` for the monitor to tail.
//!
//! Only routing fields and numbers are kept (event name, session id, cwd basename, rate
//! limits, context size); prompts, tool input and transcript content are never written.
//! Always exits 0 so a hook problem can never block the agent.

use std::io::{Read, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};

fn main() {
    let kind = std::env::args().nth(1).unwrap_or_else(|| "event".to_owned());
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return;
    }
    let Ok(payload) = serde_json::from_str::<Value>(&input) else {
        return;
    };
    let line = summarize(&kind, &payload);
    if let Err(e) = append(&payload, &line) {
        eprintln!("opw-hook: {e}");
    }
}

fn summarize(kind: &str, p: &Value) -> Value {
    let at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let mut out = Map::new();
    out.insert("at".into(), json!(at));
    out.insert("kind".into(), json!(kind));
    for key in ["hook_event_name", "session_id", "matcher", "trigger"] {
        if let Some(v) = p.get(key).filter(|v| v.is_string()) {
            out.insert(key.into(), v.clone());
        }
    }
    if let Some(limits) = p.get("rate_limits") {
        out.insert("rate_limits".into(), limits.clone());
    }
    if let Some(cw) = p.get("context_window") {
        let pick: Map<String, Value> = [
            "used_percentage",
            "remaining_percentage",
            "context_window_size",
            "total_input_tokens",
        ]
        .iter()
        .filter_map(|k| cw.get(*k).map(|v| ((*k).to_owned(), v.clone())))
        .collect();
        out.insert("context_window".into(), Value::Object(pick));
    }
    Value::Object(out)
}

fn append(payload: &Value, line: &Value) -> std::io::Result<()> {
    let session = payload
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let safe: String = session
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(64)
        .collect();
    let dir = opw_config::data_dir()
        .map_err(std::io::Error::other)?
        .join("events");
    std::fs::create_dir_all(&dir)?;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(format!("{safe}.jsonl")))?;
    writeln!(f, "{line}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_routing_fields_and_numbers() {
        let p = json!({"hook_event_name": "PreCompact", "session_id": "abc", "prompt": "secret prompt",
                       "tool_input": {"command": "cat .env"}, "context_window": {"used_percentage": 51, "transcript": "x"}});
        let s = summarize("hook", &p).to_string();
        assert!(s.contains("PreCompact") && s.contains("\"used_percentage\":51"));
        assert!(!s.contains("secret") && !s.contains(".env") && !s.contains("transcript"));
    }
}
