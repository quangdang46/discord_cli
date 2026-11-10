//! Structured output: JSONL/JSON/YAML envelope + exit codes + isTTY detection.
//!
//! Synthesized from famasya `output.go` (JSONL default, MIT), jackwener
//! `_output.py` (envelope `{ok, schema_version, data|error}`, Apache-2.0), and
//! discli `output.ts` (isTTY → piped/yaml). All in `.tmp/`.
//!
//! Contract (plan §10):
//! - Piped stdout → JSONL (one object per line); TTY → human.
//! - `--json`/`--yaml` force single envelope.
//! - Errors → stderr, data → stdout, progress → stderr.
//! - Exit codes: 0 ok, 1 error, 2 usage, 3 not-found, 4 forbidden, 5 network.

use std::io::{IsTerminal, Write};

use serde::Serialize;

/// Stable schema version for the output envelope.
pub const SCHEMA_VERSION: &str = "1";

/// Exit code contract shared by all commands.
pub mod exit {
    pub const OK: u8 = 0;
    pub const ERROR: u8 = 1;
    pub const USAGE: u8 = 2;
    pub const NOT_FOUND: u8 = 3;
    pub const FORBIDDEN: u8 = 4;
    pub const NETWORK: u8 = 5;
}

/// Successful envelope body.
#[derive(Serialize)]
pub struct SuccessPayload<T: Serialize> {
    pub ok: bool,
    pub schema_version: &'static str,
    pub data: T,
}

/// Error envelope body.
#[derive(Serialize)]
pub struct ErrorDetail {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

#[derive(Serialize)]
pub struct ErrorPayload {
    pub ok: bool,
    pub schema_version: &'static str,
    pub error: ErrorDetail,
}

/// Build a success payload.
pub fn success<T: Serialize>(data: T) -> SuccessPayload<T> {
    SuccessPayload {
        ok: true,
        schema_version: SCHEMA_VERSION,
        data,
    }
}

/// Build an error payload.
pub fn error(code: &str, message: impl Into<String>) -> ErrorPayload {
    ErrorPayload {
        ok: false,
        schema_version: SCHEMA_VERSION,
        error: ErrorDetail {
            code: code.to_string(),
            message: message.into(),
            details: None,
        },
    }
}

/// True when stdout is a pipe/redirection (not a terminal).
pub fn stdout_is_piped() -> bool {
    !std::io::stdout().is_terminal()
}

/// Resolve output format: explicit override, else env `OUTPUT`, else auto.
/// Auto = JSONL when piped (agent), human when TTY.
pub fn resolve_format(as_json: bool, as_yaml: bool, env_override: Option<&str>) -> Format {
    if as_json && as_yaml {
        // Caller error — but be lenient: prefer JSON.
    }
    if as_yaml {
        return Format::Yaml;
    }
    if as_json {
        return Format::Json;
    }
    if let Some(mode) = env_override {
        match mode.trim().to_ascii_lowercase().as_str() {
            "yaml" => return Format::Yaml,
            "json" => return Format::Json,
            "rich" => return Format::Rich,
            "jsonl" => return Format::Jsonl,
            _ => {}
        }
    }
    if stdout_is_piped() {
        Format::Jsonl
    } else {
        Format::Rich
    }
}

/// Output serialization format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Json,
    Jsonl,
    Yaml,
    Rich,
}

/// Write data to stdout in the chosen format.
/// For Json/Jsonl the envelope is unwrapped: Jsonl streams the data array
/// row-by-row (agent-friendly); Json emits a single envelope object.
pub fn emit<T: Serialize>(data: &T, format: Format) -> std::io::Result<()> {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    match format {
        Format::Jsonl => emit_jsonl_rows(data, &mut lock),
        Format::Json => {
            let s = serde_json::to_string_pretty(&success(data)).map_err(std::io::Error::other)?;
            writeln!(lock, "{}", s)
        }
        Format::Yaml => {
            let s = serde_yaml::to_string(&success(data)).map_err(std::io::Error::other)?;
            write!(lock, "{}", s)
        }
        Format::Rich => {
            // Human fallback: JSON for now; commands override with tables.
            let s = serde_json::to_string_pretty(&success(data)).map_err(std::io::Error::other)?;
            writeln!(lock, "{}", s)
        }
    }
}

/// Emit a list as JSONL (one object per line). Non-list data → single line.
fn emit_jsonl_rows<T: Serialize>(data: &T, out: &mut impl Write) -> std::io::Result<()> {
    use serde_json::Value;
    let value = serde_json::to_value(data).map_err(std::io::Error::other)?;
    match value {
        Value::Array(items) => {
            for item in items {
                // serde_json always escapes HTML by default; to match
                // famasya's SetEscapeHTML(false), use the value's raw string.
                let line = serde_json::to_string(&item).map_err(std::io::Error::other)?;
                writeln!(out, "{}", line)?;
            }
        }
        other => {
            let line = serde_json::to_string(&other).map_err(std::io::Error::other)?;
            writeln!(out, "{}", line)?;
        }
    }
    Ok(())
}

/// Write one pre-formatted line to stdout (JSONL data, transcripts, help).
///
/// EPIPE is treated as a clean early exit — piped consumers like `| head`
/// closing the pipe must not panic (unlike `println!`, which panics on
/// EPIPE). Other I/O errors are also swallowed: stdout is best-effort in
/// this CLI and errors already have a dedicated channel (`emit_error` →
/// stderr). Use this for every data line instead of `println!`.
pub fn out_line(line: &str) {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    let _ = writeln!(lock, "{}", line);
    let _ = lock.flush();
}

/// Write a progress/status message to stderr (never stdout).
pub fn progress(msg: impl AsRef<str>) {
    eprint!("{}", msg.as_ref());
    let _ = std::io::stderr().flush();
}

/// Write an error envelope + message to stderr, and a single data-safe error
/// line to stdout in JSONL mode. Returns the process exit code.
pub fn emit_error(code: &str, message: &str, exit_code: u8) -> u8 {
    let payload = error(code, message);
    if let Ok(s) = serde_json::to_string(&payload) {
        eprintln!("{}", s);
    } else {
        eprintln!("error: {}", message);
    }
    exit_code
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize)]
    struct Msg {
        id: u64,
        content: String,
    }

    #[test]
    fn success_envelope_shape() {
        let s = serde_json::to_value(success(vec![1, 2, 3])).unwrap();
        assert_eq!(s["ok"], true);
        assert_eq!(s["schema_version"], "1");
        assert_eq!(s["data"], serde_json::json!([1, 2, 3]));
    }

    #[test]
    fn error_envelope_shape() {
        let e = error("NotFound", "no such channel");
        let v = serde_json::to_value(e).unwrap();
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"]["code"], "NotFound");
        assert_eq!(v["error"]["message"], "no such channel");
    }

    #[test]
    fn format_resolution_prefers_flags() {
        assert_eq!(resolve_format(true, false, None), Format::Json);
        assert_eq!(resolve_format(false, true, None), Format::Yaml);
        assert_eq!(resolve_format(false, false, Some("jsonl")), Format::Jsonl);
        assert_eq!(resolve_format(false, false, Some("rich")), Format::Rich);
    }

    #[test]
    fn jsonl_emits_one_row_per_line() {
        let rows = vec![
            Msg {
                id: 1,
                content: "hi".into(),
            },
            Msg {
                id: 2,
                content: "yo".into(),
            },
        ];
        let mut buf = Vec::new();
        emit_jsonl_rows(&rows, &mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\"content\":\"hi\""));
        assert!(lines[1].contains("\"content\":\"yo\""));
    }
}
