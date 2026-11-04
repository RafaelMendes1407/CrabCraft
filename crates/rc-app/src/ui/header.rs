//! Main toolbar (New UI): project widget, VCS widget, run configuration widget, run/debug
//! buttons, search and settings. It doubles as the window's title bar.

use crate::app::CrabCraftApp;
use egui::{Align2, Color32, FontId, Rect, Sense, Ui, pos2, vec2};
use rc_editor::Action;
use rc_kit::i18n::{tr, trf};
use rc_kit::{ico, icons, theme};

/// Colours of the project badge (New UI picks one per project).
const BADGE_COLORS: [(u32, u32); 6] = [
    (0x7F5BFF, 0x5A3ACF),
    (0x3D7EFF, 0x2556C5),
    (0x00A389, 0x00785F),
    (0xE5713B, 0xB5521F),
    (0xD9408F, 0xA82C6A),
    (0x8E9A1B, 0x6A7410),
];

fn rgb(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// Two letters for the project badge: initials of the first two words, or the first two
/// letters ("TenProjects" → "TP", "shop" → "SH").
pub fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let caps: Vec<char> = name.chars().filter(|c| c.is_uppercase()).collect();
    let s: String = if words.len() >= 2 {
        words.iter().take(2).filter_map(|w| w.chars().next()).collect()
    } else if caps.len() >= 2 {
        caps.iter().take(2).collect()
    } else {
        name.chars().filter(|c| c.is_alphanumeric()).take(2).collect()
    };
    s.to_uppercase()
}

/// A rounded rectangle filled with a vertical gradient (a fan of triangles from its centre).
fn gradient_rounded_rect(r: Rect, radius: f32, top: Color32, bottom: Color32) -> egui::Mesh {
    let mut pts = Vec::new();
    let corners = [(r.right() - radius, r.top() + radius, -90.0f32), (r.right() - radius, r.bottom() - radius, 0.0), (r.left() + radius, r.bottom() - radius, 90.0), (r.left() + radius, r.top() + radius, 180.0)];
    for (cx, cy, start) in corners {
        for k in 0..=8 {
            let a = (start + k as f32 * 90.0 / 8.0).to_radians();
            pts.push(pos2(cx + radius * a.cos(), cy + radius * a.sin()));
        }
    }
    let color_at = |y: f32| {
        let t = ((y - r.top()) / r.height()).clamp(0.0, 1.0);
        Color32::from_rgb(
            (top.r() as f32 + (bottom.r() as f32 - top.r() as f32) * t) as u8,
            (top.g() as f32 + (bottom.g() as f32 - top.g() as f32) * t) as u8,
            (top.b() as f32 + (bottom.b() as f32 - top.b() as f32) * t) as u8,
        )
    };
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(r.center(), color_at(r.center().y));
    for p in &pts {
        mesh.colored_vertex(*p, color_at(p.y));
    }
    let n = pts.len() as u32;
    for i in 0..n {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
    mesh
}

pub fn show(app: &mut CrabCraftApp, ui: &mut Ui) {
    let rect = ui.max_rect();
    // The whole bar drags the window (behind the widgets).
    let drag = ui.interact(rect, ui.id().with("title_drag"), Sense::click_and_drag());
    if drag.drag_started() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
    if drag.double_clicked() {
        let maximized = ui.input(|i| i.viewport().maximized.unwrap_or(false));
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
    }
    let fullscreen = ui.input(|i| i.viewport().fullscreen.unwrap_or(false));
    ui.horizontal_centered(|ui| {
        theme::split_row(ui, |ui, side| match side {
            theme::Side::Left => {
                ui.spacing_mut().item_spacing.x = 4.0;
                let zoom = ui.ctx().zoom_factor().max(0.1);
                ui.add_space(rc_kit::window_chrome::leading_space(cfg!(target_os = "macos"), fullscreen, zoom));
                project_widget(app, ui);
                if app.workspace.is_some() {
                    // VCS widget: the current branch and the branches popup.
                    if app.vcs.repo.is_some() {
                        let name = app.vcs.branch_name().unwrap_or_else(|| "…".into());
                        let resp = theme::flat_button(ui, Some(ico::VCS), &name, FontId::proportional(13.0), true, true).on_hover_text(tr("Git branches and actions"));
                        egui::Popup::menu(&resp).show(|ui| crate::ui::git::branches_menu(app, ui));
                    }
                }
            }
            theme::Side::Right => {
                if cfg!(target_os = "windows") {
                    if let Some(command) = rc_kit::window_chrome::controls(ui) {
                        ui.ctx().send_viewport_cmd(command);
                    }
                    ui.add_space(8.0);
                }
                ui.add_space(6.0);
                ui.spacing_mut().item_spacing.x = 2.0;
                let keymap = app.keymap.clone();
                let shortcut = |a: Action| keymap.shortcut_text(a).map(|s| format!(" ({s})")).unwrap_or_default();
                if theme::icon_button_sized(ui, ico::SETTINGS, true, &format!("{}{}", tr("Settings"), shortcut(Action::ShowSettings)), 30.0).clicked() {
                    app.pending.push(Action::ShowSettings);
                }
                let search_tip = format!("{} (⇧⇧)", tr("Search Everywhere"));
                if theme::icon_button_sized(ui, ico::SEARCH, app.workspace.is_some(), &search_tip, 30.0).clicked() {
                    app.pending.push(Action::SearchEverywhere);
                }
                if app.workspace.is_some() {
                    ui.add_space(6.0);
                    theme::toolbar_separator(ui);
                    ui.add_space(4.0);
                    let more = theme::icon_button_sized(ui, ico::MORE_VERT, true, tr("More actions"), 30.0);
                    egui::Popup::menu(&more).show(|ui| {
                        ui.set_min_width(200.0);
                        // The user's external tools, then the settings that list them.
                        ui.menu_button(tr("External Tools"), |ui| {
                            let names: Vec<String> = app.settings.external_tools.iter().map(|t| t.name.clone()).collect();
                            for name in names {
                                if theme::menu_item(ui, Some(ico::RUN_ANYTHING), &name, false).clicked() {
                                    app.run_external_tool(&name);
                                    ui.close();
                                }
                            }
                            if !app.settings.external_tools.is_empty() {
                                ui.separator();
                            }
                            if theme::menu_item(ui, Some(ico::SETTINGS), tr("Configure External Tools…"), false).clicked() {
                                app.open_settings(crate::ui::dialogs::SettingsPage::ExternalTools);
                                ui.close();
                            }
                        });
                        if theme::menu_item(ui, Some(ico::REBUILD), tr("Rebuild Project"), false).clicked() {
                            if let Some(t) = app.workspace.as_ref().map(|w| w.ws.key_path().to_path_buf()) {
                                let outs = app.runner.build(&t, rc_dotnet::BuildAction::Rebuild);
                                crate::ui::run_windows::handle_outputs(app, outs);
                            }
                            ui.close();
                        }
                        if theme::menu_item(ui, Some(ico::DELETE), tr("Clean Project"), false).clicked() {
                            if let Some(t) = app.workspace.as_ref().map(|w| w.ws.key_path().to_path_buf()) {
                                let outs = app.runner.build(&t, rc_dotnet::BuildAction::Clean);
                                crate::ui::run_windows::handle_outputs(app, outs);
                            }
                            ui.close();
                        }
                        if theme::menu_item(ui, Some(ico::DOWNLOAD), tr("Fetch Dependencies"), false).clicked() {
                            if let Some(t) = app.workspace.as_ref().map(|w| w.ws.key_path().to_path_buf()) {
                                let outs = app.runner.build(&t, rc_dotnet::BuildAction::Restore);
                                crate::ui::run_windows::handle_outputs(app, outs);
                            }
                            ui.close();
                        }
                    });
                    if (app.runner.busy() || app.debugger.active()) && theme::icon_button_sized(ui, ico::STOP, true, &format!("{}{}", tr("Stop"), shortcut(Action::StopRun)), 30.0).clicked() {
                        app.pending.push(Action::StopRun);
                    }
                    if theme::icon_button_sized(ui, ico::DEBUG, true, &format!("{}{}", tr("Debug"), shortcut(Action::Debug)), 30.0).clicked() {
                        app.pending.push(Action::Debug);
                    }
                    if theme::icon_button_sized(ui, ico::RUN, true, &format!("{}{}", tr("Run"), shortcut(Action::Run)), 30.0).clicked() {
                        app.pending.push(Action::Run);
                    }
                    if theme::icon_button_sized(ui, ico::BUILD, true, &format!("{}{}", tr("Build Project"), shortcut(Action::Build)), 30.0).clicked() {
                        app.pending.push(Action::Build);
                    }
                    ui.add_space(2.0);
                    let (icon, label) = match app.runner.selected_config() {
                        Some(c) => (c.icon(), c.name.clone()),
                        None => (ico::CONFIG_APP, tr("No run configurations").to_string()),
                    };
                    let resp = theme::flat_button(ui, Some(icon), &label, FontId::proportional(13.0), true, true).on_hover_text(tr("Choose what Run and Debug start"));
                    egui::Popup::menu(&resp).show(|ui| {
                        ui.set_min_width(240.0);
                        if app.runner.configs.is_empty() {
                            ui.label(egui::RichText::new(tr("The workspace has no executable or test projects")).color(rc_kit::pal(ui).muted));
                        }
                        let mut chosen = None;
                        let config_item = |ui: &mut egui::Ui, i: usize, c: &crate::runner::RunConfig, chosen: &mut Option<usize>| {
                            // Temporary configurations are told apart, as in RustRover.
                            let temporary = app.runner.is_temporary(&c.name);
                            let label = if temporary { trf("{name} (temporary)", &[("name", &c.name)]) } else { c.name.clone() };
                            if theme::menu_item(ui, Some(c.icon()), &label, i == app.runner.selected).clicked() {
                                *chosen = Some(i);
                                ui.close();
                            }
                        };
                        // Configurations in folders (B-21) are listed in a submenu per folder.
                        for (i, c) in app.runner.configs.iter().enumerate().filter(|(_, c)| app.runner.folder_of(&c.name).is_none()) {
                            config_item(ui, i, c, &mut chosen);
                        }
                        for folder in app.runner.folder_names() {
                            let holds_selected = app.runner.selected_config().is_some_and(|c| app.runner.folder_of(&c.name) == Some(folder.as_str()));
                            let text = egui::RichText::new(folder.as_str()).strong();
                            let text = if holds_selected { text.color(rc_kit::pal(ui).accent) } else { text };
                            ui.menu_button(text, |ui| {
                                ui.set_min_width(220.0);
                                for (i, c) in app.runner.configs.iter().enumerate().filter(|(_, c)| app.runner.folder_of(&c.name) == Some(folder.as_str())) {
                                    config_item(ui, i, c, &mut chosen);
                                }
                            });
                        }
                        if let Some(i) = chosen {
                            app.runner.selected = i;
                            app.session_dirty = true;
                        }
                        // Folders of the selected configuration.
                        if let Some(name) = app.runner.selected_config().map(|c| c.name.clone()) {
                            ui.separator();
                            let current = app.runner.folder_of(&name).map(str::to_string);
                            ui.menu_button(trf("Move “{name}” to Folder", &[("name", &name)]), |ui| {
                                ui.set_min_width(200.0);
                                for folder in app.runner.folder_names().into_iter().filter(|f| Some(f) != current.as_ref()) {
                                    if theme::menu_item(ui, Some(ico::FOLDER), &folder, false).clicked() {
                                        app.runner.move_to_folder(&name, Some(&folder));
                                        app.session_dirty = true;
                                        ui.close();
                                    }
                                }
                                if theme::menu_item(ui, Some(ico::ADD), tr("New Folder…"), false).clicked() {
                                    app.dialog = Some(crate::ui::dialogs::Dialog::ConfigFolder { config: name.clone(), rename: None, folder: String::new() });
                                    ui.close();
                                }
                                if current.is_some() && theme::menu_item(ui, Some(ico::CLOSE), tr("Out of the Folder"), false).clicked() {
                                    app.runner.move_to_folder(&name, None);
                                    app.session_dirty = true;
                                    ui.close();
                                }
                            });
                            if let Some(folder) = current
                                && theme::menu_item(ui, Some(ico::EDIT), &trf("Rename Folder “{folder}”…", &[("folder", &folder)]), false).clicked()
                            {
                                app.dialog = Some(crate::ui::dialogs::Dialog::ConfigFolder { config: name.clone(), rename: Some(folder.clone()), folder });
                                ui.close();
                            }
                        }
                        if let Some(crate::runner::RunConfig { name, kind: crate::runner::ConfigKind::App { .. } | crate::runner::ConfigKind::Cargo { .. } }) = app.runner.selected_config().cloned() {
                            ui.separator();
                            if theme::menu_item(ui, Some(ico::SETTINGS), &trf("Edit “{name}”…", &[("name", &name)]), false).clicked() {
                                app.edit_run_config(Some(name));
                                ui.close();
                            }
                        }
                        if let Some(crate::runner::RunConfig { name, kind: crate::runner::ConfigKind::TestSelection { .. } }) = app.runner.selected_config().cloned() {
                            ui.separator();
                            if app.runner.is_temporary(&name) && theme::menu_item(ui, Some(ico::SAVE), &trf("Save “{name}” Configuration", &[("name", &name)]), false).clicked() {
                                app.runner.save_temporary(&name);
                                app.session_dirty = true;
                                ui.close();
                            }
                            if theme::menu_item(ui, Some(ico::DELETE), &trf("Delete “{name}”", &[("name", &name)]), false).clicked() {
                                app.runner.remove_test_config(&name);
                                app.session_dirty = true;
                                ui.close();
                            }
                        }
                        if app.runner.cargo_workspace.is_some() {
                            ui.separator();
                            if theme::menu_item(ui, Some(ico::DEBUG), tr("Attach to Process…"), false).clicked() {
                                app.pending.push(rc_editor::Action::AttachToProcess);
                                ui.close();
                            }
                        }
                        // Compound configurations: several applications at once.
                        if app.runner.app_names().len() > 1 {
                            ui.separator();
                            if theme::menu_item(ui, Some(ico::ADD), tr("Add Compound Configuration…"), false).clicked() {
                                let members = app.runner.app_names().into_iter().map(|n| (n, true)).collect();
                                app.dialog = Some(crate::ui::dialogs::Dialog::Compound { old: None, name: tr("All Applications").to_string(), members });
                                ui.close();
                            }
                            if let Some(crate::runner::RunConfig { name, kind: crate::runner::ConfigKind::Compound { members } }) = app.runner.selected_config().cloned() {
                                if theme::menu_item(ui, Some(ico::EDIT), &trf("Edit “{name}”…", &[("name", &name)]), false).clicked() {
                                    let all = app.runner.app_names().into_iter().map(|n| { let on = members.contains(&n); (n, on) }).collect();
                                    app.dialog = Some(crate::ui::dialogs::Dialog::Compound { old: Some(name.clone()), name: name.clone(), members: all });
                                    ui.close();
                                }
                                if theme::menu_item(ui, Some(ico::DELETE), &trf("Delete “{name}”", &[("name", &name)]), false).clicked() {
                                    app.runner.remove_compound(&name);
                                    app.session_dirty = true;
                                    ui.close();
                                }
                            }
                        }
                    });
                    // Back / Forward through the navigation history (⌘[ / ⌘]).
                    ui.add_space(4.0);
                    theme::toolbar_separator(ui);
                    ui.add_space(4.0);
                    if theme::icon_button_sized(ui, ico::FORWARD, !app.nav_forward.is_empty(), &format!("{}{}", tr("Forward"), shortcut(Action::NavigateForward)), 30.0).clicked() {
                        app.navigate(false);
                    }
                    if theme::icon_button_sized(ui, ico::BACK, !app.nav_back.is_empty(), &format!("{}{}", tr("Back"), shortcut(Action::NavigateBack)), 30.0).clicked() {
                        app.navigate(true);
                    }
                }
            }
        });
    });
}

fn project_widget(app: &mut CrabCraftApp, ui: &mut Ui) {
    let p = rc_kit::pal(ui);
    let name = app.workspace.as_ref().map_or_else(|| "CrabCraft".to_string(), |w| w.ws.name());
    let font = theme::bold(13.0);
    let galley = ui.painter().layout_no_wrap(name.clone(), font, p.text);
    let w = 8.0 + 20.0 + 8.0 + galley.size().x + 4.0 + 16.0 + 6.0;
    let (r, resp) = ui.allocate_exact_size(vec2(w, 28.0), Sense::click());
    if resp.hovered() || resp.is_pointer_button_down_on() {
        ui.painter().rect_filled(r, 6.0, if resp.is_pointer_button_down_on() { p.pressed } else { p.hover });
    }
    let sq = Rect::from_min_size(pos2(r.left() + 8.0, r.center().y - 10.0), vec2(20.0, 20.0));
    if app.workspace.is_some() {
        let hash = name.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
        let (a, b) = BADGE_COLORS[(hash as usize) % BADGE_COLORS.len()];
        ui.painter().add(egui::Shape::mesh(gradient_rounded_rect(sq, 5.0, rgb(a), rgb(b))));
        ui.painter().text(sq.center() + vec2(0.0, 0.5), Align2::CENTER_CENTER, initials(&name), theme::bold(10.0), Color32::WHITE);
    } else {
        rc_kit::branding::paint(ui, sq);
    }
    ui.painter().galley(pos2(sq.right() + 8.0, (r.center().y - galley.size().y / 2.0).round()), galley.clone(), p.text);
    icons::paint(ui.painter(), Rect::from_center_size(pos2(sq.right() + 8.0 + galley.size().x + 4.0 + 8.0, r.center().y), vec2(16.0, 16.0)), ico::CHEVRON_DOWN, &p, true);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &name));
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(260.0);
        if theme::menu_item(ui, Some(ico::ADD), tr("New Project…"), false).clicked() {
            app.new_project_dialog(None);
            ui.close();
        }
        if theme::menu_item(ui, Some(ico::NEW_FILE), tr("New File…"), false).clicked() {
            app.new_file_dialog();
            ui.close();
        }
        if let Some(sln) = app.workspace.as_ref().and_then(|w| w.ws.solution().map(std::path::Path::to_path_buf))
            && theme::menu_item(ui, Some(ico::ADD), tr("Add New Project…"), false).clicked()
        {
            app.new_project_dialog(Some(sln));
            ui.close();
        }
        if theme::menu_item(ui, Some(ico::OPEN), tr("Open…"), false).clicked() {
            app.pick_and_open();
            ui.close();
        }
        if theme::menu_item(ui, Some(ico::SOLUTION), tr("Open Cargo.toml…"), false).clicked() {
            app.pick_solution();
            ui.close();
        }
        if theme::menu_item(ui, Some(ico::OPEN), tr("Open File…"), false).clicked() {
            app.pick_file();
            ui.close();
        }
        if app.workspace.is_some() && theme::menu_item(ui, Some(ico::CLOSE), tr("Close Project"), false).clicked() {
            app.close_project();
            ui.close();
        }
        if app.workspace.is_some() && theme::menu_item(ui, Some(ico::SETTINGS), tr("Project Environment Variables…"), false).clicked() {
            app.pending.push(rc_editor::Action::ProjectEnvironment);
            ui.close();
        }
        let recents: Vec<std::path::PathBuf> = app.settings.recent_workspaces.iter().filter(|r| app.workspace.as_ref().is_none_or(|w| w.ws.key_path() != r.as_path())).cloned().collect();
        if !recents.is_empty() {
            ui.separator();
            ui.label(egui::RichText::new(tr("Recent Projects")).color(p.muted).size(12.0));
            for r in recents.iter().take(10) {
                let label = r.file_stem().map_or_else(|| r.display().to_string(), |s| s.to_string_lossy().into_owned());
                let icon = if r.is_dir() { ico::FOLDER } else { rc_kit::icons::for_file(&r.to_string_lossy()) };
                if theme::menu_item(ui, Some(icon), &label, false).on_hover_text(r.display().to_string()).clicked() {
                    app.open_workspace(r);
                    ui.close();
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn badge_initials() {
        assert_eq!(super::initials("TenProjects"), "TP");
        assert_eq!(super::initials("shop"), "SH");
        assert_eq!(super::initials("my-cool app"), "MC");
    }
}
