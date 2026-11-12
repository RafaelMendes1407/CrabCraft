//! Status bar (New UI): navigation path on the left; caret position, line ending, encoding,
//! indentation and read-only state on the right; transient messages in between.

use crate::app::CrabCraftApp;
use egui::{FontId, Ui};
use rc_kit::i18n::{tr, trf};
use rc_kit::{ico, icons, theme};

pub fn show(app: &mut CrabCraftApp, ui: &mut Ui) {
    let p = rc_kit::pal(ui);
    let now = ui.input(|i| i.time);
    for n in &mut app.notices {
        if n.at == 0.0 {
            n.at = now;
        }
    }
    // Messages stay 8 s (errors 15 s, those with a link 30 s).
    app.notices.retain(|n| now - n.at < if n.url.is_some() { 30.0 } else if n.error { 15.0 } else { 8.0 });
    if !app.notices.is_empty() {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
    }
    ui.horizontal_centered(|ui| {
        theme::split_row(ui, |ui, side| match side {
            theme::Side::Left => {
                ui.add_space(12.0);
                ui.spacing_mut().item_spacing.x = 2.0;
                if app.workspace.is_none() && !app.docs.list.is_empty() {
                    ui.label(egui::RichText::new(tr("LightEdit")).strong().size(12.5)).on_hover_text(tr("Files open without a project: only editing, no code insight"));
                    ui.add_space(10.0);
                }
                breadcrumbs(app, ui);
                let mut open = None;
                if let Some(n) = app.notices.last() {
                    ui.add_space(16.0);
                    icons::image(ui, if n.error { ico::ERROR } else { ico::INFO }, 16.0);
                    // The link's button first: a long message must not push it out of view.
                    if let Some(url) = &n.url {
                        if theme::flat_button(ui, Some(ico::EXTERNAL_LINK), tr("Open in Browser"), FontId::proportional(12.5), false, true).on_hover_text(url.as_str()).clicked() {
                            open = Some(url.clone());
                        }
                        ui.add_space(4.0);
                    }
                    let notice=ui.label(egui::RichText::new(&n.text).color(if n.error { p.error } else { p.text_muted }).size(12.5));
                    ui.ctx().accesskit_node_builder(notice.id,|node|{node.set_role(if n.error{egui::accesskit::Role::Alert}else{egui::accesskit::Role::Status});node.set_label(n.text.chars().take(2048).collect::<String>());node.set_live(if n.error{egui::accesskit::Live::Assertive}else{egui::accesskit::Live::Polite});node.set_live_atomic();});
                }
                if let Some(url) = open {
                    app.open_notice_url(url);
                }
            }
            theme::Side::Right => {
                ui.add_space(10.0);
                ui.spacing_mut().item_spacing.x = 0.0;
                if app.workspace.as_ref().is_some_and(|w|w.ws.cargo_manifest().is_some()){
                    let label=app.criticalup_provider.label();
                    if theme::flat_button(ui,None,&label,FontId::proportional(12.5),false,true).on_hover_text(tr("Rust Toolchain Provider")).clicked(){app.toggle_tool(crate::toolwindow::ToolWindow::RustProvider);}
                }
                insight_widget(app, ui);
                debugger_widget(app, ui);
                linter_widget(app, ui);
                target_widget(app, ui);
                let Some(doc) = app.docs.active_doc() else { return };
                let e = &doc.editor;
                let widget = |ui: &mut Ui, text: String, tip: &str| {
                    let r = theme::flat_button(ui, None, &text, FontId::proportional(12.5), false, true);
                    r.on_hover_text(tip)
                };
                if e.read_only {
                    icons::image(ui, ico::INFO, 16.0).on_hover_text(tr("The file is read-only"));
                    ui.add_space(4.0);
                }
                let indent = if e.indent.unit == "\t" { tr("Tab").to_string() } else { trf("{n} spaces", &[("n", &e.indent.unit.len())]) };
                let tip = match doc.style.sources.iter().map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).collect::<Vec<_>>() {
                    files if files.is_empty() => tr("Indentation").to_string(),
                    files => trf("Indentation, from the project's code style: {files}", &[("files", &files.join(", "))]),
                };
                widget(ui, indent, &tip);
                widget(ui, if doc.bom { "UTF-8 BOM".into() } else { "UTF-8".into() }, tr("File encoding"));
                widget(ui, e.buffer.line_ending().label().into(), tr("Line separator"));
                let (line, col) = e.caret_line_col();
                let sel: usize = e.selection.ranges().iter().map(|r| r.len()).sum();
                let pos = if sel > 0 {
                    trf("{line}:{col} ({n} chars)", &[("line", &(line + 1)), ("col", &(col + 1)), ("n", &sel)])
                } else {
                    format!("{}:{}", line + 1, col + 1)
                };
                if widget(ui, pos, tr("Go to Line…")).clicked() {
                    app.pending.push(rc_editor::Action::GotoLine);
                }
            }
        });
    });
}

/// The linter that runs on save in a Rust project (click: the other one).
fn linter_widget(app: &mut CrabCraftApp, ui: &mut Ui) {
    if app.runner.cargo_workspace.is_none() {
        return;
    }
    let name = if app.settings.use_clippy { "Clippy" } else { "Cargo Check" };
    if theme::flat_button(ui, None, name, FontId::proportional(12.5), false, true).on_hover_text(tr("Linter that runs on save. Click to switch.")).clicked() {
        app.toggle_linter();
    }
}

/// The target triple the Rust code is analysed for (click: the list of targets).
fn target_widget(app: &mut CrabCraftApp, ui: &mut Ui) {
    if app.runner.cargo_workspace.is_none() {
        return;
    }
    let Some(triple) = app.analysis_triple().map(str::to_string) else { return };
    let resp = theme::flat_button(ui, None, &triple, FontId::proportional(12.5), false, true).on_hover_text(tr("Target the code is analysed for. Click to pick another one."));
    egui::Popup::menu(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).align(egui::RectAlign::TOP_END).show(|ui| target_menu(app, ui));
    ui.add_space(4.0);
}

/// Above this many targets, the list gets a filter field.
const TARGET_FILTER_FROM: usize = 8;

/// The targets to analyse for: the host, the project's `[build] target` and rustup's targets,
/// each with what it is (host, from the project, not installed).
pub fn target_menu(app: &mut CrabCraftApp, ui: &mut Ui) {
    ui.set_min_width(340.0);
    let mut list = app.triple_choices.clone();
    // A target chosen in an earlier session that is no longer listed.
    if let Some(t) = &app.cargo_target
        && !list.iter().any(|c| &c.triple == t)
    {
        list.insert(0, rc_cargo::TripleChoice { triple: t.clone(), host: false, installed: false, from_config: false });
    }
    let p = rc_kit::pal(ui);
    if list.is_empty() {
        ui.label(egui::RichText::new(tr("No targets found")).size(12.5).color(p.text_muted));
        return;
    }
    if list.len() > TARGET_FILTER_FROM {
        ui.add(egui::TextEdit::singleline(&mut app.triple_filter).hint_text(tr("Filter targets")).desired_width(f32::INFINITY));
        ui.add_space(4.0);
    }
    let filter = app.triple_filter.trim().to_ascii_lowercase();
    let current = app.analysis_triple().map(str::to_string);
    let room = (ui.ctx().content_rect().height() * 0.6).max(160.0);
    let mut chosen = None;
    egui::ScrollArea::vertical().id_salt("target_menu").max_height(room).show(ui, |ui| {
        for c in list.iter().filter(|c| filter.is_empty() || c.triple.to_ascii_lowercase().contains(&filter)) {
            let mut marks: Vec<&str> = Vec::new();
            if c.host {
                marks.push(tr("host"));
            }
            if c.from_config {
                marks.push(tr("project"));
            }
            if !c.installed {
                marks.push(tr("not installed"));
            }
            let marks = marks.join(", ");
            let resp = theme::menu_entry(ui, None, &c.triple, Some(&marks).filter(|m| !m.is_empty()).map(String::as_str), current.as_deref() == Some(c.triple.as_str()), true);
            let resp = if c.installed { resp } else { resp.on_hover_text(tr("The standard library of this target is not installed: rust-analyzer may not resolve std")) };
            if resp.clicked() {
                chosen = Some(c.triple.clone());
            }
        }
    });
    if let Some(t) = chosen {
        app.set_cargo_target(Some(t));
        app.triple_filter.clear();
        ui.close();
    }
}

/// State of the debug session (click: Debug window).
fn debugger_widget(app: &mut CrabCraftApp, ui: &mut Ui) {
    let (icon, text) = if app.debugger.active() {
        (if app.debugger.stopped() { ico::PAUSE } else { ico::DEBUG }, app.debugger.state_label())
    } else {
        return;
    };
    if theme::flat_button(ui, Some(icon), &text, FontId::proportional(12.5), false, true).on_hover_text(tr("Debug")).clicked() {
        app.dock.show(crate::toolwindow::ToolWindow::Debug);
    }
    ui.add_space(4.0);
}

/// Language server state and failures with a restart button.
fn insight_widget(app: &mut CrabCraftApp, ui: &mut Ui) {
    use crate::insight::InsightStatus as S;
    let p = rc_kit::pal(ui);
    if app.runner.require_trust().is_err() {
        ui.label(tr("Restricted mode"));
        if theme::flat_button(ui, Some(ico::WARNING), tr("Trust this project"), FontId::proportional(12.5), false, true)
            .on_hover_text(tr("Restricted mode: project tools are disabled. Trust allows Cargo configuration, toolchains, build scripts and procedural macros to execute.")).clicked() { app.trust_project(); }
        return;
    }
    match app.insight.status.clone() {
        S::Idle => {}
        S::Ready => {
            let tip = match app.insight.last_completion_ms {
                Some(ms) => trf("Rust language server ready (last completion {ms} ms)", &[("ms", &ms)]),
                None => tr("Rust language server ready").to_string(),
            };
            theme::flat_button(ui, Some(ico::CHECKMARK), "Rust", FontId::proportional(12.5), false, true).on_hover_text(tip);
        }
        S::Starting | S::Loading(_) => {
            let text = match &app.insight.status {
                S::Loading(t) => t.clone(),
                _ => tr("Starting Rust language server…").to_string(),
            };
            ui.add(egui::Spinner::new().size(12.0).color(p.muted));
            ui.add_space(6.0);
            ui.label(egui::RichText::new(text).size(12.5).color(p.text_muted));
            ui.add_space(8.0);
        }
        S::Untrusted => {}
        S::RustAnalyzerMissing => {
            theme::flat_button(ui, Some(ico::WARNING), tr("rust-analyzer not found"), FontId::proportional(12.5), false, true).on_hover_text(tr("Install it with: rustup component add rust-analyzer"));
        }
        S::Failed(why) => {
            if theme::flat_button(ui, Some(ico::ERROR), tr("Rust server stopped — Restart"), FontId::proportional(12.5), false, true).on_hover_text(why).clicked() {
                app.insight.restart();
            }
        }
    }
}

/// Workspace › folders › file (each part clickable to locate it in the explorer).
fn breadcrumbs(app: &mut CrabCraftApp, ui: &mut Ui) {
    let p = rc_kit::pal(ui);
    let Some(w) = &app.workspace else { return };
    let mut parts: Vec<String> = vec![w.ws.name()];
    if let Some(doc) = app.docs.active_doc()
        && let Ok(rel) = doc.path.strip_prefix(&w.ws.root)
    {
        parts.extend(rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()));
    }
    // The items around the caret, after the file.
    let files = parts.len();
    let mut places = Vec::new();
    if let Some(doc) = app.docs.active_doc()
        && let Some(tree) = app.insight.structure.get(&doc.editor.id)
    {
        for node in crate::insight::symbols_at(tree, doc.editor.caret_line_col().0) {
            parts.push(node.name.clone());
            places.push((node.line, node.col));
        }
    }
    let n = parts.len();
    for (i, part) in parts.into_iter().enumerate() {
        let color = if i + 1 == n { p.text } else { p.text_muted };
        let resp = ui.add(egui::Label::new(egui::RichText::new(part).color(color).size(12.5)).sense(egui::Sense::click()));
        if resp.clicked() {
            match i.checked_sub(files).and_then(|at| places.get(at)) {
                Some(&(line, col)) => {
                    if let Some(d) = app.docs.active_doc_mut() {
                        d.editor.goto(line, col);
                        app.focus_editor = true;
                    }
                }
                None => app.locate_active(),
            }
        }
        if i + 1 < n {
            icons::image(ui, ico::CHEVRON_RIGHT, 16.0);
        }
    }
}
