//! External tools: programs the user sets up once (a linter, a script, `cargo expand`…) and
//! runs from the IDE on the current file or project, with macros such as `$FilePath$`.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExternalTool {
    pub name: String,
    /// The program: a name looked up on `PATH`, or a path.
    pub program: String,
    /// Arguments, split like a shell does; macros are expanded inside each one.
    pub args: String,
    /// Where it runs; empty is the project folder.
    pub working_dir: String,
}

/// What the macros stand for when a tool starts.
#[derive(Clone, Debug, Default)]
pub struct MacroContext {
    pub file: Option<PathBuf>,
    pub project: Option<PathBuf>,
    /// 1-based caret position.
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub selection: String,
}

/// The macros, with what each one means (for the settings page).
pub const MACROS: [&str; 9] = [
    "$FilePath$",
    "$FileDir$",
    "$FileName$",
    "$FileNameWithoutExtension$",
    "$FileRelativePath$",
    "$ProjectFileDir$",
    "$LineNumber$",
    "$ColumnNumber$",
    "$SelectedText$",
];

/// Replaces the macros in `text`. A macro that needs something missing (no file open, no
/// project) is an error naming it, rather than an empty argument the tool would misread.
pub fn expand(text: &str, ctx: &MacroContext) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('$') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('$') else {
            out.push_str(&rest[start..]);
            return Ok(out);
        };
        let name = &after[..end];
        let value = match name {
            "" => Some("$".to_string()),
            "FilePath" => ctx.file.as_ref().map(|f| f.display().to_string()),
            "FileDir" => ctx.file.as_ref().and_then(|f| f.parent()).map(|d| d.display().to_string()),
            "FileName" => ctx.file.as_ref().and_then(|f| f.file_name()).map(|n| n.to_string_lossy().into_owned()),
            "FileNameWithoutExtension" => ctx.file.as_ref().and_then(|f| f.file_stem()).map(|n| n.to_string_lossy().into_owned()),
            "FileRelativePath" => match (&ctx.file, &ctx.project) {
                (Some(f), Some(p)) => f.strip_prefix(p).ok().map(|r| r.display().to_string()),
                _ => None,
            },
            "ProjectFileDir" => ctx.project.as_ref().map(|p| p.display().to_string()),
            "LineNumber" => ctx.line.map(|n| n.to_string()),
            "ColumnNumber" => ctx.column.map(|n| n.to_string()),
            "SelectedText" => Some(ctx.selection.clone()),
            // Not a macro: `$` signs of the user's own text stay.
            _ if !name.chars().all(|c| c.is_ascii_alphanumeric()) => {
                out.push('$');
                rest = after;
                continue;
            }
            _ => return Err(format!("${name}$")),
        };
        match value {
            Some(v) => out.push_str(&v),
            None => return Err(format!("${name}$")),
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// The folder a tool runs in: its own (macros expanded, relative to the project), else the
/// project's, else the file's.
pub fn working_dir(tool: &ExternalTool, ctx: &MacroContext) -> Result<Option<PathBuf>, String> {
    let dir = expand(tool.working_dir.trim(), ctx)?;
    if dir.is_empty() {
        return Ok(ctx.project.clone().or_else(|| ctx.file.as_ref().and_then(|f| f.parent()).map(Path::to_path_buf)));
    }
    let dir = PathBuf::from(dir);
    Ok(Some(match &ctx.project {
        Some(p) if dir.is_relative() => p.join(dir),
        _ => dir,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> MacroContext {
        MacroContext { file: Some("/w/src/main.rs".into()), project: Some("/w".into()), line: Some(3), column: Some(7), selection: "x + 1".into() }
    }

    #[test]
    fn macros_expand_to_the_file_and_caret() {
        let c = ctx();
        assert_eq!(expand("$FilePath$", &c).unwrap(), "/w/src/main.rs");
        assert_eq!(expand("--dir=$FileDir$", &c).unwrap(), "--dir=/w/src");
        assert_eq!(expand("$FileName$ $FileNameWithoutExtension$", &c).unwrap(), "main.rs main");
        assert_eq!(expand("$FileRelativePath$:$LineNumber$:$ColumnNumber$", &c).unwrap(), "src/main.rs:3:7");
        assert_eq!(expand("$ProjectFileDir$/target", &c).unwrap(), "/w/target");
        assert_eq!(expand("$SelectedText$", &c).unwrap(), "x + 1");
        assert_eq!(expand("cost: $$5 and $ alone, $a-b$", &c).unwrap(), "cost: $5 and $ alone, $a-b$");
    }

    #[test]
    fn a_macro_without_a_value_is_an_error() {
        let none = MacroContext::default();
        assert_eq!(expand("$FilePath$", &none), Err("$FilePath$".into()));
        assert_eq!(expand("$ProjectFileDir$", &none), Err("$ProjectFileDir$".into()));
        assert_eq!(expand("$Unknown$", &ctx()), Err("$Unknown$".into()));
        assert_eq!(expand("plain", &none).unwrap(), "plain");
    }

    #[test]
    fn the_working_folder_defaults_to_the_project() {
        let mut tool = ExternalTool { name: "t".into(), program: "echo".into(), ..Default::default() };
        assert_eq!(working_dir(&tool, &ctx()).unwrap(), Some(PathBuf::from("/w")));
        tool.working_dir = "$FileDir$".into();
        assert_eq!(working_dir(&tool, &ctx()).unwrap(), Some(PathBuf::from("/w/src")));
        tool.working_dir = "docs".into();
        assert_eq!(working_dir(&tool, &ctx()).unwrap(), Some(PathBuf::from("/w/docs")));
        let loose = MacroContext { file: Some("/tmp/a.rs".into()), ..Default::default() };
        tool.working_dir.clear();
        assert_eq!(working_dir(&tool, &loose).unwrap(), Some(PathBuf::from("/tmp")));
    }
}
