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

pub fn commit_window(app: &mut CrabCraftApp, ui: &mut Ui) {
    if header(ui, tr("Commit")) {
        app.toggle_tool(ToolWindow::Commit);
    }
    if app.vcs.repo.is_none() {
        empty_state(ui, ico::TW_COMMIT, tr("No Git repository"), tr("The workspace is not inside a Git repository."));
        return;
    }
    let p = rc_kit::pal(ui);
    let changes = app.vcs.changes();
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
                    if c.untracked || c.state == FileState::Added {
                        open_file = Some(root.join(&c.path));
                    } else {
                        open_diff = Some(c.path.clone());
                    }
                }
                resp.context_menu(|ui| {
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
    child.add(egui::TextEdit::multiline(&mut app.vcs.message).hint_text(tr("Commit Message")).desired_rows(4).desired_width(f32::INFINITY));
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
    if app.vcs.repo.is_none() {
        empty_state(ui, ico::TW_VCS, tr("No Git repository"), tr("The workspace is not inside a Git repository."));
        return;
    }
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
        });
    });
    match app.vcs.tab {
        GitTab::Log => log_view(app, ui),
        GitTab::Tags => tags_view(app, ui),
        GitTab::Stashes => stashes_view(app, ui),
        GitTab::Worktrees => worktrees_view(app, ui),
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
            app.dialog = Some(Dialog::StashChanges { message: String::new(), include_untracked: false, keep_index: false });
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
        Some(StashAction::Drop(stash)) => app.dialog = Some(Dialog::DropStash { stash }),
        None => {}
    }
}

enum StashAction {
    Apply(rc_vcs::Stash),
    Pop(rc_vcs::Stash),
    Drop(rc_vcs::Stash),
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
    let config = app.editor_config.clone();
    for (rect, state) in [(left, &mut d.left), (right, &mut d.right)] {
        let mut eu = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        eu.set_clip_rect(rect.intersect(ui.clip_rect()));
        rc_editor::EditorView::new(state, &config, &app.keymap).show(&mut eu);
    }
    let _ = trf;
}
