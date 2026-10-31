//! Cargo's `--message-format=json` lines.

use rc_dotnet::{Diagnostic, Severity};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    /// A compiler message: its position, if it has one, and the text as rustc prints it.
    Compiler { diagnostic: Option<Box<Diagnostic>>, rendered: String },
    /// A finished target; `executable` for those that can run.
    Artifact { executable: Option<PathBuf> },
    BuildFinished { success: bool },
    /// Any other Cargo message (build script output, for instance).
    Other,
}

/// Parses one stdout line; `None` when it is not JSON from Cargo (a test printing, for instance).
/// `root` is the workspace root: rustc reports paths relative to it.
pub fn parse_message(line: &str, root: &Path) -> Option<Message> {
    let trimmed = line.trim();
    if !trimmed.starts_with('{') {
        return None;
    }
    let v: Value = serde_json::from_str(trimmed).ok()?;
    let reason = v.get("reason")?.as_str()?;
    Some(match reason {
        "compiler-message" => {
            let m = v.get("message")?;
            let rendered = m.get("rendered").and_then(Value::as_str).unwrap_or_default().trim_end().to_string();
            Message::Compiler { diagnostic: diagnostic(m, root).map(Box::new), rendered }
        }
        "compiler-artifact" => Message::Artifact { executable: v.get("executable").and_then(Value::as_str).map(PathBuf::from) },
        "build-finished" => Message::BuildFinished { success: v.get("success").and_then(Value::as_bool).unwrap_or(false) },
        _ => Message::Other,
    })
}

fn line_of(span: &Value, key: &str) -> Option<u32> {
    span.get(key).and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok())
}

/// The position of a rustc message. Summaries without a position or code ("aborting due to...")
/// are not problems of their own.
fn diagnostic(m: &Value, root: &Path) -> Option<Diagnostic> {
    let severity = match m.get("level")?.as_str()? {
        l if l.starts_with("error") => Severity::Error,
        "warning" => Severity::Warning,
        _ => Severity::Info,
    };
    let code = m.get("code").and_then(|c| c.get("code")).and_then(Value::as_str).unwrap_or_default().to_string();
    let spans = m.get("spans").and_then(Value::as_array).map_or(&[][..], Vec::as_slice);
    let span = spans.iter().find(|s| s.get("is_primary").and_then(Value::as_bool).unwrap_or(false)).or(spans.first());
    if span.is_none() && code.is_empty() {
        return None;
    }
    let origin = span.and_then(|s| s.get("file_name")).and_then(Value::as_str).unwrap_or_default().to_string();
    Some(Diagnostic {
        file: (!origin.is_empty()).then(|| root.join(&origin)),
        origin,
        line: span.and_then(|s| line_of(s, "line_start")),
        column: span.and_then(|s| line_of(s, "column_start")),
        end_line: span.and_then(|s| line_of(s, "line_end")),
        end_column: span.and_then(|s| line_of(s, "column_end")),
        severity,
        code,
        message: m.get("message").and_then(Value::as_str).unwrap_or_default().to_string(),
        project: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compiler(level: &str, spans: &str, code: &str) -> String {
        format!(r#"{{"reason":"compiler-message","message":{{"message":"boom","level":"{level}","code":{code},"spans":{spans},"rendered":"error: boom\n"}}}}"#)
    }

    #[test]
    fn reads_the_primary_span_of_an_error() {
        let spans = r#"[{"file_name":"src/other.rs","is_primary":false,"line_start":1,"column_start":1,"line_end":1,"column_end":2},
                        {"file_name":"src/main.rs","is_primary":true,"line_start":3,"column_start":5,"line_end":3,"column_end":9}]"#.replace('\n', " ");
        let Some(Message::Compiler { diagnostic: Some(d), rendered }) = parse_message(&compiler("error", &spans, r#"{"code":"E0308"}"#), Path::new("/w")) else { panic!() };
        assert_eq!(d.file.as_deref(), Some(Path::new("/w/src/main.rs")));
        assert_eq!((d.line, d.column, d.end_line, d.end_column), (Some(3), Some(5), Some(3), Some(9)));
        assert_eq!((d.severity, d.code.as_str(), d.message.as_str()), (Severity::Error, "E0308", "boom"));
        assert_eq!(rendered, "error: boom");
    }

    #[test]
    fn summaries_are_not_problems_but_coded_errors_without_a_span_are() {
        let m = parse_message(&compiler("error", "[]", "null"), Path::new("/w"));
        assert!(matches!(m, Some(Message::Compiler { diagnostic: None, .. })));
        let m = parse_message(&compiler("error", "[]", r#"{"code":"E0601"}"#), Path::new("/w"));
        let Some(Message::Compiler { diagnostic: Some(d), .. }) = m else { panic!() };
        assert_eq!((d.file, d.code.as_str()), (None, "E0601"));
    }

    #[test]
    fn other_messages_and_plain_output() {
        assert_eq!(parse_message(r#"{"reason":"compiler-artifact","executable":"/t/app"}"#, Path::new("/")), Some(Message::Artifact { executable: Some("/t/app".into()) }));
        assert_eq!(parse_message(r#"{"reason":"compiler-artifact","executable":null}"#, Path::new("/")), Some(Message::Artifact { executable: None }));
        assert_eq!(parse_message(r#"{"reason":"build-finished","success":false}"#, Path::new("/")), Some(Message::BuildFinished { success: false }));
        assert_eq!(parse_message(r#"{"reason":"build-script-executed"}"#, Path::new("/")), Some(Message::Other));
        assert_eq!(parse_message("hello from a test", Path::new("/")), None);
        assert_eq!(parse_message("{not json", Path::new("/")), None);
        assert_eq!(parse_message(r#"{"a":1}"#, Path::new("/")), None);
    }
}
