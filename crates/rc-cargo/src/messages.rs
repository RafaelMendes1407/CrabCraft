//! Cargo's `--message-format=json` lines.

use rc_dotnet::diagnostics::{Fix, source_hash};
use rc_dotnet::{Diagnostic, Severity};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    /// A compiler message: its position, if it has one, and the text as rustc prints it.
    Compiler {
        diagnostic: Option<Box<Diagnostic>>,
        rendered: String,
    },
    /// A finished target; `executable` for those that can run.
    Artifact {
        executable: Option<PathBuf>,
    },
    BuildFinished {
        success: bool,
    },
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
    let fix = (!origin.is_empty()).then(|| suggestion(m, &origin, root)).flatten();
    Some(Diagnostic {
        file: (!origin.is_empty()).then(|| root.join(&origin)),
        origin,
        line: span.and_then(|s| line_of(s, "line_start")),
        column: span.and_then(|s| line_of(s, "column_start")),
        end_line: span.and_then(|s| line_of(s, "line_end")),
        end_column: span.and_then(|s| line_of(s, "column_end")),
        severity,
        code: code.clone(),
        message: m.get("message").and_then(Value::as_str).unwrap_or_default().to_string(),
        project: None,
        borrow: borrow_diagnostic(m, root, &code),
        fix,
    })
}

/// Only codes whose compiler spans describe borrow/lifetime failures. No inference from
/// rendered English labels (rustc can change or localize them).
pub fn borrow_code(code: &str) -> bool {
    matches!(code, "E0499" | "E0500" | "E0501" | "E0502" | "E0503" | "E0505" | "E0506" | "E0507" | "E0515" | "E0597" | "E0621" | "E0623" | "E0716")
}
fn borrow_diagnostic(message: &Value, root: &Path, code: &str) -> Option<rc_dotnet::diagnostics::BorrowDiagnostic> {
    use rc_dotnet::diagnostics::{BorrowDiagnostic, BorrowLine, BorrowSpan};
    if !borrow_code(code) {
        return None;
    }
    let mut result = BorrowDiagnostic { root: root.to_path_buf(), spans: Vec::new(), notes: Vec::new(), limited: false };
    let mut todo = vec![(message, 0)];
    let mut remaining = 64 * 1024_usize;
    let mut nodes = 0;
    let mut span_budget = 128_usize;
    while let Some((node, depth)) = todo.pop() {
        nodes += 1;
        if nodes > 32 {
            result.limited = true;
            break;
        }
        if depth > 0
            && let Some(note) = node.get("message").and_then(Value::as_str)
        {
            if result.notes.len() < 16 && note.len() <= 2048 && note.len() <= remaining {
                result.notes.push(note.to_string());
                remaining -= note.len();
            } else {
                result.limited = true;
            }
        }
        for span in node.get("spans").and_then(Value::as_array).into_iter().flatten() {
            if span_budget == 0 {
                result.limited = true;
                break;
            }
            span_budget -= 1;
            if result.spans.len() >= 32 {
                result.limited = true;
                break;
            }
            let Some(file) = span.get("file_name").and_then(Value::as_str).filter(|name| !name.is_empty() && name.len() <= 8192) else {
                result.limited = true;
                continue;
            };
            let Some(start) = span.get("byte_start").and_then(Value::as_u64).and_then(|n| usize::try_from(n).ok()) else { continue };
            let Some(end) = span.get("byte_end").and_then(Value::as_u64).and_then(|n| usize::try_from(n).ok()).filter(|end| *end >= start) else { continue };
            let (Some(line), Some(column), Some(end_line), Some(end_column)) =
                (line_of(span, "line_start"), line_of(span, "column_start"), line_of(span, "line_end"), line_of(span, "column_end"))
            else {
                continue;
            };
            if line == 0 || column == 0 || end_line < line || end_column == 0 {
                continue;
            }
            let label = span.get("label").and_then(Value::as_str).unwrap_or("");
            if label.len() > 2048 || label.len() + file.len() > remaining {
                result.limited = true;
                continue;
            }
            remaining -= label.len() + file.len();
            let mut text = Vec::new();
            for (index, item) in span.get("text").and_then(Value::as_array).into_iter().flatten().enumerate() {
                let Some(source) = item.get("text").and_then(Value::as_str) else { continue };
                if index >= 8 || source.len() > 4096 || source.len() > remaining {
                    result.limited = true;
                    break;
                }
                remaining -= source.len();
                let Some(number) = line.checked_add(index as u32) else { break };
                text.push(BorrowLine {
                    line: number,
                    text: source.into(),
                    start: line_of(item, "highlight_start").unwrap_or(1),
                    end: line_of(item, "highlight_end").unwrap_or(1),
                });
            }
            let range = BorrowSpan {
                file: root.join(file),
                start,
                end,
                line,
                column,
                end_line,
                end_column,
                primary: span.get("is_primary").and_then(Value::as_bool).unwrap_or(false),
                label: label.into(),
                text,
            };
            if !result.spans.contains(&range) {
                result.spans.push(range);
            }
        }
        if let Some(children) = node.get("children").and_then(Value::as_array) {
            if depth >= 4 {
                if !children.is_empty() {
                    result.limited = true;
                }
            } else {
                if children.len() > 16 {
                    result.limited = true;
                }
                todo.extend(children.iter().take(16).rev().map(|child| (child, depth + 1)));
            }
        }
    }
    (!result.spans.is_empty()).then_some(result)
}

/// The first suggestion of a message that edits only `origin`, read against that file as it is now.
/// Suggestions with placeholders cannot be applied as they are.
fn suggestion(m: &Value, origin: &str, root: &Path) -> Option<Fix> {
    let children = m.get("children")?.as_array()?;
    children.iter().find_map(|child| {
        let spans = child.get("spans")?.as_array()?;
        let mut edits = Vec::new();
        for span in spans {
            let text = span.get("suggested_replacement")?.as_str()?;
            let usable = matches!(span.get("suggestion_applicability")?.as_str()?, "MachineApplicable" | "MaybeIncorrect");
            if !usable || span.get("file_name")?.as_str()? != origin {
                return None;
            }
            let start = usize::try_from(span.get("byte_start")?.as_u64()?).ok()?;
            let end = usize::try_from(span.get("byte_end")?.as_u64()?).ok()?;
            edits.push((start, end, text.to_string()));
        }
        edits.sort();
        let ordered = edits.iter().all(|e| e.0 <= e.1) && edits.windows(2).all(|w| w[0].1 <= w[1].0);
        if edits.is_empty() || !ordered {
            return None;
        }
        let source = source_hash(&std::fs::read(root.join(origin)).ok()?);
        Some(Fix { label: child.get("message")?.as_str()?.to_string(), source, edits })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiler_ranges_preserve_child_context_and_reject_malformed_positions() {
        let value: Value = serde_json::from_str(r#"{"spans":[{"file_name":"src/lib.rs","byte_start":4,"byte_end":5,"line_start":1,"line_end":1,"column_start":5,"column_end":6,"is_primary":true,"label":null,"text":[{"text":"let x = y;","highlight_start":5,"highlight_end":6}]}],"children":[{"message":"compiler note","spans":[{"file_name":"src/lib.rs","byte_start":0,"byte_end":3,"line_start":1,"line_end":1,"column_start":1,"column_end":4,"is_primary":false,"label":"borrow begins here","text":[{"text":"let x = y;","highlight_start":1,"highlight_end":4}]},{"file_name":"src/lib.rs","byte_start":10,"byte_end":1}]}]}"#).unwrap();
        let details = borrow_diagnostic(&value, Path::new("/workspace"), "E0502").unwrap();
        assert_eq!(details.spans.len(), 2);
        assert!(details.spans[0].primary);
        assert!(!details.spans[1].primary);
        assert_eq!(details.spans[1].label, "borrow begins here");
        assert_eq!(details.notes, ["compiler note"]);
        assert!(borrow_diagnostic(&value, Path::new("/workspace"), "E0308").is_none());
    }

    fn fix_of(children: &str, root: &Path) -> Option<Fix> {
        let m: Value = serde_json::from_str(&format!(r#"{{"children":{children}}}"#)).unwrap();
        suggestion(&m, "src/a.rs", root)
    }

    fn child(file: &str, kind: &str) -> String {
        format!(
            r#"{{"message":"use this","spans":[{{"file_name":"{file}","byte_start":4,"byte_end":5,"suggested_replacement":"x","suggestion_applicability":"{kind}"}}]}}"#
        )
    }

    #[test]
    fn only_complete_suggestions_for_the_same_file_become_fixes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/a.rs"), "let y = 1;\n").unwrap();
        let fix = fix_of(&format!("[{}]", child("src/a.rs", "MachineApplicable")), dir.path()).unwrap();
        assert_eq!((fix.label.as_str(), fix.edits), ("use this", vec![(4, 5, "x".to_string())]));
        assert_eq!(fix.source, source_hash(b"let y = 1;\n"));

        assert_eq!(fix_of(&format!("[{}]", child("src/a.rs", "HasPlaceholders")), dir.path()), None);
        assert_eq!(fix_of(&format!("[{}]", child("src/b.rs", "MachineApplicable")), dir.path()), None);
        assert_eq!(fix_of(r#"[{"message":"note","spans":[]}]"#, dir.path()), None);
        // A later child can still offer one.
        let list = format!("[{},{}]", child("src/a.rs", "Unspecified"), child("src/a.rs", "MaybeIncorrect"));
        assert!(fix_of(&list, dir.path()).is_some());
        // A file that cannot be read has nothing to check the offsets against.
        assert_eq!(fix_of(&format!("[{}]", child("src/a.rs", "MachineApplicable")), &dir.path().join("gone")), None);
    }

    fn compiler(level: &str, spans: &str, code: &str) -> String {
        format!(r#"{{"reason":"compiler-message","message":{{"message":"boom","level":"{level}","code":{code},"spans":{spans},"rendered":"error: boom\n"}}}}"#)
    }

    #[test]
    fn reads_the_primary_span_of_an_error() {
        let spans = r#"[{"file_name":"src/other.rs","is_primary":false,"line_start":1,"column_start":1,"line_end":1,"column_end":2},
                        {"file_name":"src/main.rs","is_primary":true,"line_start":3,"column_start":5,"line_end":3,"column_end":9}]"#
            .replace('\n', " ");
        let Some(Message::Compiler { diagnostic: Some(d), rendered }) = parse_message(&compiler("error", &spans, r#"{"code":"E0308"}"#), Path::new("/w"))
        else {
            panic!()
        };
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
        assert_eq!(
            parse_message(r#"{"reason":"compiler-artifact","executable":"/t/app"}"#, Path::new("/")),
            Some(Message::Artifact { executable: Some("/t/app".into()) })
        );
        assert_eq!(parse_message(r#"{"reason":"compiler-artifact","executable":null}"#, Path::new("/")), Some(Message::Artifact { executable: None }));
        assert_eq!(parse_message(r#"{"reason":"build-finished","success":false}"#, Path::new("/")), Some(Message::BuildFinished { success: false }));
        assert_eq!(parse_message(r#"{"reason":"build-script-executed"}"#, Path::new("/")), Some(Message::Other));
        assert_eq!(parse_message("hello from a test", Path::new("/")), None);
        assert_eq!(parse_message("{not json", Path::new("/")), None);
        assert_eq!(parse_message(r#"{"a":1}"#, Path::new("/")), None);
    }
}
