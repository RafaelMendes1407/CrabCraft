//! User settings, stored as JSON in the data directory. Unknown or missing fields fall back to
//! their defaults, so files from older and newer versions both load.

use crate::paths;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeChoice {
    Light,
    #[default]
    Dark,
    /// Follow the operating system.
    System,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RustDebugger {
    #[default]
    Gdb,
    Lldb,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeymapChoice {
    /// Rider's default for the platform (macOS or Windows/Linux).
    #[default]
    Rider,
    /// RustRover reference shortcuts for the host platform.
    RustRover,
    VisualStudio,
    /// VS Code's default for the platform.
    VsCode,
}

/// How rustfmt runs. Kept as the user typed it; `rc_cargo::Rustfmt` reads and checks it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RustfmtSettings {
    /// Extra arguments, split like a shell does: quotes keep spaces in one argument.
    pub args: String,
    /// The rustup toolchain to run (`nightly`, `1.85.0`...); empty runs the default one.
    pub channel: String,
    /// Environment variables, one `NAME=value` per line.
    pub env: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeChoice,
    /// Interface language code (`es`, `fr`…); `None` follows the system.
    pub language: Option<String>,
    pub keymap: KeymapChoice,
    pub editor_font_size: f32,
    pub editor_line_spacing: f32,
    pub ui_zoom: f32,
    pub show_inlay_hints: bool,
    pub show_indent_guides: bool,
    /// Keep the first lines of the enclosing blocks at the top of the editor.
    pub sticky_lines: bool,
    /// Preview the code under the pointer while hovering the editor's scrollbar.
    pub scrollbar_lens: bool,
    pub right_margin: usize,
    /// Save files when the window loses focus or when switching tabs (IntelliJ autosave).
    pub autosave: bool,
    /// Save edited files once nothing was typed for `autosave_delay_secs` seconds.
    pub autosave_idle: bool,
    /// Seconds without input before an idle autosave. Untrusted: clamp before use.
    pub autosave_delay_secs: u32,
    /// Open files from the explorer with a single click (otherwise a double click).
    pub single_click_open: bool,
    /// Editor tabs wrap into several rows instead of scrolling.
    pub tabs_multirow: bool,
    /// Tabs tinted by kind of file (tests, web, configuration).
    pub tab_colors: bool,
    /// Last folder a new solution was created in.
    pub new_project_location: Option<PathBuf>,
    /// The last tool window layout (sides, sizes, open windows): solutions opened for the
    /// first time start with it.
    pub tool_windows: std::collections::BTreeMap<String, crate::session::ToolWindowState>,
    pub rust_debugger: RustDebugger,
    pub gdb_path: Option<PathBuf>,
    pub lldb_dap_path: Option<PathBuf>,
    pub rust_analyzer_path: Option<PathBuf>,
    pub web_html_server: Option<PathBuf>,
    pub web_css_server: Option<PathBuf>,
    pub web_typescript_server: Option<PathBuf>,
    pub web_tsserver_path: Option<PathBuf>,
    /// Clippy instead of `cargo check` on save.
    pub use_clippy: bool,
    /// Run Rust tests with cargo-nextest instead of `cargo test`.
    pub use_nextest: bool,
    /// Reformat Rust files when they are saved.
    pub format_on_save: bool,
    /// Reformat the Rust files of a commit before committing them.
    pub format_on_commit: bool,
    /// How rustfmt runs, wherever CrabCraft formats Rust.
    pub rustfmt: RustfmtSettings,
    /// Latest commands of Run Anything, newest first.
    pub run_anything_history: Vec<String>,
    /// Projects whose `build.rs` and procedural macros may run.
    pub trusted_projects: Vec<PathBuf>,
    /// Most recent first.
    pub recent_workspaces: Vec<PathBuf>,
    /// The project open when CrabCraft quit; `None` after Close Project.
    pub last_workspace: Option<PathBuf>,
    /// Programs run from More Actions › External Tools and Find Action.
    pub external_tools: Vec<crate::external_tools::ExternalTool>,
    /// Abbreviations expanded with Tab in Rust files.
    pub live_templates: Vec<crate::live_templates::LiveTemplate>,
    /// Items and modules completion never offers to import (`std::sync::atomic::Ordering`).
    pub autoimport_exclude: Vec<String>,
    /// Add the `use` lines pasted Rust code needs, when only one import fits.
    pub imports_on_paste: bool,
    /// The debugger shows integers in hexadecimal.
    pub debugger_hex: bool,
    /// Ask crates.io (only the crate names) for newer versions of the dependencies of an open
    /// `Cargo.toml`.
    pub check_crates_io: bool,
    /// Shortcuts changed by the user, over the keymap: action id → shortcut id
    /// (`ctrl+shift+K`), empty for none.
    pub shortcuts: std::collections::BTreeMap<String, String>,
    /// The Default inspection profile (I-18). Untrusted: read with `clean`.
    pub inspection_profile: crate::inspections::InspectionProfile,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: ThemeChoice::Dark,
            language: None,
            keymap: KeymapChoice::Rider,
            editor_font_size: 13.0,
            editor_line_spacing: 1.2,
            ui_zoom: 1.0,
            show_inlay_hints: true,
            show_indent_guides: true,
            sticky_lines: true,
            scrollbar_lens: true,
            right_margin: 120,
            autosave: true,
            autosave_idle: true,
            autosave_delay_secs: 5,
            single_click_open: false,
            tabs_multirow: false,
            tab_colors: true,
            new_project_location: None,
            tool_windows: Default::default(),
            rust_debugger: RustDebugger::default(),
            gdb_path: None,
            lldb_dap_path: None,
            rust_analyzer_path: None,
            web_html_server: None,
            web_css_server: None,
            web_typescript_server: None,
            web_tsserver_path: None,
            use_clippy: false,
            use_nextest: false,
            format_on_save: false,
            format_on_commit: false,
            rustfmt: RustfmtSettings::default(),
            run_anything_history: Vec::new(),
            trusted_projects: Vec::new(),
            recent_workspaces: Vec::new(),
            last_workspace: None,
            external_tools: Vec::new(),
            live_templates: crate::live_templates::defaults(),
            autoimport_exclude: Vec::new(),
            imports_on_paste: true,
            debugger_hex: false,
            check_crates_io: true,
            shortcuts: Default::default(),
            inspection_profile: Default::default(),
        }
    }
}

impl Settings {
    pub fn file() -> PathBuf {
        paths::data_dir().join("settings.json")
    }

    /// Loads the settings; an unreadable file is kept aside as `settings.json.bak`.
    pub fn load() -> Settings {
        let file = Self::file();
        let Ok(text) = std::fs::read_to_string(&file) else { return Settings::default() };
        match serde_json::from_str(&text) {
            Ok(s) => s,
            Err(e) => {
                let _ = std::fs::copy(&file, file.with_extension("json.bak"));
                eprintln!("unreadable settings.json ({e}); using defaults");
                Settings::default()
            }
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        paths::write_atomic(&Self::file(), serde_json::to_string_pretty(self)?.as_bytes())?;
        Ok(())
    }

    /// Deletes persisted preferences and the backup of an unreadable settings file.
    pub fn clear() -> std::io::Result<()> {
        for file in [Self::file(), Self::file().with_extension("json.bak")] {
            match std::fs::remove_file(file) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                result => result?,
            }
        }
        Ok(())
    }

    /// Moves `path` to the top of the recent list (max 15 entries).
    pub fn add_recent(&mut self, path: PathBuf) {
        self.recent_workspaces.retain(|p| p != &path);
        self.recent_workspaces.insert(0, path);
        self.recent_workspaces.truncate(15);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_take_defaults() {
        let s: Settings = serde_json::from_str(r#"{"editor_font_size": 15.0, "unknown": 1}"#).unwrap();
        assert_eq!(s.editor_font_size, 15.0);
        assert_eq!(s.right_margin, 120);
    }

    #[test]
    fn recent_list_moves_to_front_without_duplicates() {
        let mut s = Settings::default();
        s.add_recent("/a".into());
        s.add_recent("/b".into());
        s.add_recent("/a".into());
        assert_eq!(s.recent_workspaces, vec![PathBuf::from("/a"), PathBuf::from("/b")]);
    }
}
