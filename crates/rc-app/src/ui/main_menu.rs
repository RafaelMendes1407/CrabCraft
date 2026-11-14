//! Central native-egui menus. Commands use the same dispatch path as keyboard shortcuts.
use crate::{app::CrabCraftApp, toolwindow::ToolWindow};
use egui::Ui;
use rc_editor::Action;
use rc_kit::{i18n::tr, ico, theme};

pub fn groups() -> Vec<(&'static str, Vec<(&'static str, Action)>)> {
    use Action::*;
    vec![
        (
            tr("File"),
            vec![
                (tr("New Scratch File…"), NewScratchFile),
                (tr("Save All"), SaveAll),
                (tr("Revert from Disk…"), RevertFromDisk),
                (tr("Show Local History"), ShowLocalHistory),
                (tr("Close Tab"), CloseTab),
                (tr("Save"), Save),
                (tr("Settings…"), ShowSettings),
            ],
        ),
        (
            tr("Edit"),
            vec![
                (tr("Undo"), Undo),
                (tr("Redo"), Redo),
                (tr("Cut"), Cut),
                (tr("Copy"), Copy),
                (tr("Paste"), Paste),
                (tr("Select All"), SelectAll),
                (tr("Find…"), Find),
                (tr("Replace…"), Replace),
                (tr("Find in Files…"), FindInFiles),
                (tr("Replace in Files…"), ReplaceInFiles),
                (tr("Duplicate Line or Selection"), DuplicateLine),
                (tr("Comment with Line Comment"), CommentLine),
                (tr("Comment with Block Comment"), CommentBlock),
                (tr("Add Selection for Next Occurrence"), SelectNextOccurrence),
                (tr("Select All Occurrences"), SelectAllOccurrences),
                (tr("Join Lines"), JoinLines),
                (tr("Toggle Case"), ToggleCase),
                (tr("Extend Selection"), ExtendSelection),
                (tr("Shrink Selection"), ShrinkSelection),
                (tr("Move Line Up"), MoveLineUp),
                (tr("Move Line Down"), MoveLineDown),
            ],
        ),
        (
            tr("View"),
            vec![
                (tr("Zoom In"), ZoomIn),
                (tr("Zoom Out"), ZoomOut),
                (tr("Reset Zoom"), ZoomReset),
                (tr("Hide All Tool Windows"), HideToolWindows),
                (tr("Collapse"), Fold),
                (tr("Expand"), Unfold),
                (tr("Collapse All"), FoldAll),
                (tr("Expand All"), UnfoldAll),
            ],
        ),
        (
            tr("Navigate"),
            vec![
                (tr("Search Everywhere"), SearchEverywhere),
                (tr("Find Action…"), FindAction),
                (tr("Go to File…"), GotoFile),
                (tr("Go to Type…"), GotoClass),
                (tr("Go to Symbol…"), GotoSymbol),
                (tr("Go to Line…"), GotoLine),
                (tr("Back"), NavigateBack),
                (tr("Forward"), NavigateForward),
                (tr("Recent Files"), RecentFiles),
                (tr("Select In…"), SelectIn),
                (tr("Recent Locations"), RecentLocations),
                (tr("Go to Declaration"), GotoDeclaration),
                (tr("Go to Implementation"), GotoImplementation),
                (tr("Go to Type Declaration"), GotoTypeDeclaration),
                (tr("Go to Super Method"), GotoSuperMethod),
                (tr("Show Callers"), ShowCallers),
                (tr("Type Hierarchy"), TypeHierarchy),
                (tr("Show Callees"), ShowCallees),
                (tr("Find Usages"), FindUsages),
                (tr("File Structure"), FileStructure),
                (tr("Show Context Info"), ShowContextInfo),
                (tr("Next Highlighted Error"), NextError),
                (tr("Previous Highlighted Error"), PreviousError),
            ],
        ),
        (
            tr("Code"),
            vec![
                (tr("Generate…"), Generate),
                (tr("Optimize Imports"), OptimizeImports),
                (tr("Code Completion"), Complete),
                (tr("Complete Current Statement"), CompleteStatement),
                (tr("Parameter Info"), ParameterInfo),
                (tr("External Documentation"), ExternalDocs),
                (tr("Expand Macro Recursively"), ExpandMacro),
                (tr("Quick Documentation"), QuickDoc),
                (tr("Show Context Actions"), ShowIntentions),
                (tr("Reformat Code"), Reformat),
                (
                    if cfg!(unix) { tr("Expand Declarative Macro One Step") } else { tr("Expand Declarative Macro One Step (unsupported on this platform)") },
                    ExpandMacroStep,
                ),
            ],
        ),
        (
            tr("Refactor"),
            vec![
                (tr("Rename…"), Rename),
                (tr("Refactor This…"), RefactorThis),
                (tr("Move public Rust item…"), MoveItem),
                (tr("Change Rust Function Signature"), ChangeSignature),
            ],
        ),
        (tr("Build"), vec![(tr("Build Project"), Build)]),
        (
            tr("Run"),
            vec![
                (tr("Run"), Run),
                (tr("Debug"), Debug),
                (tr("Profile"), Profile),
                (tr("Stop"), StopRun),
                (tr("Run Tests"), RunTests),
                (tr("Step Over"), StepOver),
                (tr("Step Into"), StepInto),
                (tr("Step Out"), StepOut),
                (tr("Run to Cursor"), RunToCursor),
                (tr("Force Step Into"), ForceStepInto),
                (tr("Force Step Over"), ForceStepOver),
                (tr("Force Run to Cursor"), ForceRunToCursor),
                (tr("Set Execution Point to Cursor"), SetExecutionPoint),
                (tr("View Breakpoints…"), ViewBreakpoints),
                (tr("Resume Program"), Resume),
                (tr("Evaluate Expression…"), Evaluate),
                (tr("Show Integers in Hex"), ToggleHexValues),
                (tr("Attach to Process…"), AttachToProcess),
                (tr("Toggle Line Breakpoint"), ToggleBreakpoint),
                (tr("Toggle Bookmark"), ToggleBookmark),
                (tr("Show Bookmarks"), ShowBookmarks),
            ],
        ),
        (
            tr("Tools"),
            vec![
                (tr("Invalidate Caches…"), InvalidateCaches),
                (tr("Inspect Code"), InspectCode),
                (tr("Project Environment Variables…"), ProjectEnvironment),
                (tr("Run Anything…"), RunAnything),
                (tr("Test Declarative Macro…"), TestDeclarativeMacro),
                (tr("Remote Rust workspace…"), RemoteDevelopment),
                (tr("WSL Rust workspace…"), WslWorkspace),
                (tr("Share in Playground…"), ShareInPlayground),
                (tr("Update Copyright"), UpdateCopyright),
            ],
        ),
        (tr("Git"), vec![(tr("Commit…"), Commit)]),
        (tr("Window"), vec![(tr("Next Tab"), NextTab), (tr("Previous Tab"), PreviousTab)]),
        (tr("Help"), vec![]),
    ]
}

fn command(ui: &mut Ui, label: &str, icon: Option<rc_kit::Ico>, enabled: bool) -> bool {
    let clicked = theme::menu_entry(ui, icon, label, None, false, enabled).clicked();
    if clicked {
        ui.close();
    }
    clicked
}

/// Use the same preconditions as the command handlers and the debug toolbar.
fn action_enabled(app: &CrabCraftApp, action: Action) -> bool {
    use crate::runner::ConfigKind;
    let selected = app.runner.selected_config();
    let paused = app.debugger.stopped();
    let document = app.docs.active_doc().is_some();
    match action {
        Action::ShowSettings | Action::ZoomIn | Action::ZoomOut | Action::ZoomReset | Action::NewScratchFile => true,
        Action::Save
        | Action::CloseTab
        | Action::NextTab
        | Action::PreviousTab
        | Action::RevertFromDisk
        | Action::ShowLocalHistory
        | Action::Find
        | Action::Replace
        | Action::GotoLine
        | Action::ToggleBreakpoint
        | Action::ToggleBookmark => document,
        Action::SaveAll => !app.docs.list.is_empty(),
        Action::Run => selected.is_some(),
        Action::Debug => paused || selected.is_some_and(|config| matches!(config.kind, ConfigKind::Cargo { .. })),
        Action::Profile => cfg!(target_os = "linux") && selected.is_some_and(|config| matches!(config.kind, ConfigKind::Cargo { .. })),
        Action::StopRun => app.runner.busy() || app.debugger.active() || app.disk_save.saves.busy() || app.disk_save.continuation.is_some(),
        Action::Resume | Action::StepOver | Action::StepInto | Action::StepOut | Action::ForceStepOver | Action::Evaluate => paused,
        Action::RunToCursor | Action::ForceRunToCursor => paused && document,
        Action::ForceStepInto => paused && app.debugger.session.as_ref().is_some_and(|session| session.can_force_step_in()),
        Action::SetExecutionPoint => paused && document && app.debugger.session.as_ref().is_some_and(|session| session.can_set_execution_point()),
        Action::AttachToProcess => app.runner.cargo_workspace.is_some(),
        Action::Commit => app.vcs.repo.is_some(),
        Action::NavigateBack => !app.nav_back.is_empty(),
        Action::NavigateForward => !app.nav_forward.is_empty(),
        Action::ExpandMacroStep if !cfg!(unix) => false,
        _ if action.is_editing() => document,
        _ => app.workspace.is_some(),
    }
}

fn configuration_editable(app: &CrabCraftApp) -> bool {
    use crate::runner::ConfigKind;
    app.runner
        .selected_config()
        .is_some_and(|config| matches!(config.kind, ConfigKind::Cargo { .. } | ConfigKind::WasmPack { .. } | ConfigKind::Compound { .. }))
}

fn disabled_hint(app: &CrabCraftApp, action: Action) -> Option<&'static str> {
    use crate::runner::ConfigKind;
    match action {
        Action::Debug => match app.runner.selected_config().map(|config| &config.kind) {
            Some(ConfigKind::WasmPack { .. }) => Some(tr("Debugging wasm-pack configurations is not supported")),
            Some(ConfigKind::Tests { .. } | ConfigKind::TestSelection { .. }) => {
                Some(tr("Test configurations cannot be debugged yet: use Debug in the gutter of the test"))
            }
            _ => Some(tr("Choose an application run configuration to debug")),
        },
        Action::Profile if !cfg!(target_os = "linux") => Some(tr("The profiler uses perf, which is only available on Linux.")),
        Action::Profile => Some(tr("Choose the run configuration of a Rust binary to profile it")),
        Action::ForceStepInto if app.debugger.stopped() => Some(tr("This debugger cannot force step into")),
        _ => None,
    }
}

fn scroll_menu(ui: &mut Ui, width: f32, id: &'static str, contents: impl FnOnce(&mut Ui)) {
    ui.set_width(width);
    // Popup placement depends on its measured size. A limit based on the current
    // anchor feeds that placement back into the next frame and can shrink forever.
    let height = (ui.ctx().content_rect().height() - 64.0).max(80.0);
    ui.set_max_height(height);
    egui::ScrollArea::vertical().id_salt(id).max_height(height).show(ui, contents);
}

pub fn show(app: &mut CrabCraftApp, ui: &mut Ui) {
    scroll_menu(ui, 200.0, "main_menu_categories", |ui| {
        for (category, actions) in groups() {
            ui.menu_button(category, |ui| {
                ui.set_width(360.0);
                let height = (ui.ctx().content_rect().height() - 64.0).max(80.0);
                ui.set_max_height(height);
                // Bound long menus independently of the editor and the tool windows.
                egui::ScrollArea::vertical().max_height(height).show(ui, |ui| {
                    if category == tr("File") {
                        file_commands(app, ui);
                    }
                    for (label, action) in actions {
                        let enabled = action_enabled(app, action);
                        let shortcut = app.keymap.shortcut_text(action);
                        let response = theme::menu_entry(ui, crate::search::action_icon(action), label, shortcut.as_deref(), false, enabled);
                        let response =
                            if !enabled { if let Some(hint) = disabled_hint(app, action) { response.on_hover_text(hint) } else { response } } else { response };
                        if response.clicked() {
                            app.pending.push(action);
                            app.focus_editor = true;
                            ui.close();
                        }
                    }
                    if category == tr("File") {
                        ui.separator();
                        if command(ui, tr("Quit CrabCraft"), None, true) {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    }
                    if category == tr("View") {
                        ui.separator();
                        ui.menu_button(tr("Tool Windows"), |ui| {
                            scroll_menu(ui, 240.0, "main_menu_tool_windows", |ui| {
                                for tool in ToolWindow::ALL {
                                    if theme::menu_entry(ui, Some(tool.icon()), tool.title(), None, app.dock.is_open(tool), app.workspace.is_some()).clicked() {
                                        app.toggle_tool(tool);
                                        ui.close();
                                    }
                                }
                            });
                        });
                    }
                    if category == tr("Build") {
                        build_commands(app, ui);
                    }
                    if category == tr("Run") && command(ui, tr("Edit Run Configuration…"), Some(ico::SETTINGS), configuration_editable(app)) {
                        app.edit_run_config(None);
                    }
                    if category == tr("Tools") {
                        tools(app, ui);
                    }
                    if category == tr("Git") && app.vcs.repo.is_some() {
                        ui.separator();
                        crate::ui::git::branches_menu(app, ui);
                    }
                    if category == tr("Window") {
                        crate::ui::editor_area::window_menu(app, ui);
                        if command(ui, tr("Minimize"), None, true) {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                        }
                    }
                    if category == tr("Help") {
                        if command(ui, tr("Find Action…"), Some(ico::SEARCH), app.workspace.is_some()) {
                            app.pending.push(Action::FindAction);
                        }
                        if command(ui, tr("About CrabCraft"), Some(ico::INFO), true) {
                            app.dialog = Some(crate::ui::dialogs::Dialog::About);
                        }
                    }
                });
            });
        }
    });
}

fn file_commands(app: &mut CrabCraftApp, ui: &mut Ui) {
    if command(ui, tr("New Project…"), Some(ico::ADD), true) {
        app.new_project_dialog();
    }
    if command(ui, tr("New File…"), Some(ico::NEW_FILE), app.workspace.is_some()) {
        app.new_file_dialog();
    }
    if command(ui, tr("Open…"), Some(ico::OPEN), true) {
        app.pick_and_open();
    }
    if command(ui, tr("Open File…"), Some(ico::OPEN), true) {
        app.pick_file();
    }
    ui.menu_button(tr("Recent Projects"), |ui| {
        scroll_menu(ui, 300.0, "main_menu_recent_projects", |ui| {
            for path in app.settings.recent_workspaces.clone() {
                let name = path.file_name().unwrap_or(path.as_os_str()).to_string_lossy();
                let chars: Vec<_> = name.chars().take(128).collect();
                let available = (ui.available_width() - 46.0).max(40.0);
                let fits =
                    |text: &str| ui.painter().layout_no_wrap(text.to_string(), egui::FontId::proportional(13.0), rc_kit::pal(ui).text).size().x <= available;
                let mut label: String = chars.iter().collect();
                if name.chars().count() > chars.len() || !fits(&label) {
                    let (mut low, mut high) = (0, chars.len());
                    while low < high {
                        let mid = low + (high - low).div_ceil(2);
                        let candidate = chars[..mid].iter().collect::<String>() + "…";
                        if fits(&candidate) {
                            low = mid;
                        } else {
                            high = mid - 1;
                        }
                    }
                    label = chars[..low].iter().collect::<String>() + "…";
                }
                if theme::menu_entry(ui, Some(ico::FOLDER), &label, None, false, true).on_hover_text(path.display().to_string()).clicked() {
                    ui.close();
                    app.open_workspace(&path);
                }
            }
        });
    });
    if command(ui, tr("Close Project"), Some(ico::CLOSE), app.workspace.is_some()) {
        app.close_project();
    }
    ui.separator();
}

fn build_commands(app: &mut CrabCraftApp, ui: &mut Ui) {
    for (label, action, icon) in [
        (tr("Rebuild Project"), rc_dotnet::BuildAction::Rebuild, ico::REBUILD),
        (tr("Clean Project"), rc_dotnet::BuildAction::Clean, ico::DELETE),
        (tr("Fetch Dependencies"), rc_dotnet::BuildAction::Restore, ico::DOWNLOAD),
    ] {
        if command(ui, label, Some(icon), app.workspace.is_some()) {
            if let Some(target) = app.workspace.as_ref().map(|w| w.ws.key_path().to_path_buf()) {
                app.build_project_after_save(&target, action);
            }
        }
    }
}

fn tools(app: &mut CrabCraftApp, ui: &mut Ui) {
    ui.separator();
    for tool in [
        ToolWindow::Cargo,
        ToolWindow::Http,
        ToolWindow::Endpoints,
        ToolWindow::Database,
        ToolWindow::Remote,
        ToolWindow::Agents,
        ToolWindow::Plugins,
        ToolWindow::Proofreading,
        ToolWindow::Containers,
        ToolWindow::DevContainers,
        ToolWindow::PrivilegedRun,
        ToolWindow::RustProvider,
        ToolWindow::Profiler,
    ] {
        if command(ui, tool.title(), Some(tool.icon()), app.workspace.is_some()) {
            app.dock.show(tool);
            app.session_dirty = true;
        }
    }
    if command(ui, tr("Local AI assistant"), Some(ico::RUN_ANYTHING), app.workspace.is_some()) {
        app.local_ai.open = true;
    }
    ui.menu_button(tr("External Tools"), |ui| {
        for name in app.settings.external_tools.iter().map(|tool| tool.name.clone()).collect::<Vec<_>>() {
            if command(ui, &name, Some(ico::RUN_ANYTHING), app.workspace.is_some()) {
                app.run_external_tool(&name);
            }
        }
        if command(ui, tr("Configure External Tools…"), Some(ico::SETTINGS), true) {
            app.open_settings(crate::ui::dialogs::SettingsPage::ExternalTools);
        }
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn main_menu_contains_every_searchable_command() {
        let actions: std::collections::HashSet<_> =
            super::groups().into_iter().flat_map(|(_, entries)| entries.into_iter().map(|(_, action)| action)).collect();
        for (label, action) in crate::search::action_items() {
            assert!(
                actions.contains(&action)
                    || matches!(
                        action,
                        rc_editor::Action::ToggleProjectWindow
                            | rc_editor::Action::ToggleTerminal
                            | rc_editor::Action::ToggleProblems
                            | rc_editor::Action::ToggleGitWindow
                    ),
                "command missing from central menus: {label}"
            );
        }
    }

    #[test]
    fn unavailable_run_and_debug_commands_are_disabled_before_a_session() {
        use super::{action_enabled, configuration_editable, disabled_hint};
        use crate::runner::{ConfigKind, RunConfig};
        use rc_editor::Action;
        let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let home = tempfile::tempdir().unwrap();
        let mut app = crate::screens::test_app(home.path(), true);
        for action in [
            Action::Run,
            Action::Debug,
            Action::Profile,
            Action::StopRun,
            Action::Resume,
            Action::StepOver,
            Action::StepInto,
            Action::StepOut,
            Action::ForceStepOver,
            Action::ForceStepInto,
            Action::Evaluate,
            Action::RunToCursor,
            Action::SetExecutionPoint,
        ] {
            assert!(!action_enabled(&app, action), "{action:?} must not be offered without its required run/debug context");
        }
        assert!(!configuration_editable(&app));
        app.runner.configs = vec![RunConfig { name: "wasm".into(), kind: ConfigKind::WasmPack { manifest: home.path().join("Cargo.toml") } }];
        assert!(action_enabled(&app, Action::Run));
        assert!(!action_enabled(&app, Action::Debug));
        assert!(!action_enabled(&app, Action::Profile));
        assert!(disabled_hint(&app, Action::Debug).is_some());
        assert!(configuration_editable(&app));
        app.runner.configs[0].kind = ConfigKind::Tests { target: home.path().join("Cargo.toml") };
        assert!(action_enabled(&app, Action::Run));
        assert!(!action_enabled(&app, Action::Debug));
        assert!(!configuration_editable(&app));
        app.runner.configs[0].kind = ConfigKind::Cargo { package: "example".into(), bin: "example".into() };
        assert!(action_enabled(&app, Action::Run));
        assert!(action_enabled(&app, Action::Debug));
        assert_eq!(action_enabled(&app, Action::Profile), cfg!(target_os = "linux"));
        assert!(configuration_editable(&app));
    }

    fn scroll_down(harness: &mut egui_kittest::Harness<'static, crate::app::CrabCraftApp>, pointer: egui::Pos2) {
        harness.input_mut().events.push(egui::Event::PointerMoved(pointer));
        harness.input_mut().events.push(egui::Event::MouseWheel {
            source: egui::MouseWheelSource::Unknown,
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -2000.0),
            phase: egui::TouchPhase::Move,
            modifiers: Default::default(),
        });
        harness.run_steps(15);
    }

    #[test]
    fn root_dropdown_scrolls_to_help_in_a_short_viewport() {
        use egui_kittest::kittest::Queryable;
        use rc_kit::i18n::tr;
        let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let home = tempfile::tempdir().unwrap();
        let mut harness = crate::screens::harness(crate::screens::test_app(home.path(), true), egui::vec2(800.0, 300.0));
        harness.get_by_label(tr("Main Menu")).click();
        harness.run_steps(3);
        scroll_down(&mut harness, egui::pos2(100.0, 200.0));
        let help_label = format!("{} ⏵", tr("Help"));
        let help = harness.get_by_label(&help_label);
        assert!(help.rect().bottom() < 300.0, "Help must remain reachable: {:?}", help.rect());
        help.click();
        harness.run_steps(3);
        harness.get_by_label(tr("About CrabCraft")).click();
        harness.run_steps(3);
        assert!(matches!(harness.state().dialog, Some(crate::ui::dialogs::Dialog::About)));
    }

    #[test]
    fn recent_projects_submenu_fits_long_names_and_scrolls() {
        use egui_kittest::kittest::Queryable;
        use rc_kit::i18n::tr;
        let _lock = crate::screens::HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let home = tempfile::tempdir().unwrap();
        let mut app = crate::screens::test_app(home.path(), true);
        app.settings.recent_workspaces = (0..60).map(|index| home.path().join(format!("project-{index:02}-{}", "語".repeat(80)))).collect();
        let mut harness = crate::screens::harness(app, egui::vec2(800.0, 340.0));
        harness.get_by_label(tr("Main Menu")).click();
        harness.run_steps(3);
        harness.get_by_label(&format!("{} ⏵", tr("File"))).click();
        harness.run_steps(3);
        harness.get_by_label(&format!("{} ⏵", tr("Recent Projects"))).click();
        harness.run_steps(3);
        let first = harness.query_all_by_label_contains("project-00-").find(|node| node.rect().width() > 100.0 && node.rect().width() < 320.0).unwrap().rect();
        assert!(first.width() < 320.0 && first.left() >= 0.0 && first.right() <= 800.0, "recent item must fit the viewport: {first:?}");
        scroll_down(&mut harness, first.center());
        let last = harness.query_all_by_label_contains("project-59-").find(|node| node.rect().width() > 100.0 && node.rect().width() < 320.0).unwrap().rect();
        assert!(last.top() >= 0.0 && last.bottom() < 340.0, "last recent project must be reachable: {last:?}");
    }
}
