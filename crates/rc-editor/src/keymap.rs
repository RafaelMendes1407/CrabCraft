//! Rider, Visual Studio and VS Code keymaps: every shortcut of the IDE maps to an [`Action`]. The editor handles the
//! editing actions itself and reports the rest to the application.

use egui::{Key, Modifiers};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    // --- caret movement (the bool is "extend selection")
    Left(bool),
    Right(bool),
    Up(bool),
    Down(bool),
    WordLeft(bool),
    WordRight(bool),
    LineStart(bool),
    LineEnd(bool),
    DocStart(bool),
    DocEnd(bool),
    PageUp(bool),
    PageDown(bool),
    ScrollUp,
    ScrollDown,
    // --- editing
    Enter,
    StartNewLine,
    StartNewLineBefore,
    Backspace,
    Delete,
    DeleteWordLeft,
    DeleteWordRight,
    DeleteLine,
    DuplicateLine,
    MoveLineUp,
    MoveLineDown,
    CommentLine,
    CommentBlock,
    Indent,
    Unindent,
    JoinLines,
    ToggleCase,
    Undo,
    Redo,
    Copy,
    Cut,
    Paste,
    SelectAll,
    ExtendSelection,
    ShrinkSelection,
    SelectNextOccurrence,
    UnselectOccurrence,
    SelectAllOccurrences,
    CloneCaretAbove,
    CloneCaretBelow,
    Escape,
    // --- folding
    Fold,
    Unfold,
    FoldAll,
    UnfoldAll,
    // --- code insight (reported to the app)
    Complete,
    ParameterInfo,
    QuickDoc,
    ExternalDocs,
    ExpandMacro,
    ShowIntentions,
    Generate,
    RefactorThis,
    GotoDeclaration,
    GotoImplementation,
    GotoTypeDeclaration,
    GotoSuperMethod,
    ShowCallers,
    TypeHierarchy,
    ShowCallees,
    FindUsages,
    Rename,
    Reformat,
    FileStructure,
    GotoLine,
    Find,
    Replace,
    FindNext,
    FindPrevious,
    NextError,
    PreviousError,
    ToggleBreakpoint,
    ToggleBookmark,
    ShowBookmarks,
    // --- IDE (reported to the app)
    SearchEverywhere,
    FindAction,
    OptimizeImports,
    RunAnything,
    GotoFile,
    GotoSymbol,
    GotoClass,
    FindInFiles,
    ReplaceInFiles,
    RecentFiles,
    SelectIn,
    RecentLocations,
    NavigateBack,
    NavigateForward,
    Save,
    SaveAll,
    CloseTab,
    NextTab,
    PreviousTab,
    Build,
    Run,
    Debug,
    StopRun,
    RunTests,
    StepOver,
    StepInto,
    StepOut,
    Resume,
    RunToCursor,
    AttachToProcess,
    Evaluate,
    ShowSettings,
    ProjectEnvironment,
    ToggleProjectWindow,
    ToggleTerminal,
    ToggleGitWindow,
    ToggleProblems,
    HideToolWindows,
    Commit,
    ShowLocalHistory,
    OpenFolder,
    InvalidateCaches,
    InspectCode,
    NewScratchFile,
    ZoomIn,
    ZoomOut,
    ZoomReset,
}

impl Action {
    /// Actions the editor widget executes itself.
    pub fn is_editing(self) -> bool {
        use Action::*;
        !matches!(
            self,
            Complete
                | ParameterInfo
                | QuickDoc
                | ExternalDocs
                | ExpandMacro
                | ShowIntentions
                | Generate
                | RefactorThis
                | GotoDeclaration
                | GotoImplementation
                | GotoTypeDeclaration
                | GotoSuperMethod
                | ShowCallers
                | TypeHierarchy
                | ShowCallees
                | FindUsages
                | Rename
                | Reformat
                | FileStructure
                | GotoLine
                | Find
                | Replace
                | FindNext
                | FindPrevious
                | NextError
                | PreviousError
                | ToggleBreakpoint
                | ToggleBookmark
        ) && !self.is_ide()
    }

    /// Global IDE actions (they work without an editor).
    pub fn is_ide(self) -> bool {
        use Action::*;
        matches!(
            self,
            SearchEverywhere
                | FindAction
                | ShowBookmarks
                | OptimizeImports
                | RunAnything
                | GotoFile
                | GotoSymbol
                | GotoClass
                | FindInFiles
                | ReplaceInFiles
                | RecentFiles
                | SelectIn
                | RecentLocations
                | NavigateBack
                | NavigateForward
                | Save
                | SaveAll
                | CloseTab
                | NextTab
                | PreviousTab
                | Build
                | Run
                | Debug
                | StopRun
                | RunTests
                | StepOver
                | StepInto
                | StepOut
                | Resume
                | RunToCursor
                | AttachToProcess
                | Evaluate
                | ShowSettings
                | ProjectEnvironment
                | ToggleProjectWindow
                | ToggleTerminal
                | ToggleGitWindow
                | ToggleProblems
                | HideToolWindows
                | Commit
                | ShowLocalHistory
                | OpenFolder
                | InvalidateCaches
                | InspectCode
                | NewScratchFile
                | ZoomIn
                | ZoomOut
                | ZoomReset
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum KeymapKind {
    /// Rider's default keymap on macOS.
    #[default]
    RiderMac,
    /// Rider's default keymap on Windows/Linux.
    Rider,
    /// Visual Studio keymap (Windows).
    VisualStudio,
    /// VS Code's default shortcuts on macOS.
    VsCodeMac,
    /// VS Code's default shortcuts on Windows (also used on Linux).
    VsCode,
}

impl KeymapKind {
    /// Shortcuts use ⌘ and are written with the macOS symbols.
    pub fn is_mac(self) -> bool {
        matches!(self, KeymapKind::RiderMac | KeymapKind::VsCodeMac)
    }
}

/// A key with modifiers. `cmd` is ⌘ on macOS and Ctrl elsewhere (egui's `command`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chord {
    pub key: Key,
    pub mods: Modifiers,
}

const fn m(alt: bool, ctrl: bool, shift: bool, mac_cmd: bool) -> Modifiers {
    Modifiers { alt, ctrl, shift, mac_cmd, command: mac_cmd || ctrl }
}

#[derive(Clone)]
pub struct Keymap {
    pub kind: KeymapKind,
    bindings: Vec<(Chord, Action)>,
}

impl Keymap {
    pub fn new(kind: KeymapKind) -> Self {
        let bindings = match kind {
            KeymapKind::RiderMac => mac_bindings(),
            KeymapKind::Rider => win_bindings(false),
            KeymapKind::VisualStudio => win_bindings(true),
            KeymapKind::VsCodeMac => vscode_bindings(true),
            KeymapKind::VsCode => vscode_bindings(false),
        };
        Keymap { kind, bindings }
    }

    /// The keymap with the user's changes: each action gets the given shortcut (or none),
    /// which another action then loses.
    pub fn with_overrides(kind: KeymapKind, overrides: &[(Action, Option<Chord>)]) -> Self {
        let mut keymap = Keymap::new(kind);
        let same = |a: &Chord, b: &Chord| a.key == b.key && (a.mods.alt, a.mods.ctrl, a.mods.shift, a.mods.mac_cmd) == (b.mods.alt, b.mods.ctrl, b.mods.shift, b.mods.mac_cmd);
        for (action, chord) in overrides {
            keymap.bindings.retain(|(c, a)| a != action && !chord.as_ref().is_some_and(|n| same(c, n)));
            if let Some(c) = chord {
                // First, so menus show the user's shortcut.
                keymap.bindings.insert(0, (*c, *action));
            }
        }
        keymap
    }

    /// The action a shortcut runs, if any.
    pub fn action_of(&self, chord: Chord) -> Option<Action> {
        self.lookup(chord.key, chord.mods)
    }

    /// The action bound to `key` with `mods`, if any. Modifiers must match exactly.
    pub fn lookup(&self, key: Key, mods: Modifiers) -> Option<Action> {
        let norm = |x: Modifiers| (x.alt, x.ctrl, x.shift, x.mac_cmd);
        self.bindings.iter().find(|(c, _)| c.key == key && norm(c.mods) == norm(mods)).map(|(_, a)| *a)
    }

    /// The first shortcut of `action`, formatted for menus and tooltips (⌘⇧F, Ctrl+Shift+F).
    pub fn shortcut_text(&self, action: Action) -> Option<String> {
        let (chord, _) = self.bindings.iter().find(|(_, a)| *a == action)?;
        Some(format_chord(*chord, self.kind.is_mac()))
    }

    pub fn chord(&self, action: Action) -> Option<Chord> {
        self.bindings.iter().find(|(_, a)| *a == action).map(|(c, _)| *c)
    }
}

/// A shortcut as stored in the settings: `ctrl+alt+shift+cmd+K`, modifiers in that order and
/// egui's key name.
pub fn chord_id(c: Chord) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for (on, name) in [(c.mods.ctrl, "ctrl"), (c.mods.alt, "alt"), (c.mods.shift, "shift"), (c.mods.mac_cmd, "cmd")] {
        if on {
            parts.push(name);
        }
    }
    parts.push(c.key.name());
    parts.join("+")
}

/// Reads [`chord_id`]'s text; `None` for anything else.
pub fn parse_chord(text: &str) -> Option<Chord> {
    let mut parts: Vec<&str> = text.split('+').collect();
    // `+` itself is a key name ("ctrl++").
    let key_name = if text.ends_with("++") {
        parts.truncate(parts.len().saturating_sub(2));
        "+"
    } else {
        parts.pop()?
    };
    let key = Key::from_name(key_name)?;
    let (mut ctrl, mut alt, mut shift, mut cmd) = (false, false, false, false);
    for p in parts {
        match p {
            "ctrl" => ctrl = true,
            "alt" => alt = true,
            "shift" => shift = true,
            "cmd" => cmd = true,
            _ => return None,
        }
    }
    Some(Chord { key, mods: m(alt, ctrl, shift, cmd) })
}

pub fn format_chord(c: Chord, mac: bool) -> String {
    let key = key_name(c.key, mac);
    if mac {
        let mut s = String::new();
        if c.mods.ctrl {
            s.push('⌃');
        }
        if c.mods.alt {
            s.push('⌥');
        }
        if c.mods.shift {
            s.push('⇧');
        }
        if c.mods.mac_cmd {
            s.push('⌘');
        }
        s + &key
    } else {
        let mut parts: Vec<String> = Vec::new();
        if c.mods.ctrl {
            parts.push("Ctrl".into());
        }
        if c.mods.alt {
            parts.push("Alt".into());
        }
        if c.mods.shift {
            parts.push("Shift".into());
        }
        parts.push(key);
        parts.join("+")
    }
}

fn key_name(key: Key, mac: bool) -> String {
    match key {
        Key::ArrowUp => "↑".into(),
        Key::ArrowDown => "↓".into(),
        Key::ArrowLeft => "←".into(),
        Key::ArrowRight => "→".into(),
        Key::Enter => if mac { "↩".into() } else { "Enter".into() },
        Key::Backspace => if mac { "⌫".into() } else { "Backspace".into() },
        Key::Delete => if mac { "⌦".into() } else { "Delete".into() },
        Key::Escape => if mac { "⎋".into() } else { "Esc".into() },
        Key::Tab => if mac { "⇥".into() } else { "Tab".into() },
        Key::Space => "Space".into(),
        Key::Home => "Home".into(),
        Key::End => "End".into(),
        Key::PageUp => "PgUp".into(),
        Key::PageDown => "PgDn".into(),
        Key::Slash => "/".into(),
        Key::Minus => "-".into(),
        Key::Equals | Key::Plus => "=".into(),
        Key::Period => ".".into(),
        Key::Comma => ",".into(),
        Key::OpenBracket => "[".into(),
        Key::CloseBracket => "]".into(),
        Key::Backtick => "`".into(),
        Key::Num0 => "0".into(),
        Key::Num1 => "1".into(),
        Key::Num9 => "9".into(),
        k => k.name().to_string(),
    }
}

fn mac_bindings() -> Vec<(Chord, Action)> {
    use Action::*;
    let none = m(false, false, false, false);
    let s = m(false, false, true, false);
    let c = m(false, false, false, true);
    let cs = m(false, false, true, true);
    let a = m(true, false, false, false);
    let as_ = m(true, false, true, false);
    let ca = m(true, false, false, true);
    let cas = m(true, false, true, true);
    let ctrl = m(false, true, false, false);
    let ctrl_s = m(false, true, true, false);
    let ctrl_c = m(false, true, false, true);
    let b = |key, mods, action| (Chord { key, mods }, action);
    vec![
        b(Key::ArrowLeft, none, Left(false)),
        b(Key::ArrowLeft, s, Left(true)),
        b(Key::ArrowRight, none, Right(false)),
        b(Key::ArrowRight, s, Right(true)),
        b(Key::ArrowUp, none, Up(false)),
        b(Key::ArrowUp, s, Up(true)),
        b(Key::ArrowDown, none, Down(false)),
        b(Key::ArrowDown, s, Down(true)),
        b(Key::ArrowLeft, a, WordLeft(false)),
        b(Key::ArrowLeft, as_, WordLeft(true)),
        b(Key::ArrowRight, a, WordRight(false)),
        b(Key::ArrowRight, as_, WordRight(true)),
        b(Key::ArrowLeft, c, LineStart(false)),
        b(Key::ArrowLeft, cs, LineStart(true)),
        b(Key::ArrowRight, c, LineEnd(false)),
        b(Key::ArrowRight, cs, LineEnd(true)),
        b(Key::Home, none, LineStart(false)),
        b(Key::Home, s, LineStart(true)),
        b(Key::End, none, LineEnd(false)),
        b(Key::End, s, LineEnd(true)),
        b(Key::Home, c, DocStart(false)),
        b(Key::Home, cs, DocStart(true)),
        b(Key::End, c, DocEnd(false)),
        b(Key::End, cs, DocEnd(true)),
        b(Key::ArrowUp, c, DocStart(false)),
        b(Key::ArrowUp, cs, DocStart(true)),
        b(Key::ArrowDown, c, DocEnd(false)),
        b(Key::ArrowDown, cs, DocEnd(true)),
        b(Key::PageUp, none, PageUp(false)),
        b(Key::PageUp, s, PageUp(true)),
        b(Key::PageDown, none, PageDown(false)),
        b(Key::PageDown, s, PageDown(true)),
        b(Key::Enter, none, Enter),
        b(Key::Enter, s, StartNewLine),
        b(Key::Enter, ca, StartNewLineBefore),
        b(Key::Backspace, none, Backspace),
        b(Key::Backspace, s, Backspace),
        b(Key::Delete, none, Delete),
        b(Key::Backspace, a, DeleteWordLeft),
        b(Key::Delete, a, DeleteWordRight),
        b(Key::Backspace, c, DeleteLine),
        b(Key::D, c, DuplicateLine),
        b(Key::ArrowUp, as_, MoveLineUp),
        b(Key::ArrowDown, as_, MoveLineDown),
        b(Key::Slash, c, CommentLine),
        b(Key::Slash, ca, CommentBlock),
        b(Key::Tab, none, Indent),
        b(Key::Tab, s, Unindent),
        b(Key::J, ctrl_s, JoinLines),
        b(Key::U, cs, ToggleCase),
        b(Key::Z, c, Undo),
        b(Key::Z, cs, Redo),
        b(Key::C, c, Copy),
        b(Key::X, c, Cut),
        b(Key::V, c, Paste),
        b(Key::A, c, SelectAll),
        b(Key::ArrowUp, a, ExtendSelection),
        b(Key::ArrowDown, a, ShrinkSelection),
        b(Key::G, ctrl, SelectNextOccurrence),
        b(Key::G, ctrl_s, UnselectOccurrence),
        b(Key::G, ctrl_c, SelectAllOccurrences),
        b(Key::Escape, none, Escape),
        b(Key::Minus, c, Fold),
        b(Key::Equals, c, Unfold),
        b(Key::Plus, c, Unfold),
        b(Key::Minus, cs, FoldAll),
        b(Key::Equals, cs, UnfoldAll),
        b(Key::Plus, cs, UnfoldAll),
        b(Key::Space, ctrl, Complete),
        // ⌘P opens Search Everywhere, so Parameter Info moves next to completion.
        b(Key::Space, ctrl_s, ParameterInfo),
        b(Key::P, c, SearchEverywhere),
        b(Key::F1, none, QuickDoc),
        b(Key::F1, s, ExternalDocs),
        b(Key::F1, a, SelectIn),
        b(Key::J, ctrl, QuickDoc),
        b(Key::Enter, a, ShowIntentions),
        b(Key::Insert, a, Generate),
        b(Key::B, c, GotoDeclaration),
        b(Key::B, ca, GotoImplementation),
        b(Key::B, cs, GotoTypeDeclaration),
        b(Key::U, c, GotoSuperMethod),
        b(Key::H, ca, ShowCallers),
        b(Key::H, cas, TypeHierarchy),
        b(Key::T, cas, RefactorThis),
        // ⌘⌥O opens a folder; JetBrains uses ⌃⌥O for this.
        b(Key::O, m(false, true, false, true), OptimizeImports),
        b(Key::A, cs, FindAction),
        b(Key::F7, a, FindUsages),
        b(Key::F6, s, Rename),
        b(Key::L, ca, Reformat),
        b(Key::F12, c, FileStructure),
        b(Key::L, c, GotoLine),
        b(Key::F, c, Find),
        b(Key::R, c, Replace),
        b(Key::G, c, FindNext),
        b(Key::G, cs, FindPrevious),
        b(Key::F3, none, FindNext),
        b(Key::F3, s, FindPrevious),
        b(Key::F2, none, NextError),
        b(Key::F2, s, PreviousError),
        b(Key::F8, c, ToggleBreakpoint),
        b(Key::F11, c, ToggleBookmark),
        b(Key::F11, cs, ShowBookmarks),
        b(Key::O, cs, GotoFile),
        b(Key::O, cas, GotoSymbol),
        b(Key::O, c, GotoClass),
        b(Key::T, c, SearchEverywhere),
        b(Key::F, cs, FindInFiles),
        b(Key::R, cs, ReplaceInFiles),
        b(Key::E, c, RecentFiles),
        b(Key::E, cs, RecentLocations),
        b(Key::OpenBracket, c, NavigateBack),
        b(Key::CloseBracket, c, NavigateForward),
        b(Key::ArrowLeft, ca, NavigateBack),
        b(Key::ArrowRight, ca, NavigateForward),
        b(Key::S, c, SaveAll),
        b(Key::W, c, CloseTab),
        b(Key::CloseBracket, cs, NextTab),
        b(Key::OpenBracket, cs, PreviousTab),
        b(Key::F9, c, Build),
        b(Key::R, ctrl, Run),
        b(Key::D, ctrl, Debug),
        b(Key::F2, c, StopRun),
        b(Key::F8, none, StepOver),
        b(Key::F7, none, StepInto),
        b(Key::F8, s, StepOut),
        b(Key::R, ca, Resume),
        b(Key::F9, a, RunToCursor),
        b(Key::F8, a, Evaluate),
        b(Key::Comma, c, ShowSettings),
        b(Key::Num1, c, ToggleProjectWindow),
        b(Key::F12, a, ToggleTerminal),
        b(Key::Num9, c, ToggleGitWindow),
        b(Key::Num6, c, ToggleProblems),
        b(Key::F12, cs, HideToolWindows),
        b(Key::K, c, Commit),
        b(Key::O, ca, OpenFolder),
        b(Key::N, cs, NewScratchFile),
        b(Key::Equals, ca, ZoomIn),
        b(Key::Plus, ca, ZoomIn),
        b(Key::Minus, ca, ZoomOut),
        b(Key::Num0, ca, ZoomReset),
    ]
}

fn win_bindings(visual_studio: bool) -> Vec<(Chord, Action)> {
    use Action::*;
    let none = m(false, false, false, false);
    let s = m(false, false, true, false);
    let c = m(false, true, false, false);
    let cs = m(false, true, true, false);
    let a = m(true, false, false, false);
    let as_ = m(true, false, true, false);
    let ca = m(true, true, false, false);
    let cas = m(true, true, true, false);
    let b = |key, mods, action| (Chord { key, mods }, action);
    let mut v = vec![
        b(Key::F1, s, ExternalDocs),
        b(Key::F1, a, SelectIn),
        b(Key::ArrowLeft, none, Left(false)),
        b(Key::ArrowLeft, s, Left(true)),
        b(Key::ArrowRight, none, Right(false)),
        b(Key::ArrowRight, s, Right(true)),
        b(Key::ArrowUp, none, Up(false)),
        b(Key::ArrowUp, s, Up(true)),
        b(Key::ArrowDown, none, Down(false)),
        b(Key::ArrowDown, s, Down(true)),
        b(Key::ArrowLeft, c, WordLeft(false)),
        b(Key::ArrowLeft, cs, WordLeft(true)),
        b(Key::ArrowRight, c, WordRight(false)),
        b(Key::ArrowRight, cs, WordRight(true)),
        b(Key::Home, none, LineStart(false)),
        b(Key::Home, s, LineStart(true)),
        b(Key::End, none, LineEnd(false)),
        b(Key::End, s, LineEnd(true)),
        b(Key::Home, c, DocStart(false)),
        b(Key::Home, cs, DocStart(true)),
        b(Key::End, c, DocEnd(false)),
        b(Key::End, cs, DocEnd(true)),
        b(Key::PageUp, none, PageUp(false)),
        b(Key::PageUp, s, PageUp(true)),
        b(Key::PageDown, none, PageDown(false)),
        b(Key::PageDown, s, PageDown(true)),
        b(Key::ArrowUp, c, ScrollUp),
        b(Key::ArrowDown, c, ScrollDown),
        b(Key::Enter, none, Enter),
        b(Key::Enter, s, StartNewLine),
        b(Key::Enter, ca, StartNewLineBefore),
        b(Key::Backspace, none, Backspace),
        b(Key::Backspace, s, Backspace),
        b(Key::Delete, none, Delete),
        b(Key::Backspace, c, DeleteWordLeft),
        b(Key::Delete, c, DeleteWordRight),
        b(Key::Tab, none, Indent),
        b(Key::Tab, s, Unindent),
        b(Key::J, cs, JoinLines),
        b(Key::U, cs, ToggleCase),
        b(Key::Z, c, Undo),
        b(Key::Z, cs, Redo),
        b(Key::C, c, Copy),
        b(Key::X, c, Cut),
        b(Key::V, c, Paste),
        b(Key::Insert, c, Copy),
        b(Key::Insert, s, Paste),
        b(Key::Delete, s, Cut),
        b(Key::A, c, SelectAll),
        b(Key::Escape, none, Escape),
        b(Key::Space, c, Complete),
        // Ctrl+P opens Search Everywhere, so Parameter Info moves next to completion.
        b(Key::Space, cs, ParameterInfo),
        b(Key::P, c, SearchEverywhere),
        b(Key::Enter, a, ShowIntentions),
        b(Key::Insert, a, Generate),
        b(Key::F7, a, FindUsages),
        b(Key::G, c, GotoLine),
        b(Key::F, c, Find),
        b(Key::H, c, Replace),
        b(Key::F3, none, FindNext),
        b(Key::F3, s, FindPrevious),
        b(Key::F2, none, NextError),
        b(Key::F2, s, PreviousError),
        b(Key::F9, none, ToggleBreakpoint),
        b(Key::F11, c, ToggleBookmark),
        b(Key::F11, cs, ShowBookmarks),
        b(Key::F, cs, FindInFiles),
        b(Key::H, cs, ReplaceInFiles),
        b(Key::Minus, ca, NavigateBack),
        b(Key::S, c, Save),
        b(Key::S, cs, SaveAll),
        b(Key::F4, c, CloseTab),
        b(Key::Tab, c, NextTab),
        b(Key::Tab, cs, PreviousTab),
        b(Key::F5, none, Debug),
        b(Key::F5, c, Run),
        b(Key::F5, s, StopRun),
        b(Key::F10, none, StepOver),
        b(Key::F11, none, StepInto),
        b(Key::F11, s, StepOut),
        b(Key::F10, c, RunToCursor),
        b(Key::F9, s, Evaluate),
        b(Key::Insert, cas, NewScratchFile),
    ];
    if visual_studio {
        v.extend([
            b(Key::F12, none, GotoDeclaration),
            b(Key::F12, c, GotoImplementation),
            b(Key::L, cs, DeleteLine),
            b(Key::D, c, DuplicateLine),
            b(Key::ArrowUp, a, MoveLineUp),
            b(Key::ArrowDown, a, MoveLineDown),
            b(Key::Slash, c, CommentLine),
            b(Key::W, c, ExtendSelection),
            b(Key::W, cs, ShrinkSelection),
            b(Key::Period, cs, SelectNextOccurrence),
            b(Key::Comma, cs, UnselectOccurrence),
            b(Key::Semicolon, cs, SelectAllOccurrences),
            b(Key::R, c, Rename),
            b(Key::R, ca, Rename),
            b(Key::K, ca, Reformat),
            b(Key::T, c, SearchEverywhere),
            b(Key::T, cs, GotoFile),
            b(Key::Comma, c, SearchEverywhere),
            b(Key::Q, c, QuickDoc),
            b(Key::Tab, ca, RecentFiles),
            b(Key::B, cs, Build),
            b(Key::Comma, ca, ShowSettings),
            b(Key::L, ca, ToggleProjectWindow),
            b(Key::Backtick, c, ToggleTerminal),
            b(Key::Num0, ca, ToggleGitWindow),
            b(Key::M, cs, Fold),
            b(Key::L, c, Unfold),
            b(Key::Equals, c, ZoomIn),
            b(Key::Plus, c, ZoomIn),
            b(Key::Minus, c, ZoomOut),
            b(Key::Num0, c, ZoomReset),
        ]);
    } else {
        v.extend([
            b(Key::Y, c, DeleteLine),
            b(Key::D, c, DuplicateLine),
            b(Key::ArrowUp, as_, MoveLineUp),
            b(Key::ArrowDown, as_, MoveLineDown),
            b(Key::Slash, c, CommentLine),
            b(Key::Slash, cs, CommentBlock),
            b(Key::W, c, ExtendSelection),
            b(Key::W, cs, ShrinkSelection),
            b(Key::J, a, SelectNextOccurrence),
            b(Key::J, as_, UnselectOccurrence),
            b(Key::J, cas, SelectAllOccurrences),
            b(Key::Minus, c, Fold),
            b(Key::Equals, c, Unfold),
            b(Key::Plus, c, Unfold),
            b(Key::Minus, cs, FoldAll),
            b(Key::Equals, cs, UnfoldAll),
            b(Key::Q, c, QuickDoc),
            b(Key::B, c, GotoDeclaration),
            b(Key::B, ca, GotoImplementation),
            b(Key::B, cs, GotoTypeDeclaration),
            b(Key::U, c, GotoSuperMethod),
            b(Key::H, ca, ShowCallers),
            b(Key::H, cas, TypeHierarchy),
            b(Key::T, cas, RefactorThis),
            b(Key::O, ca, OptimizeImports),
            b(Key::A, cs, FindAction),
            b(Key::F6, s, Rename),
            b(Key::R, cs, Rename),
            b(Key::L, ca, Reformat),
            b(Key::F12, c, FileStructure),
            b(Key::R, c, Replace),
            b(Key::N, cs, GotoFile),
            b(Key::N, cas, GotoSymbol),
            b(Key::N, c, GotoClass),
            b(Key::T, c, SearchEverywhere),
            b(Key::E, c, RecentFiles),
            b(Key::E, cs, RecentLocations),
            b(Key::ArrowLeft, ca, NavigateBack),
            b(Key::ArrowRight, ca, NavigateForward),
            b(Key::F9, cs, Build),
            b(Key::F5, a, Resume),
            b(Key::F8, a, Evaluate),
            b(Key::S, ca, ShowSettings),
            b(Key::Num1, a, ToggleProjectWindow),
            b(Key::F12, a, ToggleTerminal),
            b(Key::Num9, a, ToggleGitWindow),
            b(Key::Num6, a, ToggleProblems),
            b(Key::F12, cs, HideToolWindows),
            b(Key::K, c, Commit),
            b(Key::Equals, cas, ZoomIn),
            b(Key::Plus, cas, ZoomIn),
            b(Key::Minus, cas, ZoomOut),
            b(Key::Num0, cas, ZoomReset),
        ]);
    }
    v
}

/// VS Code's default shortcuts, from its public keyboard shortcut reference. Two-step chords
/// (⌘K ⌘S...) are not supported, so their commands keep only the single-step shortcut, if any.
/// `mac` uses ⌘ where Windows uses Ctrl, and the few macOS-only differences.
fn vscode_bindings(mac: bool) -> Vec<(Chord, Action)> {
    use Action::*;
    let none = m(false, false, false, false);
    let s = m(false, false, true, false);
    let a = m(true, false, false, false);
    let as_ = m(true, false, true, false);
    // The "primary" modifier: ⌘ on macOS, Ctrl elsewhere.
    let c = if mac { m(false, false, false, true) } else { m(false, true, false, false) };
    let cs = Modifiers { shift: true, ..c };
    let ca = Modifiers { alt: true, ..c };
    let ctrl = m(false, true, false, false);
    let ctrl_s = m(false, true, true, false);
    let b = |key, mods, action| (Chord { key, mods }, action);
    let mut v = vec![
        b(Key::ArrowLeft, none, Left(false)),
        b(Key::ArrowLeft, s, Left(true)),
        b(Key::ArrowRight, none, Right(false)),
        b(Key::ArrowRight, s, Right(true)),
        b(Key::ArrowUp, none, Up(false)),
        b(Key::ArrowUp, s, Up(true)),
        b(Key::ArrowDown, none, Down(false)),
        b(Key::ArrowDown, s, Down(true)),
        b(Key::Home, none, LineStart(false)),
        b(Key::Home, s, LineStart(true)),
        b(Key::End, none, LineEnd(false)),
        b(Key::End, s, LineEnd(true)),
        b(Key::PageUp, none, PageUp(false)),
        b(Key::PageUp, s, PageUp(true)),
        b(Key::PageDown, none, PageDown(false)),
        b(Key::PageDown, s, PageDown(true)),
        b(Key::Enter, none, Enter),
        b(Key::Enter, c, StartNewLine),
        b(Key::Enter, cs, StartNewLineBefore),
        b(Key::Backspace, none, Backspace),
        b(Key::Backspace, s, Backspace),
        b(Key::Delete, none, Delete),
        b(Key::K, cs, DeleteLine),
        b(Key::ArrowDown, as_, DuplicateLine),
        b(Key::ArrowUp, a, MoveLineUp),
        b(Key::ArrowDown, a, MoveLineDown),
        b(Key::Slash, c, CommentLine),
        b(Key::A, as_, CommentBlock),
        b(Key::Tab, none, Indent),
        b(Key::Tab, s, Unindent),
        b(Key::CloseBracket, c, Indent),
        b(Key::OpenBracket, c, Unindent),
        b(Key::Z, c, Undo),
        b(Key::Z, cs, Redo),
        b(Key::C, c, Copy),
        b(Key::X, c, Cut),
        b(Key::V, c, Paste),
        b(Key::A, c, SelectAll),
        b(Key::D, c, SelectNextOccurrence),
        b(Key::U, c, UnselectOccurrence),
        b(Key::L, cs, SelectAllOccurrences),
        b(Key::ArrowUp, ca, CloneCaretAbove),
        b(Key::ArrowDown, ca, CloneCaretBelow),
        b(Key::Escape, none, Escape),
        b(Key::OpenBracket, ca, Fold),
        b(Key::CloseBracket, ca, Unfold),
        b(Key::Space, ctrl, Complete),
        b(Key::Space, cs, ParameterInfo),
        b(Key::Period, c, ShowIntentions),
        b(Key::R, ctrl_s, RefactorThis),
        b(Key::F12, none, GotoDeclaration),
        b(Key::F12, c, GotoImplementation),
        b(Key::F12, s, FindUsages),
        b(Key::F12, as_, FindUsages),
        b(Key::H, as_, ShowCallers),
        b(Key::F2, none, Rename),
        b(Key::F, as_, Reformat),
        b(Key::O, as_, OptimizeImports),
        b(Key::O, cs, FileStructure),
        b(Key::G, ctrl, GotoLine),
        b(Key::F, c, Find),
        b(Key::F3, none, FindNext),
        b(Key::F3, s, FindPrevious),
        b(Key::F8, none, NextError),
        b(Key::F8, s, PreviousError),
        b(Key::F9, none, ToggleBreakpoint),
        b(Key::P, c, GotoFile),
        b(Key::P, cs, FindAction),
        b(Key::F1, none, FindAction),
        b(Key::T, c, GotoSymbol),
        b(Key::F, cs, FindInFiles),
        b(Key::H, cs, ReplaceInFiles),
        b(Key::R, ctrl, RecentFiles),
        b(Key::S, c, Save),
        b(Key::W, c, CloseTab),
        b(Key::Tab, ctrl, NextTab),
        b(Key::Tab, ctrl_s, PreviousTab),
        b(Key::B, cs, Build),
        b(Key::F5, none, Debug),
        b(Key::F5, ctrl, Run),
        b(Key::F5, s, StopRun),
        b(Key::F10, none, StepOver),
        b(Key::F11, none, StepInto),
        b(Key::F11, s, StepOut),
        b(Key::Comma, c, ShowSettings),
        b(Key::B, c, ToggleProjectWindow),
        b(Key::E, cs, ToggleProjectWindow),
        b(Key::Backtick, ctrl, ToggleTerminal),
        b(Key::G, ctrl_s, ToggleGitWindow),
        b(Key::M, cs, ToggleProblems),
        b(Key::N, c, NewScratchFile),
        b(Key::O, c, OpenFolder),
        b(Key::Equals, c, ZoomIn),
        b(Key::Plus, c, ZoomIn),
        b(Key::Minus, c, ZoomOut),
    ];
    if mac {
        let opt_cmd_shift = m(true, false, true, true);
        v.extend([
            b(Key::ArrowLeft, a, WordLeft(false)),
            b(Key::ArrowLeft, as_, WordLeft(true)),
            b(Key::ArrowRight, a, WordRight(false)),
            b(Key::ArrowRight, as_, WordRight(true)),
            b(Key::ArrowLeft, c, LineStart(false)),
            b(Key::ArrowLeft, cs, LineStart(true)),
            b(Key::ArrowRight, c, LineEnd(false)),
            b(Key::ArrowRight, cs, LineEnd(true)),
            b(Key::ArrowUp, c, DocStart(false)),
            b(Key::ArrowUp, cs, DocStart(true)),
            b(Key::ArrowDown, c, DocEnd(false)),
            b(Key::ArrowDown, cs, DocEnd(true)),
            b(Key::Backspace, a, DeleteWordLeft),
            b(Key::Delete, a, DeleteWordRight),
            b(Key::ArrowRight, m(false, true, true, true), ExtendSelection),
            b(Key::ArrowLeft, m(false, true, true, true), ShrinkSelection),
            b(Key::F, ca, Replace),
            b(Key::G, c, FindNext),
            b(Key::G, cs, FindPrevious),
            b(Key::Minus, ctrl, NavigateBack),
            b(Key::Minus, ctrl_s, NavigateForward),
            b(Key::ArrowRight, ca, NextTab),
            b(Key::ArrowLeft, ca, PreviousTab),
            b(Key::S, ca, SaveAll),
            b(Key::Num0, c, ZoomReset),
            b(Key::N, opt_cmd_shift, NewScratchFile),
        ]);
    } else {
        v.extend([
            b(Key::ArrowLeft, c, WordLeft(false)),
            b(Key::ArrowLeft, cs, WordLeft(true)),
            b(Key::ArrowRight, c, WordRight(false)),
            b(Key::ArrowRight, cs, WordRight(true)),
            b(Key::Home, c, DocStart(false)),
            b(Key::Home, cs, DocStart(true)),
            b(Key::End, c, DocEnd(false)),
            b(Key::End, cs, DocEnd(true)),
            b(Key::ArrowUp, c, ScrollUp),
            b(Key::ArrowDown, c, ScrollDown),
            b(Key::Backspace, c, DeleteWordLeft),
            b(Key::Delete, c, DeleteWordRight),
            b(Key::Y, c, Redo),
            b(Key::Insert, c, Copy),
            b(Key::Insert, s, Paste),
            b(Key::Delete, s, Cut),
            b(Key::ArrowRight, as_, ExtendSelection),
            b(Key::ArrowLeft, as_, ShrinkSelection),
            b(Key::I, cs, Reformat),
            b(Key::H, c, Replace),
            b(Key::ArrowLeft, a, NavigateBack),
            b(Key::ArrowRight, a, NavigateForward),
            b(Key::F4, c, CloseTab),
            b(Key::PageDown, c, NextTab),
            b(Key::PageUp, c, PreviousTab),
            b(Key::Num0, c, ZoomReset),
        ]);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rider_mac_shortcuts() {
        let k = Keymap::new(KeymapKind::RiderMac);
        assert_eq!(k.lookup(Key::D, m(false, false, false, true)), Some(Action::DuplicateLine));
        assert_eq!(k.lookup(Key::Enter, m(true, false, false, false)), Some(Action::ShowIntentions));
        assert_eq!(k.lookup(Key::B, m(false, false, false, true)), Some(Action::GotoDeclaration));
        assert_eq!(k.shortcut_text(Action::FindInFiles).as_deref(), Some("⇧⌘F"));
    }

    #[test]
    fn no_chord_is_bound_twice() {
        for kind in [KeymapKind::RiderMac, KeymapKind::Rider, KeymapKind::VisualStudio, KeymapKind::VsCodeMac, KeymapKind::VsCode] {
            let k = Keymap::new(kind);
            for (i, (a, _)) in k.bindings.iter().enumerate() {
                for (b, act) in &k.bindings[i + 1..] {
                    assert!(
                        !(a.key == b.key && (a.mods.alt, a.mods.ctrl, a.mods.shift, a.mods.mac_cmd) == (b.mods.alt, b.mods.ctrl, b.mods.shift, b.mods.mac_cmd)),
                        "{kind:?}: {a:?} bound twice ({act:?})"
                    );
                }
            }
        }
        assert_eq!(Keymap::new(KeymapKind::Rider).shortcut_text(Action::FindInFiles).as_deref(), Some("Ctrl+Shift+F"));
        for kind in [KeymapKind::Rider, KeymapKind::VisualStudio] {
            let k = Keymap::new(kind);
            assert_eq!(k.lookup(Key::P, m(false, true, false, false)), Some(Action::SearchEverywhere));
            assert_eq!(k.shortcut_text(Action::SearchEverywhere).as_deref(), Some("Ctrl+P"));
            assert_eq!(k.shortcut_text(Action::ParameterInfo).as_deref(), Some("Ctrl+Shift+Space"));
        }
        let mac = Keymap::new(KeymapKind::RiderMac);
        assert_eq!(mac.shortcut_text(Action::SearchEverywhere).as_deref(), Some("⌘P"));
        assert_eq!(mac.shortcut_text(Action::ParameterInfo).as_deref(), Some("⌃⇧Space"));
    }

    #[test]
    fn vscode_shortcuts() {
        let ctrl = m(false, true, false, false);
        let ctrl_shift = m(false, true, true, false);
        let k = Keymap::new(KeymapKind::VsCode);
        assert_eq!(k.lookup(Key::P, ctrl), Some(Action::GotoFile));
        assert_eq!(k.lookup(Key::P, ctrl_shift), Some(Action::FindAction));
        assert_eq!(k.lookup(Key::F12, m(false, false, false, false)), Some(Action::GotoDeclaration));
        assert_eq!(k.lookup(Key::Period, ctrl), Some(Action::ShowIntentions));
        assert_eq!(k.lookup(Key::D, ctrl), Some(Action::SelectNextOccurrence));
        assert_eq!(k.lookup(Key::F, m(true, false, true, false)), Some(Action::Reformat));
        assert_eq!(k.shortcut_text(Action::ToggleTerminal).as_deref(), Some("Ctrl+`"));
        assert_eq!(k.shortcut_text(Action::Rename).as_deref(), Some("F2"));

        let cmd = m(false, false, false, true);
        let mac = Keymap::new(KeymapKind::VsCodeMac);
        assert_eq!(mac.lookup(Key::P, cmd), Some(Action::GotoFile));
        assert_eq!(mac.lookup(Key::P, ctrl), None);
        assert_eq!(mac.lookup(Key::ArrowLeft, cmd), Some(Action::LineStart(false)));
        assert_eq!(mac.shortcut_text(Action::FindAction).as_deref(), Some("⇧⌘P"));
        assert_eq!(mac.shortcut_text(Action::ToggleTerminal).as_deref(), Some("⌃`"));
        // Every action of the shared part is reachable on both systems.
        for action in [Action::Save, Action::Find, Action::Build, Action::Debug, Action::GotoSymbol, Action::NextError] {
            assert!(k.chord(action).is_some() && mac.chord(action).is_some(), "{action:?}");
        }
    }

    #[test]
    fn shortcuts_round_trip_and_overrides_move_them() {
        let ctrl_shift = m(false, true, true, false);
        let c = Chord { key: Key::K, mods: ctrl_shift };
        assert_eq!(chord_id(c), "ctrl+shift+K");
        assert_eq!(parse_chord("ctrl+shift+K"), Some(c));
        assert_eq!(parse_chord("cmd+F12").map(|c| (c.key, c.mods.mac_cmd)), Some((Key::F12, true)));
        assert_eq!(parse_chord("hyper+K"), None);
        assert_eq!(parse_chord("ctrl+NoSuchKey"), None);
        assert_eq!(parse_chord(""), None);

        // Ctrl+Shift+K is Commit-less on Rider for Windows: give it to Commit, take Ctrl+K away.
        let k = Keymap::with_overrides(KeymapKind::Rider, &[(Action::Commit, Some(c)), (Action::Save, None)]);
        assert_eq!(k.action_of(c), Some(Action::Commit));
        assert_eq!(k.lookup(Key::K, m(false, true, false, false)), None, "the old shortcut is gone");
        assert_eq!(k.shortcut_text(Action::Commit).as_deref(), Some("Ctrl+Shift+K"));
        assert_eq!(k.chord(Action::Save), None);
        // A shortcut taken from another action leaves it without that one.
        let find = Keymap::new(KeymapKind::Rider).chord(Action::Find).unwrap();
        let k = Keymap::with_overrides(KeymapKind::Rider, &[(Action::GotoLine, Some(find))]);
        assert_eq!(k.action_of(find), Some(Action::GotoLine));
        assert_ne!(k.chord(Action::Find), Some(find));
    }
}
