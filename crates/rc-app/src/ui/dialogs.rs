//! Modal dialogs and small popups: Settings, About, Go to Line, New File/Folder, Rename,
//! Delete, Recent Files and Local History.

use crate::app::CrabCraftApp;
use egui::{Align2, Context, FontId, Id, Key, Ui, vec2};
use rc_core::Settings;
use rc_core::settings::{KeymapChoice, ThemeChoice};
use rc_kit::i18n::{Lang, tr, trf};
use rc_kit::{icons, theme};
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SettingsPage {
    Appearance,
    Editor,
    Keymap,
    Tools,
}

pub enum Dialog {
    /// Picks a running process for the debugger.
    Attach { filter: String, processes: Vec<rc_core::processes::Process> },
    Settings { draft: Box<Settings>, page: SettingsPage },
    About,
    GotoLine { text: String, focus: bool },
    Bookmarks { list: Vec<crate::insight::Target>, selected: usize },
    RunAnything { text: String, selected: usize },
    NewFile { dir: PathBuf, name: String, kind: crate::item_templates::ItemKind, attach: bool, public: bool, error: Option<String> },
    Rename { path: PathBuf, name: String, error: Option<String> },
    /// After the new name of a file: the other changes the rename brings.
    RenameFile(Box<crate::file_rename::FileRename>),
    ConfirmDelete { path: PathBuf },
    /// Invalidate Caches: what is rebuilt, and whether `cargo clean` runs too.
    InvalidateCaches { clean: bool },
    RecentFiles { selected: usize, filter: String },
    /// New Scratch File: the kinds to create, then the scratch files there are.
    Scratches { selected: usize, filter: String, files: Vec<PathBuf> },
    /// ⌥⏎ context actions for the active editor.
    CodeActions { list: Vec<rc_lsp::lsp_types::CodeActionOrCommand>, selected: usize },
    /// Generate › Derive: the traits a Rust type derives (checked) and others typed in.
    Derive { doc: u64, name: String, traits: Vec<(String, bool)>, other: String },
    /// Shift+F6 on a symbol.
    RenameSymbol { doc: u64, pos: usize, original: String, name: String, focus: bool },
    /// Confirm rolling back files to HEAD.
    Rollback { paths: Vec<String> },
    NewBranch { name: String },
    /// ⇧⌘E: the places the caret jumped from.
    RecentLocations { selected: usize },
    /// ⌘F12 file structure popup.
    FileStructure { filter: String, selected: usize },
    /// Right-click on a breakpoint: condition and enabled state (0-based line).
    BreakpointProperties { path: PathBuf, line: usize, condition: String, log: String, once: bool, enabled: bool, focus: bool },
    /// Edit an application run configuration: launch profile, arguments, environment, folder.
    RunOptions { name: String, project: PathBuf, profiles: Vec<String>, options: rc_core::session::RunOptions, web: bool },
    /// A compound run configuration: its name and which applications it starts.
    Compound { old: Option<String>, name: String, members: Vec<(String, bool)> },
    /// ⌥F8: evaluate an expression in the selected frame.
    Evaluate { expression: String, focus: bool },
    /// The earlier versions of a file kept by the IDE.
    LocalHistory(Box<LocalHistoryView>),
    /// The variables of every Cargo command and of rust-analyzer, `NAME=value` per line.
    ProjectEnv { text: String },
}

impl Dialog {
    pub fn settings(s: &Settings) -> Dialog {
        Dialog::Settings { draft: Box::new(s.clone()), page: SettingsPage::Appearance }
    }
}

enum Outcome {
    Keep,
    Close,
}

pub fn show(app: &mut CrabCraftApp, ctx: &Context) {
    let Some(mut dialog) = app.dialog.take() else { return };
    let outcome = match &mut dialog {
        Dialog::Settings { draft, page } => settings(app, ctx, draft, page),
        Dialog::About => about(ctx),
        Dialog::GotoLine { text, focus } => goto_line(app, ctx, text, focus),
        Dialog::Bookmarks { list, selected } => bookmarks(app, ctx, list, selected),
        Dialog::RunAnything { text, selected } => run_anything(app, ctx, text, selected),
        Dialog::NewFile { dir, name, kind, attach, public, error } => new_file(app, ctx, dir, name, *kind, attach, public, error),
        Dialog::Rename { path, name, error } => rename(app, ctx, path, name, error),
        Dialog::RenameFile(r) => rename_file(app, ctx, r),
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
        Dialog::RecentLocations { selected } => recent_locations(app, ctx, selected),
        Dialog::BreakpointProperties { path, line, condition, log, once, enabled, focus } => breakpoint_properties(app, ctx, path, *line, condition, log, once, enabled, focus),
        Dialog::Evaluate { expression, focus } => evaluate(app, ctx, expression, focus),
        Dialog::Attach { filter, processes } => attach(app, ctx, filter, processes),
        Dialog::Compound { old, name, members } => compound(app, ctx, old.as_deref(), name, members),
        Dialog::RunOptions { name, project, profiles, options, web } => run_options(app, ctx, name, project, profiles, options, *web),
        Dialog::LocalHistory(v) => local_history(app, ctx, v),
        Dialog::ProjectEnv { text } => project_env(app, ctx, text),
    };
    if matches!(outcome, Outcome::Keep) && app.dialog.is_none() {
        app.dialog = Some(dialog);
    } else if matches!(outcome, Outcome::Close) {
        app.focus_editor = true;
    }
}

fn modal<R>(ctx: &Context, id: &str, width: f32, add: impl FnOnce(&mut Ui) -> R) -> (R, bool) {
    let resp = egui::Modal::new(Id::new(id)).show(ctx, |ui| {
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

fn settings(app: &mut CrabCraftApp, ctx: &Context, draft: &mut Settings, page: &mut SettingsPage) -> Outcome {
    let p = rc_kit::theme::palette(ctx.global_style().visuals.dark_mode);
    let ((ok, cancel), esc) = modal(ctx, "settings", 720.0, |ui| {
        title(ui, tr("Settings"));
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(170.0);
                for (pg, label) in [
                    (SettingsPage::Appearance, tr("Appearance")),
                    (SettingsPage::Editor, tr("Editor")),
                    (SettingsPage::Keymap, tr("Keymap")),
                    (SettingsPage::Tools, tr("Tools")),
                ] {
                    let (r, resp) = ui.allocate_exact_size(vec2(170.0, 28.0), egui::Sense::click());
                    if *page == pg {
                        ui.painter().rect_filled(r, 6.0, p.selection);
                    } else if resp.hovered() {
                        ui.painter().rect_filled(r, 6.0, p.row_hover);
                    }
                    ui.painter().text(r.left_center() + vec2(10.0, 0.0), Align2::LEFT_CENTER, label, FontId::proportional(13.0), p.text);
                    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, *page == pg, label));
                    if resp.clicked() {
                        *page = pg;
                    }
                }
            });
            ui.add_space(16.0);
            ui.vertical(|ui| {
                ui.set_min_height(320.0);
                ui.set_width(500.0);
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
                        egui::ComboBox::from_id_salt("lang")
                            .selected_text(current.map_or(tr("System language"), |l| l.native_name()))
                            .show_ui(ui, |ui| {
                                if ui.selectable_label(current.is_none(), tr("System language")).clicked() {
                                    draft.language = None;
                                }
                                for l in Lang::ALL {
                                    if ui.selectable_label(current == Some(l), l.native_name()).clicked() {
                                        draft.language = Some(lang_code(l).to_string());
                                    }
                                }
                            });
                        ui.end_row();
                        ui.label(tr("Zoom"));
                        theme::slider(ui, egui::Slider::new(&mut draft.ui_zoom, 0.5..=3.0).step_by(0.1).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)));
                        ui.end_row();
                    }
                    SettingsPage::Editor => {
                        ui.label(tr("Rust"));
                        ui.checkbox(&mut draft.format_on_save, tr("Reformat with rustfmt on save"));
                        ui.end_row();
                        ui.label("");
                        ui.checkbox(&mut draft.use_nextest, tr("Run tests with cargo-nextest")).on_hover_text(tr("Needs cargo-nextest installed. Doctests do not run with it."));
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
                        ui.checkbox(&mut draft.autosave, tr("Save files when switching to another application"));
                        ui.end_row();
                        ui.label("");
                        ui.checkbox(&mut draft.single_click_open, tr("Open files with a single click in the Explorer"));
                        ui.end_row();
                        ui.label("");
                        ui.checkbox(&mut draft.reopen_last, tr("Reopen the last project on startup"));
                        ui.end_row();
                        ui.label("");
                        ui.checkbox(&mut draft.tabs_multirow, tr("Show tabs in multiple rows"));
                        ui.end_row();
                        ui.label("");
                        ui.checkbox(&mut draft.tab_colors, tr("Color tabs by kind of file"));
                        ui.end_row();
                    }
                    SettingsPage::Keymap => {
                        ui.label(tr("Keymap"));
                        egui::ComboBox::from_id_salt("keymap")
                            .selected_text(match draft.keymap {
                                KeymapChoice::Rider => if cfg!(target_os = "macos") { "Rider (macOS)" } else { "Rider" },
                                KeymapChoice::VisualStudio => "Visual Studio",
                                KeymapChoice::VsCode => if cfg!(target_os = "macos") { "VS Code (macOS)" } else { "VS Code" },
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut draft.keymap, KeymapChoice::Rider, if cfg!(target_os = "macos") { "Rider (macOS)" } else { "Rider" });
                                ui.selectable_value(&mut draft.keymap, KeymapChoice::VisualStudio, "Visual Studio");
                                ui.selectable_value(&mut draft.keymap, KeymapChoice::VsCode, if cfg!(target_os = "macos") { "VS Code (macOS)" } else { "VS Code" });
                            });
                        ui.end_row();
                    }
                    SettingsPage::Tools => {
                        app.load_toolchain();
                        toolchain_rows(ui, app.toolchain.as_ref());
                        for (label, value) in [
                            (tr("rust-analyzer path"), &mut draft.rust_analyzer_path),
                            (tr(".NET CLI"), &mut draft.dotnet_path),
                            (tr("Roslyn language server"), &mut draft.roslyn_path),
                            (tr("netcoredbg debugger"), &mut draft.netcoredbg_path),
                        ] {
                            ui.label(label);
                            ui.horizontal(|ui| {
                                let mut text = value.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
                                let resp = ui.add(egui::TextEdit::singleline(&mut text).hint_text(tr("Detect automatically")).desired_width(300.0));
                                if resp.changed() {
                                    *value = (!text.trim().is_empty()).then(|| PathBuf::from(text.trim()));
                                }
                                if ui.button(tr("Browse…")).clicked()
                                    && let Some(p) = rfd::FileDialog::new().pick_file()
                                {
                                    *value = Some(p);
                                }
                            });
                            ui.end_row();
                        }
                        rustfmt_rows(ui, &mut draft.rustfmt, p.error, p.text_muted);
                    }
                });
            });
        });
        buttons(ui, tr("OK"), true)
    });
    if ok {
        app.apply_settings(ctx, draft.clone());
        return Outcome::Close;
    }
    if cancel || esc { Outcome::Close } else { Outcome::Keep }
}

/// How rustfmt runs for Reformat Code, on save and at commit.
fn rustfmt_rows(ui: &mut Ui, rustfmt: &mut rc_core::RustfmtSettings, error: egui::Color32, muted: egui::Color32) {
    ui.label(tr("rustfmt arguments"));
    ui.add(egui::TextEdit::singleline(&mut rustfmt.args).hint_text("--config tab_spaces=2").desired_width(300.0));
    ui.end_row();
    ui.label(tr("rustfmt channel"));
    ui.add(egui::TextEdit::singleline(&mut rustfmt.channel).hint_text(tr("Default toolchain")).desired_width(300.0));
    ui.end_row();
    ui.label(tr("rustfmt environment"));
    ui.add(egui::TextEdit::multiline(&mut rustfmt.env).hint_text(tr("NAME=value, one per line")).desired_rows(3).desired_width(300.0));
    ui.end_row();
    ui.label("");
    match rc_cargo::Rustfmt::check(rustfmt) {
        Ok(()) => ui.label(egui::RichText::new(tr("Used by Reformat Code, on save and at commit. The channel runs through rustup.")).color(muted)),
        Err(e) => ui.label(egui::RichText::new(crate::vcs::rustfmt_failure(e)).color(error)),
    };
    ui.end_row();
}

/// The versions in use, read from the tools themselves.
fn toolchain_rows(ui: &mut Ui, info: Option<&rc_cargo::ToolchainInfo>) {
    let Some(info) = info else {
        ui.label(tr("Toolchain"));
        ui.spinner();
        ui.end_row();
        return;
    };
    let missing = tr("Not found");
    for (label, value) in [("Cargo", info.cargo.as_deref()), ("rustc", info.rustc.as_deref()), (tr("rust-analyzer"), info.rust_analyzer.as_deref())] {
        ui.label(label);
        ui.label(value.unwrap_or(missing));
        ui.end_row();
    }
    ui.label(tr("Standard library"));
    match &info.stdlib {
        Some(path) => {
            let text = path.display().to_string();
            ui.scope(|ui| {
                ui.set_max_width(420.0);
                ui.add(egui::Label::new(&text).truncate()).on_hover_text(&text);
            });
        }
        None => {
            ui.label(tr("Not found. Install it with: rustup component add rust-src"));
        }
    }
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
                ui.painter().text(r.left_center() + vec2(8.0, 0.0), Align2::LEFT_CENTER, format!("{file}:{}", mark.line + 1), FontId::proportional(13.0), p.text);
                ui.painter().text(r.right_center() - vec2(8.0, 0.0), Align2::RIGHT_CENTER, mark.preview.chars().take(44).collect::<String>(), FontId::monospace(11.5), p.muted);
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
        ui.add(egui::TextEdit::multiline(text).hint_text("RUST_LOG=debug\nDATABASE_URL=postgres://localhost/dev").font(egui::TextStyle::Monospace).desired_rows(8).desired_width(f32::INFINITY));
        ui.add_space(6.0);
        if let Err(rc_core::session::BadEnvLine(line)) = parsed {
            ui.colored_label(p.error, trf("Line {line} is not NAME=value with a valid name.", &[("line", &line.to_string())]));
        }
        ui.label(egui::RichText::new(tr("One NAME=value per line. Applied to every Cargo command, to the programs it starts and to rust-analyzer; a run configuration's variables win.")).size(12.0).color(p.muted));
        buttons(ui, tr("OK"), parsed.is_ok())
    });
    if ok && parsed.is_ok() {
        app.set_project_env(text.trim().to_string());
        return Outcome::Close;
    }
    if cancel || esc { Outcome::Close } else { Outcome::Keep }
}

fn run_options(app: &mut CrabCraftApp, ctx: &Context, name: &str, project: &std::path::Path, profiles: &[String], o: &mut rc_core::session::RunOptions, web: bool) -> Outcome {
    let settings_file = project.parent().map(|d| d.join("Properties/launchSettings.json")).filter(|p| p.exists());
    let mut open_settings = false;
    // Cargo has no launch profiles or hot reload, and always builds before running.
    let rust = project.file_name().is_some_and(|n| n == "Cargo.toml");
    let ((ok, cancel), esc) = modal(ctx, "run_options", 560.0, |ui| {
        title(ui, &trf("Run Configuration: {name}", &[("name", &name)]));
        let p = rc_kit::pal(ui);
        egui::Grid::new("run_options_grid").num_columns(2).spacing([12.0, 10.0]).show(ui, |ui| {
            if !rust {
            ui.label(tr("Launch profile"));
            let label = |v: &Option<String>| match v.as_deref() {
                None => tr("Default").to_string(),
                Some("") => tr("None (ignore launchSettings.json)").to_string(),
                Some(n) => n.to_string(),
            };
            egui::ComboBox::from_id_salt("run_profile").selected_text(label(&o.profile)).width(300.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut o.profile, None, tr("Default"));
                for pr in profiles {
                    ui.selectable_value(&mut o.profile, Some(pr.clone()), pr);
                }
                ui.selectable_value(&mut o.profile, Some(String::new()), tr("None (ignore launchSettings.json)"));
            });
            ui.end_row();
            }
            ui.label(tr("Program arguments"));
            ui.add(egui::TextEdit::singleline(&mut o.args).hint_text(if rust { "" } else { tr("The profile's arguments") }).font(egui::TextStyle::Monospace).desired_width(380.0));
            ui.end_row();
            ui.label(tr("Environment variables"));
            ui.add(egui::TextEdit::multiline(&mut o.env).hint_text(if rust { "RUST_LOG=debug\nAPP_MODE=test" } else { "ASPNETCORE_ENVIRONMENT=Development\nASPNETCORE_URLS=http://localhost:5000" }).font(egui::TextStyle::Monospace).desired_rows(4).desired_width(380.0));
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
            if rust {
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
            }
            if !rust {
            ui.label("");
            let mut watch = o.hot_reload.unwrap_or(web);
            if ui.checkbox(&mut watch, tr("Hot Reload: apply saved changes and refresh the browser (dotnet watch)")).changed() {
                o.hot_reload = if watch == web { None } else { Some(watch) };
            }
            ui.end_row();
            ui.label("");
            ui.add_enabled(!watch, egui::Checkbox::new(&mut o.build_first, tr("Build before running")));
            ui.end_row();
            }
        });
        ui.add_space(6.0);
        ui.label(egui::RichText::new(if rust { tr("One NAME=value per line.") } else { tr("Variables here are added to the profile's; one NAME=value per line.") }).size(12.0).color(p.muted));
        if settings_file.is_some() {
            ui.add_space(4.0);
            if ui.link(tr("Open launchSettings.json")).clicked() {
                open_settings = true;
            }
        }
        buttons(ui, tr("OK"), true)
    });
    if open_settings && let Some(f) = settings_file {
        app.open_file(&f);
        return Outcome::Close;
    }
    if ok {
        if *o == rc_core::session::RunOptions::default() {
            app.runner.options.remove(name);
        } else {
            app.runner.options.insert(name.to_string(), o.clone());
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

fn attach(app: &mut CrabCraftApp, ctx: &Context, filter: &mut String, processes: &[rc_core::processes::Process]) -> Outcome {
    let (chosen, cancel, esc) = {
        let ((chosen, cancel), esc) = modal(ctx, "attach_process", 560.0, |ui| {
            title(ui, tr("Attach to Process"));
            let resp = ui.add(egui::TextEdit::singleline(filter).hint_text(tr("Filter by name or PID")).desired_width(f32::INFINITY));
            resp.request_focus();
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

#[allow(clippy::too_many_arguments)]
fn breakpoint_properties(app: &mut CrabCraftApp, ctx: &Context, path: &std::path::Path, line: usize, condition: &mut String, log: &mut String, once: &mut bool, enabled: &mut bool, focus: &mut bool) -> Outcome {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let ((ok, cancel, remove), esc) = modal(ctx, "breakpoint_properties", 420.0, |ui| {
        title(ui, &trf("Breakpoint at {file}:{line}", &[("file", &name), ("line", &(line + 1))]));
        ui.checkbox(enabled, tr("Enabled"));
        ui.add_space(6.0);
        ui.label(tr("Condition"));
        let resp = ui.add(egui::TextEdit::singleline(condition).hint_text(tr("Stop only when this expression is true")).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
        if std::mem::take(focus) {
            resp.request_focus();
        }
        let mut enter = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        ui.add_space(6.0);
        ui.label(tr("Log message"));
        let resp = ui.add(egui::TextEdit::singleline(log).hint_text(tr("Print this instead of stopping; {expression} shows a value")).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
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
        if let Some(i) = app.docs.index_of(path) {
            let bps = &mut app.docs.list[i].editor.breakpoints;
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

/// A new file name gets `.cs` when it has no extension, and a C# file gets a class skeleton
/// in the namespace of its folder.
#[allow(clippy::too_many_arguments)]
fn new_file(app: &mut CrabCraftApp, ctx: &Context, dir: &std::path::Path, name: &mut String, kind: crate::item_templates::ItemKind, attach: &mut bool, public: &mut bool, error: &mut Option<String>) -> Outcome {
    let heading = trf("New {item}", &[("item", &tr(kind.title()))]);
    let parent = kind.is_rust().then(|| rc_cargo::modules::parent_module(dir)).flatten();
    let ((ok, cancel), esc) = modal(ctx, "new_file", 380.0, |ui| {
        title(ui, &heading);
        let resp = ui.add(egui::TextEdit::singleline(name).hint_text(tr("Name")).desired_width(f32::INFINITY));
        resp.request_focus();
        if !kind.is_folder() && !name.trim().is_empty() {
            let files = kind.files(dir, name);
            let detail = if kind.is_plain() || kind.is_rust() {
                // Where it lands, from the solution folder.
                let root = app.workspace.as_ref().map(|w| w.ws.root.clone()).unwrap_or_default();
                files[0].0.strip_prefix(&root).unwrap_or(&files[0].0).display().to_string()
            } else {
                let shown: Vec<String> = files.iter().filter_map(|(p, _)| p.file_name().map(|n| n.to_string_lossy().into_owned())).collect();
                format!("{} · {}", shown.join(", "), crate::item_templates::namespace_for(dir))
            };
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
        let files = kind.files(dir, name);
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
                    app.open_file(first);
                    // The caret where the template put `$0`.
                    if let (Some(d), Some(at)) = (app.docs.active_doc_mut(), content.find("$0")) {
                        let pos = content[..at].chars().count();
                        d.editor.set_selection(rc_text::Selection::point(pos));
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
            ui.label(trf("{n} changes in {files} files mention the old name. Pick the ones to make with the rename.", &[("n", &r.changes.len()), ("files", &files)]));
            if !r.asked_server {
                ui.label(egui::RichText::new(tr("No language server checked the code: only text matches are listed.")).size(12.0).color(p.muted));
            }
            if let Some(e) = &r.server_error {
                ui.label(egui::RichText::new(trf("The language server could not list the references: {error}", &[("error", e)])).size(12.0).color(p.warn));
            }
            ui.add_space(6.0);
            egui::ScrollArea::vertical().max_height(360.0).auto_shrink([false, true]).show(ui, |ui| {
                let mut i = 0;
                while i < r.changes.len() {
                    let path = r.changes[i].path.clone();
                    let end = (i..r.changes.len()).find(|&j| r.changes[j].path != path).unwrap_or(r.changes.len());
                    let shown = root.as_ref().and_then(|root| path.strip_prefix(root).ok()).unwrap_or(&path).display().to_string();
                    let mut all = r.changes[i..end].iter().all(|c| c.checked);
                    if ui.checkbox(&mut all, egui::RichText::new(&shown).strong()).changed() {
                        for c in &mut r.changes[i..end] {
                            c.checked = all;
                        }
                    }
                    for c in &mut r.changes[i..end] {
                        ui.horizontal(|ui| {
                            ui.add_space(22.0);
                            let kind = if c.from_server { tr("code reference") } else { tr("text match") };
                            let label = format!("{}: {} \u{2192} {}", c.line + 1, clip(c.before.trim()), clip(c.after.trim()));
                            ui.checkbox(&mut c.checked, egui::RichText::new(label).monospace().size(12.0));
                            ui.label(egui::RichText::new(kind).size(11.0).color(p.muted));
                        });
                    }
                    i = end;
                }
            });
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
    let p = rc_kit::theme::palette(ctx.global_style().visuals.dark_mode);
    let mut open = None;
    let (_, esc) = modal(ctx, "recent_files", 480.0, |ui| {
        title(ui, tr("Recent Files"));
        let resp = ui.add(egui::TextEdit::singleline(filter).hint_text(tr("Type to filter")).desired_width(f32::INFINITY));
        resp.request_focus();
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
        for (i, path) in items.iter().enumerate().take(20) {
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
            if resp.clicked() {
                open = Some(path.clone());
            }
        }
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
    let mut rows: Vec<Row> = Kind::ALL.into_iter().filter(|k| needle.is_empty() || scratch_kind_name(*k).to_lowercase().contains(&needle)).map(Row::New).collect();
    rows.extend(files.iter().filter(|p| needle.is_empty() || p.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase().contains(&needle))).map(Row::Open));
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
    let area = egui::Area::new(Id::new("code_actions")).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
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
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &title));
                if resp.clicked() {
                    chosen = Some(i);
                }
            }
        });
    });
    if ctx.input(|i| i.pointer.any_pressed()) && !area.response.hovered() && chosen.is_none() {
        close = true;
    }
    if let Some(i) = chosen {
        let action = list.swap_remove(i);
        if let rc_lsp::lsp_types::CodeActionOrCommand::Command(c) = &action {
            let ran = match c.command.as_str() {
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
                        let mut traits: Vec<(String, bool)> = rc_syntax::COMMON_DERIVES.iter().map(|c| (c.to_string(), t.items.iter().any(|x| x == c))).collect();
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
        let (ok, cancel) = buttons(ui, tr("Rename"), valid);
        (ok || enter && valid, cancel)
    });
    if ok {
        if *name != *original
            && let Some(d) = app.docs.list.iter().find(|d| d.editor.id == doc)
        {
            app.insight.rename(d, pos, name.trim());
        }
        return Outcome::Close;
    }
    if cancel || esc { Outcome::Close } else { Outcome::Keep }
}

fn recent_locations(app: &mut CrabCraftApp, ctx: &Context, selected: &mut usize) -> Outcome {
    let items: Vec<(PathBuf, usize, String)> = app
        .nav_back
        .iter()
        .rev()
        .take(30)
        .map(|(path, pos)| {
            let line_text = match app.docs.index_of(path) {
                Some(i) => {
                    let e = &app.docs.list[i].editor;
                    let (l, _) = e.buffer.line_col((*pos).min(e.buffer.len_chars()));
                    (l, e.buffer.line_text(l))
                }
                None => (0, String::new()),
            };
            (path.clone(), line_text.0, line_text.1)
        })
        .collect();
    let p = rc_kit::theme::palette(ctx.global_style().visuals.dark_mode);
    let mut open = None;
    let (_, esc) = modal(ctx, "recent_locations", 620.0, |ui| {
        title(ui, tr("Recent Locations"));
        if ui.input(|i| i.key_pressed(Key::ArrowDown)) {
            *selected = (*selected + 1).min(items.len().saturating_sub(1));
        }
        if ui.input(|i| i.key_pressed(Key::ArrowUp)) {
            *selected = selected.saturating_sub(1);
        }
        if ui.input(|i| i.key_pressed(Key::Enter)) {
            open = items.get(*selected).cloned();
        }
        for (i, (path, line, text)) in items.iter().enumerate() {
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), egui::Sense::click());
            if i == *selected {
                ui.painter().rect_filled(r, 6.0, p.selection);
            } else if resp.hovered() {
                ui.painter().rect_filled(r, 6.0, p.row_hover);
            }
            let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            icons::paint(ui.painter(), egui::Rect::from_center_size(r.left_top() + vec2(14.0, 13.0), vec2(16.0, 16.0)), icons::for_file(&name), &p, true);
            ui.painter().text(r.left_top() + vec2(30.0, 13.0), Align2::LEFT_CENTER, format!("{name}:{}", line + 1), theme::bold(12.5), p.text);
            ui.painter().text(r.left_top() + vec2(30.0, 31.0), Align2::LEFT_CENTER, text.trim(), FontId::monospace(12.0), p.text_muted);
            if resp.clicked() {
                open = Some((path.clone(), *line, text.clone()));
            }
        }
    });
    if let Some((path, line, _)) = open {
        app.open_at(&path, line, 0);
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
    let items: Vec<(usize, crate::insight::SymbolNode)> = flat
        .into_iter()
        .filter(|(_, n)| filter.is_empty() || rc_editor::popup::fuzzy_score(&n.name, filter).is_some())
        .collect();
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
                icons::paint(ui.painter(), egui::Rect::from_center_size(egui::pos2(x + 8.0, r.center().y), vec2(16.0, 16.0)), crate::convert::symbol_icon(n.kind), &p, true);
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
fn history_age(now_ms: u64, time_ms: u64) -> String {
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
                        icons::paint(ui.painter(), egui::Rect::from_center_size(rect.left_top() + vec2(16.0, 13.0), vec2(16.0, 16.0)), rc_kit::ico::HISTORY, &p, true);
                        ui.painter().text(rect.left_top() + vec2(32.0, 13.0), Align2::LEFT_CENTER, history_age(now_ms, r.time_ms), FontId::proportional(13.0), p.text);
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
                        let summary = if added + removed == 0 { tr("Same as the current text").to_string() } else { trf("Changed since this version: +{added} −{removed} lines", &[("added", added), ("removed", removed)]) };
                        ui.label(egui::RichText::new(summary).color(p.muted));
                        ui.add_space(4.0);
                        egui::ScrollArea::both().id_salt("local_history_preview").max_height(height - 24.0).auto_shrink([false, false]).show_rows(ui, 18.0, lines.len(), |ui, range| {
                            for (kind, line) in &lines[range] {
                                let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width().max(560.0), 18.0), egui::Sense::hover());
                                let (bg, sign, text) = match kind {
                                    '+' => (s.added_lines.gamma_multiply(0.3), "+", line.clone()),
                                    '-' => (s.deleted_lines.gamma_multiply(0.22), "-", line.clone()),
                                    _ => (p.header_bg, "", trf("Line {n}", &[("n", line)])),
                                };
                                ui.painter().rect_filled(rect, 0.0, bg);
                                ui.painter().text(rect.left_center() + vec2(6.0, 0.0), Align2::LEFT_CENTER, format!("{sign} {text}"), FontId::monospace(12.0), if sign.is_empty() { p.muted } else { p.text });
                            }
                        });
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
mod tests {
    #[test]
    fn skeleton_namespace_follows_the_project_folder() {
        let dir = tempfile::tempdir().unwrap();
        let proj = dir.path().join("Shop.Core");
        std::fs::create_dir_all(proj.join("Models")).unwrap();
        std::fs::write(proj.join("Shop.Core.csproj"), "<Project/>").unwrap();
        let s = crate::item_templates::ItemKind::Class.files(&proj.join("Models"), "Order")[0].1.clone();
        assert!(s.starts_with("namespace Shop.Core.Models;"), "{s}");
        assert!(s.contains("public class Order"));
    }
}
