//! Modal dialogs and small popups: Settings, About, Go to Line, New File/Folder, Rename,
//! Delete, Recent Files and Local History.

use crate::app::CrabCraftApp;
use egui::{Align2, Context, FontId, Id, Key, Ui, vec2};
use rc_core::Settings;
use rc_core::settings::{KeymapChoice, ThemeChoice};
use rc_kit::i18n::{Lang, tr, trf};
use rc_kit::{icons, theme};
use std::path::PathBuf;
#[path = "settings_browse.rs"]
mod settings_browse;
pub use settings_browse::SettingsBrowse;
use settings_browse::ToolPath;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SettingsPage {
    Appearance,
    Editor,
    Keymap,
    Tools,
    ExternalTools,
    LiveTemplates,
    Copyright,
    Scopes,
    Inspections,
    Backup,
}

pub enum Dialog {
    RemoteDebug(crate::remote_debug::Dialog),
    RevertDisk(crate::disk_revert::Ticket),
    CloneRepository,
    CargoDependencies,
    /// Picks a running process for the debugger.
    Attach {
        filter: String,
        processes: Vec<rc_core::processes::Process>,
    },
    Settings {
        draft: Box<Settings>,
        page: SettingsPage,
        browse: SettingsBrowse,
    },
    About,
    GotoLine {
        text: String,
        focus: bool,
    },
    Bookmarks {
        list: Vec<crate::insight::Target>,
        selected: usize,
    },
    RunAnything {
        text: String,
        selected: usize,
    },
    NewFile {
        dir: PathBuf,
        name: String,
        kind: crate::item_templates::ItemKind,
        attach: bool,
        public: bool,
        error: Option<String>,
    },
    Rename {
        path: PathBuf,
        name: String,
        error: Option<String>,
    },
    /// After the new name of a file: the other changes the rename brings.
    RenameFile(Box<crate::file_rename::FileRename>),
    /// The edits of a refactoring, to review before applying them.
    RefactorPreview(Box<crate::refactor_preview::RefactorPreview>),
    /// The conflicts of a refactoring, to continue anyway or cancel.
    RefactorConflicts(Box<crate::refactor_conflicts::Review>),
    ConfirmDelete {
        path: PathBuf,
    },
    /// Invalidate Caches: what is rebuilt, and whether `cargo clean` runs too.
    InvalidateCaches {
        clean: bool,
    },
    RecentFiles {
        selected: usize,
        filter: String,
    },
    /// New Scratch File: the kinds to create, then the scratch files there are.
    Scratches {
        selected: usize,
        filter: String,
        files: Vec<PathBuf>,
    },
    /// ⌥⏎ context actions for the active editor.
    CodeActions {
        list: Vec<rc_lsp::lsp_types::CodeActionOrCommand>,
        selected: usize,
    },
    /// Generate › Derive: the traits a Rust type derives (checked) and others typed in.
    Derive {
        doc: u64,
        name: String,
        traits: Vec<(String, bool)>,
        other: String,
    },
    /// Shift+F6 on a symbol.
    RenameSymbol {
        doc: u64,
        pos: usize,
        original: String,
        name: String,
        focus: bool,
    },
    /// Confirm rolling back files to HEAD.
    Rollback {
        paths: Vec<String>,
    },
    NewBranch {
        name: String,
    },
    /// A new tag on HEAD or on a commit (hash, short hash); a message makes it annotated.
    NewTag {
        name: String,
        target: Option<(String, String)>,
        message: String,
    },
    /// Confirm deleting a local tag.
    DeleteTag {
        name: String,
    },
    /// Warn before checking out a tag (detached HEAD).
    CheckoutTag {
        name: String,
    },
    /// Confirm pushing one tag, or every tag (`None`), and pick the remote.
    PushTags {
        name: Option<String>,
        remote: String,
    },
    /// Stash the work tree's changes with an optional message.
    StashChanges {
        message: String,
        include_untracked: bool,
        keep_index: bool,
        paths: Vec<String>,
    },
    /// Apply Patch: the file typed or picked, and whether a paste from the clipboard was
    /// requested (the loaded patch and its preview live in `app.vcs.patch`).
    ApplyPatch {
        path: String,
        paste: bool,
    },
    /// A new folder for the run configuration `config`, or the new name of the folder `rename`
    /// (B-21).
    ConfigFolder {
        config: String,
        rename: Option<String>,
        folder: String,
    },
    /// Confirm dropping a stash.
    DropStash {
        stash: rc_vcs::Stash,
    },
    StashBranch {
        stash: rc_vcs::Stash,
        name: String,
    },
    /// A new worktree at `path`, on a new branch `name` or on the existing branch `existing`.
    /// The path follows the branch until the user edits it.
    AddWorktree {
        path: String,
        new_branch: bool,
        name: String,
        existing: String,
        path_edited: bool,
    },
    /// Confirm removing a worktree; `force` after git refused because of local changes.
    RemoveWorktree {
        path: PathBuf,
        force: bool,
    },
    /// Lock a worktree, with an optional reason.
    LockWorktree {
        path: PathBuf,
        reason: String,
    },
    /// Confirm pruning the worktrees whose folders are gone.
    PruneWorktrees {
        paths: Vec<PathBuf>,
    },
    /// Merge a branch or commit into the current branch (V-07).
    Merge {
        rev: String,
        fast_forward: rc_vcs::FastForward,
        squash: bool,
        no_commit: bool,
    },
    /// Rebase the current branch onto a branch or commit (V-07).
    Rebase {
        onto: String,
        autostash: bool,
    },
    /// Confirm abandoning the merge, rebase or cherry-pick in progress.
    AbortOperation,
    /// The conflicted files; the selected ones are acted on.
    Conflicts {
        selected: std::collections::BTreeSet<String>,
    },
    /// ⇧⌘E: the places the caret jumped from.
    RecentLocations {
        selected: usize,
    },
    /// ⌘F12 file structure popup.
    FileStructure {
        filter: String,
        selected: usize,
    },
    /// Right-click on a breakpoint: condition and enabled state (0-based line).
    BreakpointProperties {
        path: PathBuf,
        line: usize,
        condition: String,
        log: String,
        once: bool,
        enabled: bool,
        focus: bool,
    },
    /// Every breakpoint and its group (D-27): the selected one and the group name being typed.
    Breakpoints {
        selected: Option<(PathBuf, usize)>,
        group: String,
    },
    /// Edit an application run configuration: launch profile, arguments, environment, folder.
    WasmOptions {
        name: String,
        manifest: PathBuf,
        options: rc_core::session::RunOptions,
    },
    RunOptions {
        name: String,
        project: PathBuf,
        options: rc_core::session::RunOptions,
    },
    /// A compound run configuration: its name and which applications it starts.
    Compound {
        old: Option<String>,
        name: String,
        members: Vec<(String, bool)>,
    },
    /// ⌥F8: evaluate an expression in the selected frame.
    Evaluate {
        expression: String,
        focus: bool,
    },
    /// The earlier versions of a file kept by the IDE.
    LocalHistory(Box<LocalHistoryView>),
    /// The variables of every Cargo command and of rust-analyzer, `NAME=value` per line.
    ProjectEnv {
        text: String,
    },
    /// Share in Playground: what is uploaded, to confirm before anything is sent.
    SharePlayground(Box<crate::playground::Share>),
}

impl Dialog {
    pub fn settings(s: &Settings) -> Dialog {
        Dialog::Settings { draft: Box::new(s.clone()), page: SettingsPage::Appearance, browse: Default::default() }
    }

    /// The Merge dialog with `rev` picked and git's default options.
    pub fn merge(rev: String) -> Dialog {
        Dialog::Merge { rev, fast_forward: rc_vcs::FastForward::Allow, squash: false, no_commit: false }
    }
}

enum Outcome {
    Keep,
    Close,
}

pub fn show(app: &mut CrabCraftApp, ctx: &Context) {
    let Some(mut dialog) = app.dialog.take() else { return };
    let outcome = match &mut dialog {
        Dialog::RevertDisk(ticket) => revert_disk(app, ctx, ticket),
        Dialog::Settings { draft, page, browse } => settings(app, ctx, draft, page, browse),
        Dialog::About => about(ctx),
        Dialog::CloneRepository => {
            if crate::clone_project::show(app, ctx) {
                Outcome::Close
            } else {
                Outcome::Keep
            }
        }
        Dialog::GotoLine { text, focus } => goto_line(app, ctx, text, focus),
        Dialog::Bookmarks { list, selected } => bookmarks(app, ctx, list, selected),
        Dialog::RunAnything { text, selected } => run_anything(app, ctx, text, selected),
        Dialog::NewFile { dir, name, kind, attach, public, error } => new_file(app, ctx, dir, name, *kind, attach, public, error),
        Dialog::Rename { path, name, error } => rename(app, ctx, path, name, error),
        Dialog::RenameFile(r) => rename_file(app, ctx, r),
        Dialog::RefactorPreview(r) => refactor_preview(app, ctx, r),
        Dialog::RefactorConflicts(r) => refactor_conflicts(app, ctx, r),
        Dialog::ConfirmDelete { path } => confirm_delete(app, ctx, path),
        Dialog::InvalidateCaches { clean } => invalidate_caches(app, ctx, clean),
        Dialog::RecentFiles { selected, filter } => recent_files(app, ctx, selected, filter),
        Dialog::Scratches { selected, filter, files } => scratches(app, ctx, selected, filter, files),
        Dialog::CodeActions { list, selected } => code_actions(app, ctx, list, selected),
        Dialog::Derive { doc, name, traits, other } => derive(app, ctx, *doc, name, traits, other),
        Dialog::RenameSymbol { doc, pos, original, name, focus } => rename_symbol(app, ctx, *doc, *pos, original, name, focus),
        Dialog::FileStructure { filter, selected } => file_structure(app, ctx, filter, selected),
        Dialog::Rollback { paths } => rollback(app, ctx, paths),
        Dialog::NewBranch { name } => new_branch(app, ctx, name),
        Dialog::NewTag { name, target, message } => new_tag(app, ctx, name, target.as_ref(), message),
        Dialog::DeleteTag { name } => delete_tag(app, ctx, name),
        Dialog::CheckoutTag { name } => checkout_tag(app, ctx, name),
        Dialog::PushTags { name, remote } => push_tags(app, ctx, name.as_deref(), remote),
        Dialog::StashChanges { message, include_untracked, keep_index, paths } => stash_changes(app, ctx, message, include_untracked, keep_index, paths),
        Dialog::StashBranch { stash, name } => stash_branch(app, ctx, stash, name),
        Dialog::CargoDependencies => {
            if crate::cargo_dependencies::show(app, ctx) {
                Outcome::Close
            } else {
                Outcome::Keep
            }
        }
        Dialog::ApplyPatch { path, paste } => apply_patch(app, ctx, path, paste),
        Dialog::DropStash { stash } => drop_stash(app, ctx, stash),
        Dialog::AddWorktree { path, new_branch, name, existing, path_edited } => add_worktree(app, ctx, path, new_branch, name, existing, path_edited),
        Dialog::RemoveWorktree { path, force } => remove_worktree(app, ctx, path, *force),
        Dialog::LockWorktree { path, reason } => lock_worktree(app, ctx, path, reason),
        Dialog::PruneWorktrees { paths } => prune_worktrees(app, ctx, paths),
        Dialog::Merge { rev, fast_forward, squash, no_commit } => merge_dialog(app, ctx, rev, fast_forward, squash, no_commit),
        Dialog::Rebase { onto, autostash } => rebase_dialog(app, ctx, onto, autostash),
        Dialog::AbortOperation => abort_operation(app, ctx),
        Dialog::Conflicts { selected } => conflicts_dialog(app, ctx, selected),
        Dialog::RecentLocations { selected } => recent_locations(app, ctx, selected),
        Dialog::BreakpointProperties { path, line, condition, log, once, enabled, focus } => {
            breakpoint_properties(app, ctx, path, *line, condition, log, once, enabled, focus)
        }
        Dialog::Breakpoints { selected, group } => breakpoints(app, ctx, selected, group),
        Dialog::Evaluate { expression, focus } => evaluate(app, ctx, expression, focus),
        Dialog::Attach { filter, processes } => attach(app, ctx, filter, processes),
        Dialog::RemoteDebug(draft) => {
            if crate::remote_debug::show(app, ctx, draft) {
                Outcome::Close
            } else {
                Outcome::Keep
            }
        }
        Dialog::Compound { old, name, members } => compound(app, ctx, old.as_deref(), name, members),
        Dialog::ConfigFolder { config, rename, folder } => config_folder(app, ctx, config, rename.as_deref(), folder),
        Dialog::WasmOptions { name, manifest, options } => wasm_options(app, ctx, name, manifest, options),
        Dialog::RunOptions { name, project, options } => run_options(app, ctx, name, project, options),
        Dialog::LocalHistory(v) => local_history(app, ctx, v),
        Dialog::ProjectEnv { text } => project_env(app, ctx, text),
        Dialog::SharePlayground(share) => share_playground(app, ctx, share),
    };
    if matches!(outcome, Outcome::Keep) && app.dialog.is_none() {
        app.dialog = Some(dialog);
    } else if matches!(outcome, Outcome::Close) {
        app.focus_editor = true;
    }
}

fn modal<R>(ctx: &Context, id: &str, width: f32, add: impl FnOnce(&mut Ui) -> R) -> (R, bool) {
    let resp = egui::Modal::new(Id::unique(id)).show(ctx, |ui| {
        ui.set_width(width);
        add(ui)
    });
    let escape = resp.should_close();
    (resp.inner, escape)
}

fn title(ui: &mut Ui, text: &str) {
    ui.label(egui::RichText::new(text).font(theme::bold(15.0)));
    ui.add_space(10.0);
}

fn buttons(ui: &mut Ui, ok: &str, enabled: bool) -> (bool, bool) {
    ui.add_space(14.0);
    let mut res = (false, false);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        res.0 = ui.add_enabled_ui(enabled, |ui| theme::primary_button(ui, ok)).inner.clicked();
        res.1 = theme::secondary_button(ui, tr("Cancel")).clicked();
    });
    res
}

/// The pages of Settings, in the order of its menu.
fn settings_pages() -> [(SettingsPage, &'static str); 10] {
    [
        (SettingsPage::Appearance, tr("Appearance")),
        (SettingsPage::Editor, tr("Editor")),
        (SettingsPage::Keymap, tr("Keymap")),
        (SettingsPage::Tools, tr("Tools")),
        (SettingsPage::ExternalTools, tr("External Tools")),
        (SettingsPage::LiveTemplates, tr("Live Templates")),
        (SettingsPage::Copyright, tr("Copyright")),
        (SettingsPage::Scopes, tr("Scopes")),
        (SettingsPage::Inspections, tr("Inspections")),
        (SettingsPage::Backup, tr("Preferences backup")),
    ]
}

/// Width of the Settings menu and of its pages: every page has the same size.
const SETTINGS_NAV_W: f32 = 200.0;
const SETTINGS_PAGE_W: f32 = 640.0;

#[derive(Clone, Copy, Debug)]
struct SettingsLayout {
    width: f32,
    nav: f32,
    page: f32,
    height: f32,
    compact: bool,
}
impl SettingsLayout {
    fn for_viewport(viewport: egui::Vec2) -> Self {
        let width = (viewport.x - 32.0).max(1.0).min(SETTINGS_NAV_W + 29.0 + SETTINGS_PAGE_W);
        let compact = width < 620.0;
        let nav = if compact { 0.0 } else { (width * 0.24).min(SETTINGS_NAV_W) };
        let page = if compact { width } else { (width - nav - 29.0).max(1.0) };
        let chrome = if compact { 156.0 } else { 120.0 };
        let height = (viewport.y * 0.7).min(620.0).min((viewport.y - chrome).max(1.0));
        Self { width, nav, page, height, compact }
    }
}

fn settings(app: &mut CrabCraftApp, ctx: &Context, draft: &mut Settings, page: &mut SettingsPage, browse: &mut SettingsBrowse) -> Outcome {
    browse.poll(draft);
    let p = rc_kit::theme::palette(ctx.global_style().visuals.dark_mode);
    // Layout depends only on the viewport, never on the selected page or its content.
    let layout = SettingsLayout::for_viewport(ctx.content_rect().size());
    let page_h = layout.height;
    let ((ok, cancel), esc) = modal(ctx, "settings", layout.width, |ui| {
        title(ui, tr("Settings"));
        if layout.compact {
            egui::ComboBox::from_id_salt("settings_category")
                .selected_text(settings_pages().into_iter().find(|(candidate, _)| candidate == page).map_or("", |(_, label)| label))
                .width(layout.width - 8.0)
                .show_ui(ui, |ui| {
                    for (candidate, label) in settings_pages() {
                        ui.selectable_value(page, candidate, label);
                    }
                });
            ui.add_space(8.0);
        }
        ui.horizontal_top(|ui| {
            if !layout.compact {
                egui::ScrollArea::vertical()
                    .id_salt("settings_navigation")
                    .max_height(page_h)
                    .min_scrolled_height(page_h)
                    .auto_shrink([false, false])
                    .max_width(layout.nav)
                    .show(ui, |ui| {
                        ui.vertical(|ui| {
                            ui.set_width(layout.nav);
                            ui.set_height(page_h);
                            for (pg, label) in settings_pages() {
                                let (r, resp) = ui.allocate_exact_size(vec2(layout.nav, 28.0), egui::Sense::click());
                                if *page == pg {
                                    ui.painter().rect_filled(r, 6.0, p.selection);
                                } else if resp.hovered() {
                                    ui.painter().rect_filled(r, 6.0, p.row_hover);
                                }
                                if resp.has_focus() {
                                    ui.painter().rect_stroke(r.shrink(1.0), 6.0, egui::Stroke::new(1.0, p.accent), egui::StrokeKind::Inside);
                                }
                                ui.painter().text(r.left_center() + vec2(10.0, 0.0), Align2::LEFT_CENTER, label, FontId::proportional(13.0), p.text);
                                resp.widget_info(|| {
                                    let mut info = egui::WidgetInfo::labeled(egui::Role::Button, true, label);
                                    info.selected = Some(*page == pg);
                                    info
                                });

                                if resp.clicked() {
                                    *page = pg;
                                }
                            }
                        })
                    });
                ui.add_space(12.0);
                let (line, _) = ui.allocate_exact_size(vec2(1.0, page_h), egui::Sense::hover());
                ui.painter().rect_filled(line, 0.0, p.control_border);
                ui.add_space(15.0);
            }
            // Reserve the floating scrollbar's full width as well as its allocated gutter.
            let content_width = (layout.page - ui.spacing().scroll.allocated_width() - ui.spacing().scroll.bar_width - 12.0).max(40.0);
            egui::ScrollArea::new([*page != SettingsPage::Tools, true])
                .max_height(page_h)
                .min_scrolled_height(page_h)
                .id_salt("settings_fixed_page")
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
                .auto_shrink([false, false])
                .max_width(layout.page)
                .show(ui, |ui| {
                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                        ui.set_width(content_width);
                        ui.set_height(page_h);
                        let name = settings_pages().into_iter().find(|(pg, _)| pg == page).map_or("", |(_, l)| l);
                        ui.label(egui::RichText::new(name).font(theme::bold(14.0)));
                        ui.add_space(10.0);
                        // Pages that lay out their own lists, sized to the page.
                        match page {
                            SettingsPage::Tools => return tools_page(ui, app, draft, browse),
                            SettingsPage::Backup => return app.settings_backup.ui(ui, draft, app.waker.clone()),
                            SettingsPage::LiveTemplates => return live_templates_page(ui, &mut draft.live_templates, p.text_muted),
                            SettingsPage::Copyright => {
                                egui::ScrollArea::vertical()
                                    .id_salt("copyright_page")
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| copyright_page(ui, app, p.text_muted, p.error));
                                return;
                            }
                            SettingsPage::Scopes => return scopes_page(ui, app, p.text_muted, p.error),
                            SettingsPage::Inspections => return inspections_page(ui, app, draft, p.text_muted),
                            SettingsPage::Keymap => return keymap_page(ui, draft),
                            _ => {}
                        }
                        egui::ScrollArea::vertical().id_salt(("settings_page", format!("{page:?}"))).auto_shrink([false, false]).show(ui, |ui| {
                            egui::Grid::new("settings_grid").num_columns(2).spacing(vec2(16.0, 10.0)).show(ui, |ui| match page {
                                SettingsPage::Appearance => {
                                    ui.label(tr("Theme"));
                                    egui::ComboBox::from_id_salt("theme")
                                        .selected_text(match draft.theme {
                                            ThemeChoice::Light => "Islands Light",
                                            ThemeChoice::Dark => "Islands Dark",
                                            ThemeChoice::System => tr("Sync with OS"),
                                        })
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(&mut draft.theme, ThemeChoice::Light, "Islands Light");
                                            ui.selectable_value(&mut draft.theme, ThemeChoice::Dark, "Islands Dark");
                                            ui.selectable_value(&mut draft.theme, ThemeChoice::System, tr("Sync with OS"));
                                        });
                                    ui.end_row();
                                    ui.label(tr("Language"));
                                    let current = draft.language.as_deref().map(Lang::from_locale);
                                    egui::ComboBox::from_id_salt("lang").selected_text(current.map_or(tr("System language"), |l| l.native_name())).show_ui(
                                        ui,
                                        |ui| {
                                            if ui.selectable_label(current.is_none(), tr("System language")).clicked() {
                                                draft.language = None;
                                            }
                                            for l in Lang::ALL {
                                                if ui.selectable_label(current == Some(l), l.native_name()).clicked() {
                                                    draft.language = Some(lang_code(l).to_string());
                                                }
                                            }
                                        },
                                    );
                                    ui.end_row();
                                    ui.label(tr("Zoom"));
                                    theme::slider(
                                        ui,
                                        egui::Slider::new(&mut draft.ui_zoom, 0.5..=3.0).step_by(0.1).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                                    );
                                    ui.end_row();
                                }
                                SettingsPage::Editor => {
                                    ui.label(tr("Rust"));
                                    ui.checkbox(&mut draft.format_on_save, tr("Reformat with rustfmt on save"));
                                    ui.end_row();
                                    ui.label("");
                                    ui.checkbox(&mut draft.use_nextest, tr("Run tests with cargo-nextest"))
                                        .on_hover_text(tr("Needs cargo-nextest installed. Doctests do not run with it."));
                                    ui.end_row();
                                    ui.label("");
                                    ui.checkbox(&mut draft.check_crates_io, tr("Show newer versions of dependencies from crates.io"))
                                        .on_hover_text(tr("Only crate names are sent to crates.io."));
                                    ui.end_row();
                                    ui.label(tr("Font size"));
                                    ui.add(egui::DragValue::new(&mut draft.editor_font_size).range(8.0..=32.0).speed(0.5).suffix(" pt"));
                                    ui.end_row();
                                    ui.label(tr("Line height"));
                                    ui.add(egui::DragValue::new(&mut draft.editor_line_spacing).range(1.0..=2.0).speed(0.05));
                                    ui.end_row();
                                    ui.label(tr("Hard wrap guide"));
                                    ui.add(egui::DragValue::new(&mut draft.right_margin).range(0..=400).suffix(tr(" columns")));
                                    ui.end_row();
                                    ui.label("");
                                    ui.checkbox(&mut draft.show_inlay_hints, tr("Show inlay hints"));
                                    ui.end_row();
                                    ui.label("");
                                    ui.checkbox(&mut draft.show_indent_guides, tr("Show indent guides"));
                                    ui.end_row();
                                    ui.label("");
                                    ui.checkbox(&mut draft.sticky_lines, tr("Show sticky lines while scrolling"));
                                    ui.end_row();
                                    ui.label("");
                                    ui.checkbox(&mut draft.scrollbar_lens, tr("Show a code preview when hovering the scrollbar"));
                                    ui.end_row();
                                    ui.label("");
                                    ui.checkbox(&mut draft.autosave, tr("Save files when switching to another application"));
                                    ui.end_row();
                                    ui.label("");
                                    ui.horizontal(|ui| {
                                        ui.checkbox(&mut draft.autosave_idle, tr("Save files automatically after a pause of"));
                                        ui.add_enabled(
                                            draft.autosave_idle,
                                            egui::DragValue::new(&mut draft.autosave_delay_secs).range(1..=600).suffix(tr(" s")),
                                        );
                                    });
                                    ui.end_row();
                                    ui.label("");
                                    ui.checkbox(&mut draft.single_click_open, tr("Open files with a single click in the Explorer"));
                                    ui.end_row();
                                    ui.label("");
                                    ui.checkbox(&mut draft.tabs_multirow, tr("Show tabs in multiple rows"));
                                    ui.end_row();
                                    ui.label("");
                                    ui.checkbox(&mut draft.tab_colors, tr("Color tabs by kind of file"));
                                    ui.end_row();
                                    ui.label("");
                                    ui.checkbox(&mut draft.imports_on_paste, tr("Add the imports pasted Rust code needs (when only one fits)"));
                                    ui.end_row();
                                    ui.label(tr("Never import"));
                                    ui.vertical(|ui| {
                                        let mut text = draft.autoimport_exclude.join("\n");
                                        if ui
                                            .add(
                                                egui::TextEdit::multiline(&mut text)
                                                    .hint_text("std::sync::atomic::Ordering")
                                                    .font(egui::TextStyle::Monospace)
                                                    .desired_rows(3)
                                                    .desired_width(300.0),
                                            )
                                            .changed()
                                        {
                                            draft.autoimport_exclude = text.lines().map(str::to_string).collect();
                                        }
                                        ui.label(
                                            egui::RichText::new(tr("Items or modules completion never offers to import, one path per line"))
                                                .size(12.0)
                                                .color(p.text_muted),
                                        );
                                    });
                                    ui.end_row();
                                }
                                SettingsPage::Tools => {}
                                SettingsPage::ExternalTools => external_tools_rows(ui, &mut draft.external_tools, p.text_muted),
                                SettingsPage::Keymap
                                | SettingsPage::LiveTemplates
                                | SettingsPage::Copyright
                                | SettingsPage::Scopes
                                | SettingsPage::Inspections
                                | SettingsPage::Backup => {}
                            });
                        });
                    })
                });
        });
        buttons(ui, tr("OK"), true)
    });
    if ok {
        cancel_shortcut_recording(ctx);
        app.settings_backup.cancel();
        app.apply_settings(ctx, draft.clone());
        if let Some(profile) = app.copyright.draft.take() {
            app.set_copyright_profile(profile);
        }
        if let Some(draft) = app.explorer.scopes.draft.take() {
            app.set_scopes(draft);
        }
        if let Some((use_project, profile)) = app.inspections.draft.take() {
            app.set_project_inspections(use_project, profile);
        }
        return Outcome::Close;
    }
    if cancel || esc {
        cancel_shortcut_recording(ctx);
        app.settings_backup.cancel();
        app.copyright.draft = None;
        app.explorer.scopes.draft = None;
        app.inspections.draft = None;
        Outcome::Close
    } else {
        Outcome::Keep
    }
}

/// Settings › Inspections (I-18): which profile the project uses (the Default one, kept in the
/// settings, or its own, kept with the project), and in it each inspection turned on or off and
/// its level. Diagnostic codes of rust-analyzer, the compiler and Clippy can be added.
fn inspections_page(ui: &mut Ui, app: &mut CrabCraftApp, draft: &mut Settings, muted: egui::Color32) {
    use crate::inspection_profile::{self as ip, Group};
    use rc_core::inspections::{Level, valid_id};
    let project_open = app.workspace.is_some();
    let (mut use_project, mut project) = app.inspections.draft.take().unwrap_or_else(|| (app.inspections.use_project, app.inspections.project.clone()));
    if project_open {
        ui.horizontal(|ui| {
            ui.label(rc_kit::i18n::tr_ctx("inspections", "Profile"));
            let name = |p: bool| if p { tr("Project (saved with this project)") } else { tr("Default (every project)") };
            egui::ComboBox::from_id_salt("inspection_profile").selected_text(name(use_project)).width(260.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut use_project, false, name(false));
                ui.selectable_value(&mut use_project, true, name(true));
            });
            if use_project && ui.button(tr("Copy from Default")).on_hover_text(tr("Replace the project's levels with those of the Default profile")).clicked() {
                project = draft.inspection_profile.clone();
            }
        });
    } else {
        ui.label(
            egui::RichText::new(tr("Open a project to give it a profile of its own. The Default profile applies to every project.")).size(12.0).color(muted),
        );
    }
    ui.label(egui::RichText::new(tr("Weak warnings get a thin underline; infos are not highlighted but keep their quick fixes. Neither is listed in Problems or marked on the scroll bar.")).size(12.0).color(muted));
    ui.add_space(6.0);
    let editing = if project_open && use_project { &mut project } else { &mut draft.inspection_profile };
    let mut remove = None;
    egui::ScrollArea::vertical().id_salt("inspections_page").max_height((ui.available_height() - 44.0).max(120.0)).auto_shrink([false, true]).show(ui, |ui| {
        // One grid for every group, so the level column lines up.
        egui::Grid::new("inspections_grid").num_columns(3).spacing(vec2(10.0, 4.0)).show(ui, |ui| {
            for group in [Group::CargoToml, Group::CratesIo, Group::Rust] {
                ui.label(egui::RichText::new(group.title()).font(theme::bold(13.0)));
                ui.end_row();
                let mut ids: Vec<String> = ip::KNOWN.iter().filter(|(_, g)| *g == group).map(|(id, _)| id.to_string()).collect();
                let custom: Vec<String> = if group == Group::Rust {
                    editing.levels.keys().filter(|k| !ip::KNOWN.iter().any(|(id, _)| id == k)).cloned().collect()
                } else {
                    Vec::new()
                };
                ids.extend(custom.iter().cloned());
                for id in &ids {
                    let level = editing.level(id).filter(|_| editing.levels.contains_key(id.as_str()));
                    let mut on = level != Some(Level::Off);
                    if ui.checkbox(&mut on, ip::title(id)).on_hover_text(id.as_str()).changed() {
                        editing.set(id, if on { None } else { Some(Level::Off) });
                    }
                    ui.add_enabled_ui(on, |ui| {
                        egui::ComboBox::from_id_salt(("inspection_level", id.as_str()))
                            .selected_text(ip::level_name(level.filter(|l| *l != Level::Off)))
                            .width(130.0)
                            .show_ui(ui, |ui| {
                                for choice in [None, Some(Level::Error), Some(Level::Warning), Some(Level::WeakWarning), Some(Level::Info)] {
                                    if ui.selectable_label(level == choice, ip::level_name(choice)).clicked() {
                                        editing.set(id, choice);
                                    }
                                }
                            });
                    });
                    if custom.contains(id) {
                        if theme::icon_button(ui, rc_kit::ico::REMOVE, true, tr("Remove")).clicked() {
                            remove = Some(id.clone());
                        }
                    } else {
                        ui.label("");
                    }
                    ui.end_row();
                }
            }
        });
    });
    if let Some(id) = remove {
        editing.set(&id, None);
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let code = &mut app.inspections.new_code;
        ui.add(
            egui::TextEdit::singleline(code)
                .hint_text(tr("Diagnostic code, such as clippy::needless_return"))
                .font(egui::TextStyle::Monospace)
                .desired_width(300.0),
        );
        let id = code.trim().to_string();
        let can_add = valid_id(&id) && !editing.levels.contains_key(&id);
        if ui
            .add_enabled(can_add, egui::Button::new(tr("Add Code")))
            .on_hover_text(tr("A code of rust-analyzer, the compiler or Clippy; prefix::* covers a whole group"))
            .clicked()
        {
            editing.set(&id, Some(Level::Warning));
            code.clear();
        }
    });
    app.inspections.draft = Some((use_project, project));
}

/// Settings › Scopes: the project's scopes (named globs) and the colours of their files. Saved
/// with the project (its session), so it needs one open.
fn scopes_page(ui: &mut Ui, app: &mut CrabCraftApp, muted: egui::Color32, error: egui::Color32) {
    use crate::scopes;
    use rc_core::session::{FileColor, NamedScope, hex_color, parse_hex_color};
    if app.workspace.is_none() {
        ui.label(egui::RichText::new(tr("Open a project to define its scopes and file colors.")).color(muted));
        return;
    }
    let mut draft = app.explorer.scopes.draft.take().unwrap_or_else(|| app.explorer.scopes.draft());
    egui::ScrollArea::vertical().id_salt("scopes_page").max_height(ui.available_height()).auto_shrink([false, true]).show(ui, |ui| {
        ui.label(egui::RichText::new(tr("Scopes")).font(theme::bold(13.0)));
        ui.label(
            egui::RichText::new(tr(
                "Globs separated by commas; ! leaves files out. A glob with / matches the path from the project folder, one without it the file name.",
            ))
            .size(12.0)
            .color(muted),
        );
        ui.add_space(6.0);
        egui::Grid::new("builtin_scopes").num_columns(2).spacing(vec2(12.0, 6.0)).show(ui, |ui| {
            for (name, what) in [
                (tr("Project Files"), tr("Every file of the project except the excluded folders").to_string()),
                (tr("Tests"), scopes::TESTS_PATTERN.to_string()),
                (tr("Changed Files"), tr("Files git reports as changed").to_string()),
            ] {
                ui.label(name);
                ui.label(egui::RichText::new(what).color(muted));
                ui.end_row();
            }
        });
        ui.add_space(6.0);
        let mut remove = None;
        for i in 0..draft.user.len() {
            ui.horizontal(|ui| {
                let old = draft.user[i].name.clone();
                ui.add(egui::TextEdit::singleline(&mut draft.user[i].name).hint_text(tr("Name")).desired_width(130.0));
                ui.add(
                    egui::TextEdit::singleline(&mut draft.user[i].pattern)
                        .hint_text("src/**/*.rs, !target/**")
                        .font(egui::TextStyle::Monospace)
                        .desired_width(290.0),
                );
                if theme::icon_button(ui, rc_kit::ico::REMOVE, true, tr("Remove")).clicked() {
                    remove = Some(i);
                }
                // A renamed scope keeps its colour.
                let new = draft.user[i].name.trim().to_string();
                if new != old.trim() {
                    for c in draft.colors.iter_mut().filter(|c| c.scope == old.trim()) {
                        c.scope = new.clone();
                    }
                }
            });
            if let Some(problem) = scopes::draft_problem(&draft, i) {
                ui.label(egui::RichText::new(problem).size(12.0).color(error));
            }
        }
        if let Some(i) = remove {
            let gone = draft.user.remove(i).name;
            draft.colors.retain(|c| c.scope != gone.trim());
        }
        if ui.button(tr("Add Scope")).clicked() {
            draft.user.push(NamedScope { name: format!("{} {}", tr("Scope"), draft.user.len() + 1), pattern: String::new() });
        }
        ui.add_space(14.0);
        ui.label(egui::RichText::new(tr("File Colors")).font(theme::bold(13.0)));
        ui.label(
            egui::RichText::new(tr("Files of a scope get its color in the Explorer and on editor tabs; the first scope that holds a file wins."))
                .size(12.0)
                .color(muted),
        );
        ui.add_space(6.0);
        let mut choices: Vec<(String, String)> =
            [scopes::PROJECT, scopes::TESTS, scopes::CHANGED].iter().map(|k| (k.to_string(), app.explorer.scopes.label(k))).collect();
        choices.extend(draft.user.iter().map(|s| s.name.trim().to_string()).filter(|n| !n.is_empty()).map(|n| (n.clone(), n)));
        let mut remove = None;
        let mut swap = None;
        let count = draft.colors.len();
        for (i, c) in draft.colors.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                let label = choices.iter().find(|(k, _)| *k == c.scope).map_or_else(|| c.scope.clone(), |(_, n)| n.clone());
                egui::ComboBox::from_id_salt(("file_color_scope", i)).selected_text(label).width(180.0).show_ui(ui, |ui| {
                    for (key, name) in &choices {
                        if ui.selectable_label(c.scope == *key, name).clicked() {
                            c.scope = key.clone();
                        }
                    }
                });
                let mut rgb = parse_hex_color(&c.color).unwrap_or(scopes::DEFAULT_COLOR);
                if ui.color_edit_button_srgb(&mut rgb).changed() {
                    c.color = hex_color(rgb);
                }
                if theme::icon_button(ui, rc_kit::ico::CHEVRON_UP, i > 0, tr("Move Up")).clicked() {
                    swap = Some(i - 1);
                }
                if theme::icon_button(ui, rc_kit::ico::CHEVRON_DOWN, i + 1 < count, tr("Move Down")).clicked() {
                    swap = Some(i);
                }
                if theme::icon_button(ui, rc_kit::ico::REMOVE, true, tr("Remove")).clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = swap
            && i + 1 < draft.colors.len()
        {
            draft.colors.swap(i, i + 1);
        }
        if let Some(i) = remove {
            draft.colors.remove(i);
        }
        let free = choices.iter().find(|(k, _)| !draft.colors.iter().any(|c| c.scope == *k)).map(|(k, _)| k.clone());
        if ui.add_enabled(free.is_some(), egui::Button::new(tr("Add Color"))).clicked()
            && let Some(scope) = free
        {
            draft.colors.push(FileColor { scope, color: hex_color(scopes::DEFAULT_COLOR) });
        }
    });
    app.explorer.scopes.draft = Some(draft);
}

/// Settings › Copyright: the header of the project's files, with a preview. Saved with the
/// project (its session), so it needs one open.
fn copyright_page(ui: &mut Ui, app: &mut CrabCraftApp, muted: egui::Color32, error: egui::Color32) {
    use crate::copyright;
    if app.workspace.is_none() {
        ui.label(egui::RichText::new(tr("Open a project to set the copyright header of its files.")).color(muted));
        return;
    }
    let mut draft = app.copyright.draft.take().unwrap_or_else(|| app.copyright.profile.clone());
    ui.add(
        egui::Label::new(
            egui::RichText::new(tr("Saved with this project. Variables: $year, $file, $project and $author (the user.name of Git).")).color(muted),
        )
        .wrap(),
    );
    ui.add_space(8.0);
    egui::Grid::new("copyright_grid").num_columns(2).spacing(vec2(16.0, 10.0)).show(ui, |ui| {
        ui.label(tr("Template"));
        ui.add(egui::TextEdit::multiline(&mut draft.template).font(egui::TextStyle::Monospace).desired_rows(4).desired_width(340.0));
        ui.end_row();
        ui.label(tr("SPDX license"));
        ui.vertical(|ui| {
            ui.add(egui::TextEdit::singleline(&mut draft.spdx).hint_text("MIT OR Apache-2.0").font(egui::TextStyle::Monospace).desired_width(340.0));
            if !draft.spdx.trim().is_empty() && !copyright::valid_spdx(&draft.spdx) {
                ui.colored_label(error, tr("Not an SPDX license expression: it is left out"));
            }
        });
        ui.end_row();
        ui.label("");
        ui.checkbox(&mut draft.on_new_files, tr("Add the header to new files"));
        ui.end_row();
        ui.label(tr("Author"));
        let author = match (&app.copyright.author, app.copyright.loading()) {
            (Some(name), _) => name.clone(),
            (None, true) => tr("Reading Git…").to_string(),
            (None, false) => tr("Git has no user.name for this project").to_string(),
        };
        ui.label(egui::RichText::new(author).color(muted));
        ui.end_row();
    });
    ui.add_space(8.0);
    ui.label(tr("Preview"));
    let project = app.copyright.project_name();
    let vars = copyright::Vars { year: copyright::current_year(), file: "main.rs", project: &project, author: app.copyright.author.as_deref().unwrap_or("") };
    let preview = copyright::comment(&copyright::expand(&draft, &vars), rc_syntax::Lang::Rust).unwrap_or_default();
    egui::Frame::new().fill(ui.visuals().extreme_bg_color).corner_radius(6.0).inner_margin(8.0).show(ui, |ui| {
        ui.set_width(480.0);
        ui.add(egui::Label::new(egui::RichText::new(preview.trim_end()).monospace()).wrap());
    });
    ui.add_space(6.0);
    if ui.button(tr("Restore the Default Template")).clicked() {
        draft.template = rc_core::session::CopyrightProfile::DEFAULT_TEMPLATE.to_string();
    }
    app.copyright.draft = Some(draft);
}

/// Settings › Keymap: each action of Find Action with its shortcut, which can be recorded
/// again (the next key pressed with its modifiers) or removed.
/// Settings › Keymap: the keymap, then every action with its shortcut, filling the page.
pub(crate) fn cancel_shortcut_recording(ctx: &Context) {
    ctx.data_mut(|data| {
        data.remove::<String>(Id::unique("recording_shortcut"));
        data.remove::<rc_editor::keymap::Chord>(Id::unique("recording_shortcut_first"));
        data.remove::<bool>(Id::unique("recording_shortcut_error"));
    });
}

fn store_recorded_shortcut(draft: &mut Settings, id: &str, shortcut: String) {
    let sequence = rc_editor::keymap::parse_sequence(&shortcut);
    let single = rc_editor::keymap::parse_chord(&shortcut);
    for (other, value) in &mut draft.shortcuts {
        if other == id || value.is_empty() {
            continue;
        }
        let existing_sequence = rc_editor::keymap::parse_sequence(value);
        let existing_single = rc_editor::keymap::parse_chord(value);
        if value == &shortcut
            || sequence.is_some_and(|(first, _)| existing_single == Some(first))
            || single.is_some_and(|chord| existing_sequence.is_some_and(|(first, _)| first == chord))
        {
            value.clear();
        }
    }
    draft.shortcuts.insert(id.to_string(), shortcut);
}

fn keymap_page(ui: &mut Ui, draft: &mut Settings) {
    ui.horizontal(|ui| {
        ui.label(tr("Keymap"));
        let name = |k: KeymapChoice| match k {
            KeymapChoice::Rider => {
                if cfg!(target_os = "macos") {
                    "Rider (macOS)"
                } else {
                    "Rider"
                }
            }
            KeymapChoice::RustRover => {
                if cfg!(target_os = "macos") {
                    tr("RustRover (macOS)")
                } else {
                    tr("RustRover (Windows/Linux)")
                }
            }
            KeymapChoice::VisualStudio => "Visual Studio",
            KeymapChoice::VsCode => {
                if cfg!(target_os = "macos") {
                    "VS Code (macOS)"
                } else {
                    "VS Code"
                }
            }
        };
        egui::ComboBox::from_id_salt("keymap").selected_text(name(draft.keymap)).width(200.0).show_ui(ui, |ui| {
            for k in [KeymapChoice::Rider, KeymapChoice::RustRover, KeymapChoice::VisualStudio, KeymapChoice::VsCode] {
                ui.selectable_value(&mut draft.keymap, k, name(k));
            }
        });
    });
    ui.add_space(8.0);
    let keymap = crate::app::build_keymap(draft);
    let recording_id = egui::Id::unique("recording_shortcut");
    let first_id = egui::Id::unique("recording_shortcut_first");
    let mode_id = egui::Id::unique("recording_shortcut_sequence");
    let error_id = egui::Id::unique("recording_shortcut_error");
    let filter_id = egui::Id::unique("shortcut_filter");
    let mut two_step = ui.ctx().data(|data| data.get_temp::<bool>(mode_id)).unwrap_or(false);
    if ui.checkbox(&mut two_step, tr("Record two-step shortcuts")).changed() {
        ui.ctx().data_mut(|data| {
            data.insert_temp(mode_id, two_step);
            data.remove::<String>(recording_id);
            data.remove::<rc_editor::keymap::Chord>(first_id);
            data.remove::<bool>(error_id);
        });
    }
    let recording: Option<String> = ui.ctx().data(|data| data.get_temp(recording_id));
    if let Some(id) = &recording {
        let pressed: Vec<_> = ui.input(|input| {
            input
                .events
                .iter()
                .filter_map(|event| match event {
                    egui::Event::Key { key, pressed: true, modifiers, repeat: false, .. } => Some((*key, *modifiers)),
                    _ => None,
                })
                .take(2)
                .collect()
        });
        for (key, mods) in &pressed {
            if *key == Key::Escape {
                ui.ctx().data_mut(|data| {
                    data.remove::<String>(recording_id);
                    data.remove::<rc_editor::keymap::Chord>(first_id);
                });
                break;
            }
            let chord = rc_editor::keymap::Chord { key: *key, mods: egui::Modifiers { command: mods.ctrl || mods.mac_cmd, ..*mods } };
            if two_step {
                if let Some(first) = ui.ctx().data(|data| data.get_temp::<rc_editor::keymap::Chord>(first_id)) {
                    store_recorded_shortcut(draft, id, format!("{} {}", rc_editor::keymap::chord_id(first), rc_editor::keymap::chord_id(chord)));
                    ui.ctx().data_mut(|data| {
                        data.remove::<String>(recording_id);
                        data.remove::<rc_editor::keymap::Chord>(first_id);
                    });
                    break;
                } else if mods.ctrl || mods.mac_cmd {
                    ui.ctx().data_mut(|data| data.insert_temp(first_id, chord));
                } else {
                    ui.ctx().data_mut(|data| {
                        data.insert_temp(error_id, true);
                        data.remove::<String>(recording_id);
                    });
                    break;
                }
            } else {
                store_recorded_shortcut(draft, id, rc_editor::keymap::chord_id(chord));
                ui.ctx().data_mut(|data| data.remove::<String>(recording_id));
                break;
            }
        }
        if !pressed.is_empty() {
            ui.input_mut(|input| {
                input.events.retain(|event| !matches!(event, egui::Event::Key { pressed: true, .. } | egui::Event::Text(_) | egui::Event::Paste(_)))
            });
        }
    }
    if ui.ctx().data(|data| data.get_temp::<bool>(error_id)).unwrap_or(false) {
        ui.label(tr("The first shortcut must include Ctrl or Command."));
    }
    let mut filter: String = ui.ctx().data(|d| d.get_temp(filter_id)).unwrap_or_default();
    if ui.add(egui::TextEdit::singleline(&mut filter).hint_text(tr("Filter actions")).desired_width(f32::INFINITY)).changed() {
        ui.ctx().data_mut(|d| d.insert_temp(filter_id, filter.clone()));
    }
    ui.add_space(6.0);
    let needle = filter.trim().to_lowercase();
    let mut items = crate::search::action_items();
    items.retain(|(label, _)| needle.is_empty() || label.to_lowercase().contains(&needle));
    items.sort_by_key(|(label, _)| label.to_lowercase());
    let p = rc_kit::pal(ui);
    // Columns: the action takes what the shortcut and the buttons leave.
    const KEYS_W: f32 = 150.0;
    const BUTTONS_W: f32 = 190.0;
    let name_w = (ui.available_width() - KEYS_W - BUTTONS_W - 16.0).max(160.0);
    egui::ScrollArea::vertical().id_salt("shortcuts").auto_shrink([false, false]).show(ui, |ui| {
        if items.is_empty() {
            ui.label(egui::RichText::new(tr("No action matches the filter")).color(p.text_muted));
        }
        egui::Grid::new("shortcuts_grid").num_columns(3).striped(true).min_row_height(26.0).spacing(vec2(8.0, 2.0)).show(ui, |ui| {
            for (label, action) in items {
                let id = format!("{action:?}");
                let left = egui::Layout::left_to_right(egui::Align::Center);
                ui.allocate_ui_with_layout(vec2(name_w, 22.0), left, |ui| {
                    ui.set_width(name_w);
                    ui.add(egui::Label::new(label).truncate()).on_hover_text(label);
                });
                let text = if recording.as_deref() == Some(id.as_str()) {
                    if two_step && ui.ctx().data(|data| data.get_temp::<rc_editor::keymap::Chord>(first_id)).is_some() {
                        tr("Press the second shortcut…").to_string()
                    } else {
                        tr("Press a shortcut…").to_string()
                    }
                } else {
                    keymap.shortcut_text(action).unwrap_or_else(|| "—".into())
                };
                let changed = draft.shortcuts.contains_key(&id);
                let text = egui::RichText::new(text).monospace();
                ui.allocate_ui_with_layout(vec2(KEYS_W, 22.0), left, |ui| {
                    ui.set_width(KEYS_W);
                    ui.add(egui::Label::new(if changed { text.strong() } else { text.color(p.text_muted) }).truncate());
                });
                ui.horizontal(|ui| {
                    ui.set_width(BUTTONS_W);
                    if ui.small_button(tr("Record Shortcut")).clicked() {
                        ui.ctx().data_mut(|data| {
                            data.insert_temp(recording_id, id.clone());
                            data.remove::<rc_editor::keymap::Chord>(first_id);
                            data.remove::<bool>(error_id);
                        });
                    }
                    if ui.small_button(tr("Remove")).clicked() {
                        ui.ctx().data_mut(|data| {
                            data.remove::<String>(recording_id);
                            data.remove::<rc_editor::keymap::Chord>(first_id);
                        });
                        draft.shortcuts.insert(id.clone(), String::new());
                    }
                    if changed && ui.small_button(tr("Reset")).clicked() {
                        ui.ctx().data_mut(|data| {
                            data.remove::<String>(recording_id);
                            data.remove::<rc_editor::keymap::Chord>(first_id);
                        });
                        draft.shortcuts.remove(&id);
                    }
                });
                ui.end_row();
            }
        });
    });
}

/// Settings › Live Templates: abbreviation, description and body of each template.
fn live_templates_page(ui: &mut Ui, templates: &mut Vec<rc_core::live_templates::LiveTemplate>, muted: egui::Color32) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(tr(
                "Type the abbreviation in a Rust file and press Tab. In the body, $1, ${2:default} are the stops Tab visits and $0 is where the caret ends.",
            ))
            .color(muted),
        )
        .wrap(),
    );
    ui.add_space(8.0);
    let mut remove = None;
    egui::ScrollArea::vertical().id_salt("live_templates").max_height((ui.available_height() - 44.0).max(120.0)).auto_shrink([false, true]).show(ui, |ui| {
        for (i, t) in templates.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut t.abbreviation).hint_text(tr("Abbreviation")).font(egui::TextStyle::Monospace).desired_width(90.0));
                ui.add(egui::TextEdit::singleline(&mut t.description).hint_text(tr("Description")).desired_width(330.0));
                if theme::icon_button(ui, rc_kit::ico::DELETE, true, tr("Remove")).clicked() {
                    remove = Some(i);
                }
            });
            ui.add(egui::TextEdit::multiline(&mut t.body).font(egui::TextStyle::Monospace).desired_rows(2).desired_width(450.0));
            ui.add_space(8.0);
        }
    });
    if let Some(i) = remove {
        templates.remove(i);
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button(tr("Add Template")).clicked() {
            templates.push(rc_core::live_templates::LiveTemplate { abbreviation: String::new(), description: String::new(), body: "$0".into() });
        }
        if ui.button(tr("Restore Defaults")).clicked() {
            *templates = rc_core::live_templates::defaults();
        }
    });
}

/// Settings › External Tools: each tool's name, program, arguments and folder.
fn external_tools_rows(ui: &mut Ui, tools: &mut Vec<rc_core::external_tools::ExternalTool>, muted: egui::Color32) {
    let mut remove = None;
    for (i, tool) in tools.iter_mut().enumerate() {
        for (label, value, hint) in [
            (tr("Name"), &mut tool.name, "clippy pedantic"),
            (tr("Program"), &mut tool.program, "cargo"),
            (tr("Arguments"), &mut tool.args, "clippy -- -W clippy::pedantic"),
            (tr("Working folder"), &mut tool.working_dir, "$ProjectFileDir$"),
        ] {
            ui.label(label);
            ui.add(egui::TextEdit::singleline(value).hint_text(hint).desired_width(300.0));
            ui.end_row();
        }
        ui.label("");
        if ui.button(tr("Remove")).clicked() {
            remove = Some(i);
        }
        ui.end_row();
    }
    if let Some(i) = remove {
        tools.remove(i);
    }
    ui.label("");
    if ui.button(tr("Add Tool")).clicked() {
        tools.push(rc_core::external_tools::ExternalTool { name: trf("Tool {n}", &[("n", &(tools.len() + 1))]), ..Default::default() });
    }
    ui.end_row();
    ui.label("");
    ui.scope(|ui| {
        ui.set_max_width(300.0);
        ui.add(
            egui::Label::new(
                egui::RichText::new(trf(
                    "Run them from More Actions › External Tools or Find Action. Macros: {list}",
                    &[("list", &rc_core::external_tools::MACROS.join(" "))],
                ))
                .color(muted),
            )
            .wrap(),
        );
    });
    ui.end_row();
}

/// Labels above controls keep long tool names and executable paths within the page.
fn tools_page(ui: &mut Ui, app: &mut CrabCraftApp, draft: &mut Settings, browse: &mut SettingsBrowse) {
    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
    let p = rc_kit::pal(ui);
    app.load_toolchain();
    toolchain_rows(ui, app);
    ui.separator();
    let debugger_label = ui.label(tr("Rust debugger"));
    egui::ComboBox::from_id_salt("rust-debugger-backend")
        .selected_text(match draft.rust_debugger {
            rc_core::settings::RustDebugger::Gdb => "GDB",
            rc_core::settings::RustDebugger::Lldb => "LLDB",
        })
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut draft.rust_debugger, rc_core::settings::RustDebugger::Gdb, "GDB");
            ui.selectable_value(&mut draft.rust_debugger, rc_core::settings::RustDebugger::Lldb, "LLDB");
        })
        .response
        .labelled_by(debugger_label.id);
    ui.add_space(8.0);
    for (label, value, target) in [
        (tr("rust-analyzer path"), &mut draft.rust_analyzer_path, ToolPath::Analyzer),
        (tr("HTML language server"), &mut draft.web_html_server, ToolPath::Html),
        (tr("CSS language server"), &mut draft.web_css_server, ToolPath::Css),
        (tr("JavaScript/TypeScript language server"), &mut draft.web_typescript_server, ToolPath::Typescript),
        (tr("Global tsserver.js path"), &mut draft.web_tsserver_path, ToolPath::Tsserver),
        (tr("GDB executable"), &mut draft.gdb_path, ToolPath::Gdb),
        (tr("LLDB DAP executable"), &mut draft.lldb_dap_path, ToolPath::Lldb),
    ] {
        let label_response = ui.label(label);
        let browse_width = ui.fonts_mut(|fonts| fonts.layout_no_wrap(tr("Browse…").to_string(), FontId::proportional(13.0), p.text)).size().x + 24.0;
        let field_width = (ui.available_width() - browse_width - ui.spacing().item_spacing.x).max(80.0);
        ui.horizontal(|ui| {
            let mut text = value.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
            let web = label == tr("HTML language server")
                || label == tr("CSS language server")
                || label == tr("JavaScript/TypeScript language server")
                || label == tr("Global tsserver.js path");
            let resp = ui
                .add_enabled(
                    !browse.busy(),
                    egui::TextEdit::singleline(&mut text)
                        .hint_text(if web { tr("Not configured; install externally") } else { tr("Detect automatically") })
                        .desired_width(field_width),
                )
                .labelled_by(label_response.id);
            if resp.changed() {
                *value = (!text.trim().is_empty()).then(|| PathBuf::from(text.trim()));
            }
            if ui.add_enabled(!browse.busy(), egui::Button::new(tr("Browse…"))).clicked() {
                browse.start(target, value.clone(), app.waker.clone());
            }
        });
        ui.add_space(8.0);
    }
    if browse.busy() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(tr("Open File…"));
            if ui.button(tr("Cancel")).clicked() {
                browse.cancel();
            }
        });
    }
    rustfmt_rows(ui, &mut draft.rustfmt, p.error, p.text_muted);
}

/// How rustfmt runs for Reformat Code, on save and at commit.
fn rustfmt_rows(ui: &mut Ui, rustfmt: &mut rc_core::RustfmtSettings, error: egui::Color32, muted: egui::Color32) {
    let arguments_label = ui.label(tr("rustfmt arguments"));
    ui.add(egui::TextEdit::singleline(&mut rustfmt.args).hint_text("--config tab_spaces=2").desired_width((ui.available_width() - 8.0).max(80.0)))
        .labelled_by(arguments_label.id);
    ui.add_space(8.0);
    let channel_label = ui.label(tr("rustfmt channel"));
    ui.add(egui::TextEdit::singleline(&mut rustfmt.channel).hint_text(tr("Default toolchain")).desired_width((ui.available_width() - 8.0).max(80.0)))
        .labelled_by(channel_label.id);
    ui.add_space(8.0);
    let environment_label = ui.label(tr("rustfmt environment"));
    ui.add(
        egui::TextEdit::multiline(&mut rustfmt.env)
            .hint_text(tr("NAME=value, one per line"))
            .desired_rows(3)
            .desired_width((ui.available_width() - 8.0).max(80.0)),
    )
    .labelled_by(environment_label.id);
    ui.add_space(8.0);
    match rc_cargo::Rustfmt::check(rustfmt) {
        Ok(()) => ui.label(egui::RichText::new(tr("Used by Reformat Code, on save and at commit. The channel runs through rustup.")).color(muted)),
        Err(e) => ui.label(egui::RichText::new(crate::vcs::rustfmt_failure(e)).color(error)),
    };
    ui.add_space(8.0);
}

/// The versions in use, read from the tools themselves, with rustup's installs and updates.
fn toolchain_rows(ui: &mut Ui, app: &mut CrabCraftApp) {
    if let Err(error) = app.runner.require_trust() {
        ui.label(tr("Toolchain"));
        ui.label(error.to_string());
        ui.end_row();
        return;
    }
    let Some(info) = app.toolchain.clone() else {
        ui.label(tr("Toolchain"));
        ui.spinner();
        ui.end_row();
        return;
    };
    let has_rustup = app.rustup().is_some();
    let missing = tr("Not found");
    for (label, value) in [("Cargo", info.cargo.as_deref()), ("rustc", info.rustc.as_deref())] {
        ui.label(label);
        ui.label(value.unwrap_or(missing));
        ui.end_row();
    }
    ui.label(tr("rust-analyzer"));
    match info.rust_analyzer.as_deref() {
        Some(v) => {
            ui.label(v);
        }
        None => {
            ui.horizontal(|ui| {
                ui.label(missing);
                if has_rustup && ui.button(tr("Install")).on_hover_text("rustup component add rust-analyzer").clicked() {
                    app.run_rustup(&["component", "add", "rust-analyzer"]);
                }
            });
        }
    }
    ui.end_row();
    ui.label(tr("Standard library"));
    match &info.stdlib {
        Some(path) => {
            let text = path.display().to_string();
            ui.scope(|ui| {
                ui.set_max_width(ui.available_width().min(420.0));
                ui.add(egui::Label::new(&text).truncate()).on_hover_text(&text);
            });
        }
        None if has_rustup => {
            ui.horizontal(|ui| {
                ui.label(missing);
                if ui.button(tr("Install")).on_hover_text("rustup component add rust-src").clicked() {
                    app.run_rustup(&["component", "add", "rust-src"]);
                }
            });
        }
        None => {
            ui.label(tr("Not found. Install it with: rustup component add rust-src"));
        }
    }
    ui.end_row();
    ui.label("rustup");
    ui.vertical(|ui| app.rustup_installer.ui(ui));
    ui.end_row();
    if !has_rustup {
        return;
    }
    ui.label(tr("Updates"));
    ui.vertical(|ui| {
        if app.checking_toolchain_updates() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(tr("Checking with rustup…"));
            });
            return;
        }
        match app.toolchain_updates.clone() {
            None => {
                if ui.button(tr("Check for Updates")).on_hover_text("rustup check").clicked() {
                    app.check_toolchain_updates();
                }
            }
            Some(Err(e)) => {
                ui.label(egui::RichText::new(e).color(rc_kit::pal(ui).error));
                if ui.button(tr("Check Again")).clicked() {
                    app.check_toolchain_updates();
                }
            }
            Some(Ok(list)) => {
                for u in &list {
                    let text = match &u.available {
                        Some(new) => trf("{name}: {current} → {new}", &[("name", &u.name), ("current", &u.current), ("new", new)]),
                        None => trf("{name}: {current}, up to date", &[("name", &u.name), ("current", &u.current)]),
                    };
                    ui.label(text);
                }
                ui.horizontal(|ui| {
                    if list.iter().any(|u| u.available.is_some()) && ui.button(tr("Update")).on_hover_text("rustup update").clicked() {
                        app.run_rustup(&["update"]);
                    }
                    if ui.button(tr("Check Again")).clicked() {
                        app.check_toolchain_updates();
                    }
                });
            }
        }
    });
    ui.end_row();
}

fn lang_code(l: Lang) -> &'static str {
    match l {
        Lang::En => "en",
        Lang::Es => "es",
        Lang::Fr => "fr",
        Lang::Pt => "pt-BR",
        Lang::De => "de",
        Lang::Zh => "zh-Hans",
        Lang::Ja => "ja",
        Lang::Pl => "pl",
    }
}

fn about(ctx: &Context) -> Outcome {
    let (close, esc) = modal(ctx, "about", 380.0, |ui| {
        ui.vertical_centered(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(64.0, 64.0), egui::Sense::hover());
            rc_kit::branding::paint(ui, r);
            ui.add_space(8.0);
            ui.label(egui::RichText::new("CrabCraft").font(theme::bold(18.0)));
            ui.label(trf("Version {version}", &[("version", &env!("CARGO_PKG_VERSION"))]));
            ui.add_space(6.0);
            ui.label(egui::RichText::new(tr("A lightweight IDE for Rust")).color(rc_kit::pal(ui).muted));
            ui.add_space(6.0);
            ui.label(egui::RichText::new(tr("Icons: IntelliJ Platform (Apache 2.0)")).size(11.5).color(rc_kit::pal(ui).muted));
            ui.collapsing(tr("Font licenses"), |ui| {
                egui::ScrollArea::vertical().id_salt("font_licenses").max_height(200.0).show(ui, |ui| {
                    ui.label(rc_kit::theme::FONT_LICENSES);
                });
            });
            ui.add_space(12.0);
            theme::primary_button(ui, tr("Close")).clicked()
        })
        .inner
    });
    if close || esc { Outcome::Close } else { Outcome::Keep }
}

fn bookmarks(app: &mut CrabCraftApp, ctx: &Context, list: &mut Vec<crate::insight::Target>, selected: &mut usize) -> Outcome {
    let p = rc_kit::theme::palette(ctx.global_style().visuals.dark_mode);
    let root = app.workspace.as_ref().map(|w| w.ws.root.clone()).unwrap_or_default();
    let mut open = None;
    let mut remove = false;
    let (_, esc) = modal(ctx, "bookmarks", 560.0, |ui| {
        title(ui, tr("Bookmarks"));
        if ui.input(|i| i.key_pressed(Key::ArrowDown)) {
            *selected = (*selected + 1).min(list.len().saturating_sub(1));
        }
        if ui.input(|i| i.key_pressed(Key::ArrowUp)) {
            *selected = selected.saturating_sub(1);
        }
        if ui.input(|i| i.key_pressed(Key::Enter)) {
            open = list.get(*selected).cloned();
        }
        remove = ui.input(|i| i.key_pressed(Key::Delete));
        if list.is_empty() {
            ui.label(egui::RichText::new(tr("No bookmarks. Ctrl+F11 marks the current line.")).color(p.muted));
        }
        egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
            for (i, mark) in list.iter().enumerate() {
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), egui::Sense::click());
                if i == *selected {
                    ui.painter().rect_filled(r, 4.0, p.selection);
                } else if resp.hovered() {
                    ui.painter().rect_filled(r, 4.0, p.row_hover);
                }
                let file = mark.path.strip_prefix(&root).unwrap_or(&mark.path).display();
                ui.painter().text(
                    r.left_center() + vec2(8.0, 0.0),
                    Align2::LEFT_CENTER,
                    format!("{file}:{}", mark.line + 1),
                    FontId::proportional(13.0),
                    p.text,
                );
                ui.painter().text(
                    r.right_center() - vec2(8.0, 0.0),
                    Align2::RIGHT_CENTER,
                    mark.preview.chars().take(44).collect::<String>(),
                    FontId::monospace(11.5),
                    p.muted,
                );
                if resp.clicked() {
                    open = Some(mark.clone());
                }
            }
        });
    });
    if remove && *selected < list.len() {
        let mark = list.remove(*selected);
        app.docs.remove_bookmark(&mark.path, mark.line);
        app.session_dirty = true;
    }
    if let Some(mark) = open {
        app.open_at(&mark.path, mark.line, mark.col);
        return Outcome::Close;
    }
    if esc { Outcome::Close } else { Outcome::Keep }
}

fn run_anything(app: &mut CrabCraftApp, ctx: &Context, text: &mut String, selected: &mut usize) -> Outcome {
    use crate::run_anything::{Item, items};
    let names: Vec<String> = app.runner.configs.iter().map(|c| c.name.clone()).collect();
    let list = items(text, &app.settings.run_anything_history, &names);
    *selected = (*selected).min(list.len().saturating_sub(1));
    let p = rc_kit::theme::palette(ctx.global_style().visuals.dark_mode);
    let mut chosen = None;
    let (_, esc) = modal(ctx, "run_anything", 520.0, |ui| {
        title(ui, tr("Run Anything"));
        let field = ui.add(egui::TextEdit::singleline(text).hint_text(tr("A Cargo command or a run configuration")).desired_width(f32::INFINITY));
        field.request_focus();
        if ui.input(|i| i.key_pressed(Key::ArrowDown)) {
            *selected = (*selected + 1).min(list.len().saturating_sub(1));
        }
        if ui.input(|i| i.key_pressed(Key::ArrowUp)) {
            *selected = selected.saturating_sub(1);
        }
        if ui.input(|i| i.key_pressed(Key::Enter)) {
            chosen = list.get(*selected).cloned();
        }
        ui.add_space(6.0);
        for (i, item) in list.iter().enumerate().take(12) {
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), egui::Sense::click());
            if i == *selected {
                ui.painter().rect_filled(r, 4.0, p.selection);
            } else if resp.hovered() {
                ui.painter().rect_filled(r, 4.0, p.row_hover);
            }
            let (icon, label) = match item {
                Item::Config(index) => (rc_kit::ico::RUN, names[*index].as_str()),
                Item::Command(command) => (rc_kit::ico::BUILD, command.as_str()),
            };
            icons::paint(ui.painter(), egui::Rect::from_center_size(r.left_center() + vec2(14.0, 0.0), vec2(16.0, 16.0)), icon, &p, true);
            ui.painter().text(r.left_center() + vec2(30.0, 0.0), Align2::LEFT_CENTER, label, FontId::proportional(13.0), p.text);
            if resp.clicked() {
                chosen = Some(item.clone());
            }
        }
    });
    if let Some(item) = chosen {
        app.run_anything(ctx, item);
        return Outcome::Close;
    }
    if esc { Outcome::Close } else { Outcome::Keep }
}

fn goto_line(app: &mut CrabCraftApp, ctx: &Context, text: &mut String, focus: &mut bool) -> Outcome {
    let Some(doc) = app.docs.active_doc() else { return Outcome::Close };
    let lines = doc.editor.buffer.len_lines();
    let ((ok, cancel), esc) = modal(ctx, "goto_line", 340.0, |ui| {
        title(ui, tr("Go to Line:Column"));
        let resp = ui.add(egui::TextEdit::singleline(text).desired_width(f32::INFINITY));
        if std::mem::take(focus) {
            resp.request_focus();
        }
        ui.label(egui::RichText::new(trf("1 to {n}", &[("n", &lines)])).size(12.0).color(rc_kit::pal(ui).muted));
        let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        let (ok, cancel) = buttons(ui, tr("OK"), true);
        (ok || enter, cancel)
    });
    if ok {
        let mut parts = text.split([':', ',']);
        let line = parts.next().and_then(|s| s.trim().parse::<usize>().ok());
        let col = parts.next().and_then(|s| s.trim().parse::<usize>().ok()).unwrap_or(1);
        if let Some(line) = line {
            app.push_nav();
            if let Some(d) = app.docs.active_doc_mut() {
                d.editor.goto(line.saturating_sub(1), col.saturating_sub(1));
            }
        }
        return Outcome::Close;
    }
    if cancel || esc { Outcome::Close } else { Outcome::Keep }
}

/// The project's environment variables, checked as they are typed.
fn project_env(app: &mut CrabCraftApp, ctx: &Context, text: &mut String) -> Outcome {
    let parsed = rc_core::session::parse_env(text);
    let ((ok, cancel), esc) = modal(ctx, "project_env", 520.0, |ui| {
        title(ui, tr("Project Environment Variables"));
        let p = rc_kit::pal(ui);
        ui.add(
            egui::TextEdit::multiline(text)
                .hint_text("RUST_LOG=debug\nDATABASE_URL=postgres://localhost/dev")
                .font(egui::TextStyle::Monospace)
                .desired_rows(8)
                .desired_width(f32::INFINITY),
        );
        ui.add_space(6.0);
        if let Err(rc_core::session::BadEnvLine(line)) = parsed {
            ui.colored_label(p.error, trf("Line {line} is not NAME=value with a valid name.", &[("line", &line.to_string())]));
        }
        ui.label(
            egui::RichText::new(tr(
                "One NAME=value per line. Applied to every Cargo command, to the programs it starts and to rust-analyzer; a run configuration's variables win.",
            ))
            .size(12.0)
            .color(p.muted),
        );
        buttons(ui, tr("OK"), parsed.is_ok())
    });
    if ok && parsed.is_ok() {
        app.set_project_env(text.trim().to_string());
        return Outcome::Close;
    }
    if cancel || esc { Outcome::Close } else { Outcome::Keep }
}

fn wasm_options(app: &mut CrabCraftApp, ctx: &Context, name: &str, manifest: &std::path::Path, options: &mut rc_core::session::RunOptions) -> Outcome {
    let valid = crate::runner::wasm_pack::plan(manifest, options, Vec::new());
    let ((ok, cancel), esc) = modal(ctx, "wasm_options", 560.0, |ui| {
        title(ui, &trf("Run Configuration: {name}", &[("name", &name)]));
        ui.label(manifest.display().to_string());
        ui.label(tr("Builds WebAssembly and JavaScript bindings; does not start a browser or publish a package."));
        egui::Grid::new("wasm_options_grid").num_columns(2).spacing([12.0, 10.0]).show(ui, |ui| {
            ui.label(tr("JavaScript target"));
            let shown = if options.wasm_target.is_empty() { "bundler" } else { options.wasm_target.as_str() };
            egui::ComboBox::from_id_salt("wasm_target").selected_text(shown).show_ui(ui, |ui| {
                for target in crate::runner::wasm_pack::TARGETS {
                    ui.selectable_value(&mut options.wasm_target, target.to_string(), *target);
                }
            });
            ui.end_row();
            ui.label(tr("Output directory"));
            ui.add(egui::TextEdit::singleline(&mut options.wasm_out_dir).hint_text("pkg").desired_width(350.0));
            ui.end_row();
            ui.label(tr("Features"));
            ui.add(egui::TextEdit::singleline(&mut options.features).hint_text("serde,logging").desired_width(350.0));
            ui.end_row();
            ui.label(tr("Environment variables"));
            ui.add(egui::TextEdit::multiline(&mut options.env).hint_text("CARGO_NET_OFFLINE=true").desired_width(350.0).desired_rows(3));
            ui.end_row();
            ui.label("");
            ui.checkbox(&mut options.wasm_dev, tr("Development build (otherwise release)"));
            ui.end_row();
        });
        ui.label(tr("Requires wasm-pack, wasm-bindgen and the wasm32-unknown-unknown target. Tools are never installed automatically; wasm-opt is disabled."));
        if let Err(error) = &valid {
            ui.colored_label(rc_kit::pal(ui).error, error.to_string());
        }
        buttons(ui, tr("OK"), valid.is_ok())
    });
    if ok {
        app.runner.options.insert(name.to_string(), options.clone());
        app.session_dirty = true;
        return Outcome::Close;
    }
    if cancel || esc { Outcome::Close } else { Outcome::Keep }
}

fn run_options(app: &mut CrabCraftApp, ctx: &Context, name: &str, project: &std::path::Path, o: &mut rc_core::session::RunOptions) -> Outcome {
    let ((ok, cancel), esc) = modal(ctx, "run_options", 560.0, |ui| {
        title(ui, &trf("Run Configuration: {name}", &[("name", &name)]));
        let p = rc_kit::pal(ui);
        egui::Grid::new("run_options_grid").num_columns(2).spacing([12.0, 10.0]).show(ui, |ui| {
            ui.label(tr("Program arguments"));
            ui.add(egui::TextEdit::singleline(&mut o.args).hint_text("").font(egui::TextStyle::Monospace).desired_width(380.0));
            ui.end_row();
            ui.label(tr("Environment variables"));
            ui.add(
                egui::TextEdit::multiline(&mut o.env)
                    .hint_text("RUST_LOG=debug\nAPP_MODE=test")
                    .font(egui::TextStyle::Monospace)
                    .desired_rows(4)
                    .desired_width(380.0),
            );
            ui.end_row();
            ui.label(tr("Working directory"));
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut o.working_dir).hint_text(tr("The project folder")).desired_width(346.0));
                if theme::icon_button(ui, rc_kit::ico::OPEN, true, tr("Choose a folder")).clicked()
                    && let Some(d) = rfd::FileDialog::new().set_directory(project.parent().unwrap_or(project)).pick_folder()
                {
                    o.working_dir = d.display().to_string();
                }
            });
            ui.end_row();
            ui.label(tr("Cargo profile"));
            ui.add(egui::TextEdit::singleline(&mut o.cargo_profile).hint_text("dev, release").desired_width(380.0));
            ui.end_row();
            ui.label(tr("Toolchain"));
            ui.add(egui::TextEdit::singleline(&mut o.channel).hint_text(tr("The default one (stable, nightly…)")).desired_width(380.0));
            ui.end_row();
            ui.label(tr("Features"));
            ui.add(egui::TextEdit::singleline(&mut o.features).hint_text("serde,logging").desired_width(380.0));
            ui.end_row();
            ui.label("RUST_BACKTRACE");
            let shown = if o.backtrace.is_empty() { "short" } else { o.backtrace.as_str() }.to_string();
            egui::ComboBox::from_id_salt("run_backtrace").selected_text(shown).width(120.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut o.backtrace, String::new(), "short");
                ui.selectable_value(&mut o.backtrace, "full".to_string(), "full");
                ui.selectable_value(&mut o.backtrace, "0".to_string(), "0");
            });
            ui.end_row();
            ui.label(tr("Input file"));
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut o.stdin_file).hint_text(tr("The console")).desired_width(346.0));
                if theme::icon_button(ui, rc_kit::ico::OPEN, true, tr("Choose a file")).clicked()
                    && let Some(f) = rfd::FileDialog::new().set_directory(project.parent().unwrap_or(project)).pick_file()
                {
                    o.stdin_file = f.display().to_string();
                }
            });
            ui.end_row();
            ui.label("");
            ui.checkbox(&mut o.emulate_terminal, tr("Emulate terminal in the output console"))
                .on_hover_text(tr("Runs the program in a pseudo-terminal: colours, terminal checks and keyboard input work as in a terminal"));
            ui.end_row();
            ui.label(tr("Before launch"));
            ui.vertical(|ui| {
                let tools: Vec<String> = app.settings.external_tools.iter().map(|t| t.name.clone()).collect();
                // Tools that no longer exist stay listed, so they can be unchecked.
                let gone: Vec<String> = o.before_launch.iter().filter(|n| !tools.contains(n)).cloned().collect();
                for name in tools.iter().chain(&gone) {
                    let mut on = o.before_launch.contains(name);
                    let label = if gone.contains(name) { trf("{name} (missing)", &[("name", name)]) } else { name.clone() };
                    if ui.checkbox(&mut on, label).changed() {
                        if on {
                            o.before_launch.push(name.clone());
                        } else {
                            o.before_launch.retain(|n| n != name);
                        }
                    }
                }
                if tools.is_empty() && gone.is_empty() {
                    ui.label(egui::RichText::new(tr("Add external tools in Settings › External Tools to run them before the build")).color(p.muted));
                }
            });
            ui.end_row();
            ui.label("");
            ui.checkbox(&mut o.allow_parallel, tr("Allow multiple instances"))
                .on_hover_text(tr("Running it again starts another instance instead of restarting it"));
            ui.end_row();
            ui.label("");
            ui.checkbox(&mut o.in_project, tr("Store as project file")).on_hover_text(tr("Saved in .crabcraft/runConfigurations to be committed and shared"));
            ui.end_row();
        });
        ui.add_space(6.0);
        ui.label(egui::RichText::new(tr("One NAME=value per line.")).size(12.0).color(p.muted));
        buttons(ui, tr("OK"), true)
    });
    if ok {
        if *o == rc_core::session::RunOptions::default() {
            app.runner.options.remove(name);
        } else {
            app.runner.options.insert(name.to_string(), o.clone());
        }
        if let Some(root) = app.workspace.as_ref().map(|w| w.ws.root.clone()) {
            let written = if o.in_project { rc_core::run_files::save(&root, name, o).map(|_| ()) } else { rc_core::run_files::remove(&root, name) };
            if let Err(e) = written {
                app.notify(trf("Could not save the run configuration in the project: {error}", &[("error", &e)]), true);
            }
        }
        app.session_dirty = true;
        return Outcome::Close;
    }
    if cancel || esc { Outcome::Close } else { Outcome::Keep }
}

fn compound(app: &mut CrabCraftApp, ctx: &Context, old: Option<&str>, name: &mut String, members: &mut [(String, bool)]) -> Outcome {
    let taken = app.runner.configs.iter().any(|c| c.name == name.trim() && Some(c.name.as_str()) != old);
    let chosen = members.iter().filter(|(_, on)| *on).count();
    let valid = !name.trim().is_empty() && !taken && chosen > 0;
    let ((ok, cancel), esc) = modal(ctx, "compound", 420.0, |ui| {
        title(ui, if old.is_some() { tr("Edit Compound Configuration") } else { tr("Add Compound Configuration") });
        ui.label(tr("Name"));
        ui.add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY));
        ui.add_space(8.0);
        ui.label(tr("Start these applications together:"));
        for (n, on) in members.iter_mut() {
            ui.checkbox(on, n.as_str());
        }
        if taken {
            ui.label(egui::RichText::new(tr("A configuration with that name already exists")).size(12.5).color(rc_kit::pal(ui).error));
        }
        buttons(ui, tr("OK"), valid)
    });
    if ok && valid {
        let list = members.iter().filter(|(_, on)| *on).map(|(n, _)| n.clone()).collect();
        app.runner.save_compound(old, name.trim().to_string(), list);
        app.session_dirty = true;
        return Outcome::Close;
    }
    if cancel || esc { Outcome::Close } else { Outcome::Keep }
}

/// Names a new folder for a run configuration, or renames the folder it is in.
fn config_folder(app: &mut CrabCraftApp, ctx: &Context, config: &str, rename: Option<&str>, folder: &mut String) -> Outcome {
    let valid = !folder.trim().is_empty() && folder.trim().chars().count() <= crate::runner::MAX_FOLDER_CHARS;
    let ((ok, cancel), esc) = modal(ctx, "config_folder", 380.0, |ui| {
        title(ui, if rename.is_some() { tr("Rename Folder") } else { tr("New Folder") });
        ui.label(tr("Folder name"));
        let resp = ui.add(egui::TextEdit::singleline(folder).desired_width(f32::INFINITY));
        if !resp.has_focus() && folder.is_empty() {
            resp.request_focus();
        }
        let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        let (ok, cancel) = buttons(ui, tr("OK"), valid);
        (ok || enter, cancel)
    });
    if ok && valid {
        match rename {
            Some(old) => app.runner.rename_folder(old, folder),
            None => app.runner.move_to_folder(config, Some(folder)),
        }
        app.session_dirty = true;
        return Outcome::Close;
    }
    if cancel || esc { Outcome::Close } else { Outcome::Keep }
}

fn attach(app: &mut CrabCraftApp, ctx: &Context, filter: &mut String, processes: &[rc_core::processes::Process]) -> Outcome {
    let (chosen, cancel, esc) = {
        let ((chosen, cancel), esc) = modal(ctx, "attach_process", 560.0, |ui| {
            title(ui, tr("Attach to Process"));
            let resp = ui.add(egui::TextEdit::singleline(filter).hint_text(tr("Filter by name or PID")).desired_width(f32::INFINITY));
            if ui.memory(|m| m.focused().is_none()) {
                resp.request_focus();
            }
            ui.add_space(6.0);
            let needle = filter.trim().to_lowercase();
            let mut chosen = None;
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                for p in processes.iter().filter(|p| needle.is_empty() || p.name.to_lowercase().contains(&needle) || p.pid.to_string().contains(&needle)) {
                    let text = format!("{}  {}  {}", p.pid, p.name, p.command);
                    if ui.add(egui::Button::new(egui::RichText::new(text).font(egui::FontId::monospace(12.0))).truncate().frame(false)).clicked() {
                        chosen = Some((p.pid, p.name.clone()));
                    }
                }
            });
            ui.add_space(8.0);
            ui.separator();
            auto_attach(app, ui);
            ui.add_space(10.0);
            let cancel = ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| theme::secondary_button(ui, tr("Cancel")).clicked()).inner;
            (chosen, cancel)
        });
        (chosen, cancel, esc)
    };
    if let Some((pid, name)) = chosen {
        let outs = app.debugger.attach_gdb(pid, &name, &app.docs);
        app.handle_debug(outs);
        return Outcome::Close;
    }
    if cancel || esc { Outcome::Close } else { Outcome::Keep }
}

/// The automatic attach of the Attach dialog (D-15): name patterns and Start/Stop Watching.
fn auto_attach(app: &mut CrabCraftApp, ui: &mut Ui) {
    let p = rc_kit::pal(ui);
    ui.label(egui::RichText::new(tr("Attach automatically to new processes named")).strong());
    let watching = app.auto_attach.is_some();
    ui.horizontal(|ui| {
        let edit = ui.add_enabled(
            !watching,
            egui::TextEdit::singleline(&mut app.auto_attach_text)
                .hint_text(tr("server*, *-worker"))
                .char_limit(crate::auto_attach::MAX_PATTERNS * (crate::auto_attach::MAX_PATTERN_CHARS + 2))
                .desired_width(ui.available_width() - 150.0),
        );
        if edit.changed() {
            app.session_dirty = true;
        }
        let ready = !crate::auto_attach::parse_patterns(&app.auto_attach_text).is_empty();
        if watching {
            if theme::secondary_button(ui, tr("Stop Watching")).clicked() {
                app.stop_auto_attach();
            }
        } else if ui.add_enabled_ui(ready, |ui| theme::primary_button(ui, tr("Start Watching"))).inner.clicked() && !app.start_auto_attach() {
            app.notify(tr("Attaching needs an open Rust project"), false);
        }
    });
    let note = match &app.auto_attach {
        Some(w) => trf(
            "Watching for {patterns}: the debugger attaches to the first one that starts while it is not debugging.",
            &[("patterns", &w.patterns.join(", "))],
        ),
        None => tr("Separate names with commas; * is any text and ? one character. Only processes started after the watch begins count.").to_string(),
    };
    ui.label(egui::RichText::new(note).size(12.0).color(if watching { p.accent } else { p.muted }));
}

#[allow(clippy::too_many_arguments)]
/// What the Breakpoints dialog asked for, done once it is drawn.
enum BpAction {
    Enable(PathBuf, usize, bool),
    EnableGroup(Option<String>, bool),
    Select(PathBuf, usize),
    GoTo(PathBuf, usize),
    Edit(PathBuf, usize),
    Remove(PathBuf, usize),
    Move(PathBuf, usize, Option<String>),
    RenameGroup(String, String),
    Ungroup(String),
}

/// View Breakpoints (Ctrl+Shift+F8): every breakpoint of the project, open file or not, by
/// group; each can be turned on or off, opened, edited, removed or moved to a group, and a
/// whole group turned on or off at once.
fn breakpoints(app: &mut CrabCraftApp, ctx: &Context, selected: &mut Option<(PathBuf, usize)>, group: &mut String) -> Outcome {
    let rows = app.breakpoint_rows();
    let groups = app.breakpoint_groups();
    let root = app.workspace.as_ref().map(|w| w.ws.root.clone());
    if selected.as_ref().is_some_and(|(p, l)| !rows.iter().any(|r| r.path == *p && r.line == *l)) {
        *selected = None;
    }
    let mut actions: Vec<BpAction> = Vec::new();
    let (close, esc) = modal(ctx, "breakpoints", 720.0, |ui| {
        let p = rc_kit::pal(ui);
        title(ui, tr("Breakpoints"));
        if app.debugger.muted {
            ui.label(egui::RichText::new(tr("Breakpoints are muted: none of them stops the program")).size(12.5).color(p.warn));
            ui.add_space(4.0);
        }
        if rows.is_empty() {
            ui.label(egui::RichText::new(tr("No breakpoints. Click the gutter of an editor to set one.")).color(p.muted));
        }
        egui::ScrollArea::vertical().id_salt("breakpoint_rows").max_height(360.0).auto_shrink([false, true]).show(ui, |ui| {
            let sections: Vec<Option<String>> = groups.iter().cloned().map(Some).chain(rows.iter().any(|r| r.group.is_none()).then_some(None)).collect();
            for section in sections {
                let members: Vec<&crate::breakpoint_list::BreakpointRow> = rows.iter().filter(|r| r.group == section).collect();
                // A header with a checkbox for the group, when there are groups at all.
                if !groups.is_empty() {
                    ui.horizontal(|ui| {
                        let on = members.iter().filter(|r| r.enabled).count();
                        let mut all = on == members.len();
                        let label = section.clone().unwrap_or_else(|| tr("No group").to_string());
                        let resp = ui.add(
                            egui::Checkbox::new(&mut all, egui::RichText::new(format!("{label} ({})", members.len())).strong())
                                .indeterminate(on > 0 && on < members.len()),
                        );
                        if resp.changed() {
                            actions.push(BpAction::EnableGroup(section.clone(), all));
                        }
                        if let Some(g) = &section {
                            resp.context_menu(|ui| {
                                if theme::menu_item(ui, Some(rc_kit::ico::EDIT), tr("Rename the Group to the Name Below"), false).clicked() {
                                    actions.push(BpAction::RenameGroup(g.clone(), group.clone()));
                                    ui.close();
                                }
                                if theme::menu_item(ui, Some(rc_kit::ico::CLOSE), tr("Ungroup"), false).clicked() {
                                    actions.push(BpAction::Ungroup(g.clone()));
                                    ui.close();
                                }
                            });
                        }
                    });
                }
                for r in members {
                    ui.horizontal(|ui| {
                        if !groups.is_empty() {
                            ui.add_space(18.0);
                        }
                        let mut on = r.enabled;
                        if ui.checkbox(&mut on, "").changed() {
                            actions.push(BpAction::Enable(r.path.clone(), r.line, on));
                        }
                        let name = r.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        let is_selected = selected.as_ref().is_some_and(|(p, l)| *p == r.path && *l == r.line);
                        let resp = ui.selectable_label(is_selected, egui::RichText::new(format!("{name}:{}", r.line + 1)).font(FontId::monospace(12.5)));
                        if resp.clicked() {
                            actions.push(BpAction::Select(r.path.clone(), r.line));
                        }
                        if resp.double_clicked() {
                            actions.push(BpAction::GoTo(r.path.clone(), r.line));
                        }
                        let folder = r
                            .path
                            .parent()
                            .map(|d| {
                                root.as_ref()
                                    .and_then(|root| d.strip_prefix(root).ok())
                                    .map(|d| d.display().to_string())
                                    .unwrap_or_else(|| d.display().to_string())
                            })
                            .unwrap_or_default();
                        ui.label(egui::RichText::new(folder).size(11.5).color(p.muted));
                        let detail = match (&r.log, &r.condition) {
                            (Some(m), _) => Some(trf("logs {message}", &[("message", m)])),
                            (None, Some(c)) => Some(trf("when {condition}", &[("condition", c)])),
                            _ => None,
                        };
                        if let Some(d) = detail {
                            ui.label(egui::RichText::new(d).font(FontId::monospace(11.5)).color(p.accent));
                        }
                    });
                }
            }
        });
        ui.add_space(8.0);
        ui.separator();
        // What can be done with the selected breakpoint.
        let chosen = selected.clone();
        ui.add_enabled_ui(chosen.is_some(), |ui| {
            ui.horizontal(|ui| {
                ui.label(tr("Group"));
                ui.add(egui::TextEdit::singleline(group).hint_text(tr("New or existing group")).desired_width(180.0));
                egui::ComboBox::from_id_salt("bp_groups").selected_text(tr("Existing groups")).width(140.0).show_ui(ui, |ui| {
                    for g in &groups {
                        if ui.selectable_label(false, g).clicked() {
                            *group = g.clone();
                        }
                    }
                });
                if let Some((path, line)) = &chosen {
                    if theme::secondary_button(ui, tr("Move to Group")).clicked() && !group.trim().is_empty() {
                        actions.push(BpAction::Move(path.clone(), *line, Some(group.clone())));
                    }
                    if theme::secondary_button(ui, tr("Remove from Group")).clicked() {
                        actions.push(BpAction::Move(path.clone(), *line, None));
                    }
                }
            });
        });
        ui.add_space(10.0);
        let mut close = false;
        ui.horizontal(|ui| {
            if let Some((path, line)) = &chosen {
                if theme::secondary_button(ui, tr("Go To")).clicked() {
                    actions.push(BpAction::GoTo(path.clone(), *line));
                }
                if theme::secondary_button(ui, tr("Edit…")).clicked() {
                    actions.push(BpAction::Edit(path.clone(), *line));
                }
                if theme::secondary_button(ui, tr("Remove")).clicked() {
                    actions.push(BpAction::Remove(path.clone(), *line));
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                close = theme::primary_button(ui, tr("Close")).clicked();
            });
        });
        close
    });
    for a in actions {
        match a {
            BpAction::Enable(path, line, on) => app.set_breakpoint_enabled(&path, line, on),
            BpAction::EnableGroup(g, on) => app.set_group_enabled(g.as_deref(), on),
            BpAction::Select(path, line) => {
                *group = app.debugger.groups.get(&(path.clone(), line)).cloned().unwrap_or_default();
                *selected = Some((path, line));
            }
            BpAction::GoTo(path, line) => {
                app.go_to_breakpoint(&path, line);
                return Outcome::Close;
            }
            BpAction::Edit(path, line) => {
                let key = (path.clone(), line);
                let enabled = app.breakpoint_rows().iter().any(|r| r.path == path && r.line == line && r.enabled);
                let condition = app.debugger.conditions.get(&key).cloned().unwrap_or_default();
                let log = app.debugger.logs.get(&key).cloned().unwrap_or_default();
                let once = app.debugger.once.contains(&key);
                app.go_to_breakpoint(&path, line);
                app.dialog = Some(Dialog::BreakpointProperties { path, line, condition, log, once, enabled, focus: true });
                return Outcome::Keep;
            }
            BpAction::Remove(path, line) => {
                app.remove_breakpoint(&path, line);
                *selected = None;
            }
            BpAction::Move(path, line, g) => app.set_breakpoint_group(&path, line, g.as_deref()),
            BpAction::RenameGroup(old, new) => app.rename_breakpoint_group(&old, &new),
            BpAction::Ungroup(g) => app.remove_breakpoint_group(&g),
        }
    }
    if close || esc { Outcome::Close } else { Outcome::Keep }
}

fn breakpoint_properties(
    app: &mut CrabCraftApp,
    ctx: &Context,
    path: &std::path::Path,
    line: usize,
    condition: &mut String,
    log: &mut String,
    once: &mut bool,
    enabled: &mut bool,
    focus: &mut bool,
) -> Outcome {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let ((ok, cancel, remove), esc) = modal(ctx, "breakpoint_properties", 420.0, |ui| {
        title(ui, &trf("Breakpoint at {file}:{line}", &[("file", &name), ("line", &(line + 1))]));
        ui.checkbox(enabled, tr("Enabled"));
        ui.add_space(6.0);
        ui.label(tr("Condition"));
        let resp = ui.add(
            egui::TextEdit::singleline(condition)
                .hint_text(tr("Stop only when this expression is true"))
                .font(egui::TextStyle::Monospace)
                .desired_width(f32::INFINITY),
        );
        if std::mem::take(focus) {
            resp.request_focus();
        }
        let mut enter = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        ui.add_space(6.0);
        ui.label(tr("Log message"));
        let resp = ui.add(
            egui::TextEdit::singleline(log)
                .hint_text(tr("Print this instead of stopping; {expression} shows a value"))
                .font(egui::TextStyle::Monospace)
                .desired_width(f32::INFINITY),
        );
        enter |= resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        ui.add_space(6.0);
        // A logpoint never stops, so the IDE cannot see it being hit.
        ui.add_enabled(log.is_empty(), egui::Checkbox::new(once, tr("Remove once hit")));
        let mut remove = false;
        ui.add_space(14.0);
        let mut res = (false, false);
        ui.horizontal(|ui| {
            remove = theme::secondary_button(ui, tr("Remove")).clicked();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                res.0 = theme::primary_button(ui, tr("Done")).clicked();
                res.1 = theme::secondary_button(ui, tr("Cancel")).clicked();
            });
        });
        (res.0 || enter, res.1, remove)
    });
    if ok || remove {
        let key = (path.to_path_buf(), line);
        let cond = condition.trim().to_string();
        // The file may be closed: its breakpoints are kept apart (D-27).
        if let Some(bps) = app.docs.breakpoints_mut(path) {
            if remove {
                bps.remove(&line);
                app.debugger.forget_breakpoint(path, line);
            } else {
                let kind = if !*enabled {
                    rc_editor::BreakpointKind::Disabled
                } else if !log.is_empty() {
                    rc_editor::BreakpointKind::Log
                } else if cond.is_empty() {
                    rc_editor::BreakpointKind::Normal
                } else {
                    rc_editor::BreakpointKind::Conditional
                };
                bps.insert(line, kind);
                if cond.is_empty() {
                    app.debugger.conditions.remove(&key);
                } else {
                    app.debugger.conditions.insert(key.clone(), cond);
                }
                if log.is_empty() {
                    app.debugger.logs.remove(&key);
                } else {
                    app.debugger.logs.insert(key.clone(), log.clone());
                }
                if *once && log.is_empty() {
                    app.debugger.once.insert(key);
                } else {
                    app.debugger.once.remove(&key);
                }
            }
            app.session_dirty = true;
        }
        return Outcome::Close;
    }
    if cancel || esc { Outcome::Close } else { Outcome::Keep }
}

fn evaluate(app: &mut CrabCraftApp, ctx: &Context, expression: &mut String, focus: &mut bool) -> Outcome {
    let stopped = app.debugger.stopped();
    let mut eval = false;
    let mut watch = false;
    let (close, esc) = modal(ctx, "evaluate", 560.0, |ui| {
        title(ui, tr("Evaluate Expression"));
        let resp = ui.add(egui::TextEdit::singleline(expression).hint_text(tr("Expression")).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
        if std::mem::take(focus) {
            resp.request_focus();
        }
        if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            eval = true;
            resp.request_focus();
        }
        ui.add_space(8.0);
        let p = rc_kit::pal(ui);
        egui::Frame::new().fill(p.control_bg).corner_radius(6.0).inner_margin(8.0).show(ui, |ui| {
            ui.set_min_height(120.0);
            ui.set_width(ui.available_width());
            if !stopped {
                ui.label(egui::RichText::new(tr("The program must be paused to evaluate expressions")).color(p.muted));
            }
            match app.debugger.eval.clone() {
                Some((_, e, None)) => {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(12.0));
                        ui.label(egui::RichText::new(e).monospace());
                    });
                }
                Some((_, e, Some(Ok(v)))) => {
                    crate::ui::debug_window::eval_tree(ui, &mut app.debugger, &e, &v);
                }
                Some((_, _, Some(Err(err)))) => {
                    ui.label(egui::RichText::new(err).color(p.error));
                }
                None => {}
            }
        });
        ui.add_space(14.0);
        let mut close = false;
        ui.horizontal(|ui| {
            if ui.add_enabled_ui(stopped && !expression.trim().is_empty(), |ui| theme::secondary_button(ui, tr("Add to Watches"))).inner.clicked() {
                watch = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                close = theme::secondary_button(ui, tr("Close")).clicked();
                if ui.add_enabled_ui(stopped && !expression.trim().is_empty(), |ui| theme::primary_button(ui, tr("Evaluate"))).inner.clicked() {
                    eval = true;
                }
            });
        });
        close
    });
    if eval && stopped && !expression.trim().is_empty() {
        app.debugger.evaluate(expression.trim().to_string());
    }
    if watch && let Some(s) = &mut app.debugger.session {
        s.add_watch(expression.trim().to_string());
    }
    if close || esc { Outcome::Close } else { Outcome::Keep }
}

/// Opens the native file creation dialog.
#[allow(clippy::too_many_arguments)]
fn new_file(
    app: &mut CrabCraftApp,
    ctx: &Context,
    dir: &std::path::Path,
    name: &mut String,
    kind: crate::item_templates::ItemKind,
    attach: &mut bool,
    public: &mut bool,
    error: &mut Option<String>,
) -> Outcome {
    let heading = trf("New {item}", &[("item", &tr(kind.title()))]);
    let parent = kind.is_rust().then(|| rc_cargo::modules::parent_module(dir)).flatten();
    let ((ok, cancel), esc) = modal(ctx, "new_file", 380.0, |ui| {
        title(ui, &heading);
        let resp = ui.add(egui::TextEdit::singleline(name).hint_text(tr("Name")).desired_width(f32::INFINITY));
        resp.request_focus();
        if !kind.is_folder() && !name.trim().is_empty() {
            let files = kind.files(dir, name);
            let root = app.workspace.as_ref().map(|w| w.ws.root.clone()).unwrap_or_default();
            let detail = files[0].0.strip_prefix(&root).unwrap_or(&files[0].0).display().to_string();
            ui.add(egui::Label::new(egui::RichText::new(detail).size(12.0).color(rc_kit::pal(ui).muted)).truncate());
        }
        if kind.is_rust() {
            match parent.as_ref().and_then(|p| p.file_name()) {
                Some(file) => {
                    ui.checkbox(attach, trf("Declare in {file}", &[("file", &file.to_string_lossy())]));
                    ui.add_enabled(*attach, egui::Checkbox::new(public, tr("Public (pub mod)")));
                }
                None => {
                    ui.label(egui::RichText::new(tr("No module here declares it: it stays outside the crate")).size(12.0).color(rc_kit::pal(ui).muted));
                }
            }
        }
        if let Some(e) = error.as_ref() {
            ui.colored_label(rc_kit::pal(ui).error, e);
        }
        let enter = ui.input(|i| i.key_pressed(Key::Enter));
        let (ok, cancel) = buttons(ui, tr("Create"), !name.trim().is_empty());
        (ok || enter && !name.trim().is_empty(), cancel)
    });
    if ok {
        let module = crate::item_templates::ItemKind::module_name(name).to_string();
        if kind.is_rust() && !rc_cargo::modules::is_module_name(&module) {
            *error = Some(tr("This name is not a valid Rust module name").to_string());
            return Outcome::Keep;
        }
        let files: Vec<(PathBuf, String)> = kind
            .files(dir, name)
            .into_iter()
            .map(|(p, c)| {
                let c = if kind.is_folder() { c } else { app.copyright.new_file_content(&p, &c) };
                (p, c)
            })
            .collect();
        if files.iter().any(|(p, _)| p.exists()) {
            *error = Some(tr("A file or folder with this name already exists").to_string());
            return Outcome::Keep;
        }
        let res: std::io::Result<()> = files.iter().try_for_each(|(p, content)| {
            if kind.is_folder() {
                std::fs::create_dir_all(p)
            } else {
                // "Folder/name.ext" creates the folders too.
                p.parent().map_or(Ok(()), std::fs::create_dir_all)?;
                std::fs::write(p, content.replace("$0", ""))
            }
        });
        match res {
            Ok(()) => {
                let (first, content) = &files[0];
                app.explorer.invalidate(first);
                if let Some(parent) = parent.as_ref().filter(|_| *attach) {
                    app.declare_module(parent, &module, *public);
                }
                if !kind.is_folder() {
                    // Apply the template caret only after its worker-prepared document arrives.
                    match content.find("$0") {
                        Some(at) => app.request_open_offset(first, content[..at].chars().count()),
                        None => app.open_file(first),
                    }
                }
                return Outcome::Close;
            }
            Err(e) => {
                *error = Some(e.to_string());
                return Outcome::Keep;
            }
        }
    }
    if cancel || esc { Outcome::Close } else { Outcome::Keep }
}

fn rename(app: &mut CrabCraftApp, ctx: &Context, path: &mut PathBuf, name: &mut String, error: &mut Option<String>) -> Outcome {
    let ((ok, cancel), esc) = modal(ctx, "rename", 380.0, |ui| {
        title(ui, tr("Rename"));
        let resp = ui.add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY));
        resp.request_focus();
        if let Some(e) = error.as_ref() {
            ui.colored_label(rc_kit::pal(ui).error, e);
        }
        let enter = ui.input(|i| i.key_pressed(Key::Enter));
        let (ok, cancel) = buttons(ui, tr("Rename"), !name.trim().is_empty());
        (ok || enter && !name.trim().is_empty(), cancel)
    });
    if ok {
        let target = path.with_file_name(name.trim());
        if target == *path {
            return Outcome::Close;
        }
        if name.trim().contains(['/', '\\']) {
            *error = Some(tr("The name cannot contain a path separator").to_string());
            return Outcome::Keep;
        }
        if target.exists() {
            *error = Some(tr("A file or folder with this name already exists").to_string());
            return Outcome::Keep;
        }
        // Nothing changes yet: first the references to the old name are looked for.
        let r = crate::file_rename::FileRename::start(app, path.clone(), target);
        app.dialog = Some(Dialog::RenameFile(Box::new(r)));
        return Outcome::Keep;
    }
    if cancel || esc { Outcome::Close } else { Outcome::Keep }
}

/// The changes a file rename brings (module declarations and paths from the language server,
/// mentions of the old name as text), each one picked or not, then the rename.
fn rename_file(app: &mut CrabCraftApp, ctx: &Context, r: &mut crate::file_rename::FileRename) -> Outcome {
    r.update(&app.docs);
    let searching = r.searching();
    if searching {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
    // Nothing else mentions the file: renamed right away.
    if !searching && r.changes.is_empty() && r.server_error.is_none() && r.error.is_none() {
        return finish_rename(app, r, false);
    }
    let old = r.from.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let new = r.to.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let root = app.workspace.as_ref().map(|w| w.ws.root.clone());
    let p = rc_kit::theme::palette(ctx.global_style().visuals.dark_mode);
    let ((update, only, cancel), esc) = modal(ctx, "rename_file", 680.0, |ui| {
        title(ui, &trf("Rename '{old}' to '{new}'", &[("old", &old), ("new", &new)]));
        if searching {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(tr("Searching for references…"));
            });
        } else {
            let mut files: Vec<&PathBuf> = r.changes.iter().map(|c| &c.path).collect();
            files.dedup();
            let files = files.len();
            ui.label(trf(
                "{n} changes in {files} files mention the old name. Pick the ones to make with the rename.",
                &[("n", &r.changes.len()), ("files", &files)],
            ));
            if !r.asked_server {
                ui.label(egui::RichText::new(tr("No language server checked the code: only text matches are listed.")).size(12.0).color(p.muted));
            }
            if let Some(e) = &r.server_error {
                ui.label(egui::RichText::new(trf("The language server could not list the references: {error}", &[("error", e)])).size(12.0).color(p.warn));
            }
            ui.add_space(6.0);
            change_list(ui, &mut r.changes, root.as_deref(), p.muted, true);
        }
        if let Some(e) = &r.error {
            ui.colored_label(p.error, e);
        }
        ui.add_space(14.0);
        let mut res = (false, false, false);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let any = r.changes.iter().any(|c| c.checked);
            res.0 = ui.add_enabled_ui(!searching && any, |ui| theme::primary_button(ui, tr("Rename and Update References"))).inner.clicked();
            res.1 = theme::secondary_button(ui, tr("Rename Only")).clicked();
            res.2 = theme::secondary_button(ui, tr("Cancel")).clicked();
        });
        res
    });
    if update || only {
        return finish_rename(app, r, update);
    }
    if cancel || esc { Outcome::Close } else { Outcome::Keep }
}

/// The refactoring's changes, grouped by file, each with a checkbox; Refactor applies the
/// checked ones.
fn refactor_preview(app: &mut CrabCraftApp, ctx: &Context, r: &mut crate::refactor_preview::RefactorPreview) -> Outcome {
    r.update();
    let loading = r.loading();
    if loading {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
    let root = app.workspace.as_ref().map(|w| w.ws.root.clone());
    let p = rc_kit::theme::palette(ctx.global_style().visuals.dark_mode);
    let title_text = r.title.clone();
    let ((apply, cancel), esc) = modal(ctx, "refactor_preview", 680.0, |ui| {
        title(ui, &title_text);
        if loading {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(tr("Listing the changes…"));
            });
        } else {
            let mut files: Vec<&PathBuf> = r.changes.iter().map(|c| &c.path).collect();
            files.dedup();
            ui.label(trf("{n} changes in {files} files. Uncheck the ones to leave out.", &[("n", &r.changes.len()), ("files", &files.len())]));
            ui.add_space(6.0);
            change_list(ui, &mut r.changes, root.as_deref(), p.muted, false);
        }
        if let Some(e) = &r.error {
            ui.colored_label(p.error, e);
        }
        buttons(ui, tr("Refactor"), !loading && r.changes.iter().any(|c| c.checked))
    });
    if apply {
        return match crate::insight::apply_file_edits(&mut app.docs, r.checked()) {
            Ok(n) => {
                app.notify(trf("Renamed in {n} files", &[("n", &n)]), false);
                Outcome::Close
            }
            Err(e) => {
                r.error = Some(e);
                Outcome::Keep
            }
        };
    }
    if cancel || esc { Outcome::Close } else { Outcome::Keep }
}

/// What may go wrong with a rename, each conflict with its place (a click opens it);
/// "Continue anyway" makes the rename (or previews it), Cancel leaves everything as it is.
fn refactor_conflicts(app: &mut CrabCraftApp, ctx: &Context, r: &mut crate::refactor_conflicts::Review) -> Outcome {
    let root = app.workspace.as_ref().map(|w| w.ws.root.clone());
    let p = rc_kit::theme::palette(ctx.global_style().visuals.dark_mode);
    let mut open = None;
    let can_continue = r.edit.is_some();
    let ((go, cancel), esc) = modal(ctx, "refactor_conflicts", 640.0, |ui| {
        title(ui, tr("Refactoring Conflicts"));
        ui.horizontal(|ui| {
            icons::image(ui, rc_kit::ico::DIALOG_WARNING, 32.0);
            ui.label(trf("Renaming to '{name}' may cause problems:", &[("name", &r.name)]));
        });
        ui.add_space(6.0);
        egui::ScrollArea::vertical().max_height(320.0).auto_shrink([false, true]).show(ui, |ui| {
            for (i, c) in r.conflicts.iter().enumerate() {
                ui.horizontal(|ui| {
                    if let Some(path) = &c.path {
                        let shown = root.as_deref().and_then(|root| path.strip_prefix(root).ok()).unwrap_or(path).display().to_string();
                        let place = format!("{shown}:{}", c.line + 1);
                        if ui.link(egui::RichText::new(place).monospace().size(12.0)).on_hover_text(tr("Open this place")).clicked() {
                            open = Some(i);
                        }
                    }
                    ui.label(egui::RichText::new(&c.message).color(if c.path.is_some() { p.text } else { p.error }));
                });
            }
        });
        if !can_continue {
            ui.label(egui::RichText::new(tr("Nothing can be renamed until these are fixed.")).size(12.0).color(p.muted));
        }
        buttons(ui, tr("Continue Anyway"), can_continue)
    });
    if let Some(c) = open.and_then(|i| r.conflicts.get(i))
        && let Some(path) = c.path.clone()
    {
        app.open_at(&path, c.line, c.col);
    }
    if go && can_continue {
        let review = std::mem::replace(r, crate::refactor_conflicts::Review { name: String::new(), conflicts: Vec::new(), edit: None, preview: None });
        // The preview, when asked for, takes this dialog's place.
        app.continue_refactoring(review);
        return Outcome::Close;
    }
    if cancel || esc { Outcome::Close } else { Outcome::Keep }
}

/// Changes grouped by file, with a checkbox per file and per change.
fn change_list(ui: &mut Ui, changes: &mut [crate::file_rename::Change], root: Option<&std::path::Path>, muted: egui::Color32, kinds: bool) {
    egui::ScrollArea::vertical().max_height(360.0).auto_shrink([false, true]).show(ui, |ui| {
        let mut i = 0;
        while i < changes.len() {
            let path = changes[i].path.clone();
            let end = (i..changes.len()).find(|&j| changes[j].path != path).unwrap_or(changes.len());
            let shown = root.and_then(|root| path.strip_prefix(root).ok()).unwrap_or(&path).display().to_string();
            let mut all = changes[i..end].iter().all(|c| c.checked);
            if ui.checkbox(&mut all, egui::RichText::new(&shown).strong()).changed() {
                for c in &mut changes[i..end] {
                    c.checked = all;
                }
            }
            for c in &mut changes[i..end] {
                ui.horizontal(|ui| {
                    ui.add_space(22.0);
                    let label = format!("{}: {} \u{2192} {}", c.line + 1, clip(c.before.trim()), clip(c.after.trim()));
                    ui.checkbox(&mut c.checked, egui::RichText::new(label).monospace().size(12.0));
                    if kinds {
                        let kind = if c.from_server { tr("code reference") } else { tr("text match") };
                        ui.label(egui::RichText::new(kind).size(11.0).color(muted));
                    }
                });
            }
            i = end;
        }
    });
}

/// Renames on disk with the picked changes (`update`) or none.
fn finish_rename(app: &mut CrabCraftApp, r: &mut crate::file_rename::FileRename, update: bool) -> Outcome {
    let changes: &[crate::file_rename::Change] = if update { &r.changes } else { &[] };
    match crate::file_rename::apply(app, &r.from, &r.to, changes) {
        Ok(0) => Outcome::Close,
        Ok(n) => {
            app.notify(trf("Renamed; updated {n} files", &[("n", &n)]), false);
            Outcome::Close
        }
        Err(e) => {
            r.error = Some(e);
            Outcome::Keep
        }
    }
}

/// A line of code short enough for one row.
fn clip(s: &str) -> String {
    if s.chars().count() > 60 { format!("{}…", s.chars().take(59).collect::<String>()) } else { s.to_string() }
}

fn revert_disk(app: &mut CrabCraftApp, ctx: &Context, ticket: &crate::disk_revert::Ticket) -> Outcome {
    let valid = ticket.index(&app.docs).is_some();
    let ((ok, cancel), esc) = modal(ctx, "revert_disk", 520.0, |ui| {
        title(ui, tr("Revert from Disk"));
        ui.label(trf("Discard the unsaved changes to {file} and load its current disk version?", &[("file", &ticket.path.display())]));
        ui.label(tr("Undo can restore the discarded text. Disk contents are not changed."));
        if !valid {
            ui.colored_label(rc_kit::pal(ui).error, crate::disk_revert::Error::Stale.to_string());
        }
        buttons(ui, tr("Discard and Reload"), valid)
    });
    if ok {
        app.start_disk_revert(ticket.clone());
        return Outcome::Close;
    }
    if cancel || esc { Outcome::Close } else { Outcome::Keep }
}

fn confirm_delete(app: &mut CrabCraftApp, ctx: &Context, path: &mut PathBuf) -> Outcome {
    let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let ((ok, cancel), esc) = modal(ctx, "delete", 420.0, |ui| {
        title(ui, tr("Delete"));
        ui.horizontal(|ui| {
            icons::image(ui, rc_kit::ico::DIALOG_WARNING, 32.0);
            ui.label(trf("Move \"{name}\" to the Trash?", &[("name", &name)]));
        });
        buttons(ui, tr("Delete"), true)
    });
    if ok {
        // To the system trash: deleting from the IDE can be undone in Finder / Explorer.
        match trash(path) {
            Ok(()) => {
                app.explorer.invalidate(path);
                let mut i = app.docs.list.len();
                while i > 0 {
                    i -= 1;
                    if app.docs.list[i].path.starts_with(&*path) {
                        app.docs.close(i);
                    }
                }
            }
            Err(e) => app.notify(trf("Could not delete {name}: {error}", &[("name", &name), ("error", &e)]), true),
        }
        return Outcome::Close;
    }
    if cancel || esc { Outcome::Close } else { Outcome::Keep }
}

fn share_playground(app: &mut CrabCraftApp, ctx: &Context, share: &mut crate::playground::Share) -> Outcome {
    use rc_cargo::playground::{Channel, EDITIONS};
    let ((ok, cancel), esc) = modal(ctx, "share_playground", 520.0, |ui| {
        let p = rc_kit::pal(ui);
        title(ui, tr("Share in Playground"));
        ui.horizontal(|ui| {
            icons::image(ui, rc_kit::ico::DIALOG_WARNING, 32.0);
            ui.vertical(|ui| {
                ui.label(tr("The code is uploaded to play.rust-lang.org, which publishes it as a public GitHub gist. Anyone with the link can read it."));
                ui.add_space(4.0);
                let count = if share.lines == 1 { tr("1 line will be sent").to_string() } else { trf("{n} lines will be sent", &[("n", &share.lines)]) };
                ui.label(egui::RichText::new(count).strong());
            });
        });
        ui.add_space(8.0);
        egui::Frame::new().fill(p.hover).stroke(egui::Stroke::new(1.0, p.island_border)).corner_radius(4.0).inner_margin(8.0).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 2.0;
            for line in share.preview.lines().chain(share.more.then_some("…")) {
                ui.add(egui::Label::new(egui::RichText::new(line).font(FontId::monospace(12.0)).color(p.text)).truncate());
            }
        });
        ui.add_space(8.0);
        egui::Grid::new("share_playground_grid").num_columns(2).spacing(vec2(16.0, 8.0)).show(ui, |ui| {
            ui.label(tr("Channel"));
            egui::ComboBox::from_id_salt("playground_channel").selected_text(share.channel.as_str()).show_ui(ui, |ui| {
                for c in Channel::ALL {
                    ui.selectable_value(&mut share.channel, c, c.as_str());
                }
            });
            ui.end_row();
            ui.label(tr("Edition"));
            egui::ComboBox::from_id_salt("playground_edition").selected_text(share.edition.as_str()).show_ui(ui, |ui| {
                for e in EDITIONS {
                    ui.selectable_value(&mut share.edition, e.to_string(), e);
                }
            });
            ui.end_row();
        });
        buttons(ui, tr("Upload and Copy Link"), true)
    });
    if ok {
        app.playground.start(share);
        app.notify(tr("Uploading to the Rust Playground…"), false);
        return Outcome::Close;
    }
    if cancel || esc { Outcome::Close } else { Outcome::Keep }
}

fn invalidate_caches(app: &mut CrabCraftApp, ctx: &Context, clean: &mut bool) -> Outcome {
    let rust = app.runner.cargo_workspace.is_some();
    let ((ok, cancel), esc) = modal(ctx, "invalidate_caches", 460.0, |ui| {
        title(ui, tr("Invalidate Caches"));
        ui.label(tr("Open files are read from disk again, and the file index, Git status, the Cargo project and the language server are rebuilt. Tabs, terminals and running programs stay open."));
        ui.add_space(6.0);
        if rust {
            ui.checkbox(clean, tr("Also delete the build output (cargo clean)"));
        }
        buttons(ui, tr("Invalidate"), true)
    });
    if ok {
        app.invalidate_caches(rust && *clean);
        return Outcome::Close;
    }
    if cancel || esc { Outcome::Close } else { Outcome::Keep }
}

fn trash(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        let script = format!("tell application \"Finder\" to delete POSIX file \"{}\"", path.display().to_string().replace('"', "\\\""));
        let status = std::process::Command::new("osascript").arg("-e").arg(script).output()?;
        if status.status.success() {
            return Ok(());
        }
        Err(std::io::Error::other(String::from_utf8_lossy(&status.stderr).trim().to_string()))
    }
    #[cfg(not(target_os = "macos"))]
    {
        if path.is_dir() { std::fs::remove_dir_all(path) } else { std::fs::remove_file(path) }
    }
}

/// ⌘E: recent files, most recent first (the current one second, so Enter switches back).
fn recent_files(app: &mut CrabCraftApp, ctx: &Context, selected: &mut usize, filter: &mut String) -> Outcome {
    let items: Vec<PathBuf> = app
        .docs
        .recent
        .iter()
        .filter(|p| filter.is_empty() || p.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase().contains(&filter.to_lowercase())))
        .cloned()
        .collect();
    *selected = (*selected).min(items.len().saturating_sub(1));
    let p = rc_kit::theme::palette(ctx.global_style().visuals.dark_mode);
    let mut open = None;
    let (_, esc) = modal(ctx, "recent_files", 480.0, |ui| {
        title(ui, tr("Recent Files"));
        let resp = ui.add(egui::TextEdit::singleline(filter).hint_text(tr("Type to filter")).desired_width(f32::INFINITY));
        resp.request_focus();
        if resp.changed() {
            *selected = 0;
        }
        if ui.input(|i| i.key_pressed(Key::ArrowDown)) {
            *selected = (*selected + 1).min(items.len().saturating_sub(1));
        }
        if ui.input(|i| i.key_pressed(Key::ArrowUp)) {
            *selected = selected.saturating_sub(1);
        }
        if ui.input(|i| i.key_pressed(Key::Enter)) {
            open = items.get(*selected).cloned();
        }
        ui.add_space(6.0);
        if items.is_empty() {
            ui.label(egui::RichText::new(tr("No recent files")).color(p.muted));
        }
        let move_selection = ui.input(|input| input.key_pressed(Key::ArrowDown) || input.key_pressed(Key::ArrowUp));
        egui::ScrollArea::vertical().max_height(400.0).auto_shrink([false, false]).show(ui, |ui| {
            for (i, path) in items.iter().enumerate() {
                let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), egui::Sense::click());
                if i == *selected {
                    ui.painter().rect_filled(r, 4.0, p.selection);
                } else if resp.hovered() {
                    ui.painter().rect_filled(r, 4.0, p.row_hover);
                }
                icons::paint(ui.painter(), egui::Rect::from_center_size(r.left_center() + vec2(14.0, 0.0), vec2(16.0, 16.0)), icons::for_file(&name), &p, true);
                ui.painter().text(r.left_center() + vec2(30.0, 0.0), Align2::LEFT_CENTER, &name, FontId::proportional(13.0), p.text);
                let dir = path.parent().map(|d| d.display().to_string()).unwrap_or_default();
                ui.painter().text(r.right_center() - vec2(8.0, 0.0), Align2::RIGHT_CENTER, dir, FontId::proportional(11.5), p.muted);
                resp.widget_info(|| {
                    let mut info = egui::WidgetInfo::labeled(egui::Role::Button, true, path.display().to_string());
                    info.selected = Some(i == *selected);
                    info
                });

                if move_selection && i == *selected {
                    resp.scroll_to_me(Some(egui::Align::Center));
                }
                if resp.clicked() {
                    open = Some(path.clone());
                }
            }
        });
    });
    if let Some(path) = open {
        app.open_file(&path);
        return Outcome::Close;
    }
    if esc { Outcome::Close } else { Outcome::Keep }
}

/// The name a kind of scratch file has in the New Scratch File list.
pub fn scratch_kind_name(kind: rc_core::scratch::Kind) -> &'static str {
    use rc_core::scratch::Kind;
    match kind {
        Kind::Rust => "Rust",
        Kind::Toml => "TOML",
        Kind::Markdown => "Markdown",
        Kind::Json => "JSON",
        Kind::Text => tr("Plain Text"),
    }
}

/// New Scratch File: a filterable list of the kinds to create, then the existing scratch files.
fn scratches(app: &mut CrabCraftApp, ctx: &Context, selected: &mut usize, filter: &mut String, files: &[PathBuf]) -> Outcome {
    use rc_core::scratch::Kind;
    enum Row<'a> {
        New(Kind),
        Open(&'a PathBuf),
    }
    let needle = filter.to_lowercase();
    let mut rows: Vec<Row> =
        Kind::ALL.into_iter().filter(|k| needle.is_empty() || scratch_kind_name(*k).to_lowercase().contains(&needle)).map(Row::New).collect();
    rows.extend(
        files.iter().filter(|p| needle.is_empty() || p.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase().contains(&needle))).map(Row::Open),
    );
    *selected = (*selected).min(rows.len().saturating_sub(1));
    let p = rc_kit::theme::palette(ctx.global_style().visuals.dark_mode);
    let mut chosen = None;
    let (_, esc) = modal(ctx, "scratches", 480.0, |ui| {
        title(ui, tr("New Scratch File"));
        let resp = ui.add(egui::TextEdit::singleline(filter).hint_text(tr("Type to filter")).desired_width(f32::INFINITY));
        resp.request_focus();
        if ui.input(|i| i.key_pressed(Key::ArrowDown)) {
            *selected = (*selected + 1).min(rows.len().saturating_sub(1));
        }
        if ui.input(|i| i.key_pressed(Key::ArrowUp)) {
            *selected = selected.saturating_sub(1);
        }
        if ui.input(|i| i.key_pressed(Key::Enter)) && !rows.is_empty() {
            chosen = Some(*selected);
        }
        ui.add_space(6.0);
        let mut header = false;
        for (i, row) in rows.iter().enumerate().take(30) {
            if matches!(row, Row::Open(_)) && !header {
                header = true;
                ui.add_space(4.0);
                ui.label(egui::RichText::new(tr("Scratches")).size(11.5).color(p.muted));
            }
            let (icon, name) = match row {
                Row::New(k) => (icons::for_file(&format!("x.{}", k.extension())), scratch_kind_name(*k).to_string()),
                Row::Open(path) => {
                    let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                    (icons::for_file(&name), name)
                }
            };
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), egui::Sense::click());
            if i == *selected {
                ui.painter().rect_filled(r, 4.0, p.selection);
            } else if resp.hovered() {
                ui.painter().rect_filled(r, 4.0, p.row_hover);
            }
            icons::paint(ui.painter(), egui::Rect::from_center_size(r.left_center() + vec2(14.0, 0.0), vec2(16.0, 16.0)), icon, &p, true);
            ui.painter().text(r.left_center() + vec2(30.0, 0.0), Align2::LEFT_CENTER, &name, FontId::proportional(13.0), p.text);
            if resp.clicked() {
                chosen = Some(i);
            }
        }
    });
    match chosen.and_then(|i| rows.get(i)) {
        Some(Row::New(kind)) => {
            match rc_core::scratch::create(&rc_core::paths::scratches_dir(), *kind) {
                Ok(path) => app.open_file(&path),
                Err(e) => app.notify(trf("Could not create the scratch file: {error}", &[("error", &e)]), true),
            }
            Outcome::Close
        }
        Some(Row::Open(path)) => {
            app.open_file(path);
            Outcome::Close
        }
        None if esc => Outcome::Close,
        None => Outcome::Keep,
    }
}

/// The list of context actions, under the caret like IntelliJ's bulb popup.
fn code_actions(app: &mut CrabCraftApp, ctx: &Context, list: &mut Vec<rc_lsp::lsp_types::CodeActionOrCommand>, selected: &mut usize) -> Outcome {
    use rc_lsp::lsp_types::CodeActionOrCommand as A;
    let p = rc_kit::theme::palette(ctx.global_style().visuals.dark_mode);
    let pos = app.caret_screen.unwrap_or(egui::pos2(200.0, 200.0));
    let mut chosen = None;
    let mut close = false;
    let n = list.len();
    if ctx.input(|i| i.key_pressed(Key::ArrowDown)) {
        *selected = (*selected + 1) % n.max(1);
    }
    if ctx.input(|i| i.key_pressed(Key::ArrowUp)) {
        *selected = (*selected + n.max(1) - 1) % n.max(1);
    }
    if ctx.input(|i| i.key_pressed(Key::Enter)) {
        chosen = Some(*selected);
    }
    if ctx.input(|i| i.key_pressed(Key::Escape)) {
        close = true;
    }
    let area = egui::Area::new(Id::unique("code_actions")).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
        egui::Frame::NONE.fill(p.popup_bg).stroke(egui::Stroke::new(1.0, p.popup_border)).corner_radius(8).inner_margin(egui::Margin::same(4)).show(ui, |ui| {
            ui.set_min_width(320.0);
            for (i, a) in list.iter().enumerate() {
                let (title, quickfix) = match a {
                    A::Command(c) => (c.title.clone(), false),
                    A::CodeAction(c) => (c.title.clone(), c.kind.as_ref().is_some_and(|k| k.as_str().starts_with("quickfix"))),
                };
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width().max(320.0), 26.0), egui::Sense::click());
                if i == *selected {
                    ui.painter().rect_filled(r, 4.0, p.selection);
                } else if resp.hovered() {
                    ui.painter().rect_filled(r, 4.0, p.row_hover);
                }
                let icon = if quickfix { rc_kit::ico::BULB_QUICKFIX } else { rc_kit::ico::BULB };
                icons::paint(ui.painter(), egui::Rect::from_center_size(r.left_center() + vec2(14.0, 0.0), vec2(16.0, 16.0)), icon, &p, true);
                ui.painter().text(r.left_center() + vec2(30.0, 0.0), Align2::LEFT_CENTER, &title, FontId::proportional(13.0), p.text);
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &title));
                if resp.clicked() {
                    chosen = Some(i);
                }
            }
        });
    });
    // The highlighted action's changes, beside the list (I-33).
    app.update_intention_preview(list, *selected);
    if app.intention_preview.busy() {
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    }
    let preview = intention_preview(app, ctx, area.response.rect);
    // A press outside the list and its preview closes it. `hovered()` is false over the rows (they
    // are on top of the area), so the rectangles are tested: a press on a row must not close the
    // list before its click.
    let inside = |pos: egui::Pos2| area.response.rect.contains(pos) || preview.rect.contains(pos);
    if ctx.input(|i| i.pointer.any_pressed() && i.pointer.press_origin().is_some_and(|pos| !inside(pos))) && chosen.is_none() {
        close = true;
    }
    if close || chosen.is_some() {
        app.intention_preview.clear();
    }
    if let Some(i) = chosen {
        let action = list.swap_remove(i);
        // Local commands, also behind a local quick fix (a code action with only a command).
        let local = match &action {
            A::Command(c) => Some(c),
            A::CodeAction(a) if a.edit.is_none() => a.command.as_ref().filter(|c| {
                [crate::insight::UPDATE_DEPENDENCY_COMMAND, crate::insight::MANIFEST_FIX_COMMAND, crate::feature_fix::ENABLE_FEATURE_COMMAND]
                    .contains(&c.command.as_str())
            }),
            A::CodeAction(_) => None,
        };
        if let Some(c) = local {
            let ran = match c.command.as_str() {
                crate::insight::UPDATE_DEPENDENCY_COMMAND => {
                    let args = c.arguments.clone().unwrap_or_default();
                    if let (Some(line), Some(version)) = (args.first().and_then(|v| v.as_u64()), args.get(1).and_then(|v| v.as_str())) {
                        app.update_dependency(usize::try_from(line).unwrap_or(usize::MAX), version);
                    }
                    true
                }
                crate::feature_fix::ENABLE_FEATURE_COMMAND => {
                    let fix =
                        c.arguments.as_ref().and_then(|a| a.first()).and_then(|v| serde_json::from_value::<crate::feature_fix::FeatureFix>(v.clone()).ok());
                    if let Some(fix) = fix {
                        app.apply_feature_fix(fix);
                    }
                    true
                }
                crate::insight::MANIFEST_FIX_COMMAND => {
                    let args = c.arguments.clone().unwrap_or_default();
                    let num = |i: usize| args.get(i).and_then(|v| v.as_u64()).and_then(|n| usize::try_from(n).ok());
                    let text = |i: usize| args.get(i).and_then(|v| v.as_str());
                    if let (Some(start), Some(end), Some(old), Some(new), Some(doc)) = (num(0), num(1), text(2), text(3), app.docs.active_doc_mut()) {
                        let rope = doc.editor.buffer.rope();
                        // Only when the text there is still what the fix was made for.
                        if start <= end && end <= rope.len_chars() && rope.slice(start..end).to_string() == old {
                            doc.editor.replace(start, end, new);
                        }
                    }
                    true
                }
                crate::insight::RENAME_COMMAND => {
                    app.run_insight_action(rc_editor::Action::Rename, pos);
                    true
                }
                crate::insight::SELECT_IN_EXPLORER => {
                    app.locate_active();
                    true
                }
                crate::insight::DERIVE_COMMAND => {
                    if let Some(d) = app.docs.active_doc()
                        && let Some(t) = crate::insight::derive_target_of(d)
                    {
                        let mut traits: Vec<(String, bool)> =
                            rc_syntax::COMMON_DERIVES.iter().map(|c| (c.to_string(), t.items.iter().any(|x| x == c))).collect();
                        let other = t.items.iter().filter(|x| !rc_syntax::COMMON_DERIVES.contains(&x.as_str())).cloned().collect::<Vec<_>>().join(", ");
                        traits.sort_by_key(|(_, on)| !on);
                        app.dialog = Some(Dialog::Derive { doc: d.editor.id, name: t.name, traits, other });
                    }
                    return Outcome::Keep;
                }
                crate::insight::SUPPRESS_COMMAND => {
                    let args = c.arguments.clone().unwrap_or_default();
                    let arg = |i: usize| args.get(i).and_then(|v| v.as_str()).unwrap_or_default().to_string();
                    if let Some(i) = app.docs.active
                        && let Some((a, b, text)) = crate::insight::suppress_edit(&app.docs.list[i], &arg(0), &arg(1))
                    {
                        app.docs.list[i].editor.replace(a, b, &text);
                    }
                    true
                }
                crate::insight::REVEAL_FILE => {
                    if let Some(doc) = app.docs.active_doc() {
                        crate::app::reveal_in_file_manager(&doc.path);
                    }
                    true
                }
                _ => false,
            };
            if ran {
                return Outcome::Close;
            }
        }
        let mut out = Vec::new();
        app.insight.run_code_action(&mut app.docs, action, &mut out);
        app.handle_insight(out);
        return Outcome::Close;
    }
    if close { Outcome::Close } else { Outcome::Keep }
}

/// The diff of the highlighted Alt+Enter row, right of the list (or under it when the window
/// is too narrow).
fn intention_preview(app: &CrabCraftApp, ctx: &Context, list: egui::Rect) -> egui::Response {
    use crate::intention_preview::State;
    const WIDTH: f32 = 480.0;
    let dark = ctx.global_style().visuals.dark_mode;
    let (p, s) = (rc_kit::theme::palette(dark), rc_kit::theme::scheme(dark));
    let screen = ctx.content_rect();
    let pos = if list.right() + 6.0 + WIDTH <= screen.right() { list.right_top() + vec2(6.0, 0.0) } else { list.left_bottom() + vec2(0.0, 6.0) };
    let root = app.workspace.as_ref().map(|w| w.ws.root.clone());
    let state = &app.intention_preview.state;
    let area = egui::Area::new(Id::unique("intention_preview")).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
        egui::Frame::NONE.fill(p.popup_bg).stroke(egui::Stroke::new(1.0, p.popup_border)).corner_radius(8).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
            ui.set_width(WIDTH);
            let muted = |ui: &mut Ui, text: &str| {
                ui.label(egui::RichText::new(text).size(12.0).color(p.muted));
            };
            match state {
                State::Idle => {}
                State::Resolving | State::Computing => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        muted(ui, tr("Loading preview…"));
                    });
                }
                State::Note(n) => muted(ui, n),
                State::Failed(e) => {
                    ui.label(egui::RichText::new(trf("Could not preview this action: {error}", &[("error", e)])).size(12.0).color(p.error));
                }
                State::Ready { files, notes } => {
                    egui::ScrollArea::vertical()
                        .id_salt("intention_preview_scroll")
                        .max_height(360.0)
                        .min_scrolled_height(360.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            for note in notes {
                                muted(ui, note);
                            }
                            if files.is_empty() && notes.is_empty() {
                                muted(ui, tr("This action changes no text"));
                            }
                            for f in files {
                                let shown = root.as_deref().and_then(|r| f.path.strip_prefix(r).ok()).unwrap_or(&f.path);
                                let name = f.path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                                ui.horizontal(|ui| {
                                    icons::image(ui, icons::for_file(&name), 16.0);
                                    ui.label(egui::RichText::new(shown.display().to_string()).size(12.5).strong());
                                    if f.error.is_none() {
                                        ui.label(egui::RichText::new(format!("+{} −{}", f.added, f.removed)).size(12.0).color(p.muted));
                                    }
                                });
                                if let Some(e) = &f.error {
                                    ui.label(egui::RichText::new(e).size(12.0).color(p.error));
                                }
                                for (kind, line) in &f.lines {
                                    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), egui::Sense::hover());
                                    let (bg, sign, text) = match kind {
                                        '+' => (s.added_lines.gamma_multiply(0.3), "+", line.clone()),
                                        '-' => (s.deleted_lines.gamma_multiply(0.22), "-", line.clone()),
                                        _ => (p.header_bg, "", trf("Line {n}", &[("n", line)])),
                                    };
                                    ui.painter().rect_filled(rect, 0.0, bg);
                                    let color = if sign.is_empty() { p.muted } else { p.text };
                                    ui.painter().with_clip_rect(rect).text(
                                        rect.left_center() + vec2(6.0, 0.0),
                                        Align2::LEFT_CENTER,
                                        format!("{sign} {text}"),
                                        FontId::monospace(12.0),
                                        color,
                                    );
                                }
                                let shown_lines = f.lines.iter().filter(|(k, _)| *k != '@').count();
                                if shown_lines < f.added + f.removed {
                                    muted(ui, &trf("{n} more changed lines", &[("n", &(f.added + f.removed - shown_lines))]));
                                }
                                ui.add_space(6.0);
                            }
                        });
                }
            }
        });
    });
    area.response
}

fn rename_symbol(app: &mut CrabCraftApp, ctx: &Context, doc: u64, pos: usize, original: &mut String, name: &mut String, focus: &mut bool) -> Outcome {
    let valid = !name.trim().is_empty() && name.trim().chars().all(|c| c.is_alphanumeric() || c == '_' || c == '@');
    let ((ok, cancel), esc) = modal(ctx, "rename_symbol", 400.0, |ui| {
        title(ui, &trf("Rename '{name}'", &[("name", original)]));
        let resp = ui.add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY));
        if std::mem::take(focus) {
            resp.request_focus();
            // Select the whole name, like IntelliJ's rename field.
            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), resp.id) {
                let n = name.chars().count();
                state.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(n))));
                state.store(ui.ctx(), resp.id);
            }
        }
        ui.label(egui::RichText::new(tr("Renames the symbol and its usages in every project.")).size(12.0).color(rc_kit::pal(ui).muted));
        let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        ui.add_space(14.0);
        let mut res = (false, false, false);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            res.0 = ui.add_enabled_ui(valid, |ui| theme::primary_button(ui, tr("Rename"))).inner.clicked();
            res.2 = ui
                .add_enabled_ui(valid, |ui| theme::secondary_button(ui, tr("Preview")))
                .inner
                .on_hover_text(tr("List the changes first and pick the ones to make"))
                .clicked();
            res.1 = theme::secondary_button(ui, tr("Cancel")).clicked();
        });
        ((res.0 || enter && valid, res.2), res.1)
    });
    let (ok, preview) = ok;
    if ok || preview {
        if *name != *original
            && let Some(d) = app.docs.list.iter().find(|d| d.editor.id == doc)
        {
            let title = preview.then(|| trf("Rename '{old}' to '{new}'", &[("old", original), ("new", &name.trim())]));
            if let Some(insight) = app.web_languages.insight(d) {
                insight.rename(d, pos, name.trim(), title);
            } else {
                app.insight.rename(d, pos, name.trim(), title);
            }
        }
        return Outcome::Close;
    }
    if cancel || esc { Outcome::Close } else { Outcome::Keep }
}

fn recent_locations(app: &mut CrabCraftApp, ctx: &Context, selected: &mut usize) -> Outcome {
    // Character positions remain meaningful when a file was closed; do not invent line 1
    // or discard the column merely because its preview buffer is no longer open.
    let items: Vec<(PathBuf, usize, Option<(usize, usize)>, String)> = app
        .nav_back
        .iter()
        .rev()
        .take(30)
        .map(|(path, position)| {
            let preview = app.docs.index_of(path).map(|index| {
                let editor = &app.docs.list[index].editor;
                let location = editor.buffer.line_col((*position).min(editor.buffer.len_chars()));
                (location, editor.buffer.line_text(location.0))
            });
            let (location, text) = match preview {
                Some((location, text)) => (Some(location), text),
                None => (None, path.display().to_string()),
            };
            (path.clone(), *position, location, text)
        })
        .collect();
    *selected = (*selected).min(items.len().saturating_sub(1));
    let p = rc_kit::theme::palette(ctx.global_style().visuals.dark_mode);
    let mut open = None;
    let (_, esc) = modal(ctx, "recent_locations", 620.0, |ui| {
        title(ui, tr("Recent Locations"));
        if ui.input(|input| input.key_pressed(Key::ArrowDown)) {
            *selected = (*selected + 1).min(items.len().saturating_sub(1));
        }
        if ui.input(|input| input.key_pressed(Key::ArrowUp)) {
            *selected = selected.saturating_sub(1);
        }
        if ui.input(|input| input.key_pressed(Key::Enter)) {
            open = items.get(*selected).map(|(path, position, _, _)| (path.clone(), *position));
        }
        let move_selection = ui.input(|input| input.key_pressed(Key::ArrowDown) || input.key_pressed(Key::ArrowUp));
        egui::ScrollArea::vertical().max_height(400.0).auto_shrink([false, false]).show(ui, |ui| {
            for (index, (path, position, location, text)) in items.iter().enumerate() {
                let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), egui::Sense::click());
                if index == *selected {
                    ui.painter().rect_filled(rect, 6.0, p.selection);
                } else if response.hovered() {
                    ui.painter().rect_filled(rect, 6.0, p.row_hover);
                }
                let name = path.file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned());
                let caption = location.map_or_else(|| name.clone(), |(line, column)| format!("{name}:{}:{}", line + 1, column + 1));
                icons::paint(
                    ui.painter(),
                    egui::Rect::from_center_size(rect.left_top() + vec2(14.0, 13.0), vec2(16.0, 16.0)),
                    icons::for_file(&name),
                    &p,
                    true,
                );
                ui.painter().text(rect.left_top() + vec2(30.0, 13.0), Align2::LEFT_CENTER, &caption, theme::bold(12.5), p.text);
                ui.painter().text(rect.left_top() + vec2(30.0, 31.0), Align2::LEFT_CENTER, text.trim(), FontId::monospace(12.0), p.text_muted);
                response.widget_info(|| {
                    let mut info = egui::WidgetInfo::labeled(egui::Role::Button, true, format!("{} · {caption}", path.display()));
                    info.selected = Some(index == *selected);
                    info
                });
                if move_selection && index == *selected {
                    response.scroll_to_me(Some(egui::Align::Center));
                }
                if response.clicked() {
                    open = Some((path.clone(), *position));
                }
            }
        });
    });
    if let Some((path, position)) = open {
        app.request_open_offset(&path, position);
        return Outcome::Close;
    }
    if esc { Outcome::Close } else { Outcome::Keep }
}

fn rollback(app: &mut CrabCraftApp, ctx: &Context, paths: &mut Vec<String>) -> Outcome {
    let ((ok, cancel), esc) = modal(ctx, "rollback", 440.0, |ui| {
        title(ui, tr("Rollback Changes"));
        ui.label(trf("Discard the changes of {n} files? This cannot be undone.", &[("n", &paths.len())]));
        for p in paths.iter().take(8) {
            ui.label(egui::RichText::new(p).size(12.0).color(rc_kit::pal(ui).muted));
        }
        buttons(ui, tr("Rollback"), true)
    });
    if ok {
        app.vcs.rollback(std::mem::take(paths));
        return Outcome::Close;
    }
    if cancel || esc { Outcome::Close } else { Outcome::Keep }
}

fn new_branch(app: &mut CrabCraftApp, ctx: &Context, name: &mut String) -> Outcome {
    let valid = !name.trim().is_empty() && !name.contains(' ') && !name.contains("..");
    let ((ok, cancel), esc) = modal(ctx, "new_branch", 380.0, |ui| {
        title(ui, tr("New Branch"));
        let resp = ui.add(egui::TextEdit::singleline(name).hint_text(tr("Branch name")).desired_width(f32::INFINITY));
        resp.request_focus();
        ui.label(egui::RichText::new(tr("The new branch is checked out.")).size(12.0).color(rc_kit::pal(ui).muted));
        let enter = ui.input(|i| i.key_pressed(Key::Enter));
        let (ok, cancel) = buttons(ui, tr("Create"), valid);
        (ok || enter && valid, cancel)
    });
    if ok {
        app.vcs.new_branch(name.trim());
        return Outcome::Close;
    }
    if cancel || esc { Outcome::Close } else { Outcome::Keep }
}

/// Cheap checks of a tag name while it is typed; git's `check-ref-format` has the last word
/// when the tag is created.
pub fn plausible_tag_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(['-', '.', '/'])
        && !name.ends_with(['.', '/'])
        && !name.ends_with(".lock")
        && !name.contains("..")
        && !name.contains("@{")
        && !name.contains("//")
        && name != "@"
        && !name.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '~' | '^' | ':' | '?' | '*' | '[' | '\\'))
}

fn new_tag(app: &mut CrabCraftApp, ctx: &Context, name: &mut String, target: Option<&(String, String)>, message: &mut String) -> Outcome {
    let valid = plausible_tag_name(name.trim());
    let ((ok, cancel), esc) = modal(ctx, "new_tag", 420.0, |ui| {
        title(ui, tr("New Tag"));
        let resp = ui.add(egui::TextEdit::singleline(name).hint_text(tr("Tag name")).desired_width(f32::INFINITY));
        if !ui.memory(|m| m.has_focus(resp.id)) && message.is_empty() && name.is_empty() {
            resp.request_focus();
        }
        let muted = rc_kit::pal(ui).muted;
        let on = match target {
            Some((_, short)) => trf("On commit {hash}", &[("hash", short)]),
            None => tr("On HEAD").to_string(),
        };
        ui.label(egui::RichText::new(on).size(12.0).color(muted));
        if !name.trim().is_empty() && !valid {
            ui.label(egui::RichText::new(tr("This is not a valid tag name")).size(12.0).color(rc_kit::pal(ui).error));
        }
        ui.add_space(6.0);
        ui.add(egui::TextEdit::multiline(message).hint_text(tr("Message (optional)")).desired_rows(3).desired_width(f32::INFINITY));
        ui.label(egui::RichText::new(tr("With a message the tag is annotated; without one it is lightweight.")).size(12.0).color(muted));
        buttons(ui, tr("Create"), valid)
    });
    if ok && valid {
        app.vcs.create_tag(name.trim(), target.map(|(h, _)| h.clone()), Some(message.clone()));
        return Outcome::Close;
    }
    if cancel || esc { Outcome::Close } else { Outcome::Keep }
}

fn delete_tag(app: &mut CrabCraftApp, ctx: &Context, name: &str) -> Outcome {
    let ((ok, cancel), esc) = modal(ctx, "delete_tag", 420.0, |ui| {
        title(ui, tr("Delete Tag"));
        ui.label(trf("Delete the tag {name}? Copies of it on remotes are kept.", &[("name", &name)]));
        buttons(ui, tr("Delete"), true)
    });
    if ok {
        app.vcs.delete_tag(name);
        return Outcome::Close;
    }
    if cancel || esc { Outcome::Close } else { Outcome::Keep }
}

fn checkout_tag(app: &mut CrabCraftApp, ctx: &Context, name: &str) -> Outcome {
    let ((ok, cancel), esc) = modal(ctx, "checkout_tag", 440.0, |ui| {
        title(ui, tr("Checkout Tag"));
        ui.horizontal(|ui| {
            icons::image(ui, rc_kit::ico::DIALOG_WARNING, 20.0);
            ui.label(trf("Checking out {name} detaches HEAD: new commits will not belong to any branch until you create one.", &[("name", &name)]));
        });
        buttons(ui, tr("Checkout"), true)
    });
    if ok {
        app.vcs.checkout_tag(name);
        return Outcome::Close;
    }
    if cancel || esc { Outcome::Close } else { Outcome::Keep }
}

fn push_tags(app: &mut CrabCraftApp, ctx: &Context, name: Option<&str>, remote: &mut String) -> Outcome {
    let remotes = app.vcs.remotes.clone();
    let valid = remotes.contains(remote);
    let ((ok, cancel), esc) = modal(ctx, "push_tags", 420.0, |ui| {
        title(ui, if name.is_some() { tr("Push Tag") } else { tr("Push All Tags") });
        ui.label(match name {
            Some(n) => trf("Push the tag {name} to:", &[("name", &n)]),
            None => tr("Push every local tag to:").to_string(),
        });
        egui::ComboBox::from_id_salt("push_tags_remote").selected_text(remote.as_str()).show_ui(ui, |ui| {
            for r in &remotes {
                ui.selectable_value(remote, r.clone(), r.as_str());
            }
        });
        ui.label(egui::RichText::new(tr("The repository's pre-push hook, if any, runs.")).size(12.0).color(rc_kit::pal(ui).muted));
        buttons(ui, tr("Push"), valid)
    });
    if ok && valid {
        app.vcs.push_tags(remote, name.map(str::to_string));
        return Outcome::Close;
    }
    if cancel || esc { Outcome::Close } else { Outcome::Keep }
}

fn stash_changes(
    app: &mut CrabCraftApp,
    ctx: &Context,
    message: &mut String,
    include_untracked: &mut bool,
    keep_index: &mut bool,
    paths: &[String],
) -> Outcome {
    let ((ok, cancel), esc) = modal(ctx, "stash_changes", 420.0, |ui| {
        title(ui, tr("Stash Changes"));
        if !paths.is_empty() {
            ui.label(trf("Only the {n} checked files will be stashed.", &[("n", &paths.len())]));
            egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
                for path in paths {
                    ui.label(path);
                }
            });
        }
        let resp = ui.add(egui::TextEdit::singleline(message).hint_text(tr("Message (optional)")).desired_width(f32::INFINITY));
        if message.is_empty() && !ui.memory(|m| m.focused().is_some()) {
            resp.request_focus();
        }
        ui.add_space(6.0);
        ui.checkbox(include_untracked, tr("Include untracked files"));
        ui.checkbox(keep_index, tr("Keep staged changes in the work tree"));
        buttons(ui, tr("Stash"), true)
    });
    if ok {
        if paths.is_empty() {
            app.vcs.stash_push(message, *include_untracked, *keep_index);
        } else {
            app.vcs.stash_push_paths(message, *include_untracked, *keep_index, paths.to_vec());
        }
        return Outcome::Close;
    }
    if cancel || esc { Outcome::Close } else { Outcome::Keep }
}

/// Apply Patch: pick a file or paste a patch, see the files it touches and whether it applies,
/// then apply it (nothing changes before Apply is pressed) and read what git printed.
fn apply_patch(app: &mut CrabCraftApp, ctx: &Context, path: &mut String, paste: &mut bool) -> Outcome {
    // Text from the clipboard: asked for with the button, or Ctrl+V outside the path field.
    if *paste || ctx.memory(|m| m.focused().is_none()) {
        let pasted = ctx.input(|i| i.events.iter().find_map(|e| if let egui::Event::Paste(t) = e { Some(t.clone()) } else { None }));
        if let Some(text) = pasted {
            *paste = false;
            path.clear();
            app.vcs.load_patch_text(text);
        }
    }
    let busy = app.vcs.busy.is_some();
    let mut load = None;
    let mut recheck = false;
    let mut apply = false;
    let mut close = false;
    let (_, esc) = modal(ctx, "apply_patch", 600.0, |ui| {
        let p = rc_kit::pal(ui);
        title(ui, tr("Apply Patch"));
        ui.horizontal(|ui| {
            let resp = ui.add(egui::TextEdit::singleline(path).hint_text(tr("Patch file")).desired_width(300.0));
            if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) && !path.trim().is_empty() {
                load = Some(PathBuf::from(path.trim()));
            }
            if theme::secondary_button(ui, tr("Browse…")).clicked() {
                let mut d = rfd::FileDialog::new().set_title(tr("Apply Patch")).add_filter(tr("Patch files"), &["patch", "diff", "eml", "mbox"]);
                if let Some(root) = app.vcs.root() {
                    d = d.set_directory(root);
                }
                if let Some(picked) = d.pick_file() {
                    *path = picked.display().to_string();
                    load = Some(picked);
                }
            }
            if theme::secondary_button(ui, tr("Paste from Clipboard")).clicked() {
                *paste = true;
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::RequestPaste);
            }
        });
        ui.add_space(8.0);
        let checking = |ui: &mut Ui| {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0));
                ui.label(egui::RichText::new(tr("Checking the patch…")).color(p.muted));
            });
        };
        if app.vcs.patch_loading {
            checking(ui);
        }
        if let Some(e) = &app.vcs.patch_error {
            ui.label(egui::RichText::new(e).color(p.error).size(12.5));
        }
        let mut applied = false;
        if let Some(patch) = &app.vcs.patch {
            let from = match &patch.path {
                Some(f) => trf("From {file}", &[("file", &f.display())]),
                None => tr("From the clipboard").to_string(),
            };
            ui.label(egui::RichText::new(from).size(12.0).color(p.muted));
            match &patch.preview {
                None => checking(ui),
                Some(Err(e)) => {
                    ui.label(egui::RichText::new(e).color(p.error).size(12.5));
                }
                Some(Ok(pv)) => {
                    let kind = match pv.kind {
                        rc_vcs::PatchKind::Mail => {
                            trf("{n} commits (git am): each one becomes a commit on the current branch; the repository's hooks run.", &[("n", &pv.commits)])
                        }
                        rc_vcs::PatchKind::Diff => tr("Changes to the files of the work tree (git apply); nothing is committed.").to_string(),
                    };
                    ui.label(egui::RichText::new(kind).size(12.5));
                    ui.add_space(4.0);
                    egui::ScrollArea::vertical().id_salt("patch_files").max_height(180.0).auto_shrink([false, true]).show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        for f in &pv.files {
                            ui.horizontal(|ui| {
                                let (status, color) = match f.status {
                                    'A' => (tr("added"), p.ok),
                                    'D' => (tr("deleted"), p.muted),
                                    'R' => (tr("renamed"), p.accent),
                                    _ => (tr("modified"), p.accent),
                                };
                                ui.label(egui::RichText::new(&f.path).size(12.5));
                                ui.label(egui::RichText::new(status).size(11.5).color(color));
                                let lines = match (f.added, f.removed) {
                                    (Some(a), Some(r)) => format!("+{a} −{r}"),
                                    _ => tr("binary").to_string(),
                                };
                                ui.label(egui::RichText::new(lines).font(FontId::monospace(11.5)).color(p.muted));
                            });
                        }
                    });
                    ui.add_space(4.0);
                    match &pv.problem {
                        None => {
                            ui.label(egui::RichText::new(tr("The patch applies cleanly.")).size(12.5).color(p.ok));
                        }
                        Some(msg) => {
                            ui.label(egui::RichText::new(tr("The patch does not apply as it is:")).size(12.5).color(p.error));
                            egui::ScrollArea::vertical().id_salt("patch_problem").max_height(80.0).auto_shrink([false, true]).show(ui, |ui| {
                                ui.label(egui::RichText::new(msg).font(FontId::monospace(11.5)).color(p.error));
                            });
                        }
                    }
                }
            }
            if let Some(r) = &patch.result {
                applied = !r.error;
                ui.add_space(6.0);
                ui.label(egui::RichText::new(tr("Git output")).size(11.5).color(p.muted));
                egui::ScrollArea::vertical().id_salt("patch_output").max_height(120.0).auto_shrink([false, true]).show(ui, |ui| {
                    let text = if r.text.trim().is_empty() { tr("Patch applied") } else { r.text.as_str() };
                    ui.label(egui::RichText::new(text).font(FontId::monospace(11.5)).color(if r.error { p.error } else { p.text }));
                });
            }
        }
        ui.add_space(6.0);
        if ui.add_enabled(!busy, egui::Checkbox::new(&mut app.vcs.patch_three_way, tr("Fall back to a three-way merge (--3way)"))).changed() {
            recheck = true;
        }
        let ready = !busy && !applied && app.vcs.patch.as_ref().is_some_and(|p| matches!(p.preview, Some(Ok(_))));
        ui.add_space(14.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if !applied {
                apply = ui.add_enabled_ui(ready, |ui| theme::primary_button(ui, tr("Apply"))).inner.clicked();
            }
            close = theme::secondary_button(ui, if applied { tr("Close") } else { tr("Cancel") }).clicked();
            if app.vcs.busy == Some("patch_apply") {
                ui.add(egui::Spinner::new().size(14.0));
            }
        });
    });
    if let Some(file) = load {
        app.vcs.load_patch_file(file);
    } else if recheck {
        app.vcs.recheck_patch();
    }
    if apply {
        app.vcs.apply_patch();
    }
    if close || esc {
        app.vcs.close_patch();
        return Outcome::Close;
    }
    Outcome::Keep
}

fn stash_branch(app: &mut CrabCraftApp, ctx: &Context, stash: &rc_vcs::Stash, name: &mut String) -> Outcome {
    let valid = plausible_branch_name(name.trim()) && app.vcs.busy.is_none();
    let ((ok, cancel), esc) = modal(ctx, "stash_branch", 460.0, |ui| {
        title(ui, tr("Create Branch from Stash"));
        ui.label(&stash.message);
        ui.label(tr("Switches to a new branch at the stash's original commit and restores its changes. The stash is kept."));
        let response = ui.add(egui::TextEdit::singleline(name).hint_text(tr("Branch name")).desired_width(f32::INFINITY));
        if name.is_empty() && !ui.memory(|m| m.focused().is_some()) {
            response.request_focus();
        }
        buttons(ui, tr("Create"), valid)
    });
    if ok && valid {
        app.vcs.stash_branch(stash, name.trim());
        return Outcome::Close;
    }
    if cancel || esc { Outcome::Close } else { Outcome::Keep }
}

fn drop_stash(app: &mut CrabCraftApp, ctx: &Context, stash: &rc_vcs::Stash) -> Outcome {
    let ((ok, cancel), esc) = modal(ctx, "drop_stash", 440.0, |ui| {
        title(ui, tr("Drop Stash"));
        ui.label(trf("Drop {name} ({message})? Its changes are lost.", &[("name", &format!("stash@{{{}}}", stash.index)), ("message", &stash.message)]));
        buttons(ui, tr("Drop"), true)
    });
    if ok {
        app.vcs.stash_drop(stash);
        return Outcome::Close;
    }
    if cancel || esc { Outcome::Close } else { Outcome::Keep }
}

/// Where a worktree for `branch` goes by default: next to the main worktree, named after the
/// repository and the branch (`shop` + `feature/login` → `../shop-feature-login`).
pub fn suggested_worktree_path(main: &std::path::Path, branch: &str) -> PathBuf {
    let repo = main.file_name().map_or_else(|| "worktree".to_string(), |n| n.to_string_lossy().into_owned());
    let branch: String = branch.trim().chars().map(|c| if c.is_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '-' }).collect();
    let branch = branch.trim_matches(['-', '.']);
    let name = if branch.is_empty() { format!("{repo}-worktree") } else { format!("{repo}-{branch}") };
    main.parent().unwrap_or(main).join(name)
}

/// A branch name worth sending to git (which has the last word with `check-ref-format`).
pub fn plausible_branch_name(name: &str) -> bool {
    plausible_tag_name(name) && name != "HEAD"
}

fn add_worktree(
    app: &mut CrabCraftApp,
    ctx: &Context,
    path: &mut String,
    new_branch: &mut bool,
    name: &mut String,
    existing: &mut String,
    path_edited: &mut bool,
) -> Outcome {
    let main = app.vcs.worktrees.first().map(|w| w.path.clone()).or_else(|| app.vcs.root().map(std::path::Path::to_path_buf)).unwrap_or_default();
    // Local branches that no worktree has checked out.
    let free: Vec<String> = app
        .vcs
        .branches
        .iter()
        .filter(|b| b.kind == rc_vcs::BranchKind::Local && !app.vcs.worktrees.iter().any(|w| w.branch.as_deref() == Some(b.name.as_str())))
        .map(|b| b.name.clone())
        .collect();
    if existing.is_empty() || !free.contains(existing) {
        *existing = free.first().cloned().unwrap_or_default();
    }
    let branch = if *new_branch { name.trim().to_string() } else { existing.clone() };
    if !*path_edited {
        *path = if branch.is_empty() { String::new() } else { suggested_worktree_path(&main, &branch).display().to_string() };
    }
    let target = PathBuf::from(path.trim());
    let branch_ok = if *new_branch { plausible_branch_name(&branch) } else { !branch.is_empty() };
    let path_ok = target.is_absolute();
    let valid = branch_ok && path_ok;
    let ((ok, cancel), esc) = modal(ctx, "add_worktree", 480.0, |ui| {
        title(ui, tr("Add Worktree"));
        let p = rc_kit::pal(ui);
        ui.horizontal(|ui| {
            ui.radio_value(new_branch, true, tr("New branch"));
            ui.add_enabled_ui(!free.is_empty(), |ui| ui.radio_value(new_branch, false, tr("Existing branch")))
                .response
                .on_disabled_hover_text(tr("Every local branch is already checked out in a worktree"));
        });
        if *new_branch {
            let resp = ui.add(egui::TextEdit::singleline(name).hint_text(tr("Branch name")).desired_width(f32::INFINITY));
            if name.is_empty() && !ui.memory(|m| m.focused().is_some()) {
                resp.request_focus();
            }
            ui.label(egui::RichText::new(tr("Starts at the current HEAD.")).size(12.0).color(p.muted));
            if !name.trim().is_empty() && !branch_ok {
                ui.label(egui::RichText::new(tr("This is not a valid branch name")).size(12.0).color(p.error));
            }
        } else {
            egui::ComboBox::from_id_salt("worktree_branch").width(ui.available_width()).selected_text(existing.as_str()).show_ui(ui, |ui| {
                for b in &free {
                    ui.selectable_value(existing, b.clone(), b.as_str());
                }
            });
        }
        ui.add_space(8.0);
        ui.label(tr("Folder"));
        ui.horizontal(|ui| {
            let edit = ui.add(egui::TextEdit::singleline(path).hint_text(tr("A new or empty folder")).desired_width(ui.available_width() - 30.0));
            if edit.changed() {
                *path_edited = true;
            }
            if theme::icon_button(ui, rc_kit::ico::OPEN, true, tr("Choose a folder")).clicked()
                && let Some(d) = rfd::FileDialog::new().set_directory(main.parent().unwrap_or(&main)).pick_folder()
            {
                // An empty folder is used as it is; inside another one a new folder is made.
                let empty = std::fs::read_dir(&d).is_ok_and(|mut e| e.next().is_none());
                *path = if empty || branch.is_empty() { d } else { d.join(suggested_worktree_path(&main, &branch).file_name().unwrap_or_default()) }
                    .display()
                    .to_string();
                *path_edited = true;
            }
        });
        if !path.trim().is_empty() && !path_ok {
            ui.label(egui::RichText::new(tr("Enter an absolute path")).size(12.0).color(p.error));
        }
        buttons(ui, tr("Add"), valid)
    });
    if ok && valid {
        let what = if *new_branch { rc_vcs::WorktreeCheckout::NewBranch { name: branch, start: None } } else { rc_vcs::WorktreeCheckout::Branch(branch) };
        app.vcs.add_worktree(target, what);
        return Outcome::Close;
    }
    if cancel || esc { Outcome::Close } else { Outcome::Keep }
}

fn remove_worktree(app: &mut CrabCraftApp, ctx: &Context, path: &std::path::Path, force: bool) -> Outcome {
    let ((ok, cancel), esc) = modal(ctx, "remove_worktree", 460.0, |ui| {
        title(ui, tr("Remove Worktree"));
        if force {
            ui.horizontal(|ui| {
                icons::image(ui, rc_kit::ico::DIALOG_WARNING, 20.0);
                ui.label(trf("{path} has modified or untracked files. Remove it anyway? Those changes are lost.", &[("path", &path.display())]));
            });
            buttons(ui, tr("Remove Anyway"), true)
        } else {
            ui.label(trf("Remove the worktree {path} and delete its folder? Its branch is kept.", &[("path", &path.display())]));
            buttons(ui, tr("Remove"), true)
        }
    });
    if ok {
        app.vcs.remove_worktree(path, force);
        return Outcome::Close;
    }
    if cancel || esc { Outcome::Close } else { Outcome::Keep }
}

fn lock_worktree(app: &mut CrabCraftApp, ctx: &Context, path: &std::path::Path, reason: &mut String) -> Outcome {
    let ((ok, cancel), esc) = modal(ctx, "lock_worktree", 440.0, |ui| {
        title(ui, tr("Lock Worktree"));
        ui.label(trf("Lock {path} so that prune and remove leave it alone (for a folder on a removable disk, say).", &[("path", &path.display())]));
        ui.add_space(6.0);
        let resp = ui.add(egui::TextEdit::singleline(reason).hint_text(tr("Reason (optional)")).desired_width(f32::INFINITY));
        if reason.is_empty() && !ui.memory(|m| m.focused().is_some()) {
            resp.request_focus();
        }
        buttons(ui, tr("Lock"), true)
    });
    if ok {
        app.vcs.lock_worktree(path, reason);
        return Outcome::Close;
    }
    if cancel || esc { Outcome::Close } else { Outcome::Keep }
}

fn prune_worktrees(app: &mut CrabCraftApp, ctx: &Context, paths: &[PathBuf]) -> Outcome {
    let ((ok, cancel), esc) = modal(ctx, "prune_worktrees", 460.0, |ui| {
        title(ui, tr("Prune Worktrees"));
        ui.label(tr("Forget these worktrees, whose folders no longer exist? Their branches are kept."));
        ui.add_space(4.0);
        egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
            for p in paths {
                ui.label(egui::RichText::new(p.display().to_string()).font(FontId::monospace(12.0)));
            }
        });
        buttons(ui, tr("Prune"), true)
    });
    if ok {
        app.vcs.prune_worktrees();
        return Outcome::Close;
    }
    if cancel || esc { Outcome::Close } else { Outcome::Keep }
}

/// The branches a merge or rebase can take, local ones first, without the current branch.
fn other_branches(app: &CrabCraftApp) -> Vec<String> {
    let mut list: Vec<&rc_vcs::Branch> = app.vcs.branches.iter().filter(|b| !b.is_head && !b.name.ends_with("/HEAD")).collect();
    list.sort_by_key(|b| b.kind != rc_vcs::BranchKind::Local);
    list.into_iter().map(|b| b.name.clone()).collect()
}

/// A branch picker: the branches in a list, or a commit given by the caller.
fn rev_picker(ui: &mut Ui, id: &str, rev: &mut String, branches: &[String]) {
    let shown = if rev.is_empty() { tr("Select a branch").to_string() } else { rev.clone() };
    egui::ComboBox::from_id_salt(id).selected_text(shown).width(ui.available_width() - 8.0).height(320.0).show_ui(ui, |ui| {
        for b in branches {
            ui.selectable_value(rev, b.clone(), b.as_str());
        }
    });
}

/// Merge a branch or commit into the current branch (V-07).
fn merge_dialog(
    app: &mut CrabCraftApp,
    ctx: &Context,
    rev: &mut String,
    fast_forward: &mut rc_vcs::FastForward,
    squash: &mut bool,
    no_commit: &mut bool,
) -> Outcome {
    use rc_vcs::FastForward as Ff;
    let branches = other_branches(app);
    let current = app.vcs.branch_name().unwrap_or_default();
    let p = rc_kit::theme::palette(ctx.global_style().visuals.dark_mode);
    let ((ok, cancel), esc) = modal(ctx, "merge", 460.0, |ui| {
        title(ui, tr("Merge"));
        ui.label(trf("Merge into {branch}:", &[("branch", &current)]));
        rev_picker(ui, "merge_rev", rev, &branches);
        ui.add_space(8.0);
        ui.radio_value(fast_forward, Ff::Allow, tr("Fast-forward when possible"));
        ui.radio_value(fast_forward, Ff::Never, tr("Always create a merge commit (--no-ff)"));
        ui.radio_value(fast_forward, Ff::Only, tr("Only fast-forward (--ff-only)"));
        ui.add_space(4.0);
        ui.checkbox(squash, tr("Squash the changes into the index, without a merge commit (--squash)"));
        ui.checkbox(no_commit, tr("Stop before committing (--no-commit)"));
        let clash = *squash && *fast_forward == Ff::Never;
        if clash {
            ui.label(egui::RichText::new(tr("--squash cannot be combined with --no-ff")).color(p.error).size(12.5));
        }
        buttons(ui, tr("Merge"), !rev.trim().is_empty() && !clash)
    });
    if ok {
        let opts = rc_vcs::MergeOptions { fast_forward: *fast_forward, squash: *squash, no_commit: *no_commit };
        app.vcs.merge(rev.trim(), opts);
        return Outcome::Close;
    }
    if cancel || esc { Outcome::Close } else { Outcome::Keep }
}

/// Rebase the current branch onto a branch or commit (V-07).
fn rebase_dialog(app: &mut CrabCraftApp, ctx: &Context, onto: &mut String, autostash: &mut bool) -> Outcome {
    let branches = other_branches(app);
    let current = app.vcs.branch_name().unwrap_or_default();
    let ((ok, cancel), esc) = modal(ctx, "rebase", 460.0, |ui| {
        title(ui, tr("Rebase"));
        ui.label(trf("Rebase {branch} onto:", &[("branch", &current)]));
        rev_picker(ui, "rebase_onto", onto, &branches);
        ui.add_space(8.0);
        ui.checkbox(autostash, tr("Stash local changes first and restore them afterwards (--autostash)"));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            icons::image(ui, rc_kit::ico::DIALOG_WARNING, 20.0);
            ui.label(trf("The commits of {branch} are rewritten. Do not rebase commits that others already have.", &[("branch", &current)]));
        });
        buttons(ui, tr("Rebase"), !onto.trim().is_empty())
    });
    if ok {
        app.vcs.rebase(onto.trim(), *autostash);
        return Outcome::Close;
    }
    if cancel || esc { Outcome::Close } else { Outcome::Keep }
}

/// Confirm abandoning the merge, rebase, cherry-pick or revert in progress.
fn abort_operation(app: &mut CrabCraftApp, ctx: &Context) -> Outcome {
    let state = app.vcs.operation.state;
    let ((ok, cancel), esc) = modal(ctx, "abort_operation", 460.0, |ui| {
        title(ui, tr("Abort"));
        ui.label(crate::ui::git::operation_text(&app.vcs));
        ui.add_space(4.0);
        ui.label(match state {
            Some(rc_vcs::RepoState::Rebasing) => {
                tr("Abort the rebase? The branch goes back to where it was before it started, and the conflicts resolved so far are lost.")
            }
            Some(rc_vcs::RepoState::CherryPicking) => {
                tr("Abort the cherry-pick? The branch goes back to where it was before it started, and the conflicts resolved so far are lost.")
            }
            Some(rc_vcs::RepoState::Reverting) => {
                tr("Abort the revert? The branch goes back to where it was before it started, and the conflicts resolved so far are lost.")
            }
            _ => tr("Abort the merge? The branch goes back to where it was before it started, and the conflicts resolved so far are lost."),
        });
        buttons(ui, tr("Abort"), state.is_some())
    });
    if ok {
        app.vcs.abort_operation();
        return Outcome::Close;
    }
    if cancel || esc || state.is_none() { Outcome::Close } else { Outcome::Keep }
}

/// The conflicted files of the operation in progress: take one side for whole files, or open
/// them to edit and mark them resolved.
fn conflicts_dialog(app: &mut CrabCraftApp, ctx: &Context, selected: &mut std::collections::BTreeSet<String>) -> Outcome {
    let entries: Vec<(String, Option<rc_vcs::ConflictKind>)> =
        app.vcs.status.as_ref().map(|s| s.conflicts().map(|e| (e.path.clone(), e.conflict)).collect()).unwrap_or_default();
    selected.retain(|p| entries.iter().any(|(e, _)| e == p));
    let busy = app.vcs.busy.is_some();
    let (ours, theirs) = crate::ui::git::side_names(&app.vcs);
    let p = rc_kit::theme::palette(ctx.global_style().visuals.dark_mode);
    let mut action: Option<ConflictAction> = None;
    let (close, esc) = modal(ctx, "conflicts", 560.0, |ui| {
        title(ui, tr("Conflicts"));
        if entries.is_empty() {
            ui.label(egui::RichText::new(tr("Every conflict is resolved.")).color(p.ok));
        } else {
            ui.label(egui::RichText::new(format!("{ours}  ↔  {theirs}")).size(12.5).color(p.muted));
            ui.add_space(4.0);
            egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                for (path, kind) in &entries {
                    ui.horizontal(|ui| {
                        let mut on = selected.contains(path);
                        let check = ui.checkbox(&mut on, path.as_str());
                        if check.double_clicked() && !busy {
                            action = Some(ConflictAction::Merge(path.clone()));
                        }
                        if check.changed() {
                            if on {
                                selected.insert(path.clone());
                            } else {
                                selected.remove(path);
                            }
                        }
                        ui.label(egui::RichText::new(conflict_kind(*kind)).size(12.0).color(p.muted));
                    });
                }
            });
        }
        ui.add_space(10.0);
        let targets: Vec<String> = if selected.is_empty() { entries.iter().map(|(p, _)| p.clone()).collect() } else { selected.iter().cloned().collect() };
        let some = !targets.is_empty() && !busy;
        ui.horizontal_wrapped(|ui| {
            // V-08: the three-pane merge tool, for one file.
            let single = (targets.len() == 1 && !busy).then(|| targets[0].clone());
            let merge = ui.add_enabled_ui(single.is_some(), |ui| theme::primary_button(ui, tr("Merge…"))).inner;
            if single.is_none() {
                merge.on_disabled_hover_text(tr("Select one file"));
            } else if merge.on_hover_text(tr("Resolve the file's changes side by side")).clicked() {
                action = single.map(ConflictAction::Merge);
            }
            if ui.add_enabled_ui(some, |ui| theme::secondary_button(ui, &trf("Accept {side}", &[("side", &ours)]))).inner.clicked() {
                action = Some(ConflictAction::Take(targets.clone(), false));
            }
            if ui.add_enabled_ui(some, |ui| theme::secondary_button(ui, &trf("Accept {side}", &[("side", &theirs)]))).inner.clicked() {
                action = Some(ConflictAction::Take(targets.clone(), true));
            }
            let one = selected.len() == 1 && !busy;
            if ui.add_enabled_ui(one, |ui| theme::secondary_button(ui, tr("Open"))).inner.on_hover_text(tr("Edit the file with its conflict markers")).clicked()
            {
                action = Some(ConflictAction::Open(targets[0].clone()));
            }
            if ui
                .add_enabled_ui(some, |ui| theme::secondary_button(ui, tr("Mark Resolved")))
                .inner
                .on_hover_text(tr("Stage the files as they are now"))
                .clicked()
            {
                action = Some(ConflictAction::MarkResolved(targets.clone()));
            }
        });
        if selected.is_empty() && !entries.is_empty() {
            ui.label(egui::RichText::new(tr("Without a selection, the buttons act on every file.")).size(12.0).color(p.muted));
        }
        ui.add_space(10.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| theme::primary_button(ui, tr("Close")).clicked()).inner
    });
    match action {
        Some(ConflictAction::Take(paths, theirs)) => app.vcs.resolve_conflicts(paths, theirs),
        Some(ConflictAction::MarkResolved(paths)) => app.vcs.mark_resolved(paths),
        Some(ConflictAction::Merge(path)) => {
            app.vcs.open_merge_tool(&path);
            return Outcome::Close;
        }
        Some(ConflictAction::Open(path)) => {
            if let Some(root) = app.vcs.root().map(|r| r.to_path_buf()) {
                app.open_file(&root.join(path));
            }
            return Outcome::Close;
        }
        None => {}
    }
    if close || esc { Outcome::Close } else { Outcome::Keep }
}

enum ConflictAction {
    /// Take one side (`true`: theirs) for whole files.
    Take(Vec<String>, bool),
    MarkResolved(Vec<String>),
    Open(String),
    /// Open the three-pane merge tool.
    Merge(String),
}

/// A short description of a kind of conflict.
fn conflict_kind(kind: Option<rc_vcs::ConflictKind>) -> &'static str {
    use rc_vcs::ConflictKind as K;
    match kind {
        Some(K::BothModified) | None => tr("both modified"),
        Some(K::BothAdded) => tr("both added"),
        Some(K::BothDeleted) => tr("both deleted"),
        Some(K::AddedByUs) => tr("added by us"),
        Some(K::AddedByThem) => tr("added by them"),
        Some(K::DeletedByUs) => tr("deleted by us"),
        Some(K::DeletedByThem) => tr("deleted by them"),
    }
}

/// Generate › Derive: check the traits; OK rewrites the type's `#[derive]` (or adds one).
fn derive(app: &mut CrabCraftApp, ctx: &Context, doc: u64, name: &str, traits: &mut [(String, bool)], other: &mut String) -> Outcome {
    let ((ok, cancel), esc) = modal(ctx, "derive", 380.0, |ui| {
        title(ui, &rc_kit::i18n::trf("Derive for {name}", &[("name", &name)]));
        egui::Grid::new("derive_traits").num_columns(3).spacing(vec2(16.0, 6.0)).show(ui, |ui| {
            for (i, (t, on)) in traits.iter_mut().enumerate() {
                ui.checkbox(on, t.as_str());
                if i % 3 == 2 {
                    ui.end_row();
                }
            }
        });
        ui.add_space(6.0);
        ui.add(egui::TextEdit::singleline(other).hint_text(tr("Other traits, separated by commas")).desired_width(f32::INFINITY));
        let enter = ui.input(|i| i.key_pressed(Key::Enter));
        let (ok, cancel) = buttons(ui, tr("OK"), true);
        (ok || enter, cancel)
    });
    if ok {
        let mut list: Vec<String> = traits.iter().filter(|(_, on)| *on).map(|(t, _)| t.clone()).collect();
        for t in other.split(',').map(str::trim).filter(|t| !t.is_empty()) {
            if !list.iter().any(|x| x == t) {
                list.push(t.to_string());
            }
        }
        // The text may have changed while the dialog was open: find the type again.
        if let Some(d) = app.docs.list.iter_mut().find(|d| d.editor.id == doc)
            && let Some(t) = crate::insight::derive_target_of(d).filter(|t| t.name == name)
            && let Some((a, b, text)) = t.edit(&list)
        {
            d.editor.replace(a, b, &text);
            app.focus_editor = true;
        }
        return Outcome::Close;
    }
    if cancel || esc { Outcome::Close } else { Outcome::Keep }
}

/// ⌘F12: the members of the active file, filtered by typing.
fn file_structure(app: &mut CrabCraftApp, ctx: &Context, filter: &mut String, selected: &mut usize) -> Outcome {
    let Some(doc) = app.docs.active_doc() else { return Outcome::Close };
    let id = doc.editor.id;
    let Some(tree) = app.insight.structure.get(&id).cloned() else {
        app.notify(tr("The file structure is not available yet"), false);
        return Outcome::Close;
    };
    let mut flat: Vec<(usize, crate::insight::SymbolNode)> = Vec::new();
    fn walk(nodes: &[crate::insight::SymbolNode], depth: usize, out: &mut Vec<(usize, crate::insight::SymbolNode)>) {
        for n in nodes {
            out.push((depth, n.clone()));
            walk(&n.children, depth + 1, out);
        }
    }
    walk(&tree, 0, &mut flat);
    let items: Vec<(usize, crate::insight::SymbolNode)> =
        flat.into_iter().filter(|(_, n)| filter.is_empty() || rc_editor::popup::fuzzy_score(&n.name, filter).is_some()).collect();
    let p = rc_kit::theme::palette(ctx.global_style().visuals.dark_mode);
    let mut open = None;
    let (_, esc) = modal(ctx, "file_structure", 460.0, |ui| {
        title(ui, &doc.name());
        let resp = ui.add(egui::TextEdit::singleline(filter).hint_text(tr("Type to filter")).desired_width(f32::INFINITY));
        resp.request_focus();
        if ui.input(|i| i.key_pressed(Key::ArrowDown)) {
            *selected = (*selected + 1).min(items.len().saturating_sub(1));
        }
        if ui.input(|i| i.key_pressed(Key::ArrowUp)) {
            *selected = selected.saturating_sub(1);
        }
        if ui.input(|i| i.key_pressed(Key::Enter)) {
            open = items.get(*selected).map(|(_, n)| (n.line, n.col));
        }
        ui.add_space(6.0);
        egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
            for (i, (depth, n)) in items.iter().enumerate() {
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), egui::Sense::click());
                if i == *selected {
                    ui.painter().rect_filled(r, 4.0, p.selection);
                } else if resp.hovered() {
                    ui.painter().rect_filled(r, 4.0, p.row_hover);
                }
                let x = r.left() + 8.0 + *depth as f32 * 16.0;
                icons::paint(
                    ui.painter(),
                    egui::Rect::from_center_size(egui::pos2(x + 8.0, r.center().y), vec2(16.0, 16.0)),
                    crate::convert::symbol_icon(n.kind),
                    &p,
                    true,
                );
                let g = ui.painter().layout_no_wrap(n.name.clone(), FontId::proportional(13.0), p.text);
                ui.painter().galley(egui::pos2(x + 22.0, r.center().y - g.size().y / 2.0), g.clone(), p.text);
                if !n.detail.is_empty() {
                    ui.painter().text(egui::pos2(x + 30.0 + g.size().x, r.center().y), Align2::LEFT_CENTER, &n.detail, FontId::proportional(12.0), p.muted);
                }
                if resp.clicked() {
                    open = Some((n.line, n.col));
                }
            }
        });
    });
    if let Some((line, col)) = open {
        app.push_nav();
        if let Some(d) = app.docs.active_doc_mut() {
            d.editor.goto(line, col);
        }
        return Outcome::Close;
    }
    if esc { Outcome::Close } else { Outcome::Keep }
}

/// The Local History dialog: the stored versions of one file, newest first, what changed since
/// the selected one, and Revert (an undoable edit of the open document).
pub struct LocalHistoryView {
    pub path: PathBuf,
    inbox: rc_core::Inbox<crate::local_history::Loaded>,
    pub revisions: Option<Vec<rc_core::history::Revision>>,
    pub selected: usize,
    pub texts: std::collections::HashMap<u64, Result<String, String>>,
    requested: std::collections::HashSet<u64>,
    /// For (revision time, buffer version): preview lines, added and removed line counts.
    preview: Option<PreviewCache>,
}

type PreviewCache = ((u64, u64), Vec<crate::local_history::PreviewLine>, usize, usize);

impl LocalHistoryView {
    pub fn new(path: PathBuf, inbox: rc_core::Inbox<crate::local_history::Loaded>) -> Self {
        LocalHistoryView { path, inbox, revisions: None, selected: 0, texts: Default::default(), requested: Default::default(), preview: None }
    }
}

/// "3 min ago", or the date for old versions.
pub(crate) fn history_age(now_ms: u64, time_ms: u64) -> String {
    let secs = now_ms.saturating_sub(time_ms) / 1000;
    match secs {
        0..60 => tr("just now").to_string(),
        60..3600 => trf("{n} min ago", &[("n", &(secs / 60))]),
        3600..86_400 => trf("{n} h ago", &[("n", &(secs / 3600))]),
        _ => crate::vcs::format_date((time_ms / 1000) as i64),
    }
}

fn byte_size(len: u64) -> String {
    match len {
        0..1024 => format!("{len} B"),
        1024..1_048_576 => format!("{:.1} KB", len as f64 / 1024.0),
        _ => format!("{:.1} MB", len as f64 / 1_048_576.0),
    }
}

fn local_history(app: &mut CrabCraftApp, ctx: &Context, v: &mut LocalHistoryView) -> Outcome {
    for msg in v.inbox.drain() {
        match msg {
            crate::local_history::Loaded::List(list) => {
                v.revisions = Some(list);
                v.selected = 0;
            }
            crate::local_history::Loaded::Text(time, text) => {
                v.texts.insert(time, text);
            }
        }
    }
    // The file was closed meanwhile: nothing to revert.
    let Some(doc_i) = app.docs.index_of(&v.path) else { return Outcome::Close };
    let revisions = v.revisions.clone().unwrap_or_default();
    v.selected = v.selected.min(revisions.len().saturating_sub(1));
    let current = revisions.get(v.selected).cloned();
    if let (Some(rev), Some(h)) = (&current, &app.docs.history)
        && v.requested.insert(rev.time_ms)
    {
        h.read(rev, v.inbox.sender());
    }
    let version = app.docs.list[doc_i].editor.buffer.version();
    if let Some(rev) = &current
        && let Some(Ok(old)) = v.texts.get(&rev.time_ms)
        && v.preview.as_ref().is_none_or(|p| p.0 != (rev.time_ms, version))
    {
        let now = app.docs.list[doc_i].editor.buffer.rope().to_string();
        let (lines, added, removed) = crate::local_history::preview(old, &now, 2000);
        v.preview = Some(((rev.time_ms, version), lines, added, removed));
    }
    let text = current.as_ref().and_then(|r| v.texts.get(&r.time_ms));
    let preview = v.preview.as_ref().filter(|p| current.as_ref().is_some_and(|r| p.0 == (r.time_ms, version)));
    let read_only = app.docs.list[doc_i].editor.read_only;
    let can_revert = !read_only && preview.is_some_and(|p| p.2 + p.3 > 0);
    let dark = ctx.global_style().visuals.dark_mode;
    let p = rc_kit::theme::palette(dark);
    let s = rc_kit::theme::scheme(dark);
    let now_ms = rc_core::history::now_ms();
    let name = app.docs.list[doc_i].name();
    let loading = v.revisions.is_none();
    let mut selected = v.selected;
    let (mut revert, mut show_diff, mut close) = (false, false, false);
    let (_, esc) = modal(ctx, "local_history", 880.0, |ui| {
        title(ui, &trf("Local History: {file}", &[("file", &name)]));
        if ui.input(|i| i.key_pressed(Key::ArrowDown)) {
            selected = (selected + 1).min(revisions.len().saturating_sub(1));
        }
        if ui.input(|i| i.key_pressed(Key::ArrowUp)) {
            selected = selected.saturating_sub(1);
        }
        let height = 360.0;
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(vec2(240.0, height), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_min_size(vec2(240.0, height));
                if loading {
                    ui.label(egui::RichText::new(tr("Loading…")).color(p.muted));
                } else if revisions.is_empty() {
                    ui.label(egui::RichText::new(tr("No local history for this file yet. Versions are kept when it is saved.")).color(p.muted));
                }
                egui::ScrollArea::vertical().id_salt("local_history_list").max_height(height).show(ui, |ui| {
                    for (i, r) in revisions.iter().enumerate() {
                        let (rect, resp) = ui.allocate_exact_size(vec2(236.0, 40.0), egui::Sense::click());
                        if i == selected {
                            ui.painter().rect_filled(rect, 4.0, p.selection);
                        } else if resp.hovered() {
                            ui.painter().rect_filled(rect, 4.0, p.row_hover);
                        }
                        icons::paint(
                            ui.painter(),
                            egui::Rect::from_center_size(rect.left_top() + vec2(16.0, 13.0), vec2(16.0, 16.0)),
                            rc_kit::ico::HISTORY,
                            &p,
                            true,
                        );
                        ui.painter().text(
                            rect.left_top() + vec2(32.0, 13.0),
                            Align2::LEFT_CENTER,
                            history_age(now_ms, r.time_ms),
                            FontId::proportional(13.0),
                            p.text,
                        );
                        ui.painter().text(rect.left_top() + vec2(32.0, 30.0), Align2::LEFT_CENTER, byte_size(r.len), FontId::proportional(11.5), p.muted);
                        if resp.clicked() {
                            selected = i;
                        }
                    }
                });
            });
            // A fixed-height divider (a separator would take the whole window height).
            let (line, _) = ui.allocate_exact_size(vec2(13.0, height), egui::Sense::hover());
            ui.painter().vline(line.center().x, line.y_range(), egui::Stroke::new(1.0, p.island_border));
            let width = ui.available_width();
            ui.allocate_ui_with_layout(vec2(width, height), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_min_size(vec2(width, height));
                match (text, preview) {
                    (Some(Err(e)), _) => {
                        ui.label(egui::RichText::new(trf("Could not read this version: {error}", &[("error", e)])).color(p.error));
                    }
                    (_, Some((_, lines, added, removed))) => {
                        let summary = if added + removed == 0 {
                            tr("Same as the current text").to_string()
                        } else {
                            trf("Changed since this version: +{added} −{removed} lines", &[("added", added), ("removed", removed)])
                        };
                        ui.label(egui::RichText::new(summary).color(p.muted));
                        ui.add_space(4.0);
                        egui::ScrollArea::both().id_salt("local_history_preview").max_height(height - 24.0).auto_shrink([false, false]).show_rows(
                            ui,
                            18.0,
                            lines.len(),
                            |ui, range| {
                                for (kind, line) in &lines[range] {
                                    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width().max(560.0), 18.0), egui::Sense::hover());
                                    let (bg, sign, text) = match kind {
                                        '+' => (s.added_lines.gamma_multiply(0.3), "+", line.clone()),
                                        '-' => (s.deleted_lines.gamma_multiply(0.22), "-", line.clone()),
                                        _ => (p.header_bg, "", trf("Line {n}", &[("n", line)])),
                                    };
                                    ui.painter().rect_filled(rect, 0.0, bg);
                                    ui.painter().text(
                                        rect.left_center() + vec2(6.0, 0.0),
                                        Align2::LEFT_CENTER,
                                        format!("{sign} {text}"),
                                        FontId::monospace(12.0),
                                        if sign.is_empty() { p.muted } else { p.text },
                                    );
                                }
                            },
                        );
                    }
                    _ if current.is_some() => {
                        ui.label(egui::RichText::new(tr("Loading…")).color(p.muted));
                    }
                    _ => {}
                }
            });
        });
        ui.add_space(14.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            revert = ui.add_enabled_ui(can_revert, |ui| theme::primary_button(ui, tr("Revert to This Version"))).inner.clicked();
            close = theme::secondary_button(ui, tr("Close")).clicked();
            show_diff = ui.add_enabled_ui(preview.is_some(), |ui| theme::secondary_button(ui, tr("Show Diff"))).inner.clicked();
        });
    });
    let old = current.as_ref().and_then(|r| v.texts.get(&r.time_ms)).and_then(|t| t.as_ref().ok()).cloned();
    v.selected = selected;
    if revert && let Some(old) = old {
        // One undoable edit of the whole text: Undo brings the current version back.
        let e = &mut app.docs.list[doc_i].editor;
        let len = e.buffer.len_chars();
        e.replace(0, len, &old);
        app.docs.list[doc_i].preview = false;
        app.docs.activate(doc_i);
        return Outcome::Close;
    }
    if show_diff && let (Some(old), Some(rev)) = (old, &current) {
        let now = app.docs.list[doc_i].editor.buffer.rope().to_string();
        let title = format!("{name} ({} ↔ {})", history_age(now_ms, rev.time_ms), tr("Current"));
        app.vcs.show_diff(title, v.path.clone(), &old, &now, dark);
        return Outcome::Close;
    }
    if close || esc { Outcome::Close } else { Outcome::Keep }
}

#[cfg(test)]
mod responsive_settings_tests {
    use super::*;
    #[test]
    fn settings_layout_stays_inside_small_and_zoomed_viewports() {
        for viewport in [vec2(1280.0, 800.0), vec2(800.0, 500.0), vec2(800.0, 300.0), vec2(640.0, 400.0), vec2(480.0, 320.0)] {
            let layout = SettingsLayout::for_viewport(viewport);
            assert!(layout.width + 14.0 <= viewport.x);
            assert!(layout.height + if layout.compact { 124.0 } else { 88.0 } <= viewport.y);
            assert!(layout.page > 0.0 && layout.height > 0.0);
        }
        assert!(SettingsLayout::for_viewport(vec2(640.0, 400.0)).compact);
        assert!(!SettingsLayout::for_viewport(vec2(1280.0, 800.0)).compact);
    }
    #[test]
    fn settings_controls_remain_reachable_at_small_logical_sizes() {
        use egui_kittest::kittest::Queryable;
        let _home = crate::screens::HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let home = tempfile::tempdir().unwrap();
        for size in [vec2(1280.0, 800.0), vec2(800.0, 500.0), vec2(800.0, 300.0), vec2(640.0, 400.0), vec2(480.0, 320.0)] {
            let mut harness = crate::screens::harness(crate::screens::test_app(home.path(), true), size);
            let mut widths = Vec::new();
            for page in [SettingsPage::Appearance, SettingsPage::Editor, SettingsPage::Tools] {
                harness.state_mut().open_settings(page);
                harness.run_steps(5);
                let modal = harness.ctx.memory(|memory| memory.area_rect(Id::new("settings"))).unwrap();
                let ok = harness.get_by_label(tr("OK")).rect();
                let cancel = harness.get_by_label(tr("Cancel")).rect();
                let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
                assert!(viewport.contains_rect(modal), "modal outside {size:?}: {modal:?}");
                assert!(viewport.contains_rect(ok) && viewport.contains_rect(cancel), "buttons outside {size:?}: {ok:?}, {cancel:?}");
                widths.push(modal.width());
            }
            assert!(widths.windows(2).all(|pair| (pair[0] - pair[1]).abs() < 1.0), "page widths differ at {size:?}: {widths:?}");
        }
    }
}
