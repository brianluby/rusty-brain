//! Fail-open stdin/stdout helpers.
//!
//! Reading parses one JSON value; an empty or invalid stream degrades to
//! `Value::Null` (never an error) so the harness can still render a fail-open
//! response. Writing serializes a value and appends a newline.

use std::io::{Read, Write};

/// Read all of stdin and parse it as one JSON value. Fail-open: any read or
/// parse failure (including empty/whitespace input) degrades to `Value::Null`.
pub fn read_stdin_json() -> serde_json::Value {
    let stdin = std::io::stdin();
    read_json_from(stdin.lock())
}

/// Pure core: read everything from `reader` and parse one JSON value. Any error
/// (I/O, empty, invalid) degrades to `Value::Null`.
fn read_json_from<R: Read>(mut reader: R) -> serde_json::Value {
    let mut buf = String::new();
    if reader.read_to_string(&mut buf).is_err() {
        return serde_json::Value::Null;
    }
    if buf.trim().is_empty() {
        return serde_json::Value::Null;
    }
    serde_json::from_str(&buf).unwrap_or(serde_json::Value::Null)
}

/// Write `value` as JSON to stdout, followed by a newline. Best-effort: write
/// failures are swallowed (the process still exits 0 in the harness).
pub fn write_stdout(value: &serde_json::Value) {
    let rendered = render_to_string(value);
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    let _ = handle.write_all(rendered.as_bytes());
    let _ = handle.flush();
}

/// Pure core: serialize `value` to a compact JSON string with a trailing
/// newline. Serialization of an already-valid `serde_json::Value` cannot fail;
/// the fallback string keeps the function total without unwrap.
fn render_to_string(value: &serde_json::Value) -> String {
    let body = serde_json::to_string(value).unwrap_or_else(|_| "{\"continue\":true}".to_string());
    format!("{body}\n")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn parse_reader_reads_valid_object() {
        let raw = br#"{"hook_event_name":"SessionStart","cwd":"/tmp"}"#;
        let value = read_json_from(&raw[..]);
        assert_eq!(
            value.get("hook_event_name").and_then(|v| v.as_str()),
            Some("SessionStart")
        );
    }

    #[test]
    fn empty_stream_is_null() {
        let raw: &[u8] = b"";
        let value = read_json_from(raw);
        assert_eq!(value, serde_json::Value::Null);
    }

    #[test]
    fn invalid_json_is_null() {
        let raw: &[u8] = b"not json at all {{{";
        let value = read_json_from(raw);
        assert_eq!(value, serde_json::Value::Null);
    }

    #[test]
    fn whitespace_only_is_null() {
        let raw: &[u8] = b"   \n\t  ";
        let value = read_json_from(raw);
        assert_eq!(value, serde_json::Value::Null);
    }

    #[test]
    fn render_to_string_appends_newline() {
        let value = serde_json::json!({"continue": true});
        let out = render_to_string(&value);
        assert!(out.ends_with('\n'), "output must end with newline: {out:?}");
        assert!(out.contains("\"continue\":true"));
    }
}
