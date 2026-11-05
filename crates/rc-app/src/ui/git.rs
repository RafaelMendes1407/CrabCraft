//! Git UI: the Commit tool window (changes, message, commit), the Git tool window (log and
//! the files of a commit), the branches popup of the toolbar and the side-by-side diff.

use crate::app::CrabCraftApp;
use crate::toolwindow::ToolWindow;
use crate::ui::toolwindows::{empty_state, header};
use egui::{Align2, Color32, FontId, Rect, Sense, Ui, pos2, vec2};
use rc_kit::i18n::{tr, trf};
use rc_kit::{ico, icons, theme};
use crate::ui::dialogs::Dialog;
use crate::vcs::{GitTab, PatchSource};
use rc_vcs::FileState;

/// IntelliJ's file status colours.
fn state_color(state: FileState, untracked: bool, dark: bool) -> Color32 {
    let rgb = |c: u32| Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8);
    match (state, untracked, dark) {
        (_, true, true) => rgb(0xD1675A),
        (_, true, false) => rgb(0x993300),
        (FileState::Added, _, true) => rgb(0x73BD79),
        (FileState::Added, _, false) => rgb(0x067D17),
        (FileState::Deleted, _, true) => rgb(0x8C8C8C),
        (FileState::Deleted, _, false) => rgb(0x6C6C6C),
        (FileState::Unmerged, _, _) => rgb(0xD5756C),
        (_, _, true) => rgb(0x6F9EF7),
        (_, _, false) => rgb(0x0A54C9),
    }
}

fn changelist_label(name: &str) -> &str {
    if name.is_empty() { tr("Default Changelist") } else { name }
}

fn changelists_bar(app: &mut CrabCraftApp, ui: &mut Ui) {
    let mut selected = app.vcs.changelists.active.clone();
    ui.horizontal_wrapped(|ui| {
        ui.label(tr("Changelist:"));
        egui::ComboBox::from_id_salt("active_changelist").selected_text(changelist_label(&selected)).show_ui(ui, |ui| {
            for name in app.vcs.changelists.lists.keys() {
                let count = app.vcs.changes().iter().filter(|c| app.vcs.changelists.list_of(&c.path) == name).count();
                ui.selectable_value(&mut selected, name.clone(), format!("{} ({count})", changelist_label(name)));
            }
        });
        ui.menu_button(tr("Manage Changelists"), |ui| {
            ui.add(egui::TextEdit::singleline(&mut app.vcs.changelist_name).hint_text(tr("Changelist name")).char_limit(100));
            let name = app.vcs.changelist_name.trim().to_string();
            let mutable = !app.vcs.changelists.active.is_empty();
            let result = if ui.add_enabled(!name.is_empty(), egui::Button::new(tr("New Changelist"))).clicked() {
                app.vcs.changelists.create(&name)
            } else if ui.add_enabled(mutable && !name.is_empty(), egui::Button::new(tr("Rename Changelist"))).clicked() {
                app.vcs.changelists.rename_active(&name)
            } else if ui.add_enabled(mutable, egui::Button::new(tr("Delete Changelist"))).on_hover_text(tr("Moves its files to the default changelist without changing their contents.")).clicked() {
                app.vcs.changelists.delete_active(&mut app.vcs.message)
            } else { return };
            match result {
                Ok(()) => { app.session_dirty = true; app.vcs.changelist_name.clear(); ui.close(); }
                Err(error) => app.vcs.error = Some(match error {
                    rc_core::changelists::ChangelistError::AlreadyExists => tr("A changelist with this name already exists"),
                    _ => tr("Choose a unique changelist name of up to 100 characters"),
                }.to_string()),
            }
        });
    });
    if selected != app.vcs.changelists.active && app.vcs.changelists.select(&selected, &mut app.vcs.message).is_ok() {
        app.session_dirty = true;
    }
}

pub fn commit_window(app: &mut CrabCraftApp, ui: &mut Ui) {
    if header(ui, tr("Commit")) {
        app.toggle_tool(ToolWindow::Commit);
    }
    if app.runner.require_trust().is_err() {
        empty_state(ui, ico::WARNING, tr("Restricted mode"), tr("Trust this project before running project tools or code"));
        if ui.button(tr("Trust this project")).clicked() { app.trust_project(); }
        return;
    }
    if app.vcs.repo.is_none() {
        empty_state(ui, ico::TW_COMMIT, tr("No Git repository"), tr("The workspace is not inside a Git repository."));
        return;
    }
    operation_banner(app, ui);
    let p = rc_kit::pal(ui);
    changelists_bar(app, ui);
    let changes: Vec<_> = app.vcs.changes().into_iter().filter(|c| app.vcs.changelists.in_active(&c.path)).collect();
    let root = app.vcs.repo.as_ref().map(|r| r.root().to_path_buf()).unwrap_or_default();
    let mut patch_action: Option<(PatchSource, bool)> = None;
    // Toolbar.
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        if theme::icon_button(ui, ico::REFRESH, true, tr("Refresh")).clicked() {
            app.vcs.refresh();
        }
        let included: Vec<String> = changes.iter().filter(|c| !app.vcs.excluded.contains(&c.path)).map(|c| c.path.clone()).collect();
        if theme::icon_button(ui, ico::REVERT, !included.is_empty(), tr("Rollback…")).clicked() {
            app.dialog = Some(crate::ui::dialogs::Dialog::Rollback { paths: included.clone() });
        }
        if theme::icon_button(ui, ico::DIFF, !changes.is_empty(), tr("Show Diff")).clicked()
            && let Some(c) = changes.iter().find(|c| !c.untracked)
        {
            app.vcs.open_diff(&c.path);
        }
        let busy = app.vcs.busy.is_some();
        if ui.add_enabled(!included.is_empty() && !busy, egui::Button::new(tr("Shelve Checked…"))).clicked() {
            app.shelves.files = included.clone(); app.vcs.tab = GitTab::Shelves; app.dock.show(ToolWindow::Git);
        }
        if ui.add_enabled_ui(!included.is_empty() && !busy, |ui| theme::secondary_button(ui, tr("Stash Checked…"))).inner.clicked() {
            app.dialog = Some(Dialog::StashChanges { message: String::new(), include_untracked: false, keep_index: false, paths: included.clone() });
        }
        if theme::icon_button(ui, ico::EXPORT, !included.is_empty() && !busy, tr("Create Patch from the checked files…")).clicked() {
            patch_action = Some((PatchSource::Changes(included.clone()), false));
        }
        if theme::icon_button(ui, ico::COPY, !included.is_empty() && !busy, tr("Copy the checked files as a patch")).clicked() {
            patch_action = Some((PatchSource::Changes(included.clone()), true));
        }
    });
    let list_h = (ui.available_height() - 198.0).max(80.0);
    let mut open_diff = None;
    let mut open_file = None;
    egui::ScrollArea::vertical().id_salt("commit_changes").max_height(list_h).auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        if changes.is_empty() {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| ui.label(egui::RichText::new(tr("No changes")).color(p.muted)));
        }
        let groups = [(tr("Changes"), false), (tr("Unversioned Files"), true)];
        for (title, untracked) in groups {
            let items: Vec<&crate::vcs::Change> = changes.iter().filter(|c| c.untracked == untracked).collect();
            if items.is_empty() {
                continue;
            }
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
            ui.painter().text(pos2(r.left() + 12.0, r.center().y), Align2::LEFT_CENTER, format!("{title}  {}", items.len()), theme::bold(12.5), p.text);
            for c in items {
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width().max(200.0), 24.0), Sense::click());
                if resp.hovered() {
                    ui.painter().rect_filled(r.shrink2(vec2(6.0, 0.0)), 4.0, p.row_hover);
                }
                // Include checkbox.
                let cb = Rect::from_center_size(pos2(r.left() + 22.0, r.center().y), vec2(14.0, 14.0));
                let included = !app.vcs.excluded.contains(&c.path);
                ui.painter().rect(cb, 3.0, if included { p.accent } else { p.control_bg }, egui::Stroke::new(1.0, if included { p.accent } else { p.control_border }), egui::StrokeKind::Inside);
                if included {
                    let s = egui::Stroke::new(1.6, Color32::WHITE);
                    ui.painter().line_segment([cb.left_center() + vec2(3.0, 0.0), cb.center_bottom() + vec2(-1.0, -3.5)], s);
                    ui.painter().line_segment([cb.center_bottom() + vec2(-1.0, -3.5), cb.right_top() + vec2(-3.0, 3.5)], s);
                }
                let name = c.path.rsplit('/').next().unwrap_or(&c.path).to_string();
                let dir = c.path.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default();
                icons::paint(ui.painter(), Rect::from_center_size(pos2(r.left() + 46.0, r.center().y), vec2(16.0, 16.0)), icons::for_file(&name), &p, true);
                let color = state_color(c.state, c.untracked, p.dark);
                let g = ui.painter().layout_no_wrap(name.clone(), FontId::proportional(13.0), color);
                ui.painter().galley(pos2(r.left() + 60.0, (r.center().y - g.size().y / 2.0).round()), g.clone(), color);
                ui.painter().text(pos2(r.left() + 68.0 + g.size().x, r.center().y), Align2::LEFT_CENTER, dir, FontId::proportional(12.0), p.muted);
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Checkbox, true, &name));
                if resp.clicked() && ui.input(|i| i.pointer.interact_pos()).is_some_and(|pt| pt.x < r.left() + 34.0) {
                    if included {
                        app.vcs.excluded.insert(c.path.clone());
                    } else {
                        app.vcs.excluded.remove(&c.path);
                    }
                }
                if resp.double_clicked() {
                    if c.state == FileState::Unmerged {
                        app.vcs.open_merge_tool(&c.path);
                    } else if c.untracked || c.state == FileState::Added {
                        open_file = Some(root.join(&c.path));
                    } else {
                        open_diff = Some(c.path.clone());
                    }
                }
                resp.context_menu(|ui| {
                    ui.menu_button(tr("Move to Changelist"), |ui| {
                        let lists: Vec<String> = app.vcs.changelists.lists.keys().cloned().collect();
                        for name in lists {
                            let label = changelist_label(&name);
                            if ui.selectable_label(app.vcs.changelists.list_of(&c.path) == name, label).clicked() {
                                let _ = app.vcs.changelists.move_file(&c.path, &name);
                                app.session_dirty = true;
                                ui.close();
                            }
                        }
                    });
                    if c.state == FileState::Unmerged && theme::menu_item(ui, Some(ico::DIFF), tr("Merge…"), false).clicked() {
                        app.vcs.open_merge_tool(&c.path);
                        ui.close();
                    }
                    if theme::menu_item(ui, Some(ico::DIFF), tr("Show Diff"), false).clicked() {
                        open_diff = Some(c.path.clone());
                        ui.close();
                    }
                    if theme::menu_item(ui, Some(ico::OPEN), tr("Open"), false).clicked() {
                        open_file = Some(root.join(&c.path));
                        ui.close();
                    }
                    if theme::menu_item(ui, Some(ico::REVERT), tr("Rollback…"), false).clicked() {
                        app.dialog = Some(crate::ui::dialogs::Dialog::Rollback { paths: vec![c.path.clone()] });
                        ui.close();
                    }
                    ui.separator();
                    if theme::menu_item(ui, Some(ico::EXPORT), tr("Create Patch…"), false).clicked() {
                        patch_action = Some((PatchSource::Changes(vec![c.path.clone()]), false));
                        ui.close();
                    }
                    if theme::menu_item(ui, Some(ico::COPY), tr("Copy as Patch"), false).clicked() {
                        patch_action = Some((PatchSource::Changes(vec![c.path.clone()]), true));
                        ui.close();
                    }
                });
            }
        }
    });
    ui.separator();
    // Message and buttons.
    ui.add_space(4.0);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(ui.available_rect_before_wrap().shrink2(vec2(10.0, 4.0))));
    if child.add(egui::TextEdit::multiline(&mut app.vcs.message).hint_text(tr("Commit Message")).desired_rows(4).desired_width(f32::INFINITY)).changed() {
        app.session_dirty = true;
    }
    child.horizontal(|ui| {
        ui.checkbox(&mut app.vcs.amend, tr("Amend"));
        let tip = match app.vcs.signing.as_ref().and_then(|c| c.key.as_deref()) {
            Some(key) => trf("Signs with the key {key} (user.signingkey)", &[("key", &key)]),
            None => tr("Signs with the committer's key; set user.signingkey to choose another").to_string(),
        };
        if ui.checkbox(&mut app.vcs.sign, tr("Sign with GPG")).on_hover_text(tip).changed() {
            app.vcs.sign_touched = true;
        }
    });
    if app.runner.cargo_workspace.is_some() && child.checkbox(&mut app.settings.format_on_commit, tr("Reformat with rustfmt")).changed() {
        let _ = app.settings.save();
    }
    child.horizontal(|ui| {
        let busy = app.vcs.busy.is_some();
        if ui.add_enabled_ui(!busy, |ui| theme::primary_button(ui, tr("Commit"))).inner.clicked() {
            app.commit_changes(false);
        }
        if ui.add_enabled_ui(!busy, |ui| theme::secondary_button(ui, tr("Commit and Push…"))).inner.clicked() {
            app.commit_changes(true);
        }
        if busy {
            ui.add(egui::Spinner::new().size(14.0));
        }
    });
    if let Some(e) = &app.vcs.error {
        child.label(egui::RichText::new(e).color(p.error).size(12.5));
    }
    ui.advance_cursor_after_rect(ui.available_rect_before_wrap());
    if let Some(path) = open_diff {
        app.vcs.open_diff(&path);
    }
    if let Some(path) = open_file {
        app.open_file(&path);
    }
    if let Some((source, copy)) = patch_action {
        create_patch(app, source, copy);
    }
}

/// Creates a patch: copied to the clipboard, or saved where the user picks.
pub fn create_patch(app: &mut CrabCraftApp, source: PatchSource, copy: bool) {
    if copy {
        app.vcs.create_patch(source, None);
        return;
    }
    let name = match &source {
        PatchSource::Changes(_) => "changes.patch".to_string(),
        PatchSource::Commits(c) if c.len() == 1 => format!("{}.patch", c[0].chars().take(8).collect::<String>()),
        PatchSource::Commits(_) => "commits.patch".to_string(),
    };
    let mut dialog = rfd::FileDialog::new().set_title(tr("Create Patch")).set_file_name(name).add_filter(tr("Patch files"), &["patch", "diff"]);
    if let Some(root) = app.vcs.root() {
        dialog = dialog.set_directory(root);
    }
    if let Some(path) = dialog.save_file() {
        app.vcs.create_patch(source, Some(path));
    }
}

pub fn git_window(app: &mut CrabCraftApp, ui: &mut Ui) {
    if header(ui, tr("Git")) {
        app.toggle_tool(ToolWindow::Git);
    }
    if app.runner.require_trust().is_err() {
        empty_state(ui, ico::WARNING, tr("Restricted mode"), tr("Trust this project before running project tools or code"));
        if ui.button(tr("Trust this project")).clicked() { app.trust_project(); }
        return;
    }
    if app.vcs.repo.is_none() {
        empty_state(ui, ico::TW_VCS, tr("No Git repository"), tr("The workspace is not inside a Git repository."));
        return;
    }
    operation_banner(app, ui);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        if theme::tab(ui, app.vcs.tab == GitTab::Log, Some(ico::HISTORY), tr("Log"), false).0.clicked() {
            app.vcs.tab = GitTab::Log;
        }
        let tags = trf("Tags ({n})", &[("n", &app.vcs.tags.len())]);
        if theme::tab(ui, app.vcs.tab == GitTab::Tags, Some(ico::VCS), &tags, false).0.clicked() {
            app.vcs.tab = GitTab::Tags;
        }
        let stashes = trf("Stashes ({n})", &[("n", &app.vcs.stashes.len())]);
        if theme::tab(ui, app.vcs.tab == GitTab::Stashes, Some(ico::HISTORY), &stashes, false).0.clicked() {
            app.vcs.tab = GitTab::Stashes;
        }
        if theme::tab(ui, app.vcs.tab == GitTab::Shelves, Some(ico::HISTORY), tr("Shelves"), false).0.clicked() { app.vcs.tab = GitTab::Shelves; }
        if theme::tab(ui, app.vcs.tab == GitTab::HistoryEdit, Some(ico::HISTORY), tr("Edit History"), false).0.clicked() { app.vcs.tab = GitTab::HistoryEdit; }
        let worktrees = trf("Worktrees ({n})", &[("n", &app.vcs.worktrees.len())]);
        if theme::tab(ui, app.vcs.tab == GitTab::Worktrees, Some(ico::FOLDER), &worktrees, false).0.clicked() {
            app.vcs.tab = GitTab::Worktrees;
        }
        if app.vcs.busy.is_some() {
            ui.add(egui::Spinner::new().size(14.0));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(8.0);
            if theme::icon_button(ui, ico::IMPORT, app.vcs.busy.is_none(), tr("Apply Patch…")).clicked() {
                app.vcs.close_patch();
                app.dialog = Some(Dialog::ApplyPatch { path: String::new(), paste: false });
            }
            let free = app.vcs.busy.is_none() && app.vcs.operation.state.is_none();
            if ui.add_enabled_ui(free, |ui| theme::secondary_button(ui, tr("Rebase…"))).inner.clicked() {
                app.dialog = Some(Dialog::Rebase { onto: String::new(), autostash: false });
            }
            if ui.add_enabled_ui(free, |ui| theme::secondary_button(ui, tr("Merge…"))).inner.clicked() {
                app.dialog = Some(Dialog::merge(String::new()));
            }
        });
    });
    match app.vcs.tab {
        GitTab::Log => log_view(app, ui),
        GitTab::Tags => tags_view(app, ui),
        GitTab::Stashes => stashes_view(app, ui),
        GitTab::Worktrees => worktrees_view(app, ui),
        GitTab::HistoryEdit => { if let Some(repo) = app.vcs.repo.clone() { let unsaved = app.docs.list.iter().any(|doc| doc.is_modified() && doc.path.starts_with(repo.root())) || !app.rustfmt_jobs.is_empty(); app.history_edit.show(ui, repo, app.vcs.busy.is_some(), unsaved); } },
        GitTab::Shelves => { if let Some(repo) = app.vcs.repo.clone() { let checked = app.vcs.included(); let unsaved = app.docs.list.iter().any(|doc| doc.is_modified() && doc.path.starts_with(repo.root())) || !app.rustfmt_jobs.is_empty(); app.shelves.show(ui, repo, checked, app.vcs.busy.is_some(), unsaved); } },
    }
    // Git refused to remove a worktree with local changes: offer to force it.
    if let Some(path) = app.vcs.force_remove.take()
        && app.dialog.is_none()
    {
        app.dialog = Some(Dialog::RemoveWorktree { path, force: true });
    }
}

/// The filters of the Log (V-06): message text, author, branch and the path of a file history.
fn log_filters(app: &mut CrabCraftApp, ui: &mut Ui) {
    let p = rc_kit::pal(ui);
    let mut filter = app.vcs.log_filter.clone();
    let mut apply = false;
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        // Typed text is applied with Enter or when the field loses focus.
        let text = ui.add(egui::TextEdit::singleline(&mut app.vcs.log_text_draft).hint_text(tr("Text or message")).desired_width(170.0));
        let author = ui.add(egui::TextEdit::singleline(&mut app.vcs.log_author_draft).hint_text(tr("Author")).desired_width(120.0));
        if text.lost_focus() || author.lost_focus() {
            filter.text = app.vcs.log_text_draft.clone();
            filter.author = app.vcs.log_author_draft.clone();
            apply = true;
        }
        let all = tr("All branches").to_string();
        let shown = filter.branch.clone().unwrap_or_else(|| all.clone());
        egui::ComboBox::from_id_salt("log_branch").selected_text(shown).width(150.0).show_ui(ui, |ui| {
            if ui.selectable_label(filter.branch.is_none(), &all).clicked() {
                filter.branch = None;
                apply = true;
            }
            let local = app.vcs.branches.iter().filter(|b| b.kind == rc_vcs::BranchKind::Local);
            let remote = app.vcs.branches.iter().filter(|b| b.kind != rc_vcs::BranchKind::Local);
            for b in local.chain(remote) {
                if ui.selectable_label(filter.branch.as_deref() == Some(b.name.as_str()), &b.name).clicked() {
                    filter.branch = Some(b.name.clone());
                    apply = true;
                }
            }
        });
        if let Some(path) = filter.path.clone() {
            ui.label(egui::RichText::new(trf("History of {path}", &[("path", &path)])).size(12.5).color(p.accent));
            if theme::icon_button(ui, ico::CLOSE, true, tr("Show every file")).clicked() {
                filter.path = None;
                apply = true;
            }
        }
        if !filter.is_empty() && theme::secondary_button(ui, tr("Clear Filters")).clicked() {
            filter = crate::vcs::LogFilter::default();
            apply = true;
        }
        if app.vcs.log_loading {
            ui.add(egui::Spinner::new().size(14.0));
        }
    });
    if apply {
        app.vcs.set_log_filter(filter);
    }
}

fn log_view(app: &mut CrabCraftApp, ui: &mut Ui) {
    let p = rc_kit::pal(ui);
    log_filters(app, ui);
    ui.add_space(2.0);
    let avail = ui.available_rect_before_wrap();
    let left = Rect::from_min_size(avail.min, vec2((avail.width() * 0.62).max(300.0), avail.height()));
    let right = Rect::from_min_max(pos2(left.right() + 1.0, avail.top()), avail.max);
    ui.painter().vline(left.right() + 0.5, avail.y_range(), egui::Stroke::new(1.0, p.island_border));
    let mut select = None;
    let mut patch_action: Option<(PatchSource, bool)> = None;
    // Older commits on request, below the list.
    let more_h = if app.vcs.log_more { 30.0 } else { 0.0 };
    let list = Rect::from_min_max(left.min, pos2(left.right(), left.bottom() - more_h));
    if app.vcs.log_more {
        let mut mu = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_max(pos2(left.left() + 8.0, list.bottom() + 2.0), left.max)));
        if mu.add_enabled_ui(!app.vcs.log_loading, |ui| theme::secondary_button(ui, tr("Load More"))).inner.clicked() {
            app.vcs.load_more_log();
        }
    }
    let mut lu = ui.new_child(egui::UiBuilder::new().max_rect(list));
    lu.set_clip_rect(list.intersect(ui.clip_rect()));
    let log = app.vcs.log.clone();
    if log.is_empty() && !app.vcs.log_loading && !app.vcs.log_filter.is_empty() {
        lu.add_space(20.0);
        lu.vertical_centered(|ui| ui.label(egui::RichText::new(tr("No commits match the filters")).color(p.muted)));
    }
    egui::ScrollArea::vertical().id_salt("git_log").auto_shrink([false, false]).show_rows(&mut lu, 24.0, log.len(), |ui, range| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for c in &log[range] {
            let selected = app.vcs.selected_commit.as_deref() == Some(c.hash.as_str()) || app.vcs.marked_commits.contains(&c.hash);
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::click());
            if selected {
                ui.painter().rect_filled(r.shrink2(vec2(4.0, 0.0)), 4.0, p.selection);
            } else if resp.hovered() {
                ui.painter().rect_filled(r.shrink2(vec2(4.0, 0.0)), 4.0, p.row_hover);
            }
            let dot = pos2(r.left() + 16.0, r.center().y);
            ui.painter().vline(dot.x, r.y_range(), egui::Stroke::new(1.5, p.accent.gamma_multiply(0.6)));
            ui.painter().circle(dot, 4.0, p.island_bg, egui::Stroke::new(2.0, p.accent));
            let mut x = r.left() + 30.0;
            for rf in c.refs.iter().take(3) {
                let label = match rf {
                    rc_vcs::RefLabel::Head => "HEAD".to_string(),
                    rc_vcs::RefLabel::LocalBranch(n) | rc_vcs::RefLabel::RemoteBranch(n) | rc_vcs::RefLabel::Tag(n) => n.clone(),
                    other => format!("{other:?}"),
                };
                let rect = rc_kit::theme::paint_badge(ui, pos2(x, r.center().y), &label);
                x = rect.right() + 4.0;
            }
            let date = crate::vcs::format_date(c.author_time);
            let meta = format!("{}  {}", c.author_name, date);
            let mg = ui.painter().layout_no_wrap(meta, FontId::proportional(12.0), p.muted);
            let maxw = (r.right() - mg.size().x - 16.0 - x).max(40.0);
            let mut job = egui::text::LayoutJob::simple_singleline(c.subject.clone(), FontId::proportional(13.0), p.text);
            job.wrap.max_width = maxw;
            job.wrap.max_rows = 1;
            job.wrap.break_anywhere = true;
            job.wrap.overflow_character = Some('…');
            let g = ui.painter().layout_job(job);
            ui.painter().galley(pos2(x, (r.center().y - g.size().y / 2.0).round()), g, p.text);
            ui.painter().galley(pos2(r.right() - mg.size().x - 10.0, (r.center().y - mg.size().y / 2.0).round()), mg, p.muted);
            if resp.clicked() {
                let m = ui.input(|i| i.modifiers);
                app.vcs.mark_commit(&c.hash, m.command, m.shift);
                select = Some(c.hash.clone());
            }
            resp.context_menu(|ui| {
                if theme::menu_item(ui, Some(ico::ADD), tr("New Tag…"), false).clicked() {
                    app.dialog = Some(Dialog::NewTag { name: String::new(), target: Some((c.hash.clone(), c.short_hash().to_string())), message: String::new() });
                    ui.close();
                }
                ui.separator();
                // V-07: cherry-pick the commit (or the marked ones), merge it or rebase onto it.
                let free = app.vcs.busy.is_none() && app.vcs.operation.state.is_none();
                let picks = cherry_pick_commits(&app.vcs, &c.hash);
                let label = if picks.len() > 1 { trf("Cherry-Pick {n} Commits", &[("n", &picks.len())]) } else { tr("Cherry-Pick").to_string() };
                let merges = picks.iter().any(|h| app.vcs.log.iter().any(|c| &c.hash == h && c.is_merge()));
                let pick = ui.add_enabled_ui(free && !merges, |ui| theme::menu_item(ui, None, &label, false)).inner;
                if merges {
                    pick.on_disabled_hover_text(tr("Cherry-picking a merge commit is not supported"));
                } else if pick.clicked() {
                    app.vcs.cherry_pick(picks);
                    ui.close();
                }
                let target = ref_name(c).unwrap_or_else(|| c.short_hash().to_string());
                ui.add_enabled_ui(free, |ui| {
                    if theme::menu_item(ui, None, &trf("Merge {rev} into Current…", &[("rev", &target)]), false).clicked() {
                        app.dialog = Some(Dialog::merge(target.clone()));
                        ui.close();
                    }
                    if theme::menu_item(ui, None, &trf("Rebase Current onto {rev}…", &[("rev", &target)]), false).clicked() {
                        app.dialog = Some(Dialog::Rebase { onto: target.clone(), autostash: false });
                        ui.close();
                    }
                });
                ui.separator();
                let commits = app.vcs.patch_commits(&c.hash);
                let busy = app.vcs.busy.is_some();
                let (create, copy) = if commits.len() > 1 {
                    (trf("Create Patch of {n} Commits…", &[("n", &commits.len())]), trf("Copy {n} Commits as Patch", &[("n", &commits.len())]))
                } else {
                    (tr("Create Patch…").to_string(), tr("Copy as Patch").to_string())
                };
                ui.add_enabled_ui(!busy, |ui| {
                    if theme::menu_item(ui, Some(ico::EXPORT), &create, false).clicked() {
                        patch_action = Some((PatchSource::Commits(commits.clone()), false));
                        ui.close();
                    }
                    if theme::menu_item(ui, Some(ico::COPY), &copy, false).clicked() {
                        patch_action = Some((PatchSource::Commits(commits.clone()), true));
                        ui.close();
                    }
                });
            });
        }
    });
    if let Some(h) = select {
        app.vcs.select_commit(&h);
    }
    if let Some((source, copy)) = patch_action {
        create_patch(app, source, copy);
    }
    // Details of the selected commit.
    let mut ru = ui.new_child(egui::UiBuilder::new().max_rect(right.shrink2(vec2(10.0, 4.0))));
    ru.set_clip_rect(right.intersect(ui.clip_rect()));
    let mut diff = None;
    if let Some(h) = app.vcs.selected_commit.clone()
        && let Some(c) = app.vcs.log.iter().find(|c| c.hash == h).cloned()
    {
        ru.label(egui::RichText::new(&c.subject).font(theme::bold(13.0)));
        if !c.body.trim().is_empty() {
            ru.label(egui::RichText::new(c.body.trim()).size(12.5));
        }
        ru.label(egui::RichText::new(format!("{}  {} <{}>  {}", c.short_hash(), c.author_name, c.author_email, crate::vcs::format_date(c.author_time))).size(12.0).color(p.muted));
        if let Some((_, sig)) = app.vcs.signature.as_ref().filter(|(s, _)| *s == h)
            && let Some((text, good)) = signature_text(sig)
        {
            ru.label(egui::RichText::new(text).size(12.0).color(if good { p.ok } else { p.error }));
        }
        ru.add_space(6.0);
        let files = app.vcs.commit_files.clone();
        egui::ScrollArea::vertical().id_salt("commit_files").auto_shrink([false, false]).show(&mut ru, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for f in &files {
                let name = f.path.rsplit('/').next().unwrap_or(&f.path).to_string();
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::click());
                if resp.hovered() {
                    ui.painter().rect_filled(r, 4.0, p.row_hover);
                }
                icons::paint(ui.painter(), Rect::from_center_size(pos2(r.left() + 10.0, r.center().y), vec2(16.0, 16.0)), icons::for_file(&name), &p, true);
                ui.painter().text(pos2(r.left() + 24.0, r.center().y), Align2::LEFT_CENTER, &f.path, FontId::proportional(12.5), p.text);
                if resp.double_clicked() || resp.clicked() {
                    diff = Some(f.path.clone());
                }
            }
        });
        if let Some(path) = diff {
            app.vcs.open_commit_diff(&h, &path);
        }
    } else {
        ru.add_space(20.0);
        ru.vertical_centered(|ui| ui.label(egui::RichText::new(tr("Select a commit to see its changes")).color(p.muted)));
    }
    ui.advance_cursor_after_rect(avail);
}

/// What the details of a commit say of its signature, and whether it is good; nothing when the
/// commit is not signed.
pub fn signature_text(sig: &rc_vcs::Signature) -> Option<(String, bool)> {
    use rc_vcs::SignatureStatus as S;
    let who = if sig.signer.is_empty() { sig.key.clone() } else { sig.signer.clone() };
    let key = sig.key.as_str();
    Some(match sig.status {
        S::Unsigned => return None,
        S::Good => (trf("Signed by {signer} (key {key})", &[("signer", &who), ("key", &key)]), true),
        S::GoodUntrusted => (trf("Signed by {signer} (key {key}, not trusted)", &[("signer", &who), ("key", &key)]), true),
        S::Expired => (trf("Signed by {signer}, but the signature has expired", &[("signer", &who)]), false),
        S::ExpiredKey => (trf("Signed by {signer} with a key that has expired", &[("signer", &who)]), false),
        S::RevokedKey => (trf("Signed by {signer} with a revoked key", &[("signer", &who)]), false),
        S::Bad => (trf("Bad signature from {signer}", &[("signer", &who)]), false),
        S::CannotCheck => (trf("Signed with the key {key}, which is not available to check it", &[("key", &key)]), false),
    })
}

/// The Tags view of the Git window: the tags with their commit and message, and what can be
/// done with them (create, push, check out, delete). The output of the last push stays below.
fn tags_view(app: &mut CrabCraftApp, ui: &mut Ui) {
    let p = rc_kit::pal(ui);
    let busy = app.vcs.busy.is_some();
    let has_remote = !app.vcs.remotes.is_empty();
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        if ui.add_enabled_ui(!busy, |ui| theme::secondary_button(ui, tr("New Tag…"))).inner.clicked() {
            app.dialog = Some(Dialog::NewTag { name: String::new(), target: None, message: String::new() });
        }
        let can_push = !busy && has_remote && !app.vcs.tags.is_empty();
        let push_all = ui.add_enabled_ui(can_push, |ui| theme::secondary_button(ui, tr("Push All Tags…"))).inner;
        if push_all.clicked() {
            app.dialog = Some(Dialog::PushTags { name: None, remote: default_remote(&app.vcs.remotes) });
        }
        if !has_remote {
            push_all.on_disabled_hover_text(tr("The repository has no remote"));
        }
        if theme::icon_button(ui, ico::REFRESH, true, tr("Refresh")).clicked() {
            app.vcs.refresh();
        }
    });
    let avail = ui.available_rect_before_wrap();
    let output_h = if app.vcs.output.is_some() { (avail.height() * 0.35).min(110.0) } else { 0.0 };
    let body = Rect::from_min_max(avail.min, pos2(avail.right(), avail.bottom() - output_h));
    let left = Rect::from_min_size(body.min, vec2((body.width() * 0.55).max(260.0).min(body.width()), body.height()));
    let right = Rect::from_min_max(pos2(left.right() + 1.0, body.top()), body.max);
    ui.painter().vline(left.right() + 0.5, body.y_range(), egui::Stroke::new(1.0, p.island_border));
    let tags = app.vcs.tags.clone();
    let mut lu = ui.new_child(egui::UiBuilder::new().max_rect(left));
    lu.set_clip_rect(left.intersect(ui.clip_rect()));
    let mut action: Option<Dialog> = None;
    if tags.is_empty() {
        lu.add_space(20.0);
        lu.vertical_centered(|ui| ui.label(egui::RichText::new(tr("No tags")).color(p.muted)));
    }
    egui::ScrollArea::vertical().id_salt("git_tags").auto_shrink([false, false]).show_rows(&mut lu, 40.0, tags.len(), |ui, range| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for t in &tags[range] {
            let selected = app.vcs.selected_tag.as_deref() == Some(t.name.as_str());
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
            if selected {
                ui.painter().rect_filled(r.shrink2(vec2(4.0, 1.0)), 4.0, p.selection);
            } else if resp.hovered() {
                ui.painter().rect_filled(r.shrink2(vec2(4.0, 1.0)), 4.0, p.row_hover);
            }
            let top = r.top() + 12.0;
            let g = ui.painter().layout_no_wrap(t.name.clone(), theme::bold(13.0), p.text);
            ui.painter().galley(pos2(r.left() + 14.0, top - g.size().y / 2.0), g.clone(), p.text);
            let mut x = r.left() + 22.0 + g.size().x;
            if t.annotated {
                x = theme::paint_badge(ui, pos2(x, top), tr("annotated")).right() + 6.0;
            }
            ui.painter().text(pos2(x, top), Align2::LEFT_CENTER, t.short_target(), FontId::monospace(11.5), p.muted);
            ui.painter().text(pos2(r.right() - 10.0, top), Align2::RIGHT_CENTER, crate::vcs::format_date(t.time), FontId::proportional(11.5), p.muted);
            let summary = if t.annotated { t.message.lines().next().unwrap_or("").to_string() } else { subject_of(&app.vcs.log, &t.target) };
            let mut job = egui::text::LayoutJob::simple_singleline(summary, FontId::proportional(12.0), p.muted);
            job.wrap.max_width = (r.width() - 28.0).max(40.0);
            job.wrap.max_rows = 1;
            job.wrap.break_anywhere = true;
            job.wrap.overflow_character = Some('…');
            let sg = ui.painter().layout_job(job);
            ui.painter().galley(pos2(r.left() + 14.0, r.top() + 29.0 - sg.size().y / 2.0), sg, p.muted);
            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, &t.name));
            if resp.clicked() {
                app.vcs.selected_tag = Some(t.name.clone());
            }
            resp.context_menu(|ui| {
                if let Some(d) = tag_actions(ui, &t.name, busy, has_remote, &app.vcs.remotes) {
                    action = Some(d);
                }
            });
        }
    });
    // Details of the selected tag.
    let mut ru = ui.new_child(egui::UiBuilder::new().max_rect(right.shrink2(vec2(10.0, 6.0))));
    ru.set_clip_rect(right.intersect(ui.clip_rect()));
    match app.vcs.selected_tag.as_ref().and_then(|n| tags.iter().find(|t| &t.name == n)) {
        Some(t) => {
            ru.label(egui::RichText::new(&t.name).font(theme::bold(14.0)));
            let kind = if t.annotated { tr("Annotated tag") } else { tr("Lightweight tag") };
            ru.label(egui::RichText::new(format!("{kind}  ·  {}  ·  {}", t.short_target(), crate::vcs::format_date(t.time))).size(12.0).color(p.muted));
            ru.add_space(6.0);
            ru.horizontal_wrapped(|ui| {
                if let Some(d) = tag_buttons(ui, &t.name, busy, has_remote, &app.vcs.remotes) {
                    action = Some(d);
                }
            });
            ru.add_space(6.0);
            if t.annotated {
                egui::ScrollArea::vertical().id_salt("tag_message").auto_shrink([false, true]).show(&mut ru, |ui| {
                    ui.label(egui::RichText::new(&t.message).size(12.5));
                });
            } else {
                ru.label(egui::RichText::new(subject_of(&app.vcs.log, &t.target)).size(12.5));
            }
        }
        None => {
            ru.add_space(20.0);
            ru.vertical_centered(|ui| ui.label(egui::RichText::new(tr("Select a tag to see its details")).color(p.muted)));
        }
    }
    // The last push's output.
    output_panel(app, ui, Rect::from_min_max(pos2(avail.left(), body.bottom()), avail.max), tr("Push output"), tr("Everything up-to-date"));
    ui.advance_cursor_after_rect(avail);
    if let Some(d) = action {
        app.dialog = Some(d);
    }
}

/// What git printed for the last push or prune ([`crate::vcs::Vcs::output`]), in `rect`.
fn output_panel(app: &mut CrabCraftApp, ui: &mut Ui, rect: Rect, label: &str, empty: &str) {
    let Some(o) = app.vcs.output.clone() else { return };
    let p = rc_kit::pal(ui);
    ui.painter().hline(rect.x_range(), rect.top() + 0.5, egui::Stroke::new(1.0, p.island_border));
    let mut ou = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(10.0, 4.0))));
    ou.set_clip_rect(rect.intersect(ui.clip_rect()));
    ou.horizontal(|ui| {
        ui.label(egui::RichText::new(label).size(11.5).color(p.muted));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if theme::icon_button(ui, ico::CLOSE_SMALL, true, tr("Close")).clicked() {
                app.vcs.output = None;
            }
        });
    });
    egui::ScrollArea::vertical().id_salt("git_output").auto_shrink([false, false]).stick_to_bottom(true).show(&mut ou, |ui| {
        let color = if o.error { p.error } else { p.text };
        ui.label(egui::RichText::new(if o.text.is_empty() { empty } else { o.text.as_str() }).font(FontId::monospace(12.0)).color(color));
    });
}

/// What a worktree row or its details asked for.
enum WorktreeAction {
    Open(std::path::PathBuf),
    OpenInNewWindow(std::path::PathBuf),
    Unlock(std::path::PathBuf),
    Dialog(Dialog),
}

/// The Worktrees view of the Git window: every worktree of the repository (`git worktree
/// list`) with its branch and state, and what can be done with it (open, lock, remove, prune).
fn worktrees_view(app: &mut CrabCraftApp, ui: &mut Ui) {
    let p = rc_kit::pal(ui);
    let busy = app.vcs.busy.is_some();
    let worktrees = app.vcs.worktrees.clone();
    let current = app.vcs.current_worktree().map(|w| w.path.clone());
    let prunable: Vec<std::path::PathBuf> = worktrees.iter().filter(|w| w.prunable.is_some() && w.locked.is_none()).map(|w| w.path.clone()).collect();
    let mut action: Option<WorktreeAction> = None;
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        if ui.add_enabled_ui(!busy, |ui| theme::secondary_button(ui, tr("Add Worktree…"))).inner.clicked() {
            action = Some(WorktreeAction::Dialog(Dialog::AddWorktree { path: String::new(), new_branch: true, name: String::new(), existing: String::new(), path_edited: false }));
        }
        let prune = ui.add_enabled_ui(!busy && !prunable.is_empty(), |ui| theme::secondary_button(ui, tr("Prune…"))).inner;
        if prune.clicked() {
            action = Some(WorktreeAction::Dialog(Dialog::PruneWorktrees { paths: prunable.clone() }));
        }
        if prunable.is_empty() {
            prune.on_disabled_hover_text(tr("No worktree has lost its folder"));
        }
        if theme::icon_button(ui, ico::REFRESH, true, tr("Refresh")).clicked() {
            app.vcs.refresh();
        }
    });
    let avail = ui.available_rect_before_wrap();
    let output_h = if app.vcs.output.is_some() { (avail.height() * 0.35).min(110.0) } else { 0.0 };
    let body = Rect::from_min_max(avail.min, pos2(avail.right(), avail.bottom() - output_h));
    let left = Rect::from_min_size(body.min, vec2((body.width() * 0.55).max(260.0).min(body.width()), body.height()));
    let right = Rect::from_min_max(pos2(left.right() + 1.0, body.top()), body.max);
    ui.painter().vline(left.right() + 0.5, body.y_range(), egui::Stroke::new(1.0, p.island_border));
    let mut lu = ui.new_child(egui::UiBuilder::new().max_rect(left));
    lu.set_clip_rect(left.intersect(ui.clip_rect()));
    egui::ScrollArea::vertical().id_salt("git_worktrees").auto_shrink([false, false]).show_rows(&mut lu, 40.0, worktrees.len(), |ui, range| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for w in &worktrees[range] {
            let selected = app.vcs.selected_worktree.as_deref() == Some(w.path.as_path());
            let is_current = current.as_deref() == Some(w.path.as_path());
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
            if selected {
                ui.painter().rect_filled(r.shrink2(vec2(4.0, 1.0)), 4.0, p.selection);
            } else if resp.hovered() {
                ui.painter().rect_filled(r.shrink2(vec2(4.0, 1.0)), 4.0, p.row_hover);
            }
            let top = r.top() + 12.0;
            icons::paint(ui.painter(), Rect::from_center_size(pos2(r.left() + 20.0, top), vec2(16.0, 16.0)), ico::FOLDER, &p, true);
            let name = w.name();
            let color = if w.prunable.is_some() { p.muted } else { p.text };
            let g = ui.painter().layout_no_wrap(name.clone(), theme::bold(13.0), color);
            ui.painter().galley(pos2(r.left() + 34.0, top - g.size().y / 2.0), g.clone(), color);
            let mut x = r.left() + 42.0 + g.size().x;
            for badge in worktree_badges(w, is_current) {
                x = theme::paint_badge(ui, pos2(x, top), badge).right() + 6.0;
            }
            ui.painter().text(pos2(r.right() - 10.0, top), Align2::RIGHT_CENTER, w.short_head(), FontId::monospace(11.5), p.muted);
            let mut job = egui::text::LayoutJob::simple_singleline(format!("{}  ·  {}", worktree_checkout(w), w.path.display()), FontId::proportional(12.0), p.muted);
            job.wrap.max_width = (r.width() - 48.0).max(40.0);
            job.wrap.max_rows = 1;
            job.wrap.break_anywhere = true;
            job.wrap.overflow_character = Some('…');
            let sg = ui.painter().layout_job(job);
            ui.painter().galley(pos2(r.left() + 34.0, r.top() + 29.0 - sg.size().y / 2.0), sg, p.muted);
            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, &name));
            if resp.clicked() {
                app.vcs.selected_worktree = Some(w.path.clone());
            }
            if resp.double_clicked() && can_open(w, is_current) {
                action = Some(WorktreeAction::Open(w.path.clone()));
            }
            resp.context_menu(|ui| {
                if let Some(a) = worktree_actions(ui, w, is_current, busy, true) {
                    action = Some(a);
                    ui.close();
                }
            });
        }
    });
    // Details of the selected worktree.
    let mut ru = ui.new_child(egui::UiBuilder::new().max_rect(right.shrink2(vec2(10.0, 6.0))));
    ru.set_clip_rect(right.intersect(ui.clip_rect()));
    match app.vcs.selected_worktree.as_ref().and_then(|s| worktrees.iter().find(|w| &w.path == s)) {
        Some(w) => {
            let is_current = current.as_deref() == Some(w.path.as_path());
            ru.label(egui::RichText::new(w.name()).font(theme::bold(14.0)));
            ru.label(egui::RichText::new(w.path.display().to_string()).size(12.0).color(p.muted));
            ru.label(egui::RichText::new(format!("{}  ·  {}", worktree_checkout(w), w.short_head())).size(12.0).color(p.muted));
            if let Some(reason) = &w.locked {
                let text = if reason.is_empty() { tr("Locked").to_string() } else { trf("Locked: {reason}", &[("reason", reason)]) };
                ru.label(egui::RichText::new(text).size(12.0));
            }
            if w.prunable.is_some() {
                ru.label(egui::RichText::new(tr("Its folder no longer exists; Prune forgets it.")).size(12.0).color(p.error));
            }
            ru.add_space(6.0);
            ru.horizontal_wrapped(|ui| {
                if let Some(a) = worktree_actions(ui, w, is_current, busy, false) {
                    action = Some(a);
                }
            });
        }
        None => {
            ru.add_space(20.0);
            ru.vertical_centered(|ui| ui.label(egui::RichText::new(tr("Select a worktree to see its details")).color(p.muted)));
        }
    }
    output_panel(app, ui, Rect::from_min_max(pos2(avail.left(), body.bottom()), avail.max), tr("Prune output"), tr("Nothing to prune"));
    ui.advance_cursor_after_rect(avail);
    match action {
        Some(WorktreeAction::Open(path)) => app.open_workspace(&path),
        Some(WorktreeAction::OpenInNewWindow(path)) => app.open_in_new_window(&path),
        Some(WorktreeAction::Unlock(path)) => app.vcs.unlock_worktree(&path),
        Some(WorktreeAction::Dialog(d)) => app.dialog = Some(d),
        None => {}
    }
}

/// The badges after a worktree's name.
fn worktree_badges(w: &rc_vcs::Worktree, current: bool) -> Vec<&'static str> {
    let mut out = Vec::new();
    if w.main {
        out.push(tr("main"));
    }
    if current {
        out.push(tr("current"));
    }
    if w.locked.is_some() {
        out.push(tr("locked"));
    }
    if w.prunable.is_some() {
        out.push(tr("prunable"));
    }
    out
}

/// What a worktree has checked out: its branch, a detached HEAD or nothing (bare).
fn worktree_checkout(w: &rc_vcs::Worktree) -> String {
    match &w.branch {
        Some(b) => b.clone(),
        None if w.bare => tr("bare repository").to_string(),
        None => tr("Detached HEAD").to_string(),
    }
}

/// A worktree opens as a project when it has a folder and is not the one already open.
fn can_open(w: &rc_vcs::Worktree, current: bool) -> bool {
    !current && !w.bare && w.prunable.is_none()
}

/// The actions on a worktree, as menu items (`menu`) or buttons.
fn worktree_actions(ui: &mut Ui, w: &rc_vcs::Worktree, current: bool, busy: bool, menu: bool) -> Option<WorktreeAction> {
    let item = |ui: &mut Ui, text: &str, enabled: bool, why: Option<&str>| {
        let r = ui.add_enabled_ui(enabled, |ui| if menu { theme::menu_item(ui, None, text, false) } else { theme::secondary_button(ui, text) }).inner;
        match why {
            Some(why) if !enabled => r.on_disabled_hover_text(why).clicked(),
            _ => r.clicked(),
        }
    };
    let openable = can_open(w, current);
    let why_not = if current { tr("This worktree is already open") } else { tr("Its folder no longer exists") };
    let mut out = None;
    if item(ui, tr("Open"), openable, Some(why_not)) {
        out = Some(WorktreeAction::Open(w.path.clone()));
    }
    if item(ui, tr("Open in New Window"), openable, Some(why_not)) {
        out = Some(WorktreeAction::OpenInNewWindow(w.path.clone()));
    }
    if w.locked.is_some() {
        if item(ui, tr("Unlock"), !busy, None) {
            out = Some(WorktreeAction::Unlock(w.path.clone()));
        }
    } else if item(ui, tr("Lock…"), !busy && !w.main, Some(tr("The main worktree cannot be locked")).filter(|_| !busy)) {
        out = Some(WorktreeAction::Dialog(Dialog::LockWorktree { path: w.path.clone(), reason: String::new() }));
    }
    let removable = !w.main && !current && w.locked.is_none();
    let why = if w.main {
        tr("The main worktree cannot be removed")
    } else if current {
        tr("Open another project before removing this worktree")
    } else {
        tr("Unlock the worktree first")
    };
    if item(ui, tr("Remove…"), !busy && removable, Some(why).filter(|_| !busy)) {
        out = Some(WorktreeAction::Dialog(Dialog::RemoveWorktree { path: w.path.clone(), force: false }));
    }
    out
}

/// The Stashes view of the Git window: the stashes, the files the selected one saved (click
/// for its diff) and what can be done with it (apply, pop, drop).
fn stashes_view(app: &mut CrabCraftApp, ui: &mut Ui) {
    let p = rc_kit::pal(ui);
    let busy = app.vcs.busy.is_some();
    let dirty = app.vcs.status.as_ref().is_some_and(|s| !s.entries.is_empty());
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        let stash = ui.add_enabled_ui(!busy && dirty, |ui| theme::secondary_button(ui, tr("Stash Changes…"))).inner;
        if stash.clicked() {
            app.dialog = Some(Dialog::StashChanges { message: String::new(), include_untracked: false, keep_index: false, paths: Vec::new() });
        }
        if !dirty {
            stash.on_disabled_hover_text(tr("No local changes to stash"));
        }
        ui.checkbox(&mut app.vcs.restore_index, tr("Restore index")).on_hover_text(tr("Apply and Pop also restore which changes were staged."));
        if theme::icon_button(ui, ico::REFRESH, true, tr("Refresh")).clicked() {
            app.vcs.refresh();
        }
    });
    let avail = ui.available_rect_before_wrap();
    let left = Rect::from_min_size(avail.min, vec2((avail.width() * 0.5).max(260.0).min(avail.width()), avail.height()));
    let right = Rect::from_min_max(pos2(left.right() + 1.0, avail.top()), avail.max);
    ui.painter().vline(left.right() + 0.5, avail.y_range(), egui::Stroke::new(1.0, p.island_border));
    let stashes = app.vcs.stashes.clone();
    let mut lu = ui.new_child(egui::UiBuilder::new().max_rect(left));
    lu.set_clip_rect(left.intersect(ui.clip_rect()));
    let mut select = None;
    let mut action: Option<StashAction> = None;
    if stashes.is_empty() {
        lu.add_space(20.0);
        lu.vertical_centered(|ui| ui.label(egui::RichText::new(tr("No stashes")).color(p.muted)));
    }
    egui::ScrollArea::vertical().id_salt("git_stashes").auto_shrink([false, false]).show_rows(&mut lu, 40.0, stashes.len(), |ui, range| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for st in &stashes[range] {
            let selected = app.vcs.selected_stash.as_deref() == Some(st.hash.as_str());
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
            if selected {
                ui.painter().rect_filled(r.shrink2(vec2(4.0, 1.0)), 4.0, p.selection);
            } else if resp.hovered() {
                ui.painter().rect_filled(r.shrink2(vec2(4.0, 1.0)), 4.0, p.row_hover);
            }
            let top = r.top() + 12.0;
            let name = format!("stash@{{{}}}", st.index);
            let g = ui.painter().layout_no_wrap(name.clone(), FontId::monospace(12.0), p.text);
            ui.painter().galley(pos2(r.left() + 14.0, top - g.size().y / 2.0), g.clone(), p.text);
            if let Some(b) = &st.branch {
                theme::paint_badge(ui, pos2(r.left() + 22.0 + g.size().x, top), b);
            }
            ui.painter().text(pos2(r.right() - 10.0, top), Align2::RIGHT_CENTER, crate::vcs::format_date(st.time), FontId::proportional(11.5), p.muted);
            let mut job = egui::text::LayoutJob::simple_singleline(stash_summary(&st.message), FontId::proportional(12.0), p.muted);
            job.wrap.max_width = (r.width() - 28.0).max(40.0);
            job.wrap.max_rows = 1;
            job.wrap.break_anywhere = true;
            job.wrap.overflow_character = Some('…');
            let sg = ui.painter().layout_job(job);
            ui.painter().galley(pos2(r.left() + 14.0, r.top() + 29.0 - sg.size().y / 2.0), sg, p.muted);
            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, &name));
            if resp.clicked() && !selected {
                select = Some(st.hash.clone());
            }
            resp.context_menu(|ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    if theme::menu_item(ui, None, tr("Apply"), false).clicked() {
                        action = Some(StashAction::Apply(st.clone()));
                        ui.close();
                    }
                    if theme::menu_item(ui, None, tr("Pop"), false).clicked() {
                        action = Some(StashAction::Pop(st.clone()));
                        ui.close();
                    }
                    if theme::menu_item(ui, None, tr("Create Branch…"), false).clicked() {
                        action = Some(StashAction::Branch(st.clone()));
                        ui.close();
                    }
                    if theme::menu_item(ui, Some(ico::DELETE), tr("Drop…"), false).clicked() {
                        action = Some(StashAction::Drop(st.clone()));
                        ui.close();
                    }
                });
            });
        }
    });
    // Details of the selected stash.
    let mut ru = ui.new_child(egui::UiBuilder::new().max_rect(right.shrink2(vec2(10.0, 6.0))));
    ru.set_clip_rect(right.intersect(ui.clip_rect()));
    let mut diff = None;
    match app.vcs.selected_stash.as_ref().and_then(|h| stashes.iter().find(|s| &s.hash == h)) {
        Some(st) => {
            ru.label(egui::RichText::new(stash_summary(&st.message)).font(theme::bold(13.5)));
            ru.label(egui::RichText::new(format!("stash@{{{}}}  ·  {}  ·  {}", st.index, &st.hash[..st.hash.len().min(8)], crate::vcs::format_date(st.time))).size(12.0).color(p.muted));
            ru.add_space(6.0);
            ru.horizontal_wrapped(|ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    if theme::secondary_button(ui, tr("Apply")).on_hover_text(tr("Apply the changes and keep the stash")).clicked() {
                        action = Some(StashAction::Apply(st.clone()));
                    }
                    if theme::secondary_button(ui, tr("Pop")).on_hover_text(tr("Apply the changes and drop the stash")).clicked() {
                        action = Some(StashAction::Pop(st.clone()));
                    }
                    if theme::secondary_button(ui, tr("Create Branch…")).clicked() {
                        action = Some(StashAction::Branch(st.clone()));
                    }
                    if theme::secondary_button(ui, tr("Drop…")).clicked() {
                        action = Some(StashAction::Drop(st.clone()));
                    }
                });
            });
            ru.add_space(6.0);
            let files = app.vcs.stash_files.clone();
            egui::ScrollArea::vertical().id_salt("stash_files").auto_shrink([false, false]).show(&mut ru, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for f in &files {
                    let name = f.path.rsplit('/').next().unwrap_or(&f.path).to_string();
                    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::click());
                    if resp.hovered() {
                        ui.painter().rect_filled(r, 4.0, p.row_hover);
                    }
                    icons::paint(ui.painter(), Rect::from_center_size(pos2(r.left() + 10.0, r.center().y), vec2(16.0, 16.0)), icons::for_file(&name), &p, true);
                    let g = ui.painter().layout_no_wrap(f.path.clone(), FontId::proportional(12.5), p.text);
                    ui.painter().galley(pos2(r.left() + 24.0, r.center().y - g.size().y / 2.0), g.clone(), p.text);
                    let kind = if f.status == '?' { tr("untracked") } else { stash_status(f.status) };
                    ui.painter().text(pos2(r.left() + 32.0 + g.size().x, r.center().y), Align2::LEFT_CENTER, kind, FontId::proportional(11.5), p.muted);
                    if resp.clicked() {
                        diff = Some(f.clone());
                    }
                }
            });
        }
        None => {
            ru.add_space(20.0);
            ru.vertical_centered(|ui| ui.label(egui::RichText::new(tr("Select a stash to see its changes")).color(p.muted)));
        }
    }
    ui.advance_cursor_after_rect(avail);
    if let Some(h) = select {
        app.vcs.select_stash(&h);
    }
    if let (Some(f), Some(h)) = (diff, app.vcs.selected_stash.clone()) {
        app.vcs.open_stash_diff(&h, &f);
    }
    match action {
        Some(StashAction::Apply(st)) => app.vcs.stash_apply(&st, false),
        Some(StashAction::Pop(st)) => app.vcs.stash_apply(&st, true),
        Some(StashAction::Branch(stash)) => app.dialog = Some(Dialog::StashBranch { stash, name: String::new() }),
        Some(StashAction::Drop(stash)) => app.dialog = Some(Dialog::DropStash { stash }),
        None => {}
    }
}

enum StashAction {
    Apply(rc_vcs::Stash),
    Pop(rc_vcs::Stash),
    Drop(rc_vcs::Stash),
    Branch(rc_vcs::Stash),
}

/// `On main: message` → `message`; `WIP on main: abc123 subject` stays as it is.
fn stash_summary(message: &str) -> String {
    match message.strip_prefix("On ").and_then(|r| r.split_once(": ")) {
        Some((_, m)) => m.to_string(),
        None => message.to_string(),
    }
}

/// A word for a `--name-status` code.
fn stash_status(code: char) -> &'static str {
    match code {
        'A' => tr("added"),
        'D' => tr("deleted"),
        'R' => tr("renamed"),
        'C' => tr("copied"),
        _ => tr("modified"),
    }
}

/// The subject of a commit when the loaded log has it.
fn subject_of(log: &[rc_vcs::Commit], hash: &str) -> String {
    log.iter().find(|c| c.hash == hash).map(|c| c.subject.clone()).unwrap_or_default()
}

/// `origin` when there is one, else the first remote.
fn default_remote(remotes: &[String]) -> String {
    remotes.iter().find(|r| *r == "origin").or_else(|| remotes.first()).cloned().unwrap_or_default()
}

/// The actions on a tag, as menu items.
fn tag_actions(ui: &mut Ui, name: &str, busy: bool, has_remote: bool, remotes: &[String]) -> Option<Dialog> {
    let mut out = None;
    ui.add_enabled_ui(!busy, |ui| {
        if theme::menu_item(ui, None, tr("Checkout…"), false).clicked() {
            out = Some(Dialog::CheckoutTag { name: name.to_string() });
            ui.close();
        }
        if ui.add_enabled_ui(has_remote, |ui| theme::menu_item(ui, Some(ico::UP), tr("Push…"), false)).inner.clicked() {
            out = Some(Dialog::PushTags { name: Some(name.to_string()), remote: default_remote(remotes) });
            ui.close();
        }
        if theme::menu_item(ui, Some(ico::DELETE), tr("Delete…"), false).clicked() {
            out = Some(Dialog::DeleteTag { name: name.to_string() });
            ui.close();
        }
    });
    out
}

/// The actions on a tag, as buttons.
fn tag_buttons(ui: &mut Ui, name: &str, busy: bool, has_remote: bool, remotes: &[String]) -> Option<Dialog> {
    let mut out = None;
    if ui.add_enabled_ui(!busy, |ui| theme::secondary_button(ui, tr("Checkout…"))).inner.clicked() {
        out = Some(Dialog::CheckoutTag { name: name.to_string() });
    }
    let push = ui.add_enabled_ui(!busy && has_remote, |ui| theme::secondary_button(ui, tr("Push…"))).inner;
    if push.clicked() {
        out = Some(Dialog::PushTags { name: Some(name.to_string()), remote: default_remote(remotes) });
    }
    if !has_remote {
        push.on_disabled_hover_text(tr("The repository has no remote"));
    }
    if ui.add_enabled_ui(!busy, |ui| theme::secondary_button(ui, tr("Delete…"))).inner.clicked() {
        out = Some(Dialog::DeleteTag { name: name.to_string() });
    }
    out
}

/// The branches popup of the toolbar's VCS widget.
pub fn branches_menu(app: &mut CrabCraftApp, ui: &mut Ui) {
    ui.set_min_width(280.0);
    let busy = app.vcs.busy.is_some();
    if ui.add_enabled(!busy, egui::Button::new(tr("Update Project…"))).clicked() {
        app.vcs.pull();
        ui.close();
    }
    if theme::menu_item(ui, Some(ico::COMMIT), tr("Commit…"), false).clicked() {
        app.dock.show(ToolWindow::Commit);
        ui.close();
    }
    if ui.add_enabled(!busy, egui::Button::new(tr("Push…"))).clicked() {
        app.vcs.push();
        ui.close();
    }
    if ui.add_enabled(!busy, egui::Button::new(tr("Fetch"))).clicked() {
        app.vcs.fetch();
        ui.close();
    }
    ui.separator();
    if theme::menu_item(ui, Some(ico::ADD), tr("New Branch…"), false).clicked() {
        app.dialog = Some(crate::ui::dialogs::Dialog::NewBranch { name: String::new() });
        ui.close();
    }
    let free = !busy && app.vcs.operation.state.is_none();
    ui.add_enabled_ui(free, |ui| {
        if theme::menu_item(ui, None, tr("Merge into Current…"), false).clicked() {
            app.dialog = Some(Dialog::merge(String::new()));
            ui.close();
        }
        if theme::menu_item(ui, None, tr("Rebase Current onto…"), false).clicked() {
            app.dialog = Some(Dialog::Rebase { onto: String::new(), autostash: false });
            ui.close();
        }
    });
    let p = rc_kit::pal(ui);
    // Every branch, in a list that scrolls inside the window; the actions above stay in view.
    let room = (ui.ctx().content_rect().bottom() - ui.cursor().top() - 24.0).max(120.0);
    egui::ScrollArea::vertical().id_salt("branches_menu").max_height(room).show(ui, |ui| {
        for (title, kind) in [(tr("Local"), rc_vcs::BranchKind::Local), (tr("Remote"), rc_vcs::BranchKind::Remote)] {
            let list: Vec<rc_vcs::Branch> = app.vcs.branches.iter().filter(|b| b.kind == kind).cloned().collect();
            if list.is_empty() {
                continue;
            }
            ui.separator();
            ui.label(egui::RichText::new(title).size(11.5).color(p.muted));
            for b in list {
                let mut text = b.name.clone();
                if b.ahead > 0 {
                    text.push_str(&format!("  ↑{}", b.ahead));
                }
                if b.behind > 0 {
                    text.push_str(&format!("  ↓{}", b.behind));
                }
                let resp = ui.add_enabled(!busy, egui::Button::selectable(b.is_head, text));
                // Tab / arrow keys move through the list: keep the focused branch in view.
                if resp.gained_focus() {
                    resp.scroll_to_me(None);
                }
                if resp.clicked() && !b.is_head {
                    app.vcs.checkout(&b);
                    ui.close();
                }
            }
        }
    });
}

/// The side-by-side diff in the editor area (two read-only editors scrolled together).
pub fn diff_view(app: &mut CrabCraftApp, ui: &mut Ui) {
    let p = rc_kit::pal(ui);
    let full = ui.max_rect();
    let head = Rect::from_min_size(full.min, vec2(full.width(), 38.0));
    let mut close = false;
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(head.shrink2(vec2(10.0, 0.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    if let Some(d) = &app.vcs.diff {
        icons::image(&mut child, ico::DIFF, 16.0);
        child.label(egui::RichText::new(&d.title).font(theme::bold(13.0)));
        child.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if theme::icon_button(ui, ico::CLOSE_SMALL, true, tr("Close")).clicked() {
                close = true;
            }
            if theme::menu_item(ui, Some(ico::OPEN), tr("Open File"), false).clicked() {
                close = true;
                let path = app.vcs.diff.as_ref().map(|d| d.path.clone());
                if let Some(path) = path {
                    app.open_file(&path);
                }
            }
        });
    }
    ui.painter().hline(full.x_range(), head.bottom() - 0.5, egui::Stroke::new(1.0, p.island_border));
    if close || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.vcs.diff = None;
        app.focus_editor = true;
        return;
    }
    let Some(d) = &mut app.vcs.diff else { return };
    let body = Rect::from_min_max(pos2(full.left(), head.bottom()), full.max);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body));
    diff_editors(d, &mut child, &app.editor_config, &app.keymap);
}

/// Shared aligned, synchronized native diff panes.
pub fn diff_editors(d: &mut crate::vcs::DiffView, ui: &mut Ui, config: &rc_editor::EditorConfig, keymap: &rc_editor::Keymap) {
    let p = rc_kit::pal(ui);
    let body = ui.max_rect();
    let mid = body.center().x;
    let left = Rect::from_min_max(body.min, pos2(mid - 0.5, body.bottom()));
    let right = Rect::from_min_max(pos2(mid + 0.5, body.top()), body.max);
    ui.painter().vline(mid, body.y_range(), egui::Stroke::new(1.0, p.island_border));
    // The side under the pointer leads the scroll.
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let lead_right = pointer.is_some_and(|pt| right.contains(pt));
    let (lead, follow) = if lead_right { (&mut d.right, &mut d.left) } else { (&mut d.left, &mut d.right) };
    let off = lead.scroll_offset();
    follow.set_scroll_offset(off);
    for (rect, state) in [(left, &mut d.left), (right, &mut d.right)] {
        let mut eu = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        eu.set_clip_rect(rect.intersect(ui.clip_rect()));
        rc_editor::EditorView::new(state, config, keymap).show(&mut eu);
    }
    let _ = trf;
}

/// The commits a cherry-pick from `clicked` applies, oldest first: the marked ones when
/// `clicked` is among them, else just `clicked`.
fn cherry_pick_commits(vcs: &crate::vcs::Vcs, clicked: &str) -> Vec<String> {
    let mut commits = vcs.patch_commits(clicked);
    commits.reverse();
    commits
}

/// The local branch, else the remote branch or tag, pointing at a commit of the log.
fn ref_name(c: &rc_vcs::Commit) -> Option<String> {
    let local = c.refs.iter().find_map(|r| if let rc_vcs::RefLabel::LocalBranch(n) = r { Some(n.clone()) } else { None });
    local.or_else(|| {
        c.refs.iter().find_map(|r| match r {
            rc_vcs::RefLabel::RemoteBranch(n) | rc_vcs::RefLabel::Tag(n) => Some(n.clone()),
            _ => None,
        })
    })
}

/// A branch pointing at `hash` (local first), else its short hash.
pub fn rev_name(vcs: &crate::vcs::Vcs, hash: &str) -> String {
    match vcs.branches.iter().filter(|b| b.target == hash).min_by_key(|b| b.kind != rc_vcs::BranchKind::Local) {
        Some(b) => b.name.clone(),
        None => hash.chars().take(8).collect(),
    }
}

/// What the operation in progress does, in one line.
pub fn operation_text(vcs: &crate::vcs::Vcs) -> String {
    let op = &vcs.operation;
    let hash = op.target.clone().unwrap_or_default();
    let target = if hash.is_empty() { "?".to_string() } else { rev_name(vcs, &hash) };
    let short: String = hash.chars().take(8).collect();
    let branch = op.branch.clone().or_else(|| vcs.status.as_ref().and_then(|s| s.branch.head.clone())).unwrap_or_else(|| tr("HEAD").to_string());
    let subject = vcs.log.iter().find(|c| c.hash == hash).map(|c| c.subject.clone()).unwrap_or_default();
    match op.state {
        Some(rc_vcs::RepoState::Merging) => trf("Merging {rev} into {branch}", &[("rev", &target), ("branch", &branch)]),
        Some(rc_vcs::RepoState::Rebasing) => match op.step {
            Some((n, total)) => trf("Rebasing {branch} onto {rev} (commit {n} of {total})", &[("branch", &branch), ("rev", &target), ("n", &n), ("total", &total)]),
            None => trf("Rebasing {branch} onto {rev}", &[("branch", &branch), ("rev", &target)]),
        },
        Some(rc_vcs::RepoState::CherryPicking) => trf("Cherry-picking {rev} {subject}", &[("rev", &short), ("subject", &subject)]),
        Some(rc_vcs::RepoState::Reverting) => trf("Reverting {rev} {subject}", &[("rev", &short), ("subject", &subject)]),
        _ => String::new(),
    }
}

/// The merge, rebase or cherry-pick in progress (V-07): what it does, its conflicts and
/// Continue / Skip / Abort; while one runs, its progress and Cancel; afterwards, what git said.
pub fn operation_banner(app: &mut CrabCraftApp, ui: &mut Ui) {
    let running = app.vcs.sequence_running();
    let state = app.vcs.operation.state;
    let output = app.vcs.op_output.clone();
    if state.is_none() && !running && output.is_none() {
        return;
    }
    let p = rc_kit::pal(ui);
    let conflicts = app.vcs.conflicts();
    let busy = app.vcs.busy.is_some();
    let mut action: Option<Dialog> = None;
    let frame = egui::Frame::new().fill(p.header_bg).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        if running {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0));
                let line = app.vcs.op_progress.clone().unwrap_or_else(|| tr("Running Git…").to_string());
                ui.label(egui::RichText::new(line).size(12.5));
                if theme::secondary_button(ui, tr("Cancel")).on_hover_text(tr("Stop Git; what it already did stays in progress")).clicked() {
                    app.vcs.cancel_operation();
                }
            });
        } else if let Some(state) = state {
            ui.label(egui::RichText::new(operation_text(&app.vcs)).font(theme::bold(13.0)));
            ui.horizontal_wrapped(|ui| {
                if conflicts.is_empty() {
                    ui.label(egui::RichText::new(tr("No conflicts left")).size(12.5).color(p.ok));
                } else {
                    ui.label(egui::RichText::new(trf("{n} conflicted files", &[("n", &conflicts.len())])).size(12.5).color(p.error));
                    if ui.add_enabled_ui(!busy, |ui| theme::primary_button(ui, tr("Resolve Conflicts…"))).inner.clicked() {
                        action = Some(Dialog::Conflicts { selected: Default::default() });
                    }
                }
                let cont = ui.add_enabled_ui(!busy && conflicts.is_empty(), |ui| theme::secondary_button(ui, tr("Continue"))).inner;
                if !conflicts.is_empty() {
                    cont.on_disabled_hover_text(tr("Resolve every conflict first"));
                } else if cont.on_hover_text(tr("Commit the resolved changes and go on")).clicked() {
                    app.vcs.continue_operation();
                }
                if state != rc_vcs::RepoState::Merging
                    && ui.add_enabled_ui(!busy, |ui| theme::secondary_button(ui, tr("Skip Commit"))).inner.on_hover_text(tr("Leave this commit out and go on")).clicked()
                {
                    app.vcs.skip_operation();
                }
                if ui.add_enabled_ui(!busy, |ui| theme::secondary_button(ui, tr("Abort…"))).inner.clicked() {
                    action = Some(Dialog::AbortOperation);
                }
            });
        }
        if !running && let Some(o) = &output {
            ui.horizontal(|ui| {
                // The last lines git printed; all of them in the tooltip.
                let lines: Vec<&str> = o.text.lines().filter(|l| !l.trim().is_empty()).collect();
                let shown = if lines.is_empty() { tr("Done").to_string() } else { lines[lines.len().saturating_sub(4)..].join("\n") };
                let resp = ui.label(egui::RichText::new(shown).font(FontId::monospace(11.5)).color(if o.error { p.error } else { p.muted }));
                if lines.len() > 4 {
                    resp.on_hover_text(&o.text);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    if theme::icon_button(ui, ico::CLOSE_SMALL, true, tr("Close")).clicked() {
                        app.vcs.op_output = None;
                    }
                });
            });
        }
    });
    let r = frame.response.rect;
    ui.painter().hline(r.x_range(), r.bottom() + 0.5, egui::Stroke::new(1.0, p.island_border));
    if let Some(d) = action {
        app.dialog = Some(d);
    }
}

/// Names of the two sides of a conflict: the index's stage 2 ("ours") and stage 3 ("theirs").
/// During a rebase, ours is what the branch is rebased onto and theirs your commit replayed.
pub fn side_names(vcs: &crate::vcs::Vcs) -> (String, String) {
    let op = &vcs.operation;
    let branch = vcs.status.as_ref().and_then(|s| s.branch.head.clone()).unwrap_or_else(|| tr("HEAD").to_string());
    let short = |h: &Option<String>| h.as_deref().unwrap_or("").chars().take(8).collect::<String>();
    match op.state {
        Some(rc_vcs::RepoState::Rebasing) => {
            let onto = op.target.as_deref().map(|h| rev_name(vcs, h)).unwrap_or_default();
            (trf("Upstream ({rev})", &[("rev", &onto)]), trf("Your commit ({rev})", &[("rev", &short(&op.commit))]))
        }
        Some(rc_vcs::RepoState::Merging) => {
            let theirs = op.target.as_deref().map(|h| rev_name(vcs, h)).unwrap_or_default();
            (trf("Yours ({branch})", &[("branch", &branch)]), trf("Theirs ({rev})", &[("rev", &theirs)]))
        }
        Some(rc_vcs::RepoState::CherryPicking | rc_vcs::RepoState::Reverting) => (trf("Yours ({branch})", &[("branch", &branch)]), trf("Commit {rev}", &[("rev", &short(&op.target))])),
        _ => (tr("Yours").to_string(), tr("Theirs").to_string()),
    }
}

/// The three-pane merge tool in the editor area (V-08): ours | result | theirs, with arrows to
/// take a side for each change, a toolbar for the selected change and the whole file, and
/// Save and Mark Resolved.
pub fn merge_view(app: &mut CrabCraftApp, ui: &mut Ui) {
    let Some(tool) = &mut app.vcs.merge_tool else { return };
    match merge_editor(tool, ui, &app.editor_config, &app.keymap, tr("Save and Mark Resolved")) {
        MergeAction::None => {},
        MergeAction::Close => { app.vcs.merge_tool = None; app.focus_editor = true; },
        MergeAction::Save => app.vcs.save_merge_tool(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergeAction { None, Close, Save }

/// Shared native merge editor; the caller decides whether saving stages a Git file or writes
/// an explicitly requested standalone output.
pub fn merge_editor(tool: &mut crate::merge_tool::MergeTool, ui: &mut Ui, editor_config: &rc_editor::EditorConfig, keymap: &rc_editor::Keymap, save_label: &str) -> MergeAction {
    use rc_vcs::{Pane, Take};
    let p = rc_kit::pal(ui);
    let full = ui.max_rect();
    tool.sync();
    let busy = tool.saving;
    let conflicts = tool.model.conflicts_left();
    let unresolved = tool.model.unresolved();
    let mut close = false;
    let mut save = false;
    // Header: the file, what is left, Close.
    let head = Rect::from_min_size(full.min, vec2(full.width(), 38.0));
    let mut hu = ui.new_child(egui::UiBuilder::new().max_rect(head.shrink2(vec2(10.0, 0.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    icons::image(&mut hu, ico::DIFF, 16.0);
    hu.label(egui::RichText::new(trf("Merge {file}", &[("file", &tool.rel)])).font(theme::bold(13.0)));
    let (status, color) = if unresolved == 0 {
        (tr("All changes resolved").to_string(), p.ok)
    } else if conflicts > 0 {
        (trf("{n} conflicts left", &[("n", &conflicts)]), p.error)
    } else {
        (trf("{n} changes not taken", &[("n", &unresolved)]), p.muted)
    };
    hu.label(egui::RichText::new(status).size(12.5).color(color));
    hu.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if theme::icon_button(ui, ico::CLOSE_SMALL, !busy, tr("Close")).clicked() {
            close = true;
        }
        if let Some(e) = &tool.error {
            ui.label(egui::RichText::new(e).size(12.5).color(p.error));
        } else if tool.confirm_save {
            ui.label(egui::RichText::new(trf("{n} changes are not resolved.", &[("n", &unresolved)])).size(12.5).color(p.error));
        }
    });
    ui.painter().hline(full.x_range(), head.bottom() - 0.5, egui::Stroke::new(1.0, p.island_border));
    // Toolbar: the selected change, the whole file, save.
    let bar = Rect::from_min_size(pos2(full.left(), head.bottom()), vec2(full.width(), 36.0));
    let mut bu = ui.new_child(egui::UiBuilder::new().max_rect(bar.shrink2(vec2(10.0, 0.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    let has_current = tool.current.is_some_and(|c| c < tool.model.chunks.len());
    let none = tool.model.chunks.is_empty();
    if theme::icon_button(&mut bu, ico::CHEVRON_UP, !none, tr("Previous Change")).clicked() {
        tool.step(false);
    }
    if theme::icon_button(&mut bu, ico::CHEVRON_DOWN, !none, tr("Next Change")).clicked() {
        tool.step(true);
    }
    bu.add_enabled_ui(has_current && !busy, |ui| {
        let cur = tool.current.unwrap_or(0);
        if theme::secondary_button(ui, tr("Accept Left")).on_hover_text(tool.names.0.as_str()).clicked() {
            tool.take(cur, Take::Ours);
        }
        if theme::secondary_button(ui, tr("Accept Right")).on_hover_text(tool.names.1.as_str()).clicked() {
            tool.take(cur, Take::Theirs);
        }
        if theme::secondary_button(ui, tr("Accept Both")).on_hover_text(tr("Left, then right")).clicked() {
            tool.take(cur, Take::Both);
        }
        if theme::secondary_button(ui, tr("Keep Base")).on_hover_text(tr("Neither change: the text both sides started from")).clicked() {
            tool.take(cur, Take::Base);
        }
    });
    bu.separator();
    bu.add_enabled_ui(!busy, |ui| {
        if theme::secondary_button(ui, tr("Apply Non-Conflicting Changes")).clicked() {
            tool.take_non_conflicting();
        }
        if theme::secondary_button(ui, tr("All from Left")).on_hover_text(tool.names.0.as_str()).clicked() {
            tool.take_file(false);
        }
        if theme::secondary_button(ui, tr("All from Right")).on_hover_text(tool.names.1.as_str()).clicked() {
            tool.take_file(true);
        }
    });
    bu.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if busy {
            ui.add(egui::Spinner::new().size(14.0));
        } else if tool.confirm_save {
            if theme::primary_button(ui, tr("Save Anyway")).clicked() {
                save = true;
            }
            if theme::secondary_button(ui, tr("Cancel")).clicked() {
                tool.confirm_save = false;
            }
        } else if theme::primary_button(ui, save_label).clicked() {
            if unresolved > 0 {
                tool.confirm_save = true;
            } else {
                save = true;
            }
        }
    });
    // Column titles.
    let gap = 26.0;
    let titles = Rect::from_min_size(pos2(full.left(), bar.bottom()), vec2(full.width(), 24.0));
    ui.painter().rect_filled(titles, 0.0, p.header_bg);
    let col = (full.width() - 2.0 * gap) / 3.0;
    let xs = [full.left(), full.left() + col + gap, full.left() + 2.0 * (col + gap)];
    for (x, name) in xs.iter().zip([tool.names.0.as_str(), tr("Result"), tool.names.1.as_str()]) {
        ui.painter().text(pos2(x + 10.0, titles.center().y), Align2::LEFT_CENTER, name, theme::bold(12.0), p.text);
    }
    let body = Rect::from_min_max(pos2(full.left(), titles.bottom()), full.max);
    let rects = xs.map(|x| Rect::from_min_max(pos2(x, body.top()), pos2(x + col, body.bottom())));
    // Scroll: the pane under the pointer leads; the others follow through the changes.
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let panes = [Pane::Ours, Pane::Result, Pane::Theirs];
    let lead = rects.iter().position(|r| pointer.is_some_and(|pt| r.contains(pt))).unwrap_or(1);
    let row_h = tool.center.row_height();
    if tool.reveal && row_h > 0.0 {
        tool.reveal = false;
        if let Some(c) = tool.current.filter(|c| *c < tool.model.chunks.len()) {
            for (i, pane) in panes.iter().enumerate() {
                let line = tool.line_of(c, *pane);
                let e = match i {
                    0 => &mut tool.left,
                    1 => &mut tool.center,
                    _ => &mut tool.right,
                };
                e.set_scroll_offset(vec2(0.0, (line.saturating_sub(4) as f32 * row_h).max(0.0)));
            }
        }
    } else if row_h > 0.0 {
        let lead_state = match lead {
            0 => &tool.left,
            1 => &tool.center,
            _ => &tool.right,
        };
        let y = lead_state.scroll_offset().y;
        let top = (y / row_h).floor();
        let frac = y - top * row_h;
        for (i, pane) in panes.iter().enumerate() {
            if i == lead {
                continue;
            }
            let line = tool.model.map_line(panes[lead], top as usize, *pane);
            let e = match i {
                0 => &mut tool.left,
                1 => &mut tool.center,
                _ => &mut tool.right,
            };
            let x = e.scroll_offset().x;
            e.set_scroll_offset(vec2(x, line as f32 * row_h + frac));
        }
    }
    let mut config = editor_config.clone();
    config.sticky_lines = 0;
    for (rect, i) in rects.iter().zip(0..3) {
        let state = match i {
            0 => &mut tool.left,
            1 => &mut tool.center,
            _ => &mut tool.right,
        };
        let mut eu = ui.new_child(egui::UiBuilder::new().max_rect(*rect));
        eu.set_clip_rect(rect.intersect(ui.clip_rect()));
        rc_editor::EditorView::new(state, &config, keymap).show(&mut eu);
    }
    // The arrows between the panes: take the left side (») or the right side («) of a change.
    let mut act: Option<(usize, Take)> = None;
    for (g, gx) in [(0usize, rects[0].right()), (1, rects[1].right())] {
        let strip = Rect::from_min_max(pos2(gx, body.top()), pos2(gx + gap, body.bottom()));
        ui.painter().rect_filled(strip, 0.0, p.header_bg);
        let (state, pane, take, glyph, tip) =
            if g == 0 { (&tool.left, Pane::Ours, Take::Ours, "»", tool.names.0.as_str()) } else { (&tool.right, Pane::Theirs, Take::Theirs, "«", tool.names.1.as_str()) };
        let rh = state.row_height();
        if rh <= 0.0 {
            continue;
        }
        let scroll = state.scroll_offset().y;
        for (ci, c) in tool.model.chunks.iter().enumerate() {
            let changed = match c.kind {
                rc_vcs::ChunkKind::Conflict | rc_vcs::ChunkKind::Same => true,
                rc_vcs::ChunkKind::Ours => g == 0,
                rc_vcs::ChunkKind::Theirs => g == 1,
            };
            if !changed || c.resolved {
                continue;
            }
            let line = tool.line_of(ci, pane);
            let y = body.top() + line as f32 * rh - scroll + rh / 2.0;
            if y < body.top() + 4.0 || y > body.bottom() - 4.0 {
                continue;
            }
            let r = Rect::from_center_size(pos2(strip.center().x, y), vec2(20.0, rh.min(20.0)));
            let id = ui.id().with(("merge_take", g, ci));
            let resp = ui.interact(r, id, Sense::click()).on_hover_text(trf("Accept {side}", &[("side", &tip)]));
            let color = if resp.hovered() { p.accent } else { p.text };
            if resp.hovered() {
                ui.painter().rect_filled(r, 3.0, p.row_hover);
            }
            ui.painter().text(r.center(), Align2::CENTER_CENTER, glyph, FontId::proportional(15.0), color);
            if resp.clicked() && !busy {
                act = Some((ci, take));
            }
        }
    }
    if let Some((ci, take)) = act {
        tool.take(ci, take);
    }
    if !busy && (close || ui.input(|i| i.key_pressed(egui::Key::Escape))) {
        return MergeAction::Close;
    }
    if save { MergeAction::Save } else { MergeAction::None }
}
