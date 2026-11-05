//! Central native-egui menus. Commands use the same dispatch path as keyboard shortcuts.
use crate::{app::CrabCraftApp, toolwindow::ToolWindow};
use egui::{Id, Ui};
use rc_editor::Action;
use rc_kit::{i18n::tr, ico, theme};

pub fn toggle(ctx: &egui::Context) {
    ctx.data_mut(|data| {
        let id = Id::new("main_menu_visible");
        let visible = data.get_temp::<bool>(id).unwrap_or(false);
        data.insert_temp(id, !visible);
    });
}

pub fn visible(ctx: &egui::Context) -> bool {
    ctx.data(|data| {
        data.get_temp::<bool>(Id::new("main_menu_visible"))
            .unwrap_or(false)
    })
}

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
                (
                    tr("Add Selection for Next Occurrence"),
                    SelectNextOccurrence,
                ),
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
                    if cfg!(unix) {
                        tr("Expand Declarative Macro One Step")
                    } else {
                        tr("Expand Declarative Macro One Step (unsupported on this platform)")
                    },
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
        (
            tr("Window"),
            vec![(tr("Next Tab"), NextTab), (tr("Previous Tab"), PreviousTab)],
        ),
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

pub fn show(app: &mut CrabCraftApp, ui: &mut Ui) {
    egui::MenuBar::new().ui(ui, |ui| {
        for (category, actions) in groups() {
            ui.menu_button(category, |ui| {
                ui.set_width(360.0);
                let height =
                    (ui.ctx().content_rect().bottom() - ui.cursor().top() - 12.0).max(100.0);
                ui.set_max_height(height);
                // Bound long menus independently of the editor and the tool windows.
                egui::ScrollArea::vertical()
                    .max_height(height)
                    .show(ui, |ui| {
                        if category == tr("File") {
                            file_commands(app, ui);
                        }
                        for (label, action) in actions {
                            let enabled = match action {
                                Action::ShowSettings
                                | Action::ZoomIn
                                | Action::ZoomOut
                                | Action::ZoomReset
                                | Action::NewScratchFile => true,
                                Action::Save
                                | Action::CloseTab
                                | Action::NextTab
                                | Action::PreviousTab
                                | Action::RevertFromDisk
                                | Action::ShowLocalHistory
                                | Action::Find
                                | Action::Replace
                                | Action::GotoLine => app.docs.active_doc().is_some(),
                                Action::SaveAll => !app.docs.list.is_empty(),
                                Action::ExpandMacroStep if !cfg!(unix) => false,
                                _ if action.is_editing() => app.docs.active_doc().is_some(),
                                _ => app.workspace.is_some(),
                            };
                            let shortcut = app.keymap.shortcut_text(action);
                            if theme::menu_entry(
                                ui,
                                crate::search::action_icon(action),
                                label,
                                shortcut.as_deref(),
                                false,
                                enabled,
                            )
                            .clicked()
                            {
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
                                for tool in ToolWindow::ALL {
                                    if theme::menu_entry(
                                        ui,
                                        Some(tool.icon()),
                                        tool.title(),
                                        None,
                                        app.dock.is_open(tool),
                                        app.workspace.is_some(),
                                    )
                                    .clicked()
                                    {
                                        app.toggle_tool(tool);
                                        ui.close();
                                    }
                                }
                            });
                        }
                        if category == tr("Build") {
                            build_commands(app, ui);
                        }
                        if category == tr("Run")
                            && command(
                                ui,
                                tr("Edit Run Configuration…"),
                                Some(ico::SETTINGS),
                                app.workspace.is_some(),
                            )
                        {
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
                                ui.ctx()
                                    .send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                            }
                        }
                        if category == tr("Help") {
                            if command(
                                ui,
                                tr("Find Action…"),
                                Some(ico::SEARCH),
                                app.workspace.is_some(),
                            ) {
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
        app.new_project_dialog(None);
    }
    if command(
        ui,
        tr("New File…"),
        Some(ico::NEW_FILE),
        app.workspace.is_some(),
    ) {
        app.new_file_dialog();
    }
    if command(ui, tr("Open…"), Some(ico::OPEN), true) {
        app.pick_and_open();
    }
    if command(ui, tr("Open File…"), Some(ico::OPEN), true) {
        app.pick_file();
    }
    ui.menu_button(tr("Recent Projects"), |ui| {
        for path in app.settings.recent_workspaces.clone() {
            if command(ui, &path.display().to_string(), Some(ico::FOLDER), true) {
                app.open_workspace(&path);
            }
        }
    });
    if command(
        ui,
        tr("Close Project"),
        Some(ico::CLOSE),
        app.workspace.is_some(),
    ) {
        app.close_project();
    }
    ui.separator();
}

fn build_commands(app: &mut CrabCraftApp, ui: &mut Ui) {
    for (label, action, icon) in [
        (
            tr("Rebuild Project"),
            rc_dotnet::BuildAction::Rebuild,
            ico::REBUILD,
        ),
        (
            tr("Clean Project"),
            rc_dotnet::BuildAction::Clean,
            ico::DELETE,
        ),
        (
            tr("Fetch Dependencies"),
            rc_dotnet::BuildAction::Restore,
            ico::DOWNLOAD,
        ),
    ] {
        if command(ui, label, Some(icon), app.workspace.is_some()) {
            if let Some(target) = app
                .workspace
                .as_ref()
                .map(|w| w.ws.key_path().to_path_buf())
            {
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
    if command(
        ui,
        tr("Local AI assistant"),
        Some(ico::RUN_ANYTHING),
        app.workspace.is_some(),
    ) {
        app.local_ai.open = true;
    }
    ui.menu_button(tr("External Tools"), |ui| {
        for name in app
            .settings
            .external_tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect::<Vec<_>>()
        {
            if command(ui, &name, Some(ico::RUN_ANYTHING), app.workspace.is_some()) {
                app.run_external_tool(&name);
            }
        }
        if command(
            ui,
            tr("Configure External Tools…"),
            Some(ico::SETTINGS),
            true,
        ) {
            app.open_settings(crate::ui::dialogs::SettingsPage::ExternalTools);
        }
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn main_menu_contains_every_searchable_command() {
        let actions: std::collections::HashSet<_> = super::groups()
            .into_iter()
            .flat_map(|(_, entries)| entries.into_iter().map(|(_, action)| action))
            .collect();
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
}
