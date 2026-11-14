//! Explicit, allowlisted import of portable editor preferences. No programs or trust are imported.
use crate::Settings;
use serde_json::{Map, Value, json};
use std::{
    collections::BTreeSet,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Source {
    VsCode,
    RustRover,
}
#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Format,
    Value,
    Limit,
    Cancelled,
}
#[derive(Clone, Debug)]
pub struct Preview {
    pub changes: Map<String, Value>,
    pub ignored: Vec<String>,
}
impl Preview {
    pub fn apply(&self, current: &Settings) -> Result<Settings, Error> {
        let mut value = serde_json::to_value(current).map_err(|_| Error::Value)?;
        let object = value.as_object_mut().ok_or(Error::Value)?;
        for (key, value) in &self.changes {
            object.insert(key.clone(), value.clone());
        }
        serde_json::from_value(value).map_err(|_| Error::Value)
    }
}
pub fn read(source: Source, path: &Path, cancel: &AtomicBool) -> Result<Preview, Error> {
    use std::io::Read;
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let mut file = options.open(path).map_err(Error::Io)?;
    let meta = file.metadata().map_err(Error::Io)?;
    if !meta.is_file() {
        return Err(Error::Format);
    }
    if meta.len() > 1024 * 1024 {
        return Err(Error::Limit);
    }
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let n = file.read(&mut chunk).map_err(Error::Io)?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..n]);
        if bytes.len() > 1024 * 1024 {
            return Err(Error::Limit);
        }
    }
    let text = std::str::from_utf8(bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes)).map_err(|_| Error::Format)?;
    parse(source, text, cancel)
}
pub fn parse(source: Source, text: &str, cancel: &AtomicBool) -> Result<Preview, Error> {
    if text.len() > 1024 * 1024 {
        return Err(Error::Limit);
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }
    let mut changes = Map::new();
    let mut ignored = BTreeSet::new();
    changes.insert("keymap".into(), json!(if source == Source::VsCode { "VsCode" } else { "RustRover" }));
    match source {
        Source::VsCode => {
            let value: Value = serde_json::from_str(&jsonc(text)?).map_err(|_| Error::Format)?;
            let mut object = value.as_object().ok_or(Error::Format)?.clone();
            if let Some(rust) = object.remove("[rust]") {
                object.extend(rust.as_object().ok_or(Error::Value)?.clone());
            }
            for (key, value) in &object {
                vscode(key, value, &mut changes, &mut ignored)?;
            }
        }
        Source::RustRover => {
            use quick_xml::events::Event;
            let mut reader = quick_xml::Reader::from_str(text);
            let mut depth = 0usize;
            let mut count = 0usize;
            let mut root = false;
            let mut components = Vec::new();
            loop {
                if cancel.load(Ordering::Relaxed) {
                    return Err(Error::Cancelled);
                }
                count += 1;
                if count > 65536 {
                    return Err(Error::Limit);
                }
                let event = reader.read_event().map_err(|_| Error::Format)?;
                match event {
                    Event::Start(ref element) | Event::Empty(ref element) => {
                        let name = String::from_utf8_lossy(element.name().as_ref()).into_owned();
                        if depth == 0 {
                            if root || !matches!(name.as_str(), "application" | "component" | "scheme" | "keymap") {
                                return Err(Error::Format);
                            }
                            root = true;
                        }
                        let mut attrs = std::collections::BTreeMap::new();
                        for attr in element.attributes() {
                            let attr = attr.map_err(|_| Error::Format)?;
                            let key = std::str::from_utf8(attr.key.as_ref()).map_err(|_| Error::Format)?.to_string();
                            let value = attr.normalized_value(quick_xml::XmlVersion::Implicit1_0).map_err(|_| Error::Format)?.into_owned();
                            if value.len() > 4096 || attrs.insert(key, value).is_some() {
                                return Err(Error::Limit);
                            }
                        }
                        if name == "component" {
                            components.push(attrs.get("name").cloned().unwrap_or_default());
                        }
                        if name == "option" {
                            let key = attrs.get("name").ok_or(Error::Format)?;
                            let value = attrs.get("value").map(String::as_str).unwrap_or("");
                            let component = components.last().map(String::as_str).unwrap_or("");
                            match (key.as_str(), component) {
                                ("FONT_SIZE", "DefaultFont") | ("EDITOR_FONT_SIZE", "") => {
                                    number("editor_font_size", value.parse().map_err(|_| Error::Value)?, 8.0, 48.0, &mut changes)?
                                }
                                ("LINE_SPACING", "DefaultFont") | ("EDITOR_LINE_SPACING", "") => {
                                    number("editor_line_spacing", value.parse().map_err(|_| Error::Value)?, 1.0, 2.0, &mut changes)?
                                }
                                ("IS_INDENT_GUIDES_SHOWN", "EditorSettings") => {
                                    changes.insert("show_indent_guides".into(), json!(boolean(value)?));
                                }
                                ("RIGHT_MARGIN", "EditorSettings") => {
                                    let n: u64 = value.parse().map_err(|_| Error::Value)?;
                                    if !(40..=300).contains(&n) {
                                        return Err(Error::Value);
                                    }
                                    changes.insert("right_margin".into(), json!(n));
                                }
                                ("saveOnFrameDeactivation", "GeneralSettings") => {
                                    changes.insert("autosave".into(), json!(boolean(value)?));
                                }
                                _ => {
                                    ignored.insert(key.clone());
                                }
                            }
                        }
                        if name == "scheme" {
                            if let Some(parent) = attrs.get("parent_scheme") {
                                theme(parent, &mut changes, &mut ignored);
                            }
                        }
                        if name == "global_color_scheme" {
                            if let Some(name) = attrs.get("name") {
                                theme(name, &mut changes, &mut ignored);
                            }
                        }
                        if matches!(event, Event::Start(_)) {
                            depth += 1;
                            if depth > 32 {
                                return Err(Error::Limit);
                            }
                        } else if name == "component" {
                            components.pop();
                        }
                    }
                    Event::End(element) => {
                        depth = depth.checked_sub(1).ok_or(Error::Format)?;
                        if element.name().as_ref() == b"component" {
                            components.pop();
                        }
                    }
                    Event::DocType(_) | Event::GeneralRef(_) => return Err(Error::Format),
                    Event::Text(text) if !text.iter().all(u8::is_ascii_whitespace) => {
                        return Err(Error::Format);
                    }
                    Event::Eof => break,
                    _ => (),
                }
            }
            if !root || depth != 0 {
                return Err(Error::Format);
            }
        }
    }
    if ignored.len() > 256 {
        return Err(Error::Limit);
    }
    Ok(Preview { changes, ignored: ignored.into_iter().collect() })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn jsonc_strings_rust_overrides_and_safe_allowlist_survive_import() {
        let preview = parse(
            Source::VsCode,
            r#"{
          // Rust preferences override the general ones.
          "editor.fontSize": 12, "editor.lineHeight": 24,
          "workbench.colorTheme": "Default Light Modern",
          "terminal.integrated.defaultProfile.linux":"danger // literal",
          "rust-analyzer.server.path":"./malicious",
          "[rust]": {"editor.fontSize":16,"editor.rulers":[100,],},
        }"#,
            &AtomicBool::new(false),
        )
        .unwrap();
        let mut old = Settings::default();
        old.trusted_projects.push("/kept".into());
        let new = preview.apply(&old).unwrap();
        assert_eq!(new.editor_font_size, 16.0);
        assert_eq!(new.editor_line_spacing, 1.5);
        assert_eq!(new.right_margin, 100);
        assert_eq!(new.theme, crate::settings::ThemeChoice::Light);
        assert_eq!(new.keymap, crate::settings::KeymapChoice::VsCode);
        assert_eq!(new.trusted_projects, old.trusted_projects);
        assert_eq!(new.rust_analyzer_path, None);
        assert!(preview.ignored.iter().any(|key| key == "rust-analyzer.server.path"));
        assert!(jsonc(r#"{"url":"https://example.test/*literal*/",}"#).unwrap().contains("https://example.test/*literal*/"));
    }
    #[test]
    fn rust_editor_xml_imports_only_recognized_preferences() {
        let text = r#"<application><component name="DefaultFont"><option name="FONT_SIZE" value="18"/><option name="LINE_SPACING" value="1.4"/></component><component name="EditorSettings"><option name="RIGHT_MARGIN" value="110"/><option name="IS_INDENT_GUIDES_SHOWN" value="false"/></component><component name="Unknown"><option name="FONT_SIZE" value="500"/></component></application>"#;
        let preview = parse(Source::RustRover, text, &AtomicBool::new(false)).unwrap();
        let settings = preview.apply(&Settings::default()).unwrap();
        assert_eq!(settings.editor_font_size, 18.0);
        assert_eq!(settings.right_margin, 110);
        assert!(!settings.show_indent_guides);
        assert_eq!(settings.keymap, crate::settings::KeymapChoice::RustRover);
        assert_eq!(preview.ignored, vec!["FONT_SIZE"]);
        assert!(matches!(
            parse(Source::RustRover, r#"<!DOCTYPE application [<!ENTITY x SYSTEM "file:///etc/passwd">]><application/>"#, &AtomicBool::new(false)),
            Err(Error::Format)
        ));
        assert!(matches!(parse(Source::RustRover, "<application><component></application>", &AtomicBool::new(false)), Err(Error::Format)));
    }
    #[test]
    fn invalid_limits_and_cancelled_input_never_produce_an_import() {
        assert!(matches!(parse(Source::VsCode, r#"{"editor.fontSize":9999}"#, &AtomicBool::new(false)), Err(Error::Value)));
        assert!(matches!(parse(Source::VsCode, "/* unfinished", &AtomicBool::new(false)), Err(Error::Format)));
        assert!(matches!(parse(Source::VsCode, "{}", &AtomicBool::new(true)), Err(Error::Cancelled)));
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(read(Source::VsCode, dir.path(), &AtomicBool::new(false)), Err(Error::Format | Error::Io(_))));
    }

    #[test]
    #[cfg(unix)]
    fn special_files_are_rejected_without_waiting_for_a_writer() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("preferences.json");
        assert!(std::process::Command::new("mkfifo").arg(&fifo).status().unwrap().success());
        let start = std::time::Instant::now();
        assert!(matches!(read(Source::VsCode, &fifo, &AtomicBool::new(false)), Err(Error::Format)));
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
    }
}
fn number(key: &str, n: f64, min: f64, max: f64, out: &mut Map<String, Value>) -> Result<(), Error> {
    if !n.is_finite() || !(min..=max).contains(&n) {
        return Err(Error::Value);
    }
    out.insert(key.into(), json!(n));
    Ok(())
}
fn boolean(value: &str) -> Result<bool, Error> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(Error::Value),
    }
}
fn theme(name: &str, out: &mut Map<String, Value>, ignored: &mut BTreeSet<String>) {
    let mapped = match name {
        "Default Dark+" | "Default Dark Modern" | "Dark+ (default dark)" | "Darcula" | "Dark" => Some("Dark"),
        "Default Light+" | "Default Light Modern" | "Light+ (default light)" | "Default" | "Light" => Some("Light"),
        _ => None,
    };
    if let Some(theme) = mapped {
        out.insert("theme".into(), json!(theme));
    } else {
        ignored.insert("theme".into());
    }
}
fn vscode(key: &str, value: &Value, out: &mut Map<String, Value>, ignored: &mut BTreeSet<String>) -> Result<(), Error> {
    match key {
        "editor.fontSize" => number("editor_font_size", value.as_f64().ok_or(Error::Value)?, 8.0, 48.0, out)?,
        "editor.lineHeight" => {
            let n = value.as_f64().ok_or(Error::Value)?;
            if n > 0.0 {
                let size = out.get("editor_font_size").and_then(Value::as_f64).unwrap_or(13.0);
                number("editor_line_spacing", if n <= 8.0 { n } else { n / size }, 1.0, 2.0, out)?;
            }
        }
        "window.zoomLevel" => number("ui_zoom", 1.2f64.powf(value.as_f64().ok_or(Error::Value)?), 0.5, 2.0, out)?,
        "editor.formatOnSave" => {
            out.insert("format_on_save".into(), json!(value.as_bool().ok_or(Error::Value)?));
        }
        "editor.stickyScroll.enabled" => {
            out.insert("sticky_lines".into(), json!(value.as_bool().ok_or(Error::Value)?));
        }
        "editor.guides.indentation" => {
            out.insert("show_indent_guides".into(), json!(value.as_bool().ok_or(Error::Value)?));
        }
        "editor.inlayHints.enabled" => {
            out.insert(
                "show_inlay_hints".into(),
                json!(match value.as_str() {
                    Some("off") => false,
                    Some("on" | "onUnlessPressed" | "offUnlessPressed") => true,
                    _ => return Err(Error::Value),
                }),
            );
        }
        "files.autoSave" => {
            out.insert(
                "autosave".into(),
                json!(match value.as_str() {
                    Some("off") => false,
                    Some("onFocusChange" | "onWindowChange") => true,
                    _ => {
                        ignored.insert(key.into());
                        return Ok(());
                    }
                }),
            );
        }
        "workbench.colorTheme" => theme(value.as_str().ok_or(Error::Value)?, out, ignored),
        "editor.rulers" => {
            let rules = value.as_array().ok_or(Error::Value)?;
            if let Some(first) = rules.first() {
                let n = first.as_u64().or_else(|| first.get("column").and_then(Value::as_u64)).ok_or(Error::Value)?;
                if !(40..=300).contains(&n) {
                    return Err(Error::Value);
                }
                out.insert("right_margin".into(), json!(n));
            }
        }
        _ => {
            ignored.insert(key.chars().take(160).collect());
        }
    }
    Ok(())
}
/// Strip comments/trailing commas without interpreting strings or changing UTF-8 offsets.
fn jsonc(text: &str) -> Result<String, Error> {
    let mut bytes = text.as_bytes().to_vec();
    let mut i = 0;
    let mut quoted = false;
    let mut escaped = false;
    while i < bytes.len() {
        let b = bytes[i];
        if quoted {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                quoted = false;
            }
            i += 1;
            continue;
        }
        if b == b'"' {
            quoted = true;
            i += 1;
            continue;
        }
        if b == b'/' && bytes.get(i + 1) == Some(&b'/') {
            while i < bytes.len() && bytes[i] != b'\n' {
                bytes[i] = b' ';
                i += 1;
            }
            continue;
        }
        if b == b'/' && bytes.get(i + 1) == Some(&b'*') {
            bytes[i] = b' ';
            bytes[i + 1] = b' ';
            i += 2;
            let mut closed = false;
            while i < bytes.len() {
                if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                    bytes[i] = b' ';
                    bytes[i + 1] = b' ';
                    i += 2;
                    closed = true;
                    break;
                }
                if !matches!(bytes[i], b'\n' | b'\r') {
                    bytes[i] = b' ';
                }
                i += 1;
            }
            if !closed {
                return Err(Error::Format);
            }
            continue;
        }
        i += 1;
    }
    if quoted {
        return Err(Error::Format);
    }
    quoted = false;
    escaped = false;
    i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if quoted {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                quoted = false;
            }
        } else if b == b'"' {
            quoted = true;
        } else if b == b',' {
            let mut j = i + 1;
            while bytes.get(j).is_some_and(u8::is_ascii_whitespace) {
                j += 1;
            }
            if matches!(bytes.get(j), Some(b'}' | b']')) {
                bytes[i] = b' ';
            }
        }
        i += 1;
    }
    String::from_utf8(bytes).map_err(|_| Error::Format)
}
