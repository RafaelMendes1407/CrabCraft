//! Offscreen captures of the whole window (wgpu, no input sent to any real window), for
//! visual review and the README. Run with `cargo test -p rc-app screens -- --ignored`;
//! images go to `target/snapshots/`.

use crate::app::CrabCraftApp;
use egui::vec2;
use egui_kittest::Harness;
use rc_core::{Settings, Waker};
use std::path::{Path, PathBuf};

pub fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures").canonicalize().unwrap()
}

/// Tests that point `CRABCRAFT_HOME` somewhere hold this lock: the variable is process-wide.
pub static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// An app with its own data folder (never the user's settings or sessions).
pub fn test_app(home: &Path, dark: bool) -> CrabCraftApp {
    // SAFETY: tests that read CRABCRAFT_HOME set it to the same kind of temp folder first.
    unsafe { std::env::set_var("CRABCRAFT_HOME", home) };
    let mut settings = Settings::default();
    settings.theme = if dark { rc_core::settings::ThemeChoice::Dark } else { rc_core::settings::ThemeChoice::Light };
    CrabCraftApp::with_settings(settings, Waker::noop(), None)
}

pub fn harness(app: CrabCraftApp, size: egui::Vec2) -> Harness<'static, CrabCraftApp> {
    let mut first = true;
    let mut frame = eframe::Frame::_new_kittest();
    let mut h = Harness::builder().with_size(size).build_ui_state(
        move |ui, app: &mut CrabCraftApp| {
            if std::mem::take(&mut first) {
                rc_kit::theme::install_fonts(ui.ctx());
                return;
            }
            let ctx = ui.ctx().clone();
            eframe::App::logic(app, &ctx, &mut frame);
            let bounds = ctx.content_rect();
            let mut root = ui.new_child(egui::UiBuilder::new().max_rect(bounds));
            root.set_clip_rect(bounds);
            eframe::App::ui(app, &mut root, &mut frame);
        },
        app,
    );
    h.run_steps(4);
    h
}

#[test]
#[ignore = "Requires a GPU; writes window captures to target/snapshots"]
fn screens() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    for dark in [true, false] {
        let home = tempfile::tempdir().unwrap();
        let suffix = if dark { "dark" } else { "light" };
        // Welcome screen with a couple of recent projects.
        let mut app = test_app(home.path(), dark);
        app.settings.recent_workspaces = vec![fixtures().join("Shop/Shop.sln"), fixtures().join("TenProjects/TenProjects.slnx")];
        let mut h = harness(app, vec2(1280.0, 800.0));
        h.render().unwrap().save(out.join(format!("welcome-{suffix}.png"))).unwrap();
        // Long Spanish labels wrap inside the buttons.
        rc_kit::i18n::set_lang(rc_kit::i18n::Lang::Es);
        h.run_steps(3);
        h.render().unwrap().save(out.join(format!("welcome-es-{suffix}.png"))).unwrap();
        rc_kit::i18n::set_lang(rc_kit::i18n::Lang::En);
        // New Solution.
        h.state_mut().new_project_dialog(None);
        h.run_steps(4);
        h.render().unwrap().save(out.join(format!("new-solution-{suffix}.png"))).unwrap();

        // A file on its own, without a workspace (Finder's "Open With").
        let mut app = test_app(home.path(), dark);
        app.open_path(&fixtures().join("Shop/src/Shop.Api/appsettings.json"));
        assert!(app.workspace.is_none());
        let mut h = harness(app, vec2(1280.0, 800.0));
        h.run_steps(4);
        h.render().unwrap().save(out.join(format!("loose-file-{suffix}.png"))).unwrap();

        // Razor and solution files.
        let samples = tempfile::Builder::new().prefix("crabcraft-razor").tempdir().unwrap();
        let razor = samples.path().join("Orders.razor");
        std::fs::write(&razor, "@page \"/orders\"\n@using Shop.Core\n@inject OrderService Orders\n\n<PageTitle>Orders</PageTitle>\n\n<h1 class=\"title\">Orders &amp; quotes</h1>\n\n@if (orders is null)\n{\n    <p><em>Loading…</em></p>\n}\nelse\n{\n    <table class=\"table\">\n        @foreach (var order in orders)\n        {\n            <tr @key=\"order.Id\">\n                <td>@order.Customer</td>\n                <td>@order.Total.ToString(\"0.00\")</td>\n                <td><button class=\"btn\" @onclick=\"() => Remove(order)\">Remove</button></td>\n            </tr>\n        }\n    </table>\n}\n\n@code {\n    private List<Order>? orders;\n\n    protected override async Task OnInitializedAsync()\n    {\n        // Load the sample orders.\n        orders = await Orders.GetAllAsync();\n    }\n\n    private void Remove(Order order) => orders?.Remove(order);\n}\n").unwrap();
        let mut app = test_app(home.path(), dark);
        let sln = fixtures().join("Shop/Shop.sln");
        app.open_path(&sln);
        app.open_file(&sln);
        app.open_path(&razor);
        let mut h = harness(app, vec2(1280.0, 800.0));
        h.run_steps(6);
        h.render().unwrap().save(out.join(format!("razor-{suffix}.png"))).unwrap();
        let i = h.state().docs.index_of(&sln).unwrap();
        h.state_mut().docs.activate(i);
        h.run_steps(6);
        h.render().unwrap().save(out.join(format!("sln-{suffix}.png"))).unwrap();

        // A solution with files open.
        let mut app = test_app(home.path(), dark);
        app.open_workspace(&fixtures().join("Shop/Shop.sln"));
        app.explorer.expanded.insert(format!("proj:{}", fixtures().join("Shop/src/Shop.Api/Shop.Api.csproj").display()));
        app.explorer.expanded.insert(format!("deps:{}", fixtures().join("Shop/src/Shop.Api/Shop.Api.csproj").display()));
        app.open_file(&fixtures().join("Shop/src/Shop.Api/Program.cs"));
        app.open_file(&fixtures().join("Shop/src/Shop.Api/Controllers/WeatherForecastController.cs"));
        app.open_file(&fixtures().join("Shop/src/Shop.Api/Shop.Api.csproj"));
        app.docs.activate(1);
        app.locate_active();
        let mut h = harness(app, vec2(1440.0, 900.0));
        h.run_steps(4);
        h.render().unwrap().save(out.join(format!("workspace-{suffix}.png"))).unwrap();
        // Settings (appearance page, with the zoom slider).
        let s = h.state().settings.clone();
        h.state_mut().dialog = Some(crate::ui::dialogs::Dialog::settings(&s));
        h.run_steps(4);
        h.render().unwrap().save(out.join(format!("settings-{suffix}.png"))).unwrap();
        // The Tests window before any run: it offers to run them all.
        h.state_mut().dialog = None;
        h.state_mut().dock.show(crate::toolwindow::ToolWindow::Tests);
        h.run_steps(4);
        h.render().unwrap().save(out.join(format!("tests-empty-{suffix}.png"))).unwrap();
        // A compound run configuration.
        let members = h.state().runner.app_names().into_iter().map(|n| (n, true)).collect();
        h.state_mut().dialog = Some(crate::ui::dialogs::Dialog::Compound { old: None, name: "All Applications".into(), members });
        h.run_steps(4);
        h.render().unwrap().save(out.join(format!("compound-{suffix}.png"))).unwrap();
        h.state_mut().dialog = None;
        // Editing an application run configuration.
        let api = h.state().runner.configs.iter().find(|c| c.name.starts_with("Shop.Api")).cloned().unwrap();
        if let crate::runner::ConfigKind::App { project, .. } = api.kind {
            let options = rc_core::session::RunOptions { args: "--seed 42".into(), env: "ASPNETCORE_ENVIRONMENT=Staging".into(), ..Default::default() };
            h.state_mut().dialog = Some(crate::ui::dialogs::Dialog::RunOptions { name: api.name.clone(), project, profiles: vec!["http".into(), "https".into()], options, web: true });
            h.run_steps(4);
            h.render().unwrap().save(out.join(format!("run-options-{suffix}.png"))).unwrap();
            h.state_mut().dialog = None;
        }
        // Add › API Controller.
        h.state_mut().dialog = Some(crate::ui::dialogs::Dialog::NewFile { dir: fixtures().join("Shop/src/Shop.Api/Controllers"), name: "Customers".into(), kind: crate::item_templates::ItemKind::ApiController, attach: true, public: false, error: None });
        h.run_steps(4);
        h.render().unwrap().save(out.join(format!("add-item-{suffix}.png"))).unwrap();
        h.state_mut().dialog = None;
        // File › New File.
        h.state_mut().dialog = Some(crate::ui::dialogs::Dialog::NewFile { dir: fixtures().join("Shop/src/Shop.Api"), name: "Properties/notes.md".into(), kind: crate::item_templates::ItemKind::File, attach: true, public: false, error: None });
        h.run_steps(4);
        h.render().unwrap().save(out.join(format!("new-file-{suffix}.png"))).unwrap();
        h.state_mut().dialog = None;
        // Many tabs: one scrolling row, then several rows, with a pinned and a preview tab.
        {
            let app = h.state_mut();
            for f in ["Shop/src/Shop.Core/Order.cs", "Shop/src/Shop.Core/OrderLine.cs", "Shop/src/Shop.Core/PriceCalculator.cs", "Shop/src/Shop.Api/Controllers/OrdersController.cs", "Shop/src/Shop.Api/appsettings.json", "Shop/tests/Shop.Core.XunitTests/PriceCalculatorTests.cs", "Shop/src/Shop.Cli/Program.cs", "Shop/Shop.sln"] {
                app.open_file(&fixtures().join(f));
            }
            let i = app.docs.index_of(&fixtures().join("Shop/src/Shop.Core/Order.cs").canonicalize().unwrap()).unwrap();
            app.docs.list[i].pinned = true;
            app.docs.open_preview(&fixtures().join("Shop/src/Shop.Core/Shop.Core.csproj")).unwrap();
        }
        h.run_steps(4);
        h.render().unwrap().save(out.join(format!("tabs-{suffix}.png"))).unwrap();
        h.state_mut().settings.tabs_multirow = true;
        h.run_steps(4);
        h.render().unwrap().save(out.join(format!("tabs-rows-{suffix}.png"))).unwrap();
        h.state_mut().settings.tabs_multirow = false;
        // Right-click in the editor: the context menu.
        let calc = fixtures().join("Shop/src/Shop.Core/PriceCalculator.cs");
        h.state_mut().open_file(&calc);
        h.run_steps(3);
        let at = egui::pos2(760.0, 300.0);
        h.event(egui::Event::PointerMoved(at));
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed: true, modifiers: Default::default() });
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed: false, modifiers: Default::default() });
        h.run_steps(4);
        h.render().unwrap().save(out.join(format!("editor-menu-{suffix}.png"))).unwrap();
        h.event(egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() });
        h.run_steps(2);
        // ⌘ over a name: a link; Back is enabled in the toolbar.
        h.state_mut().push_nav_at(calc.canonicalize().unwrap(), 0);
        h.event(egui::Event::ModifiersChanged(egui::Modifiers::COMMAND));
        h.event(egui::Event::PointerMoved(egui::pos2(640.0, 320.0)));
        h.run_steps(3);
        h.render().unwrap().save(out.join(format!("navigation-{suffix}.png"))).unwrap();
        h.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
        h.state_mut().nav_back.clear();
        h.run_steps(2);
        // Reformat Code on a JSON file.
        let json = samples.path().join("settings.json");
        std::fs::write(&json, "{\"Logging\":{\"LogLevel\":{\"Default\":\"Information\"}},\"AllowedHosts\":\"*\",\"Ports\":[5000,5001]}\n").unwrap();
        h.state_mut().open_file(&json);
        h.state_mut().pending.push(rc_editor::Action::Reformat);
        h.run_steps(4);
        let formatted = h.state().docs.active_doc().unwrap().editor.buffer.rope().to_string();
        assert!(formatted.contains("\"LogLevel\": {\n"), "{formatted}");
        h.render().unwrap().save(out.join(format!("json-format-{suffix}.png"))).unwrap();
        // Markdown: editor and preview side by side, then the preview alone.
        let md = samples.path().join("README.md");
        std::fs::create_dir_all(samples.path().join("docs")).unwrap();
        std::fs::copy(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/icon-64.png"), samples.path().join("docs/icon.png")).unwrap();
        std::fs::write(
            &md,
            "# Shop\n\nSample solution for **CrabCraft**: an API, a CLI and *three* test projects.\n\n![icon](docs/icon.png)\n\n## Projects\n\n- `Shop.Core`: orders and prices\n- `Shop.Api`: ASP.NET Core minimal API\n  - Swagger at `/swagger`\n- [ ] Blazor front end\n- [x] Tests\n\n## Run\n\n```bash\ndotnet run --project src/Shop.Api\n```\n\n| Project | Framework |\n| --- | --- |\n| Shop.Core | net8.0 |\n| Shop.Api | net8.0 |\n\n> Prices include VAT.\n\nSee the [.NET docs](https://learn.microsoft.com/dotnet/).\n",
        )
        .unwrap();
        h.state_mut().open_file(&md);
        h.run_steps(6);
        h.render().unwrap().save(out.join(format!("markdown-{suffix}.png"))).unwrap();
        if let Some(d) = h.state_mut().docs.active_doc_mut() {
            d.md_view = Some(crate::docs::MdView::Preview);
        }
        h.run_steps(4);
        h.render().unwrap().save(out.join(format!("markdown-preview-{suffix}.png"))).unwrap();
        // NuGet: installed packages with updates from nuget.org, one selected.
        h.state_mut().dock.show(crate::toolwindow::ToolWindow::Nuget);
        h.state_mut().dock.sizes.insert(crate::toolwindow::ToolWindow::Nuget, 420.0);
        h.run_steps(3);
        h.state_mut().nuget.select_for_test("xunit");
        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_secs(25) && !h.state().nuget.ready_for_test() {
            h.run_steps(1);
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        h.run_steps(3);
        h.render().unwrap().save(out.join(format!("nuget-{suffix}.png"))).unwrap();
        h.state_mut().dock.bottom = None;
        // Split right, then a tab in its own window.
        {
            let app = h.state_mut();
            let program = app.docs.list.iter().find(|d| d.name() == "Program.cs").map(|d| d.editor.id).unwrap();
            let group = app.layout.group_of(program).unwrap();
            app.layout.move_doc(program, crate::layout::Drop::Split { group, edge: crate::layout::Edge::Right });
        }
        h.run_steps(5);
        h.render().unwrap().save(out.join(format!("split-{suffix}.png"))).unwrap();
        {
            let app = h.state_mut();
            let group = app.layout.main_target();
            app.layout.unsplit_all(group);
            let json = app.docs.list.iter().find(|d| d.name() == "appsettings.json").map(|d| d.editor.id).unwrap();
            app.layout.move_doc(json, crate::layout::Drop::Window(egui::pos2(80.0, 80.0)));
        }
        h.run_steps(5);
        h.render().unwrap().save(out.join(format!("floating-{suffix}.png"))).unwrap();
        {
            let app = h.state_mut();
            while !app.layout.windows.is_empty() {
                app.layout.dock_window(0);
            }
        }
        // The explorer moved to the right side.
        h.state_mut().dock.move_to(crate::toolwindow::ToolWindow::Explorer, crate::toolwindow::Side::Right);
        h.run_steps(4);
        h.render().unwrap().save(out.join(format!("explorer-right-{suffix}.png"))).unwrap();
    }
}

/// Phase 1 closure: a 10-project solution opens with every project in the explorer, and the
/// session (open tabs, caret, folds, tool windows, expanded nodes) is restored on reopen.
/// Close Project, quit, start: the welcome screen. Quitting with a project open reopens it.
#[test]
fn closed_projects_are_not_reopened_on_startup() {
    use crate::cli::OpenRequest;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let sln = fixtures().join("Shop/Shop.sln");
    let mut app = test_app(home.path(), true);
    app.open_workspace(&sln);
    assert_eq!(crate::app::startup_paths(&app.settings, Vec::new()), vec![OpenRequest::path(&sln)]);
    app.close_project();
    assert!(crate::app::startup_paths(&rc_core::Settings::load(), Vec::new()).is_empty());
    assert_eq!(rc_core::Settings::load().recent_workspaces.first(), Some(&sln));
    app.open_workspace(&sln);
    let mut settings = rc_core::Settings::load();
    assert_eq!(crate::app::startup_paths(&settings, Vec::new()), vec![OpenRequest::path(&sln)]);
    settings.reopen_last = false;
    assert!(crate::app::startup_paths(&settings, Vec::new()).is_empty());
    // Files from Finder win.
    let file = fixtures().join("Shop/README.md");
    assert_eq!(crate::app::startup_paths(&settings, vec![OpenRequest::path(&file)]), vec![OpenRequest::path(&file)]);
}

#[test]
fn the_rustfmt_settings_show_what_is_wrong_with_them() {
    use crate::ui::dialogs::{Dialog, SettingsPage};
    use egui_kittest::kittest::Queryable;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let mut app = test_app(home.path(), true);
    let mut draft = app.settings.clone();
    draft.rustfmt.env = "RUST_LOG=debug\nnot a pair".into();
    app.dialog = Some(Dialog::Settings { draft: Box::new(draft), page: SettingsPage::Tools });
    let mut h = harness(app, vec2(1280.0, 800.0));
    h.run_steps(3);
    for label in ["rustfmt arguments", "rustfmt channel", "rustfmt environment"] {
        h.get_by_label(rc_kit::i18n::tr(label));
    }
    let wrong = crate::vcs::rustfmt_failure(rc_cargo::FormatError::Environment("not a pair".into()));
    h.get_by_label(&wrong);
    if let Some(Dialog::Settings { draft, .. }) = &mut h.state_mut().dialog {
        draft.rustfmt.env = "RUST_LOG=debug".into();
    }
    h.run_steps(3);
    assert!(h.query_by_label(&wrong).is_none());
}

#[test]
fn files_dropped_on_the_welcome_screen_open_in_the_editor() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let file = home.path().join("cosa.txt");
    std::fs::write(&file, "hola").unwrap();
    let mut h = harness(test_app(home.path(), true), vec2(1280.0, 800.0));
    #[derive(Debug)]
    struct Dropped(PathBuf);
    impl egui::DroppedFile for Dropped {
        fn path(&self) -> &Path {
            &self.0
        }
        fn bytes(&self) -> Result<Vec<u8>, String> {
            std::fs::read(&self.0).map_err(|e| e.to_string())
        }
    }
    h.input_mut().dropped_files.push(std::sync::Arc::new(Dropped(file.clone())));
    h.run_steps(3);
    let app = h.state();
    assert!(app.workspace.is_none());
    assert_eq!(app.docs.active_doc().map(|d| d.path.clone()), Some(file.canonicalize().unwrap()));
    assert!(app.notices.iter().all(|n| !n.error), "{:?}", app.notices.iter().map(|n| &n.text).collect::<Vec<_>>());
}

#[test]
fn scratch_files_are_created_outside_the_project_and_reopened_from_the_list() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let mut h = harness(test_app(home.path(), true), vec2(1280.0, 800.0));
    let dir = rc_core::paths::scratches_dir();
    assert!(dir.starts_with(home.path()));
    // The first row is a Rust scratch file.
    h.state_mut().pending.push(rc_editor::Action::NewScratchFile);
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    let first = dir.join("scratch_1.rs").canonicalize().unwrap();
    assert!(h.state().dialog.is_none());
    assert_eq!(h.state().docs.active_doc().map(|d| d.path.clone()), Some(first.clone()));
    // Outside every Cargo package, so rust-analyzer never gets it.
    assert!(h.state().docs.active_doc().is_some_and(|d| d.scratch && !crate::insight::has_insight(d)));
    assert_eq!(h.state().docs.active_doc().map(|d| d.editor.buffer.rope().to_string()), Some("fn main() {
    
}
".into()));
    // Filtering by kind creates a Markdown one.
    h.state_mut().pending.push(rc_editor::Action::NewScratchFile);
    h.run_steps(2);
    h.input_mut().events.push(egui::Event::Text("mark".into()));
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert_eq!(h.state().docs.active_doc().map(|d| d.path.clone()), dir.join("scratch_1.md").canonicalize().ok());
    // The existing ones are listed after the kinds and open again.
    h.state_mut().pending.push(rc_editor::Action::NewScratchFile);
    h.run_steps(2);
    match &h.state().dialog {
        Some(crate::ui::dialogs::Dialog::Scratches { files, .. }) => assert_eq!(files.len(), 2),
        _ => panic!("no scratch list"),
    }
    h.input_mut().events.push(egui::Event::Text("scratch_1.rs".into()));
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert_eq!(h.state().docs.active_doc().map(|d| d.path.clone()), Some(first));
    assert_eq!(h.state().docs.list.len(), 2);
}

#[test]
fn ten_project_solution_opens_and_its_session_is_restored() {
    use egui_kittest::kittest::Queryable;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let sln = fixtures().join("TenProjects/TenProjects.slnx");
    let file_a = fixtures().join("TenProjects/src/Module03/Class1.cs");
    let file_b = fixtures().join("TenProjects/src/Module07/Class1.cs");

    let mut app = test_app(home.path(), true);
    app.open_workspace(&sln);
    let w = app.workspace.as_ref().unwrap();
    assert_eq!(w.solution.as_ref().unwrap().projects().len(), 10);
    app.open_file(&file_a);
    app.open_file(&file_b);
    app.docs.list[0].editor.goto(2, 8);
    app.dock.toggle(crate::toolwindow::ToolWindow::Problems);
    app.docs.activate(0);
    let mut h = harness(app, vec2(1280.0, 800.0));
    // Every project is listed under the expanded "src" solution folder.
    for i in 1..=10 {
        assert!(h.query_all_by_label(&format!("Module{i:02}")).count() >= 1, "Module{i:02} not in the explorer");
    }
    h.run_steps(2);
    let mut app = h.into_state();
    app.close_workspace();

    let mut app = test_app(home.path(), true);
    app.open_workspace(&sln);
    assert_eq!(app.docs.list.len(), 2);
    assert_eq!(app.docs.active, Some(0));
    assert_eq!(app.docs.list[0].path, file_a.canonicalize().unwrap());
    assert_eq!(app.docs.list[0].editor.caret_line_col(), (2, 8));
    assert!(app.dock.is_open(crate::toolwindow::ToolWindow::Problems));
    assert!(app.explorer.expanded.contains("sf:/src"));
}

/// Drags with the primary button from `from` to `to` in a few steps, like a hand would.
fn drag(h: &mut Harness<'static, CrabCraftApp>, from: egui::Pos2, to: egui::Pos2) {
    let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
    h.input_mut().events.push(egui::Event::PointerMoved(from));
    h.run_steps(1);
    h.input_mut().events.push(button(from, true));
    h.run_steps(1);
    for i in 1..=8 {
        h.input_mut().events.push(egui::Event::PointerMoved(from + (to - from) * (i as f32 / 8.0)));
        h.run_steps(1);
    }
    h.input_mut().events.push(button(to, false));
    h.run_steps(3);
}

#[test]
fn tabs_drag_into_splits_and_floating_windows_and_the_layout_is_restored() {
    use egui_kittest::kittest::Queryable;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("demo");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n").unwrap();
    for f in ["lib.rs", "main.rs", "util.rs"] {
        std::fs::write(root.join("src").join(f), "fn f() {}\n").unwrap();
    }
    let root = root.canonicalize().unwrap();
    let mut app = test_app(&home.path().join("home"), true);
    app.open_workspace(&root);
    for f in ["lib.rs", "main.rs", "util.rs"] {
        app.open_file(&root.join("src").join(f));
    }
    let size = vec2(1280.0, 800.0);
    let mut h = harness(app, size);
    let id = |h: &Harness<'static, CrabCraftApp>, name: &str| h.state().docs.list.iter().find(|d| d.name() == name).map(|d| d.editor.id).unwrap();
    let tab = |h: &Harness<'static, CrabCraftApp>, name: &str| h.get_by_label(name).rect().center();

    // main.rs onto the right edge of the editor: a second group beside the first.
    let editor = h.state().layout.main.groups()[0].id;
    let from = tab(&h, "main.rs");
    drag(&mut h, from, egui::pos2(size.x - 120.0, size.y / 2.0));
    let app = h.state();
    assert!(matches!(app.layout.main, crate::layout::Node::Split { vertical: true, .. }), "{:?}", app.layout.main);
    assert_ne!(app.layout.group_of(id(&h, "main.rs")), Some(editor));
    assert_eq!(app.docs.active_doc().map(|d| d.name()), Some("main.rs".into()));

    // util.rs out of the window: a floating window with it.
    let from = tab(&h, "util.rs");
    drag(&mut h, from, egui::pos2(size.x + 200.0, 100.0));
    let app = h.state();
    assert_eq!(app.layout.windows.len(), 1);
    assert_eq!(app.layout.windows[0].root.groups()[0].tabs, vec![id(&h, "util.rs")]);
    assert_eq!(app.layout.main.groups().len(), 2);

    // Reopening the project brings the groups and the window back.
    let mut app = h.into_state();
    app.close_workspace();
    app.open_workspace(&root);
    let names = |app: &CrabCraftApp, g: &crate::layout::Group| g.tabs.iter().filter_map(|t| app.docs.list.iter().find(|d| d.editor.id == *t)).map(|d| d.name()).collect::<Vec<_>>();
    let main: Vec<Vec<String>> = app.layout.main.groups().into_iter().map(|g| names(&app, g)).collect();
    assert_eq!(main, vec![vec!["lib.rs".to_string()], vec!["main.rs".to_string()]]);
    assert_eq!(names(&app, app.layout.windows[0].root.groups()[0]), vec!["util.rs".to_string()]);
}

fn run_frames(h: &mut Harness<'static, CrabCraftApp>, what: &str, mut done: impl FnMut(&CrabCraftApp) -> bool) {
    let start = std::time::Instant::now();
    while start.elapsed() < std::time::Duration::from_secs(240) {
        h.run_steps(1);
        if done(h.state()) {
            h.run_steps(3);
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    panic!("timed out waiting for {what}");
}

#[test]
#[ignore = "Requires a GPU and the .NET SDK; writes tool window captures to target/snapshots"]
fn screens_tools() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    let home = tempfile::tempdir().unwrap();
    let mut app = test_app(home.path(), true);
    app.settings.roslyn_path = Some("/nonexistent".into());
    app.open_workspace(&fixtures().join("Shop/Shop.sln"));
    let test_file = std::fs::read_dir(fixtures().join("Shop/tests/Shop.Core.XunitTests")).unwrap().flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|e| e == "cs")).unwrap();
    app.open_file(&test_file);
    let mut h = harness(app, vec2(1440.0, 900.0));
    // Tests.
    {
        let app = h.state_mut();
        app.runner.selected = app.runner.configs.iter().position(|c| c.name == "All Tests").unwrap();
        let outs = app.runner.run_selected();
        crate::ui::run_windows::handle_outputs(app, outs);
    }
    run_frames(&mut h, "tests", |a| a.runner.tests.as_ref().is_some_and(|t| !t.running));
    h.render().unwrap().save(out.join("tests-dark.png")).unwrap();
    // Build.
    {
        let app = h.state_mut();
        let target = app.workspace.as_ref().unwrap().ws.key_path().to_path_buf();
        let outs = app.runner.build(&target, rc_dotnet::BuildAction::Build);
        crate::ui::run_windows::handle_outputs(app, outs);
    }
    run_frames(&mut h, "build", |a| a.runner.build.as_ref().is_some_and(|b| b.result.is_some()));
    h.render().unwrap().save(out.join("build-dark.png")).unwrap();
    // Run Shop.Cli.
    {
        let app = h.state_mut();
        app.runner.selected = app.runner.configs.iter().position(|c| c.name.starts_with("Shop.Cli")).unwrap();
        let outs = app.runner.run_selected();
        crate::ui::run_windows::handle_outputs(app, outs);
    }
    run_frames(&mut h, "run", |a| a.runner.runs.first().is_some_and(|r| r.exit.is_some()));
    h.render().unwrap().save(out.join("run-dark.png")).unwrap();
    // Search Everywhere and the find bar.
    {
        let app = h.state_mut();
        app.pending.push(rc_editor::Action::GotoFile);
    }
    h.run_steps(2);
    h.state_mut().search_everywhere.as_mut().unwrap().text = "pricalc".into();
    h.run_steps(4);
    h.render().unwrap().save(out.join("search-dark.png")).unwrap();
    {
        let app = h.state_mut();
        app.search_everywhere = None;
        app.find_bar = Some(crate::search::FindBar { query: "Assert".into(), replace: Some("Check".into()), focus: true, ..Default::default() });
    }
    h.run_steps(4);
    h.render().unwrap().save(out.join("findbar-dark.png")).unwrap();
    // Terminal.
    {
        let app = h.state_mut();
        app.find_bar = None;
        app.open_terminal(None);
    }
    run_frames(&mut h, "terminal", |a| a.terminals.active().is_some_and(|t| t.session.screen_lines().iter().any(|l| !l.trim().is_empty())));
    h.render().unwrap().save(out.join("terminal-dark.png")).unwrap();
}

#[test]
#[ignore = "Requires a GPU; writes Git captures to target/snapshots"]
fn screens_git() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    let home = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path().canonicalize().unwrap().join("Shop");
    let copy = std::process::Command::new("cp").args(["-R", fixtures().join("Shop").to_str().unwrap(), root.to_str().unwrap()]).status().unwrap();
    assert!(copy.success());
    let git = |args: &[&str]| assert!(std::process::Command::new("git").args(args).current_dir(&root).output().unwrap().status.success());
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.name", "Ada Lovelace"]);
    git(&["config", "user.email", "ada@example.com"]);
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "Initial shop"]);
    let calc = root.join("src/Shop.Core/PriceCalculator.cs");
    let text = std::fs::read_to_string(&calc).unwrap();
    std::fs::write(&calc, text.replacen("public", "/// <summary>Prices orders.</summary>\npublic", 1)).unwrap();
    git(&["commit", "-qam", "Document the price calculator"]);
    git(&["checkout", "-qb", "feature/coupons"]);
    std::fs::write(root.join("src/Shop.Core/Coupon.cs"), "namespace Shop.Core;\n\npublic record Coupon(string Code, decimal Percent);\n").unwrap();

    let mut app = test_app(home.path(), true);
    app.settings.roslyn_path = Some("/nonexistent".into());
    app.open_workspace(&root.join("Shop.sln"));
    app.open_file(&calc);
    let doc = app.docs.active_doc_mut().unwrap();
    let t = doc.editor.buffer.rope().to_string();
    let at = doc.editor.buffer.rope().byte_to_char(t.find("{").unwrap() + 1);
    doc.editor.replace(at, at, "\n    private const decimal FreeShippingFrom = 50m;");
    app.save_all_docs();
    app.dock.show(crate::toolwindow::ToolWindow::Commit);
    app.dock.show(crate::toolwindow::ToolWindow::Git);
    app.vcs.message = "Add coupons".into();
    let mut h = harness(app, vec2(1440.0, 900.0));
    run_frames(&mut h, "status", |a| a.vcs.changes().len() >= 2 && !a.vcs.log.is_empty() && !a.docs.active_doc().unwrap().editor.line_changes.is_empty());
    let first = h.state().vcs.log[0].hash.clone();
    h.state_mut().vcs.select_commit(&first);
    run_frames(&mut h, "commit files", |a| !a.vcs.commit_files.is_empty());
    h.render().unwrap().save(out.join("git-dark.png")).unwrap();
    {
        let app = h.state_mut();
        let (path, text) = (calc.clone(), app.docs.active_doc().unwrap().editor.buffer.rope().to_string());
        app.vcs.toggle_blame(&path, text, &mut app.docs);
    }
    run_frames(&mut h, "blame", |a| a.docs.active_doc().unwrap().editor.annotations.is_some());
    h.render().unwrap().save(out.join("blame-dark.png")).unwrap();
    h.state_mut().vcs.open_diff("src/Shop.Core/PriceCalculator.cs");
    run_frames(&mut h, "diff", |a| a.vcs.diff.is_some());
    h.render().unwrap().save(out.join("diff-dark.png")).unwrap();
}

#[test]
#[ignore = "Requires a GPU, the .NET SDK and the Roslyn server in target/test-home; writes captures"]
fn screens_insight() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    let home = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-home").canonicalize().unwrap();
    let mut app = test_app(&home, true);
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path().canonicalize().unwrap().join("Shop");
    assert!(std::process::Command::new("cp").args(["-R", fixtures().join("Shop").to_str().unwrap(), root.to_str().unwrap()]).status().unwrap().success());
    app.open_workspace(&root.join("Shop.sln"));
    let program = root.join("src/Shop.Cli/Program.cs");
    app.open_file(&program);
    let mut h = harness(app, vec2(1440.0, 900.0));
    run_frames(&mut h, "Roslyn", |a| a.insight.status == crate::insight::InsightStatus::Ready && a.docs.active_doc().is_some_and(|d| !d.editor.semantic.is_empty()));
    if std::env::var_os("CRABCRAFT_DEBUG_DIAGS").is_some() {
        let d = h.state().docs.active_doc().unwrap();
        for x in &d.editor.diagnostics {
            eprintln!("DIAG {:?} {:?} {} {:?} unnecessary={}", d.editor.buffer.line_col(x.start), x.severity, x.message, x.code, x.unnecessary);
        }
        eprintln!("SEM0 {:?}", d.editor.semantic.first());
    }
    // Completion after typing "calculator." on a new line inside the file.
    {
        let app = h.state_mut();
        let doc = app.docs.active_doc_mut().unwrap();
        let text = doc.editor.buffer.rope().to_string();
        let line = text.lines().position(|l| l.contains("Calculate(")).unwrap_or(5);
        let pos = doc.editor.buffer.pos_at(line + 1, 0);
        doc.editor.replace(pos, pos, "var c = new PriceCalculator();\nc.\n");
        doc.editor.selection = rc_text::Selection::point(pos + "var c = new PriceCalculator();\nc.".len());
        let i = app.docs.active.unwrap();
        app.insight.on_editor_events(&mut app.docs.list[i], &[rc_editor::EditorEvent::Typed('.')]);
    }
    run_frames(&mut h, "completion", |a| a.docs.active_doc().and_then(|d| d.editor.popups.completion.as_ref()).is_some_and(|c| !c.filtered.is_empty() && c.doc.is_some()));
    h.render().unwrap().save(out.join("completion-dark.png")).unwrap();
    // Parameter info inside the call.
    {
        let app = h.state_mut();
        let doc = app.docs.active_doc_mut().unwrap();
        doc.editor.popups.close_all();
        let caret = doc.editor.caret();
        doc.editor.replace(caret, caret, "Calculate(");
        let i = app.docs.active.unwrap();
        app.insight.on_editor_events(&mut app.docs.list[i], &[rc_editor::EditorEvent::Typed('(')]);
    }
    run_frames(&mut h, "signature", |a| a.docs.active_doc().is_some_and(|d| d.editor.popups.signature.is_some()));
    h.render().unwrap().save(out.join("signature-dark.png")).unwrap();
    // Hover over a type, with the broken call's diagnostic.
    {
        let app = h.state_mut();
        let doc = app.docs.active_doc_mut().unwrap();
        doc.editor.popups.close_all();
        let text = doc.editor.buffer.rope().to_string();
        let pos = doc.editor.buffer.rope().byte_to_char(text.find("PriceCalculator()").unwrap() + 3);
        let i = app.docs.active.unwrap();
        app.insight.on_editor_events(&mut app.docs.list[i], &[rc_editor::EditorEvent::HoverAt { pos, screen: egui::pos2(700.0, 300.0) }]);
    }
    run_frames(&mut h, "hover", |a| a.docs.active_doc().is_some_and(|d| d.editor.popups.hover.is_some()));
    h.render().unwrap().save(out.join("hover-dark.png")).unwrap();
    // Context actions.
    {
        let app = h.state_mut();
        app.docs.active_doc_mut().unwrap().editor.popups.close_all();
        app.caret_screen = Some(egui::pos2(500.0, 320.0));
        app.run_insight_action(rc_editor::Action::ShowIntentions, egui::pos2(500.0, 320.0));
    }
    run_frames(&mut h, "actions", |a| matches!(a.dialog, Some(crate::ui::dialogs::Dialog::CodeActions { .. })) || a.notices.iter().any(|n| n.text.contains("context")));
    h.render().unwrap().save(out.join("actions-dark.png")).unwrap();
    h.state_mut().insight.stop();
}

/// The README's main capture: Roslyn colours and inlay hints, the explorer and a finished run.
#[test]
#[ignore = "Requires a GPU, the .NET SDK and Roslyn in target/test-home; writes target/snapshots/readme-*.png"]
fn screens_readme() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    let roslyn = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-home/tools/roslyn").canonicalize().expect("run the rc-lsp tests first");
    let roslyn = std::fs::read_dir(&roslyn).unwrap().flatten().map(|e| e.path().join("Microsoft.CodeAnalysis.LanguageServer")).find(|p| p.exists()).unwrap();
    let shop = fixtures().join("Shop");
    let calc = shop.join("src/Shop.Core/PriceCalculator.cs");
    for dark in [true, false] {
        let home = tempfile::tempdir().unwrap();
        let mut app = test_app(home.path(), dark);
        app.settings.roslyn_path = Some(roslyn.clone());
        app.open_workspace(&shop.join("Shop.sln"));
        app.explorer.expanded.insert(format!("proj:{}", shop.join("src/Shop.Core/Shop.Core.csproj").display()));
        app.open_file(&shop.join("src/Shop.Core/Order.cs"));
        app.open_file(&calc);
        app.locate_active();
        app.docs.active_doc_mut().unwrap().editor.goto(26, 12);
        let mut h = harness(app, vec2(1440.0, 900.0));
        run_frames(&mut h, "Roslyn", |a| {
            a.insight.status == crate::insight::InsightStatus::Ready && a.docs.active_doc().is_some_and(|d| !d.editor.semantic.is_empty() && !d.editor.inlays.is_empty()) && a.vcs.branch_name().is_some() && !a.runner.configs.is_empty()
        });
        {
            let app = h.state_mut();
            app.runner.selected = app.runner.configs.iter().position(|c| c.name.starts_with("Shop.Cli")).unwrap();
            app.pending.push(rc_editor::Action::Run);
        }
        run_frames(&mut h, "Shop.Cli", |a| a.runner.runs.first().is_some_and(|r| r.exit.is_some()));
        h.run_steps(10);
        let suffix = if dark { "dark" } else { "light" };
        h.render().unwrap().save(out.join(format!("readme-workspace-{suffix}.png"))).unwrap();
        h.state_mut().insight.stop();
    }
}

/// Rust and Cargo file icons and the source/test root folders in the explorer and the tabs.
#[test]
#[ignore = "Requires a GPU; writes target/snapshots/rust-icons-*.png"]
fn screens_rust_icons() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    let project = tempfile::Builder::new().prefix("crabcraft-icons").tempdir().unwrap();
    let root = project.path().canonicalize().unwrap();
    for d in ["src", "tests", "examples", "benches", "docs"] {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"icons\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::write(root.join("Cargo.lock"), "version = 4\n").unwrap();
    std::fs::write(root.join("rustfmt.toml"), "max_width = 100\n").unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(root.join("tests/it.rs"), "#[test]\nfn it() {}\n").unwrap();
    std::fs::write(root.join("docs/notes.md"), "# Notes\n").unwrap();
    for dark in [false, true] {
        let home = tempfile::tempdir().unwrap();
        let mut app = test_app(home.path(), dark);
        app.open_workspace(&root);
        for d in ["src", "tests"] {
            app.explorer.expanded.insert(format!("dir:{}", root.join(d).display()));
        }
        app.open_file(&root.join("Cargo.toml"));
        app.open_file(&root.join("Cargo.lock"));
        app.open_file(&root.join("src/main.rs"));
        let mut h = harness(app, vec2(1100.0, 600.0));
        h.run_steps(10);
        let suffix = if dark { "dark" } else { "light" };
        h.render().unwrap().save(out.join(format!("rust-icons-{suffix}.png"))).unwrap();
        h.state_mut().insight.stop();
    }
}

/// Local history on a Cargo project: every save keeps a version, the dialog lists them newest
/// first, and Revert puts an older text in the editor as one undoable edit (the file on disk
/// stays as it was until the next save).
#[test]
fn local_history_lists_saved_versions_and_reverts_as_an_undoable_edit() {
    use crate::ui::dialogs::Dialog;
    use egui_kittest::kittest::Queryable;
    use rc_kit::i18n::tr;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("demo");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n").unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    let root = root.canonicalize().unwrap();
    let main = root.join("src/main.rs");
    let mut app = test_app(home.path(), true);
    app.open_workspace(&root);
    app.open_file(&main);
    let mut h = harness(app, vec2(1200.0, 800.0));
    let ctx = h.ctx.clone();
    const ONE: &str = "fn main() {\n    println!(\"one\");\n}\n";
    const TWO: &str = "fn main() {\n    println!(\"two\");\n}\n";
    for text in [ONE, TWO] {
        let d = h.state_mut().docs.active_doc_mut().unwrap();
        let len = d.editor.buffer.len_chars();
        d.editor.replace(0, len, text);
        h.state_mut().run_ide_action(&ctx, rc_editor::Action::Save);
        h.run_steps(2);
    }
    assert_eq!(std::fs::read_to_string(&main).unwrap(), TWO);

    h.state_mut().run_ide_action(&ctx, rc_editor::Action::ShowLocalHistory);
    let versions = |a: &CrabCraftApp| match &a.dialog {
        Some(Dialog::LocalHistory(v)) => v.revisions.as_ref().map(|r| r.len()),
        _ => None,
    };
    run_frames(&mut h, "the local history", |a| versions(a).is_some());
    // The text it was opened with, then each save, newest first.
    assert_eq!(versions(h.state()), Some(3));
    assert!(home.path().join("history").is_dir());

    // The newest version is the saved text: nothing to revert. The one before is selected
    // with the arrow keys and previewed.
    h.key_press(egui::Key::ArrowDown);
    let loaded = |a: &CrabCraftApp| match &a.dialog {
        Some(Dialog::LocalHistory(v)) => v.selected == 1 && v.revisions.as_ref().and_then(|r| r.get(1)).is_some_and(|r| v.texts.get(&r.time_ms).is_some_and(|t| t.as_deref() == Ok(ONE))),
        _ => false,
    };
    run_frames(&mut h, "the selected version", loaded);
    h.get_by_label(tr("Revert to This Version")).click();
    h.run_steps(3);
    assert!(h.state().dialog.is_none());
    let doc = h.state().docs.active_doc().unwrap();
    assert_eq!(doc.editor.buffer.rope().to_string(), ONE);
    assert!(doc.is_modified());
    assert_eq!(std::fs::read_to_string(&main).unwrap(), TWO, "reverting does not write the file");

    // Undo brings the current version back in one step.
    h.state_mut().docs.active_doc_mut().unwrap().editor.undo();
    assert_eq!(h.state().docs.active_doc().unwrap().editor.buffer.rope().to_string(), TWO);
    assert!(!h.state().docs.active_doc().unwrap().is_modified());
}

/// A Cargo workspace of two packages, `alpha` and `beta`, that both mention `needle_text`.
fn two_crates(root: &Path) {
    let write = |rel: &str, text: &str| {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    };
    write("Cargo.toml", "[workspace]\nmembers = [\"alpha\", \"beta\"]\nresolver = \"2\"\n");
    write("alpha/Cargo.toml", "[package]\nname = \"alpha\"\nversion = \"0.1.0\"\nedition = \"2021\"\n");
    write("alpha/src/lib.rs", "pub mod alpha_util;\n\npub fn alpha() -> &'static str {\n    \"needle_text\"\n}\n");
    write("alpha/src/alpha_util.rs", "pub fn util() {}\n");
    write("beta/Cargo.toml", "[package]\nname = \"beta\"\nversion = \"0.1.0\"\nedition = \"2021\"\n");
    write("beta/src/beta_util.rs", "pub fn util() {}\n");
    write("beta/src/lib.rs", "// The beta crate.\n\n\npub const NEEDLE: &str = \"needle_text\";\n");
}

/// A harness on [`two_crates`], once Cargo's model and the file index are loaded.
fn two_crates_harness(home: &Path, dark: bool) -> (Harness<'static, CrabCraftApp>, PathBuf) {
    let root = home.join("ws");
    two_crates(&root);
    let root = root.canonicalize().unwrap();
    let mut app = test_app(&home.join("home"), dark);
    app.open_workspace(&root);
    let mut h = harness(app, vec2(1280.0, 800.0));
    run_frames(&mut h, "the Cargo workspace and the file index", |a| {
        a.runner.cargo_workspace.as_ref().is_some_and(|w| w.packages.len() == 2) && a.files.index.as_ref().is_some_and(|i| i.snapshot().len() >= 6)
    });
    (h, root)
}

fn press_search_everywhere(h: &mut Harness<'static, CrabCraftApp>) {
    let chord = h.state().keymap.chord(rc_editor::Action::SearchEverywhere).unwrap();
    assert_eq!(chord.key, egui::Key::P, "Ctrl+P / ⌘P opens Search Everywhere");
    h.key_press_modifiers(chord.mods, chord.key);
    h.run_steps(3);
}

fn se<'a>(h: &'a Harness<'static, CrabCraftApp>) -> &'a crate::search::SearchEverywhere {
    h.state().search_everywhere.as_ref().expect("Search Everywhere is open")
}

#[test]
fn ctrl_p_searches_files_and_text_in_the_chosen_scope() {
    use crate::search::{SeScope, SeTab};
    use egui_kittest::kittest::Queryable;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let (mut h, root) = two_crates_harness(dir.path(), false);

    // Ctrl+P opens on All; Tab goes through Classes to Files and Shift+Tab back.
    press_search_everywhere(&mut h);
    assert_eq!(se(&h).tab, SeTab::All);
    for t in ["All", "Classes", "Files", "Symbols", "Actions", "Text", "Workspace"] {
        assert!(h.query_by_label(t).is_some(), "{t} is not shown");
    }
    h.key_press(egui::Key::Tab);
    h.run_steps(1);
    assert_eq!(se(&h).tab, SeTab::Classes);
    h.key_press(egui::Key::Tab);
    h.run_steps(1);
    h.key_press(egui::Key::Tab);
    h.run_steps(1);
    assert_eq!(se(&h).tab, SeTab::Symbols);
    h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::Tab);
    h.run_steps(1);
    assert_eq!(se(&h).tab, SeTab::Files);

    // Files: what is typed finds the file.
    h.input_mut().events.push(egui::Event::Text("alpha_util".into()));
    h.run_steps(3);
    assert_eq!(se(&h).text, "alpha_util");
    assert!(h.query_by_label("alpha_util.rs").is_some(), "alpha_util.rs not found");

    // Text: every line with the text, then Enter opens the first one at its line.
    h.state_mut().search_everywhere.as_mut().unwrap().text = "needle_text".into();
    for _ in 0..3 {
        h.key_press(egui::Key::Tab);
        h.run_steps(1);
    }
    assert_eq!(se(&h).tab, SeTab::Text);
    run_frames(&mut h, "both text matches", |a| a.search_everywhere.as_ref().is_some_and(|s| s.text_hits.len() == 2 && !s.text_searching()));
    let hits: Vec<(String, usize)> = se(&h).text_hits.iter().map(|t| (t.rel.clone(), t.line)).collect();
    assert_eq!(hits, vec![("alpha/src/lib.rs".to_string(), 3), ("beta/src/lib.rs".to_string(), 3)]);
    assert!(se(&h).text_hits[0].preview.starts_with("\"needle_text\""), "{:?}", se(&h).text_hits[0]);
    assert!(h.query_by_label("lib.rs:4").is_some() || h.query_all_by_label("\"needle_text\"").count() > 0);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert!(h.state().search_everywhere.is_none());
    let doc = h.state().docs.active_doc().expect("a document");
    assert_eq!(doc.path, root.join("alpha/src/lib.rs"));
    assert_eq!(doc.editor.caret_line_col(), (3, 5), "at the match, after the quote");

    // The scope selector offers each crate; `beta` keeps only its own files and lines.
    press_search_everywhere(&mut h);
    h.get_by_label("Workspace").click();
    h.run_steps(2);
    assert!(h.query_by_label("Workspace and Dependencies").is_some());
    h.get_by_label("Crate beta").click();
    h.run_steps(2);
    assert!(h.state().search_everywhere.is_some(), "choosing a scope keeps the popup open");
    assert_eq!(se(&h).scope, SeScope::Package { name: "beta".into(), rel: "beta".into() });
    {
        let s = h.state_mut().search_everywhere.as_mut().unwrap();
        s.tab = SeTab::Files;
        s.text = "util".into();
    }
    h.run_steps(3);
    assert!(h.query_by_label("beta_util.rs").is_some(), "beta's file is in the beta scope");
    assert!(h.query_by_label("alpha_util.rs").is_none(), "alpha's file is outside the beta scope");
    {
        let s = h.state_mut().search_everywhere.as_mut().unwrap();
        s.tab = SeTab::Text;
        s.text = "needle_text".into();
    }
    run_frames(&mut h, "the text match in beta", |a| a.search_everywhere.as_ref().is_some_and(|s| !s.text_hits.is_empty() && !s.text_searching()));
    let rels: Vec<&str> = se(&h).text_hits.iter().map(|t| t.rel.as_str()).collect();
    assert_eq!(rels, vec!["beta/src/lib.rs"]);

    // The scope is remembered when the popup opens again.
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(h.state().search_everywhere.is_none());
    press_search_everywhere(&mut h);
    assert_eq!(se(&h).scope, SeScope::Package { name: "beta".into(), rel: "beta".into() });
}

#[test]
#[ignore = "Requires a GPU; writes Search Everywhere captures to target/snapshots"]
fn screens_search_everywhere() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    for dark in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let (mut h, _) = two_crates_harness(dir.path(), dark);
        let suffix = if dark { "dark" } else { "light" };
        press_search_everywhere(&mut h);
        {
            let s = h.state_mut().search_everywhere.as_mut().unwrap();
            s.tab = crate::search::SeTab::Text;
            s.text = "needle".into();
        }
        run_frames(&mut h, "text matches", |a| a.search_everywhere.as_ref().is_some_and(|s| s.text_hits.len() == 2 && !s.text_searching()));
        h.render().unwrap().save(out.join(format!("search-everywhere-text-{suffix}.png"))).unwrap();
        {
            use egui_kittest::kittest::Queryable;
            h.get_by_label("Workspace").click();
        }
        h.run_steps(3);
        h.render().unwrap().save(out.join(format!("search-everywhere-scope-{suffix}.png"))).unwrap();
    }
}


/// A project of `n` files that all match `item_`, with Search Everywhere open on them.
fn many_files_harness(home: &Path, n: usize, tab: crate::search::SeTab) -> Harness<'static, CrabCraftApp> {
    let root = home.join("ws");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"many\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n").unwrap();
    for i in 0..n {
        std::fs::write(root.join(format!("src/item_{i:03}.rs")), "pub fn f() {}\n").unwrap();
    }
    let root = root.canonicalize().unwrap();
    let mut app = test_app(&home.join("home"), true);
    app.open_workspace(&root);
    let mut h = harness(app, vec2(1280.0, 900.0));
    run_frames(&mut h, "the file index", |a| a.files.index.as_ref().is_some_and(|i| i.is_ready() && i.snapshot().len() > n));
    press_search_everywhere(&mut h);
    h.state_mut().search_everywhere.as_mut().unwrap().tab = tab;
    h.input_mut().events.push(egui::Event::Text("item_".into()));
    h.run_steps(3);
    h
}

fn wheel(h: &mut Harness<'static, CrabCraftApp>, at: egui::Pos2, dy: f32) {
    h.input_mut().events.push(egui::Event::PointerMoved(at));
    h.run_steps(1);
    h.input_mut().events.push(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: vec2(0.0, dy), phase: egui::TouchPhase::Move, modifiers: egui::Modifiers::NONE });
    h.run_steps(1);
}

#[test]
fn search_everywhere_results_grow_and_keep_their_wheel_scroll() {
    use crate::search::SeTab;
    use egui_kittest::kittest::Queryable;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let mut h = many_files_harness(dir.path(), 150, SeTab::Files);

    // The Files tab lists them all, and the popup grows to most of the window.
    assert!(h.query_by_label("item_000.rs").is_some(), "item_000.rs is not listed");
    assert!(h.query_by_label("item_149.rs").is_some(), "the Files tab stops before 150 files");
    let popup = h.ctx.memory(|m| m.area_rect(egui::Id::new("search_everywhere"))).unwrap();
    assert!(popup.height() > 600.0, "the popup does not grow: {popup:?}");

    // A wheel scroll stays where it is left: the first row moves up and does not snap back.
    let top = h.get_by_label("item_000.rs").rect().top();
    wheel(&mut h, popup.center(), -300.0);
    h.run_steps(10);
    let scrolled = h.get_by_label("item_000.rs").rect().top();
    assert!(scrolled < top - 100.0, "the wheel did not scroll: {top} -> {scrolled}");
    h.run_steps(10);
    assert_eq!(h.get_by_label("item_000.rs").rect().top(), scrolled, "the scroll snapped back");

    // The keyboard still scrolls the selected row into view, here back to the first one.
    h.key_press(egui::Key::ArrowDown);
    h.key_press(egui::Key::ArrowUp);
    h.run_steps(5);
    assert_eq!(h.state().search_everywhere.as_ref().unwrap().selected, 0);
    assert_eq!(h.get_by_label("item_000.rs").rect().top(), top, "the selected row is not revealed");
    for _ in 0..60 {
        h.key_press(egui::Key::ArrowDown);
    }
    h.run_steps(5);
    assert_eq!(h.state().search_everywhere.as_ref().unwrap().selected, 60);
    assert!(h.get_by_label("item_000.rs").rect().top() < top - 600.0, "ArrowDown does not follow the selection");

    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    h.render().unwrap().save(out.join("search-everywhere-many-files-dark.png")).unwrap();
}

#[test]
fn search_everywhere_all_tab_offers_more_files() {
    use crate::search::SeTab;
    use egui_kittest::kittest::Queryable;
    use rc_kit::i18n::tr;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let mut h = many_files_harness(dir.path(), 30, SeTab::All);

    // All shows the first files and a row that opens the Files tab with the rest.
    assert!(h.query_by_label("item_000.rs").is_some());
    assert!(h.query_by_label("item_029.rs").is_none(), "All lists every file");
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    h.render().unwrap().save(out.join("search-everywhere-all-more-dark.png")).unwrap();
    h.get_by_label(tr("… more")).click();
    h.run_steps(3);
    let se = h.state().search_everywhere.as_ref().expect("Search Everywhere stays open");
    assert_eq!(se.tab, SeTab::Files);
    assert_eq!(se.text, "item_");
    assert!(h.query_by_label("item_029.rs").is_some(), "the Files tab does not list the rest");
}

#[test]
fn find_in_files_suggests_a_rust_file_mask() {
    use egui_kittest::kittest::Queryable;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]
name = \"mask\"
version = \"0.1.0\"
edition = \"2021\"

[workspace]
").unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}
").unwrap();
    let mut app = test_app(&dir.path().join("home"), false);
    app.open_workspace(&root.canonicalize().unwrap());
    app.pending.push(rc_editor::Action::FindInFiles);
    let mut h = harness(app, vec2(1280.0, 800.0));
    h.run_steps(3);
    assert!(h.state().find_in_files.is_some(), "Find in Files did not open");
    let hint = |h: &Harness<'static, CrabCraftApp>, mask: &'static str| h.query_by(|n| n.placeholder() == Some(mask)).is_some();
    assert!(hint(&h, "*.rs"), "the file mask does not suggest *.rs");
    assert!(!hint(&h, "*.cs"), "the file mask still suggests *.cs");
}

/// Sixty branches in a small window: the branches popup stays inside it, its actions stay at the
/// top and the list scrolls down to the last branch.
#[test]
fn the_branches_popup_scrolls_to_every_branch_inside_the_window() {
    use egui_kittest::kittest::Queryable;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("demo");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n").unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    let root = root.canonicalize().unwrap();
    let git = |args: &[&str]| assert!(std::process::Command::new("git").args(args).current_dir(&root).output().unwrap().status.success(), "git {args:?}");
    git(&["init", "-q", "-b", "trunk"]);
    git(&["config", "user.name", "Ada Lovelace"]);
    git(&["config", "user.email", "ada@example.com"]);
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "Start"]);
    for n in 0..60 {
        git(&["branch", &format!("feature/topic-{n:02}")]);
    }
    let mut app = test_app(&dir.path().join("home"), true);
    app.open_workspace(&root);
    let size = vec2(900.0, 500.0);
    let mut h = harness(app, size);
    run_frames(&mut h, "the branches", |a| a.vcs.branch_name().as_deref() == Some("trunk") && a.vcs.branches.len() >= 61);
    let last = h.state().vcs.branches.iter().filter(|b| b.kind == rc_vcs::BranchKind::Local && !b.is_head).last().map(|b| b.name.clone()).unwrap();
    h.get_by_label("trunk").click();
    h.run_steps(3);
    h.render().unwrap().save(out.join("branches-popup-top.png")).unwrap();
    // The popup ends inside the window, with New Branch… still there.
    let popups: Vec<egui::Rect> = h.ctx.memory(|m| m.areas().visible_layer_ids().into_iter().filter(|l| l.order == egui::Order::Foreground).filter_map(|l| m.area_rect(l.id)).collect());
    assert!(!popups.is_empty());
    for r in &popups {
        assert!(r.bottom() <= size.y && r.top() >= 0.0, "the popup leaves the window: {r:?}");
    }
    let new_branch = h.get_by_label("New Branch…").rect();
    assert!(new_branch.bottom() <= size.y);
    // The last branch is below the visible part of the list until the list is scrolled.
    assert!(h.get_by_label(&last).rect().top() > size.y, "{:?}", h.get_by_label(&last).rect());
    let over = h.get_by_label("feature/topic-00").rect().center();
    for _ in 0..40 {
        h.input_mut().events.push(egui::Event::PointerMoved(over));
        h.input_mut().events.push(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Line, delta: vec2(0.0, -3.0), modifiers: egui::Modifiers::NONE, phase: egui::TouchPhase::Move });
        h.run_steps(1);
    }
    h.run_steps(3);
    h.render().unwrap().save(out.join("branches-popup-bottom.png")).unwrap();
    let r = h.get_by_label(&last).rect();
    assert!(r.top() >= 0.0 && r.bottom() <= size.y, "the last branch is out of view: {r:?}");
    // The actions above the list did not scroll away.
    assert_eq!(h.get_by_label("New Branch…").rect(), new_branch);
}

/// A right click at `at` in the rendered window, as a mouse sends it.
fn right_click(h: &mut Harness<'static, CrabCraftApp>, at: egui::Pos2) {
    h.event(egui::Event::PointerMoved(at));
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed: true, modifiers: Default::default() });
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed: false, modifiers: Default::default() });
    h.run_steps(2);
}

/// Clicks the context menu's `entry` and returns the text the frames after it copied.
fn click_entry(h: &mut Harness<'static, CrabCraftApp>, entry: &str) -> (Vec<String>, bool) {
    use egui_kittest::kittest::Queryable;
    h.get_by_label(entry).click();
    let (mut copied, mut paste) = (Vec::new(), false);
    for _ in 0..3 {
        h.run_steps(1);
        for c in &h.output().platform_output.commands {
            if let egui::OutputCommand::CopyText(t) = c {
                copied.push(t.clone());
            }
        }
        paste |= h.output().viewport_output.values().any(|v| v.commands.contains(&egui::ViewportCommand::RequestPaste));
    }
    (copied, paste)
}

#[test]
fn cut_copy_and_paste_from_the_editor_context_menu() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let file = home.path().join("notes.txt");
    std::fs::write(&file, "alpha beta\ngamma\n").unwrap();
    let mut app = test_app(home.path(), true);
    app.open_file(&file);
    let mut h = harness(app, vec2(1280.0, 800.0));
    h.run_steps(3);
    let text = |h: &Harness<'static, CrabCraftApp>| h.state().docs.active_doc().map(|d| d.editor.buffer.rope().to_string()).unwrap_or_default();
    // Select "alpha" and right-click on it: the selection stays.
    let select_alpha = |h: &mut Harness<'static, CrabCraftApp>| {
        let doc = &mut h.state_mut().docs.list[0];
        doc.editor.set_selection(rc_text::Selection::single(rc_text::Range::new(0, 5)));
        h.run_steps(2);
        h.state().caret_screen.unwrap() + vec2(-8.0, -6.0)
    };
    let at = select_alpha(&mut h);
    right_click(&mut h, at);
    let (copied, _) = click_entry(&mut h, "Copy");
    assert_eq!(copied, vec!["alpha".to_string()]);
    assert_eq!(text(&h), "alpha beta\ngamma\n");

    let at = select_alpha(&mut h);
    right_click(&mut h, at);
    let (copied, _) = click_entry(&mut h, "Cut");
    assert_eq!(copied, vec!["alpha".to_string()]);
    assert_eq!(text(&h), " beta\ngamma\n");

    // Paste asks the platform for the clipboard; its answer goes into the editor.
    h.state_mut().docs.list[0].editor.goto(1, 0);
    h.run_steps(2);
    let at = h.state().caret_screen.unwrap() + vec2(2.0, -6.0);
    right_click(&mut h, at);
    let (_, requested) = click_entry(&mut h, "Paste");
    assert!(requested, "the clipboard was not requested");
    h.event(egui::Event::Paste("alpha".into()));
    h.run_steps(2);
    assert_eq!(text(&h), " beta\nalphagamma\n");
}


/// The VS Code keymap drives the app: Ctrl+P finds files, Ctrl+Shift+P actions, and Ctrl+D
/// adds the next occurrence in the editor.
#[test]
fn vscode_shortcuts_reach_their_actions() {
    use crate::search::SeTab;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let (mut h, root) = two_crates_harness(dir.path(), false);
    h.state_mut().keymap = rc_editor::Keymap::new(rc_editor::KeymapKind::VsCode);
    let ctrl = egui::Modifiers::CTRL;

    h.key_press_modifiers(ctrl, egui::Key::P);
    h.run_steps(3);
    assert_eq!(se(&h).tab, SeTab::Files);
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(h.state().search_everywhere.is_none());
    h.key_press_modifiers(ctrl | egui::Modifiers::SHIFT, egui::Key::P);
    h.run_steps(3);
    assert_eq!(se(&h).tab, SeTab::Actions);
    h.key_press(egui::Key::Escape);
    h.run_steps(3);

    let file = root.join("beta/src/lib.rs");
    h.state_mut().open_at(&file, 3, 14);
    h.run_steps(3);
    h.key_press_modifiers(ctrl, egui::Key::D);
    h.run_steps(2);
    let doc = h.state().docs.active_doc().unwrap();
    let rope = doc.editor.buffer.rope();
    let texts: Vec<String> = doc.editor.selection.ranges().iter().map(|r| rope.slice(r.start()..r.end()).to_string()).collect();
    assert_eq!(texts, ["NEEDLE"], "Ctrl+D selects the word at the caret first");
}

/// The project's `.editorconfig` and `rustfmt.toml` set the indentation and right margin of
/// its files, and saving trims trailing whitespace and ends the file with one newline.
#[test]
fn the_project_code_style_applies_on_open_and_save() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("styled");
    let write = |name: &str, text: &str| {
        let p = root.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    };
    write("Cargo.toml", "[package]\nname = \"styled\"\nversion = \"0.1.0\"\nedition = \"2021\"\n");
    write(".editorconfig", "root = true\n[*]\nindent_style = space\nindent_size = 2\ntrim_trailing_whitespace = true\ninsert_final_newline = true\n[*.md]\ntrim_trailing_whitespace = false\n");
    write("rustfmt.toml", "hard_tabs = true\nmax_width = 90\n");
    // Four-space lines: detection alone would pick four spaces.
    write("notes.txt", "a  \n    b\t\n    c");
    write("src/main.rs", "fn main() {\n    let x = 1;\n}\n");
    write("README.md", "keep  \n");
    let root = root.canonicalize().unwrap();
    let mut app = test_app(&dir.path().join("home"), false);
    app.open_workspace(&root);

    app.open_file(&root.join("notes.txt"));
    let doc = app.docs.active_doc().unwrap();
    assert_eq!(doc.editor.indent.unit, "  ");
    assert_eq!(doc.editor.right_margin, None);
    app.open_file(&root.join("src/main.rs"));
    let doc = app.docs.active_doc().unwrap();
    assert_eq!(doc.editor.indent.unit, "\t", "rustfmt.toml wins for Rust files");
    assert_eq!(doc.editor.right_margin, Some(90));

    app.open_file(&root.join("notes.txt"));
    let i = app.docs.active.unwrap();
    app.docs.list[i].editor.goto(0, 0);
    app.docs.list[i].editor.replace(0, 0, "z");
    app.docs.save(i).unwrap();
    // The caret's line keeps its spaces; the others lose theirs, and a newline ends the file.
    assert_eq!(std::fs::read_to_string(root.join("notes.txt")).unwrap(), "za  \n    b\n    c\n");

    app.open_file(&root.join("README.md"));
    let i = app.docs.active.unwrap();
    app.docs.list[i].editor.replace(0, 0, "x");
    app.docs.list[i].editor.goto(0, 0);
    app.docs.save(i).unwrap();
    assert_eq!(std::fs::read_to_string(root.join("README.md")).unwrap(), "xkeep  \n");
}

/// The Build window's Sync tab lists each load of the Cargo model with what Cargo printed;
/// a broken manifest shows Cargo's error there.
#[test]
fn the_sync_tab_shows_each_cargo_load() {
    use egui_kittest::kittest::Queryable;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let (mut h, root) = two_crates_harness(dir.path(), false);
    run_frames(&mut h, "the project and dependency loads", |a| a.sync_log.entries.len() >= 2);
    let kinds: Vec<_> = h.state().sync_log.entries.iter().map(|e| e.kind).collect();
    assert!(kinds.contains(&crate::sync_log::SyncKind::Project) && kinds.contains(&crate::sync_log::SyncKind::Dependencies), "{kinds:?}");
    let project = h.state().sync_log.entries.iter().find(|e| e.kind == crate::sync_log::SyncKind::Project).unwrap().clone();
    assert!(project.log.success);
    assert!(project.log.command.starts_with("cargo metadata --no-deps"), "{}", project.log.command);

    h.state_mut().toggle_tool(crate::toolwindow::ToolWindow::Build);
    h.run_steps(3);
    h.get_by_label(rc_kit::i18n::tr("Sync")).click();
    h.run_steps(3);
    assert!(h.state().sync_log.tab_open);
    h.get_by_label(rc_kit::i18n::tr("Cargo project"));

    // A broken manifest: the reload fails and the tab says so, with Cargo's message.
    std::fs::write(root.join("Cargo.toml"), "[workspace\nmembers = [\n").unwrap();
    h.state_mut().reload_cargo();
    run_frames(&mut h, "the failed reload", |a| a.sync_log.last_failed());
    let last = h.state().sync_log.entries.last().unwrap().clone();
    assert_eq!(last.kind, crate::sync_log::SyncKind::Project);
    assert!(last.log.stderr.contains("Cargo.toml"), "{}", last.log.stderr);
    h.run_steps(3);
    h.get_by_label(&rc_kit::i18n::trf("{name} (failed)", &[("name", &rc_kit::i18n::tr("Sync"))]));
}

#[test]
#[ignore = "Requires a GPU; writes target/snapshots/sync-tab-*.png"]
fn sync_tab_capture() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    for dark in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let (mut h, root) = two_crates_harness(dir.path(), dark);
        run_frames(&mut h, "both loads", |a| a.sync_log.entries.len() >= 2);
        std::fs::write(root.join("Cargo.toml"), "[workspace\nmembers = [\n").unwrap();
        h.state_mut().reload_cargo();
        run_frames(&mut h, "the failed reload", |a| a.sync_log.last_failed());
        h.state_mut().toggle_tool(crate::toolwindow::ToolWindow::Build);
        h.state_mut().sync_log.tab_open = true;
        h.run_steps(4);
        h.render().unwrap().save(out.join(format!("sync-tab-{}.png", if dark { "dark" } else { "light" }))).unwrap();
    }
}
