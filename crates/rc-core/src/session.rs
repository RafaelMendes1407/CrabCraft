//! Per-workspace session: open editors with their caret, scroll and folds, the active tab,
//! the layout of the tool windows and the expanded explorer nodes. Restored when the
//! workspace is opened again.

use crate::paths;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OpenFile {
    pub path: PathBuf,
    pub line: usize,
    pub col: usize,
    pub scroll_y: f32,
    pub folded: Vec<usize>,
    pub pinned: bool,
}

/// A run configuration for some tests: by full name, or every test of a module/class.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedTestConfig {
    pub name: String,
    /// The project (`Cargo.toml`, solution or test project) the tests belong to.
    pub target: PathBuf,
    pub tests: Vec<String>,
    /// `tests` holds one module or class: all of its tests run.
    pub class: bool,
}

/// What the user changed in an application run configuration (Run › Edit Configuration).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RunOptions {
    /// `None`: the configuration's launch profile; `Some("")`: no profile at all
    /// (`--no-launch-profile`); `Some(name)`: that profile.
    pub profile: Option<String>,
    /// Program arguments (a command line); empty: the profile's.
    pub args: String,
    /// `NAME=value` per line, over the profile's environment.
    pub env: String,
    /// Empty: the profile's or the project folder.
    pub working_dir: String,
    pub build_first: bool,
    /// Run with `dotnet watch` (Hot Reload and browser refresh); `None`: on for web projects.
    pub hot_reload: Option<bool>,
    /// Rust: toolchain (`cargo +channel`); empty: the default one.
    pub channel: String,
    /// Rust: Cargo profile; empty: `dev`.
    pub cargo_profile: String,
    /// Rust: features, separated by commas or spaces.
    pub features: String,
    /// Rust: `RUST_BACKTRACE`; empty: `short`.
    pub backtrace: String,
    /// Rust: a file to read as the program's input.
    pub stdin_file: String,
    /// Rust: external tools (by name) run first, in order; one that fails stops the launch.
    pub before_launch: Vec<String>,
    /// Running it again starts another instance instead of restarting the running one.
    pub allow_parallel: bool,
    /// Rust: start the program in a pseudo-terminal (colours, `isatty`, interactive input).
    pub emulate_terminal: bool,
    /// Kept in the project (`.crabcraft/runConfigurations`) to be shared, not in the session.
    pub in_project: bool,
}

impl Default for RunOptions {
    fn default() -> Self {
        RunOptions { profile: None, args: String::new(), env: String::new(), working_dir: String::new(), build_first: true, hot_reload: None, channel: String::new(), cargo_profile: String::new(), features: String::new(), backtrace: String::new(), stdin_file: String::new(), before_launch: Vec::new(), allow_parallel: false, emulate_terminal: false, in_project: false }
    }
}

impl RunOptions {
    /// `NAME=value` lines as pairs (blank lines and `#` comments skipped).
    pub fn env_pairs(&self) -> Vec<(String, String)> {
        self.env
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .filter_map(|l| l.split_once('=').map(|(k, v)| (k.trim().to_string(), v.trim().to_string())))
            .filter(|(k, _)| !k.is_empty())
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolWindowState {
    pub visible: bool,
    /// Width (side windows) or height (bottom windows) in points.
    pub size: f32,
    /// `left`, `right` or `bottom` when moved from its default side.
    pub side: Option<String>,
}

impl Default for ToolWindowState {
    fn default() -> Self {
        ToolWindowState { visible: false, size: 280.0, side: None }
    }
}

/// How the editors are arranged: groups of tabs (indexes into `Session::files`) split side by
/// side or one above the other.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum EditorNode {
    Group { tabs: Vec<usize>, active: Option<usize> },
    Split { vertical: bool, ratio: f32, a: Box<EditorNode>, b: Box<EditorNode> },
}

/// A floating editor window: its groups and where it was on screen (x, y, width, height).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EditorWindow {
    pub root: EditorNode,
    pub rect: Option<[f32; 4]>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub files: Vec<OpenFile>,
    pub active: Option<usize>,
    /// The editor groups of the main window (none: every file in one group).
    pub editor_layout: Option<EditorNode>,
    /// Floating editor windows.
    pub editor_windows: Vec<EditorWindow>,
    /// Tool window id → state.
    pub tool_windows: BTreeMap<String, ToolWindowState>,
    /// Explorer nodes expanded (paths relative to the workspace root, or node keys).
    pub expanded: Vec<String>,
    pub run_configuration: Option<String>,
    /// Breakpoints: file → 0-based lines (with an optional condition).
    pub breakpoints: BTreeMap<PathBuf, Vec<(usize, Option<String>)>>,
    /// Breakpoints that are disabled: file → 0-based lines.
    pub disabled_breakpoints: BTreeMap<PathBuf, Vec<usize>>,
    /// Breakpoint groups (D-27): file → (0-based line, group name).
    pub breakpoint_groups: BTreeMap<PathBuf, Vec<(usize, String)>>,
    /// Logpoints: file → 0-based lines and their messages.
    pub logpoints: BTreeMap<PathBuf, Vec<(usize, String)>>,
    /// Breakpoints removed by their first stop: file → 0-based lines.
    pub remove_once_hit: BTreeMap<PathBuf, Vec<usize>>,
    /// Bookmarks: file → 0-based lines.
    pub bookmarks: BTreeMap<PathBuf, Vec<usize>>,
    /// Folders of the run configurations (B-21): configuration name → folder name.
    pub config_folders: BTreeMap<String, String>,
    /// Compound run configurations: name and the configurations they start.
    pub compounds: Vec<(String, Vec<String>)>,
    /// Test configurations made from a gutter run and saved (Save Configuration).
    pub test_configs: Vec<SavedTestConfig>,
    /// Edited application configurations, by name.
    pub run_options: BTreeMap<String, RunOptions>,
    /// Folders and files left out of the index, Find in Files and the analysis (root-relative,
    /// `/`-separated). Read with [`crate::workspace::clean_excluded`].
    pub excluded: Vec<String>,
    /// `Cargo.toml` files of other Cargo projects analysed with the workspace (root-relative).
    pub attached_projects: Vec<String>,
    /// Environment of every Cargo command and of rust-analyzer: `NAME=value` per line.
    pub project_env: String,
    /// Cargo features the user turned on for analysis, as `package/feature`. Read with
    /// `rc_cargo::valid_feature_entry`: the file is untrusted.
    pub cargo_features: Vec<String>,
    /// The target triple the code is analysed for; empty: the host. Read with
    /// `rc_cargo::valid_triple`: the file is untrusted.
    pub cargo_target: String,
    /// The copyright header of the project's files. Read with care: the file is untrusted.
    pub copyright: CopyrightProfile,
    /// Scopes the user defined (named sets of files). Untrusted: read with
    /// [`clean_scopes`].
    pub scopes: Vec<NamedScope>,
    /// File colours: the first scope that holds a file gives its colour. Untrusted: read with
    /// [`clean_file_colors`].
    pub file_colors: Vec<FileColor>,
}

/// A named set of files: globs separated by `,` or `;`, `!` to leave files out
/// (`src/**/*.rs, !**/generated/**`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NamedScope {
    pub name: String,
    pub pattern: String,
}

/// The colour of the files of a scope: the scope's key (a built-in key such as
/// `builtin:tests`, or a user scope's name) and `#rrggbb`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FileColor {
    pub scope: String,
    pub color: String,
}

/// Prefix of the keys of the scopes CrabCraft defines; user scopes cannot use it.
pub const BUILTIN_SCOPE_PREFIX: &str = "builtin:";
/// Longest scope name and pattern kept from a session file.
const MAX_SCOPE_NAME: usize = 120;
const MAX_SCOPE_PATTERN: usize = 4000;
const MAX_SCOPES: usize = 200;

/// User scopes read from a session file: names trimmed, not empty, not reserved, unique (the
/// first wins), within a sane size. The patterns are not checked here (a bad glob is shown
/// where the scope is used).
pub fn clean_scopes(list: &[NamedScope]) -> Vec<NamedScope> {
    let mut out: Vec<NamedScope> = Vec::new();
    for s in list {
        let name = s.name.trim();
        if name.is_empty() || name.len() > MAX_SCOPE_NAME || name.starts_with(BUILTIN_SCOPE_PREFIX) || s.pattern.len() > MAX_SCOPE_PATTERN || out.iter().any(|o| o.name == name) {
            continue;
        }
        out.push(NamedScope { name: name.to_string(), pattern: s.pattern.trim().to_string() });
        if out.len() == MAX_SCOPES {
            break;
        }
    }
    out
}

/// `#rrggbb` (or `rrggbb`) as red, green and blue.
pub fn parse_hex_color(text: &str) -> Option<[u8; 3]> {
    let hex = text.trim().trim_start_matches('#');
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?])
}

/// Red, green and blue as `#rrggbb`.
pub fn hex_color(rgb: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}

/// File colours read from a session file: valid colours of scopes that exist (`known` says
/// which keys do), one per scope (the first wins), colours written as `#rrggbb`.
pub fn clean_file_colors(list: &[FileColor], known: impl Fn(&str) -> bool) -> Vec<FileColor> {
    let mut out: Vec<FileColor> = Vec::new();
    for c in list {
        let Some(rgb) = parse_hex_color(&c.color) else { continue };
        if !known(&c.scope) || out.iter().any(|o| o.scope == c.scope) {
            continue;
        }
        out.push(FileColor { scope: c.scope.clone(), color: hex_color(rgb) });
        if out.len() == MAX_SCOPES {
            break;
        }
    }
    out
}

/// The template a project's files start with (`$year`, `$file`, `$project`, `$author`), its
/// SPDX license and whether new files get it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CopyrightProfile {
    pub template: String,
    /// An SPDX license expression (`MIT OR Apache-2.0`); empty: no license line.
    pub spdx: String,
    pub on_new_files: bool,
}

impl CopyrightProfile {
    /// Written for CrabCraft.
    pub const DEFAULT_TEMPLATE: &str = "Copyright (c) $year $author\nPart of $project.";
}

impl Default for CopyrightProfile {
    fn default() -> Self {
        CopyrightProfile { template: Self::DEFAULT_TEMPLATE.to_string(), spdx: String::new(), on_new_files: false }
    }
}

/// A line of an environment that is not `NAME=value` with a valid name (1-based).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BadEnvLine(pub usize);

/// Whether `name` can be an environment variable: not empty, without `=`, spaces or NUL.
pub fn valid_env_name(name: &str) -> bool {
    !name.is_empty() && !name.chars().any(|c| c == '=' || c == '\0' || c.is_whitespace())
}

/// `NAME=value` lines as pairs (blank lines and `#` comments skipped); the first invalid line
/// is an error. A later line wins over an earlier one with the same name.
pub fn parse_env(text: &str) -> Result<Vec<(String, String)>, BadEnvLine> {
    let mut pairs: Vec<(String, String)> = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else { return Err(BadEnvLine(i + 1)) };
        let (name, value) = (name.trim(), value.trim());
        if !valid_env_name(name) || value.contains('\0') {
            return Err(BadEnvLine(i + 1));
        }
        pairs.retain(|(n, _)| n != name);
        pairs.push((name.to_string(), value.to_string()));
    }
    Ok(pairs)
}

impl Session {
    /// File of the session of `workspace` (keyed by a hash of its canonical path).
    pub fn file_for(workspace: &Path) -> PathBuf {
        let canon = workspace.canonicalize().unwrap_or_else(|_| workspace.to_path_buf());
        let name = canon.file_name().and_then(|n| n.to_str()).unwrap_or("workspace");
        paths::sessions_dir().join(format!("{name}-{:016x}.json", fnv(canon.to_string_lossy().as_bytes())))
    }

    pub fn load(workspace: &Path) -> Session {
        std::fs::read_to_string(Self::file_for(workspace)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, workspace: &Path) -> anyhow::Result<()> {
        paths::write_atomic(&Self::file_for(workspace), serde_json::to_string_pretty(self)?.as_bytes())?;
        Ok(())
    }
}

/// FNV-1a: a stable hash for file names (std's hasher is not stable across runs).
fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x100000001b3))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_round_trip_per_workspace() {
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: tests in this crate do not read CRABCRAFT_HOME concurrently with other values.
        unsafe { std::env::set_var("CRABCRAFT_HOME", dir.path()) };
        let ws = dir.path().join("MySolution");
        std::fs::create_dir_all(&ws).unwrap();
        let mut s = Session::default();
        s.files.push(OpenFile { path: ws.join("Program.cs"), line: 12, col: 4, scroll_y: 120.0, folded: vec![3], pinned: false });
        s.active = Some(0);
        s.save(&ws).unwrap();
        assert_eq!(Session::load(&ws), s);
        assert_eq!(Session::load(&dir.path().join("Other")), Session::default());
    }

    #[test]
    fn project_environments_are_validated() {
        let pairs = parse_env("# comment\n\n A = 1 \nPATH_X=a=b\nA=2\nEMPTY=\n").unwrap();
        assert_eq!(pairs, vec![("PATH_X".to_string(), "a=b".to_string()), ("A".to_string(), "2".to_string()), ("EMPTY".to_string(), String::new())]);
        assert_eq!(parse_env("OK=1\nno value"), Err(BadEnvLine(2)));
        assert_eq!(parse_env("=x"), Err(BadEnvLine(1)));
        assert_eq!(parse_env("TWO WORDS=x"), Err(BadEnvLine(1)));
        assert_eq!(parse_env("NUL=a\0b"), Err(BadEnvLine(1)));
        assert_eq!(parse_env(""), Ok(Vec::new()));
    }

    #[test]
    fn scopes_and_file_colors_from_a_session_are_cleaned() {
        let scope = |name: &str, pattern: &str| NamedScope { name: name.into(), pattern: pattern.into() };
        let raw = vec![scope(" Sources ", " src/**/*.rs "), scope("", "*"), scope("builtin:tests", "*"), scope("Sources", "other"), scope("Docs", "a[")];
        assert_eq!(clean_scopes(&raw), vec![scope("Sources", "src/**/*.rs"), scope("Docs", "a[")]);
        let color = |scope: &str, color: &str| FileColor { scope: scope.into(), color: color.into() };
        let raw = vec![color("Sources", "#A0B1C2"), color("Sources", "#000000"), color("Gone", "#ffffff"), color("builtin:tests", "green"), color("builtin:tests", "12345"), color("builtin:tests", "1a2b3c")];
        let known = |k: &str| k == "Sources" || k == "builtin:tests";
        assert_eq!(clean_file_colors(&raw, known), vec![color("Sources", "#a0b1c2"), color("builtin:tests", "#1a2b3c")]);
        assert_eq!(parse_hex_color("#ffé000"), None);
    }
}
