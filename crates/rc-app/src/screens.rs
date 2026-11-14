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
    // Tests decide when buffers are saved; the idle autosave test turns it back on.
    settings.autosave_idle = false;
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

/// Typing in a Rust file of a real Cargo project and then pausing saves it, without rustfmt
/// and without running the project; the buffer stays unsaved while the user keeps typing.
#[test]
fn idle_autosave_writes_edited_rust_files_after_the_pause() {
    use std::time::{Duration, Instant};
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let root = project.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"idle-save\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::write(root.join("build.rs"), "fn main() { std::fs::write(\"build-ran\", \"yes\").unwrap(); }\n").unwrap();
    let main = root.join("src/main.rs");
    std::fs::write(&main, "fn main() {}\n").unwrap();
    let mut app = test_app(home.path(), true);
    app.settings.autosave_idle = true;
    app.settings.autosave_delay_secs = 1;
    app.open_workspace_and_wait(&root);
    app.open_file_and_wait(&main);
    app.docs.active_doc_mut().unwrap().editor.selection = rc_text::Selection::point(0);
    let mut h = harness(app, egui::vec2(1200.0, 800.0));
    h.run_steps(2);
    h.event(egui::Event::Text("//   spaced\n".into()));
    h.run_steps(2);
    assert!(h.state().docs.active_doc().unwrap().is_modified());
    // Still typing inside the delay: nothing is written.
    std::thread::sleep(Duration::from_millis(600));
    h.event(egui::Event::Text("// more\n".into()));
    h.run_steps(2);
    std::thread::sleep(Duration::from_millis(600));
    h.run_steps(2);
    assert_eq!(std::fs::read_to_string(&main).unwrap(), "fn main() {}\n");
    let start = Instant::now();
    while h.state().docs.active_doc().unwrap().is_modified() || h.state().disk_save.saves.busy() {
        assert!(start.elapsed() < Duration::from_secs(15), "idle autosave did not run");
        std::thread::sleep(Duration::from_millis(50));
        h.run_steps(1);
    }
    assert_eq!(std::fs::read_to_string(&main).unwrap(), "//   spaced\n// more\nfn main() {}\n");
    assert!(!root.join("build-ran").exists());
    let output = std::process::Command::new("cargo")
        .args(["check", "--offline", "--quiet", "--manifest-path"])
        .arg(root.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", home.path().join("target"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}

/// Alt+Down and Alt+Up move the caret's line in the default Rider keymap, in a Rust file of a
/// real Cargo project; the moved code still compiles and nothing runs on its own.
#[test]
fn alt_arrows_move_lines_in_a_rust_file() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let root = project.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"move-lines\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::write(root.join("build.rs"), "fn main() { std::fs::write(\"build-ran\", \"yes\").unwrap(); }\n").unwrap();
    let main = root.join("src/main.rs");
    let before = "fn main() {\n    let a = 1;\n    let b = a + 1;\n    println!(\"{b}\");\n}\n";
    std::fs::write(&main, before).unwrap();
    let mut app = test_app(home.path(), true);
    app.open_workspace_and_wait(&root);
    app.open_file_and_wait(&main);
    // The caret on `let a = 1;`.
    app.docs.active_doc_mut().unwrap().editor.selection = rc_text::Selection::point(16);
    let mut h = harness(app, egui::vec2(1200.0, 800.0));
    h.run_steps(2);
    let alt = egui::Modifiers { alt: true, ..Default::default() };
    let key = |h: &mut Harness<'static, CrabCraftApp>, key| {
        h.event(egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: alt });
        h.run_steps(2);
    };
    let text = |h: &Harness<'static, CrabCraftApp>| h.state().docs.active_doc().unwrap().editor.buffer.rope().to_string();
    key(&mut h, egui::Key::ArrowDown);
    assert_eq!(text(&h), "fn main() {\n    let b = a + 1;\n    let a = 1;\n    println!(\"{b}\");\n}\n");
    key(&mut h, egui::Key::ArrowUp);
    key(&mut h, egui::Key::ArrowUp);
    assert_eq!(text(&h), "    let a = 1;\nfn main() {\n    let b = a + 1;\n    println!(\"{b}\");\n}\n");
    key(&mut h, egui::Key::ArrowDown);
    assert_eq!(text(&h), before);
    h.state_mut().save_all_and_wait();
    assert_eq!(std::fs::read_to_string(&main).unwrap(), before);
    assert!(!root.join("build-ran").exists());
    let output = std::process::Command::new("cargo")
        .args(["check", "--offline", "--quiet", "--manifest-path"])
        .arg(root.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", home.path().join("target"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}

/// CrabCraft always starts on the welcome screen: neither a closed project nor the one open
/// when it quit is reopened. Paths from the command line or the system still open.
#[test]
fn startup_always_shows_the_welcome_screen() {
    use crate::cli::OpenRequest;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let sln = fixtures().join("RustWeb");
    let mut app = test_app(home.path(), true);
    app.open_workspace_and_wait(&sln);
    app.save_all_and_wait();
    assert!(crate::app::startup_paths(Vec::new()).is_empty());
    app.close_project();
    app.save_all_and_wait();
    assert!(crate::app::startup_paths(Vec::new()).is_empty());
    assert_eq!(rc_core::Settings::load().recent_workspaces.first(), Some(&sln));
    // Files from the command line or the file manager still open.
    let file = fixtures().join("RustWeb/Cargo.toml");
    assert_eq!(crate::app::startup_paths(vec![OpenRequest::path(&file)]), vec![OpenRequest::path(&file)]);
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
    app.dialog = Some(Dialog::Settings { draft: Box::new(draft), page: SettingsPage::Tools, browse: Default::default() });
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
    // Outside every Cargo package: rust-analyzer gets it as a file on its own.
    assert!(h.state().docs.active_doc().is_some_and(|d| d.scratch && crate::insight::has_insight(d)));
    assert_eq!(
        h.state().docs.active_doc().map(|d| d.editor.buffer.rope().to_string()),
        Some(
            "fn main() {
    
}
"
            .into()
        )
    );
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
    app.open_workspace_and_wait(&root);
    for f in ["lib.rs", "main.rs", "util.rs"] {
        app.open_file_and_wait(&root.join("src").join(f));
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
    app.open_workspace_and_wait(&root);
    let names = |app: &CrabCraftApp, g: &crate::layout::Group| {
        g.tabs.iter().filter_map(|t| app.docs.list.iter().find(|d| d.editor.id == *t)).map(|d| d.name()).collect::<Vec<_>>()
    };
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
#[ignore = "Requires a GPU; writes Git tag and stash captures to target/snapshots"]
fn screens_git_tags_and_stashes() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    let home = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path().canonicalize().unwrap();
    let git = |args: &[&str]| assert!(std::process::Command::new("git").args(args).current_dir(&root).output().unwrap().status.success(), "{args:?}");
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.name", "Ada Lovelace"]);
    git(&["config", "user.email", "ada@example.com"]);
    git(&["config", "tag.gpgsign", "false"]);
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"shop\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn price(n: u32) -> u32 {\n    n * 2\n}\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "Initial shop"]);
    git(&["tag", "v0.1.0"]);
    std::fs::write(root.join("src/lib.rs"), "/// Prices an order.\npub fn price(n: u32) -> u32 {\n    n * 2\n}\n").unwrap();
    git(&["commit", "-qam", "Document price"]);
    git(&["tag", "-a", "v0.2.0", "-m", "Second release\n\nDocuments the price function."]);
    std::fs::write(root.join("src/lib.rs"), "/// Prices an order.\npub fn price(n: u32) -> u32 {\n    n * 3\n}\n").unwrap();
    std::fs::write(root.join("src/discount.rs"), "pub fn discount(p: u32) -> u32 {\n    p / 10\n}\n").unwrap();
    git(&["stash", "push", "--include-untracked", "-m", "Try a higher price with a discount"]);
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"shop\"\nversion = \"0.3.0\"\nedition = \"2021\"\n").unwrap();
    git(&["stash", "push"]);

    let mut app = test_app(home.path(), true);
    app.open_workspace_and_wait(&root);
    app.trust_project_tools_for_test();
    app.dock.show(crate::toolwindow::ToolWindow::Git);
    app.dock.sizes.insert(crate::toolwindow::ToolWindow::Git, 420.0);
    let mut h = harness(app, vec2(1440.0, 900.0));
    run_frames(&mut h, "tags", |a| a.vcs.tags.len() == 2);
    {
        let app = h.state_mut();
        app.vcs.tab = crate::vcs::GitTab::Tags;
        app.vcs.selected_tag = Some("v0.2.0".into());
        app.vcs.output =
            Some(crate::vcs::OpOutput { text: "To /srv/git/shop.git\n*\trefs/tags/v0.2.0:refs/tags/v0.2.0\t[new tag]\nDone".into(), error: false });
    }
    h.run_steps(4);
    h.render().unwrap().save(out.join("git-tags-dark.png")).unwrap();
    h.state_mut().dialog = Some(crate::ui::dialogs::Dialog::NewTag { name: "v0.3.0".into(), target: None, message: "Third release".into() });
    h.run_steps(4);
    h.render().unwrap().save(out.join("git-new-tag-dark.png")).unwrap();
    h.state_mut().dialog = Some(crate::ui::dialogs::Dialog::CheckoutTag { name: "v0.1.0".into() });
    h.run_steps(4);
    h.render().unwrap().save(out.join("git-checkout-tag-dark.png")).unwrap();
    h.state_mut().dialog = None;
    run_frames(&mut h, "stashes", |a| a.vcs.stashes.len() == 2);
    {
        let app = h.state_mut();
        app.vcs.tab = crate::vcs::GitTab::Stashes;
        let hash = app.vcs.stashes[1].hash.clone();
        app.vcs.select_stash(&hash);
    }
    run_frames(&mut h, "stash files", |a| a.vcs.stash_files.len() == 2);
    h.run_steps(4);
    h.render().unwrap().save(out.join("git-stashes-dark.png")).unwrap();
    let stash = h.state().vcs.stashes[1].clone();
    h.state_mut().dialog = Some(crate::ui::dialogs::Dialog::DropStash { stash });
    h.run_steps(4);
    h.render().unwrap().save(out.join("git-drop-stash-dark.png")).unwrap();
    h.state_mut().dialog =
        Some(crate::ui::dialogs::Dialog::StashChanges { message: "Refactor prices".into(), include_untracked: true, keep_index: false, paths: Vec::new() });
    h.run_steps(4);
    h.render().unwrap().save(out.join("git-stash-changes-dark.png")).unwrap();
}

#[test]
#[ignore = "Requires a GPU; writes Git worktree captures to target/snapshots"]
fn screens_git_worktrees() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    let home = tempfile::tempdir().unwrap();
    let place = tempfile::tempdir().unwrap();
    let place = place.path().canonicalize().unwrap();
    let root = place.join("shop");
    std::fs::create_dir_all(root.join("src")).unwrap();
    let git = |args: &[&str]| assert!(std::process::Command::new("git").args(args).current_dir(&root).output().unwrap().status.success(), "{args:?}");
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.name", "Ada Lovelace"]);
    git(&["config", "user.email", "ada@example.com"]);
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"shop\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn price(n: u32) -> u32 {\n    n * 2\n}\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "Initial shop"]);
    git(&["branch", "release/0.1"]);
    let wt = |args: &[&str]| git(&[&["worktree", "add", "-q"], args].concat());
    wt(&["-b", "feature/login", "--", place.join("shop-feature-login").to_str().unwrap()]);
    wt(&["--", place.join("shop-release-0.1").to_str().unwrap(), "release/0.1"]);
    wt(&["--detach", "--", place.join("shop-bisect").to_str().unwrap()]);
    git(&["worktree", "lock", "--reason", "On the USB disk", "--", place.join("shop-release-0.1").to_str().unwrap()]);
    std::fs::remove_dir_all(place.join("shop-bisect")).unwrap();

    let mut app = test_app(home.path(), true);
    app.open_workspace_and_wait(&root);
    app.trust_project_tools_for_test();
    app.dock.show(crate::toolwindow::ToolWindow::Git);
    app.dock.sizes.insert(crate::toolwindow::ToolWindow::Git, 420.0);
    let mut h = harness(app, vec2(1440.0, 900.0));
    run_frames(&mut h, "worktrees", |a| a.vcs.worktrees.len() == 4 && !a.vcs.branches.is_empty());
    {
        let app = h.state_mut();
        app.vcs.tab = crate::vcs::GitTab::Worktrees;
        app.vcs.selected_worktree = Some(place.join("shop-release-0.1"));
    }
    h.run_steps(4);
    h.render().unwrap().save(out.join("git-worktrees-dark.png")).unwrap();
    h.state_mut().dialog = Some(crate::ui::dialogs::Dialog::AddWorktree {
        path: String::new(),
        new_branch: true,
        name: "fix/rounding".into(),
        existing: String::new(),
        path_edited: false,
    });
    h.run_steps(4);
    h.render().unwrap().save(out.join("git-add-worktree-dark.png")).unwrap();
    h.state_mut().dialog = Some(crate::ui::dialogs::Dialog::RemoveWorktree { path: place.join("shop-feature-login"), force: true });
    h.run_steps(4);
    h.render().unwrap().save(out.join("git-remove-worktree-dark.png")).unwrap();
    h.state_mut().dialog = Some(crate::ui::dialogs::Dialog::PruneWorktrees { paths: vec![place.join("shop-bisect")] });
    h.run_steps(4);
    h.render().unwrap().save(out.join("git-prune-worktrees-dark.png")).unwrap();
}

/// V-06 and V-14: the history of a renamed file in the Log, with a signed commit selected.
#[test]
#[ignore = "Requires a GPU; writes target/snapshots/git-file-history-dark.png"]
fn screens_git_file_history() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    let home = tempfile::tempdir().unwrap();
    let place = tempfile::tempdir().unwrap();
    let root = place.path().canonicalize().unwrap().join("shop");
    std::fs::create_dir_all(root.join("src")).unwrap();
    let git = |args: &[&str]| assert!(std::process::Command::new("git").args(args).current_dir(&root).output().unwrap().status.success(), "{args:?}");
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.name", "Ada Lovelace"]);
    git(&["config", "user.email", "ada@example.com"]);
    git(&["config", "commit.gpgsign", "false"]);
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"shop\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::write(root.join("src/prices.rs"), "pub fn price(n: u32) -> u32 {\n    n * 2\n}\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "Initial shop"]);
    std::fs::write(root.join("README.md"), "# Shop\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "Add a readme"]);
    git(&["mv", "src/prices.rs", "src/lib.rs"]);
    git(&["commit", "-q", "-m", "Rename prices.rs to lib.rs"]);
    std::fs::write(root.join("src/lib.rs"), "pub fn price(n: u32) -> u32 {\n    n * 3\n}\n").unwrap();
    git(&["commit", "-q", "-am", "Raise prices"]);

    let mut app = test_app(home.path(), true);
    app.open_workspace_and_wait(&root);
    app.trust_project_tools_for_test();
    app.dock.show(crate::toolwindow::ToolWindow::Git);
    app.dock.sizes.insert(crate::toolwindow::ToolWindow::Git, 420.0);
    let mut h = harness(app, vec2(1440.0, 900.0));
    run_frames(&mut h, "the log", |a| a.vcs.log.len() == 4 && !a.vcs.branches.is_empty());
    h.state_mut().show_git_history(&root.join("src/lib.rs"));
    run_frames(&mut h, "the history", |a| !a.vcs.log_loading && a.vcs.log.len() == 3);
    let newest = h.state().vcs.log[0].hash.clone();
    h.state_mut().vcs.select_commit(&newest);
    h.state_mut().vcs.signature = Some((
        newest,
        rc_vcs::Signature { status: rc_vcs::SignatureStatus::Good, signer: "Ada Lovelace <ada@example.com>".into(), key: "TEST-SIGNATURE".into() },
    ));
    run_frames(&mut h, "the files", |a| !a.vcs.commit_files.is_empty());
    h.state_mut().vcs.signature = Some((
        h.state().vcs.log[0].hash.clone(),
        rc_vcs::Signature { status: rc_vcs::SignatureStatus::Good, signer: "Ada Lovelace <ada@example.com>".into(), key: "TEST-SIGNATURE".into() },
    ));
    h.run_steps(4);
    h.render().unwrap().save(out.join("git-file-history-dark.png")).unwrap();
}

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
        app.open_workspace_and_wait(&root);
        for d in ["src", "tests"] {
            app.explorer.expanded.insert(format!("dir:{}", root.join(d).display()));
        }
        app.open_file_and_wait(&root.join("Cargo.toml"));
        app.open_file_and_wait(&root.join("Cargo.lock"));
        app.open_file_and_wait(&root.join("src/main.rs"));
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
    app.open_workspace_and_wait(&root);
    app.open_file_and_wait(&main);
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
        Some(Dialog::LocalHistory(v)) => {
            v.selected == 1 && v.revisions.as_ref().and_then(|r| r.get(1)).is_some_and(|r| v.texts.get(&r.time_ms).is_some_and(|t| t.as_deref() == Ok(ONE)))
        }
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
    app.open_workspace_and_wait(&root);
    app.trust_project_tools_for_test();
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
    app.open_workspace_and_wait(&root);
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
    h.input_mut().events.push(egui::Event::MouseWheel {
        source: egui::MouseWheelSource::Unknown,
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(0.0, dy),
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers::NONE,
    });
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
    let popup = h.ctx.memory(|m| m.area_rect(egui::Id::unique("search_everywhere"))).unwrap();
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
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]
name = \"mask\"
version = \"0.1.0\"
edition = \"2021\"

[workspace]
",
    )
    .unwrap();
    std::fs::write(
        root.join("src/main.rs"),
        "fn main() {}
",
    )
    .unwrap();
    let mut app = test_app(&dir.path().join("home"), false);
    app.open_workspace_and_wait(&root.canonicalize().unwrap());
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
    app.open_workspace_and_wait(&root);
    app.trust_project_tools_for_test();
    let size = vec2(900.0, 500.0);
    let mut h = harness(app, size);
    run_frames(&mut h, "the branches", |a| a.vcs.branch_name().as_deref() == Some("trunk") && a.vcs.branches.len() >= 61);
    let last = h.state().vcs.branches.iter().rfind(|b| b.kind == rc_vcs::BranchKind::Local && !b.is_head).map(|b| b.name.clone()).unwrap();
    h.get_by_label("trunk").click();
    h.run_steps(3);
    h.render().unwrap().save(out.join("branches-popup-top.png")).unwrap();
    // The popup ends inside the window, with New Branch… still there.
    let popups: Vec<egui::Rect> =
        h.ctx.memory(|m| m.areas().visible_layer_ids().into_iter().filter(|l| l.order == egui::Order::Foreground).filter_map(|l| m.area_rect(l.id)).collect());
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
        h.input_mut().events.push(egui::Event::MouseWheel {
            source: egui::MouseWheelSource::Unknown,
            unit: egui::MouseWheelUnit::Line,
            delta: vec2(0.0, -3.0),
            modifiers: egui::Modifiers::NONE,
            phase: egui::TouchPhase::Move,
        });
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
    app.open_file_and_wait(&file);
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
    run_frames(&mut h, "file navigation", |app| !app.disk_open.busy());
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
    write(
        ".editorconfig",
        "root = true\n[*]\nindent_style = space\nindent_size = 2\ntrim_trailing_whitespace = true\ninsert_final_newline = true\n[*.md]\ntrim_trailing_whitespace = false\n",
    );
    write("rustfmt.toml", "hard_tabs = true\nmax_width = 90\n");
    // Four-space lines: detection alone would pick four spaces.
    write("notes.txt", "a  \n    b\t\n    c");
    write("src/main.rs", "fn main() {\n    let x = 1;\n}\n");
    write("README.md", "keep  \n");
    let root = root.canonicalize().unwrap();
    let mut app = test_app(&dir.path().join("home"), false);
    app.open_workspace_and_wait(&root);

    app.open_file_and_wait(&root.join("notes.txt"));
    let doc = app.docs.active_doc().unwrap();
    assert_eq!(doc.editor.indent.unit, "  ");
    assert_eq!(doc.editor.right_margin, None);
    app.open_file_and_wait(&root.join("src/main.rs"));
    let doc = app.docs.active_doc().unwrap();
    assert_eq!(doc.editor.indent.unit, "\t", "rustfmt.toml wins for Rust files");
    assert_eq!(doc.editor.right_margin, Some(90));

    app.open_file_and_wait(&root.join("notes.txt"));
    let i = app.docs.active.unwrap();
    app.docs.list[i].editor.goto(0, 0);
    app.docs.list[i].editor.replace(0, 0, "z");
    app.docs.save(i).unwrap();
    // The caret's line keeps its spaces; the others lose theirs, and a newline ends the file.
    assert_eq!(std::fs::read_to_string(root.join("notes.txt")).unwrap(), "za  \n    b\n    c\n");

    app.open_file_and_wait(&root.join("README.md"));
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

#[test]
#[ignore = "Requires a GPU; writes target/snapshots/light-edit-*.png"]
fn light_edit_capture() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    for dark in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        two_crates(&dir.path().join("ws"));
        let mut app = test_app(&dir.path().join("home"), dark);
        app.open_path(&dir.path().join("ws/beta/src/lib.rs"));
        assert!(app.workspace.is_none());
        let mut h = harness(app, vec2(1280.0, 800.0));
        h.run_steps(6);
        h.render().unwrap().save(out.join(format!("light-edit-{}.png", if dark { "dark" } else { "light" }))).unwrap();
    }
}

/// A Rust file opened without its project is in LightEdit: the editor says which Cargo
/// project it belongs to and opens it on request.
#[test]
fn light_edit_offers_the_files_cargo_project() {
    use egui_kittest::kittest::Queryable;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    two_crates(&ws);
    let ws = ws.canonicalize().unwrap();
    assert_eq!(crate::docs::cargo_project_of(&ws.join("beta/src/lib.rs")), Some(ws.clone()), "the workspace, not the member");
    let loose = dir.path().join("notes.txt");
    std::fs::write(&loose, "plain\n").unwrap();

    let mut app = test_app(&dir.path().join("home"), false);
    app.open_file_and_wait(&loose);
    app.open_file_and_wait(&ws.join("beta/src/lib.rs"));
    assert!(app.workspace.is_none());
    assert_eq!(app.docs.list[0].light_project, None);
    let mut h = harness(app, vec2(1280.0, 800.0));
    h.run_steps(4);
    h.get_by_label(rc_kit::i18n::tr("LightEdit"));
    h.get_by_label(rc_kit::i18n::tr("Open Project")).click();
    h.run_steps(4);
    assert_eq!(h.state().workspace.as_ref().map(|w| w.ws.root.canonicalize().unwrap()), Some(ws));
    assert!(h.query_by_label(rc_kit::i18n::tr("LightEdit")).is_none());
}

#[cfg(unix)]
#[test]
#[ignore = "Requires a GPU; writes target/snapshots/external-tools-*.png"]
fn external_tools_capture() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let (mut h, root) = two_crates_harness(dir.path(), true);
    h.state_mut().settings.external_tools = vec![rc_core::external_tools::ExternalTool {
        name: "Count lines".into(),
        program: "wc".into(),
        args: "-l $FileRelativePath$".into(),
        working_dir: String::new(),
    }];
    h.state_mut().open_file_and_wait(&root.join("beta/src/lib.rs"));
    h.state_mut().run_external_tool("Count lines");
    run_frames(&mut h, "the tool", |a| a.runner.runs.first().is_some_and(|r| r.exit.is_some()));
    h.run_steps(4);
    h.render().unwrap().save(out.join("external-tools-run.png")).unwrap();
    h.state_mut().open_settings(crate::ui::dialogs::SettingsPage::ExternalTools);
    h.run_steps(4);
    h.render().unwrap().save(out.join("external-tools-settings.png")).unwrap();
}

#[test]
#[ignore = "Requires a GPU, rustup and the network; writes target/snapshots/toolchain-updates.png"]
fn toolchain_updates_capture() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    let home = tempfile::tempdir().unwrap();
    let mut app = test_app(home.path(), true);
    app.check_toolchain_updates();
    app.open_settings(crate::ui::dialogs::SettingsPage::Tools);
    let mut h = harness(app, vec2(1280.0, 800.0));
    run_frames(&mut h, "rustup check and the toolchain", |a| a.toolchain_updates.is_some() && a.toolchain.is_some());
    h.run_steps(4);
    h.render().unwrap().save(out.join("toolchain-updates.png")).unwrap();
}

#[test]
#[ignore = "Requires a GPU; writes target/snapshots/before-launch.png"]
fn before_launch_capture() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("demo");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    let mut app = test_app(&dir.path().join("home"), true);
    app.settings.external_tools = vec![
        rc_core::external_tools::ExternalTool { name: "Generate schema".into(), program: "sh".into(), ..Default::default() },
        rc_core::external_tools::ExternalTool { name: "Lint SQL".into(), program: "sqlfluff".into(), ..Default::default() },
    ];
    app.open_workspace_and_wait(&root);
    app.trust_project_tools_for_test();
    let mut h = harness(app, vec2(1280.0, 800.0));
    run_frames(&mut h, "the workspace", |a| !a.runner.configs.is_empty());
    let name = h.state().runner.configs[0].name.clone();
    h.state_mut()
        .runner
        .options
        .insert(name.clone(), rc_core::session::RunOptions { before_launch: vec!["Lint SQL".into(), "Old tool".into()], ..Default::default() });
    h.state_mut().edit_run_config(Some(name));
    h.run_steps(4);
    h.render().unwrap().save(out.join("before-launch.png")).unwrap();
}

/// A live template: its abbreviation and Tab insert the snippet with its first stop selected;
/// what is typed replaces it and Tab goes on to the body.
#[test]
fn live_templates_expand_with_tab_and_visit_their_stops() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let (mut h, root) = two_crates_harness(dir.path(), false);
    let file = root.join("beta/src/lib.rs");
    h.state_mut().open_file_and_wait(&file);
    let end = h.state().docs.active_doc().unwrap().editor.buffer.len_chars();
    h.state_mut().docs.active_doc_mut().unwrap().editor.selection = rc_text::Selection::point(end);
    h.run_steps(3);
    h.event(egui::Event::Text("test".into()));
    h.run_steps(2);
    h.key_press(egui::Key::Tab);
    h.run_steps(2);
    let selected = h.state().docs.active_doc().unwrap().editor.selected_text();
    assert_eq!(selected, "name", "the first stop is selected");
    h.event(egui::Event::Text("works".into()));
    h.run_steps(2);
    h.key_press(egui::Key::Tab);
    h.run_steps(2);
    h.event(egui::Event::Text("assert!(true);".into()));
    h.run_steps(2);
    let doc = h.state().docs.active_doc().unwrap();
    let text = doc.editor.buffer.rope().to_string();
    assert!(text.ends_with("#[test]\nfn works() {\n    assert!(true);\n}"), "{text}");
    assert!(doc.editor.snippet.is_none(), "the final stop ends the snippet");
    // Tab elsewhere indents as usual.
    h.key_press(egui::Key::Tab);
    h.run_steps(2);
    assert!(h.state().docs.active_doc().unwrap().editor.buffer.rope().to_string().contains("assert!(true);  \n"));
}

/// Types `text` one character at a time, as a user would (each one may open completion).
fn type_chars(h: &mut Harness<'static, CrabCraftApp>, text: &str) {
    for c in text.chars() {
        h.event(egui::Event::Text(c.to_string()));
        h.run_steps(2);
    }
}

fn completion_labels(h: &Harness<'static, CrabCraftApp>) -> Vec<String> {
    let doc = h.state().docs.active_doc().unwrap();
    doc.editor.popups.completion.as_ref().map(|c| c.filtered.iter().map(|&i| c.items[i].label.clone()).collect()).unwrap_or_default()
}

/// Cargo.toml of a real Cargo workspace is completed without the language server: typing a
/// key, a table header after `[`, ⌃Space for the keys of that table, then the value of a key
/// with a fixed set, each accepted with Enter.
#[test]
fn cargo_toml_completes_tables_keys_and_values() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let (mut h, root) = two_crates_harness(dir.path(), false);
    let file = root.join("alpha/Cargo.toml");
    h.state_mut().open_file_and_wait(&file);
    let end = h.state().docs.active_doc().unwrap().editor.buffer.len_chars();
    h.state_mut().docs.active_doc_mut().unwrap().editor.selection = rc_text::Selection::point(end);
    h.run_steps(3);
    let text = |h: &Harness<'static, CrabCraftApp>| h.state().docs.active_doc().unwrap().editor.buffer.rope().to_string();

    // Typing a key opens the list of [package] keys, with its description; `-` keeps it open.
    type_chars(&mut h, "rust-v");
    assert_eq!(completion_labels(&h).first().map(String::as_str), Some("rust-version"));
    let c = h.state().docs.active_doc().unwrap().editor.popups.completion.clone().unwrap();
    assert!(c.selected_item().and_then(|i| i.doc.as_deref()).is_some_and(|d| d.contains("Rust")));
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert!(text(&h).ends_with("edition = \"2021\"\nrust-version = \"\""), "{}", text(&h));
    type_chars(&mut h, "1.80");

    // `[` on a new line offers the tables the file does not have yet.
    h.key_press(egui::Key::End);
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    type_chars(&mut h, "[");
    let labels = completion_labels(&h);
    assert!(labels.contains(&"dependencies".to_string()) && !labels.contains(&"package".to_string()), "{labels:?}");
    type_chars(&mut h, "profile.rel");
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert!(text(&h).ends_with("rust-version = \"1.80\"\n[profile.release]"), "{}", text(&h));

    // ⌃Space lists the profile's keys; choosing `panic` goes on to its values.
    h.key_press(egui::Key::End);
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    let chord = h.state().keymap.chord(rc_editor::Action::Complete).unwrap();
    h.key_press_modifiers(chord.mods, chord.key);
    h.run_steps(2);
    assert!(completion_labels(&h).contains(&"codegen-units".to_string()), "{:?}", completion_labels(&h));
    type_chars(&mut h, "pan");
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert_eq!(completion_labels(&h), vec!["unwind", "abort"]);
    h.key_press(egui::Key::ArrowDown);
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert!(text(&h).ends_with("[profile.release]\npanic = \"abort\""), "{}", text(&h));
}

#[test]
#[ignore = "Requires a GPU; writes target/snapshots/cargo-toml-completion.png"]
fn cargo_toml_completion_capture() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let (mut h, root) = two_crates_harness(dir.path(), true);
    h.state_mut().open_file_and_wait(&root.join("alpha/Cargo.toml"));
    let end = h.state().docs.active_doc().unwrap().editor.buffer.len_chars();
    h.state_mut().docs.active_doc_mut().unwrap().editor.selection = rc_text::Selection::point(end);
    h.run_steps(3);
    type_chars(&mut h, "re");
    h.run_steps(4);
    h.render().unwrap().save(out.join("cargo-toml-completion.png")).unwrap();
}

#[test]
#[ignore = "Requires a GPU; writes target/snapshots/live-templates.png"]
fn live_templates_capture() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    let home = tempfile::tempdir().unwrap();
    let mut app = test_app(home.path(), false);
    app.open_settings(crate::ui::dialogs::SettingsPage::LiveTemplates);
    let mut h = harness(app, vec2(1280.0, 800.0));
    h.run_steps(4);
    h.render().unwrap().save(out.join("live-templates.png")).unwrap();
}

#[test]
#[ignore = "Requires a GPU; writes target/snapshots/refactor-preview.png"]
fn refactor_preview_capture() {
    use rc_lsp::lsp_types as lsp;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let (mut h, root) = two_crates_harness(dir.path(), true);
    let file = root.join("beta/src/lib.rs");
    let uri = rc_lsp::path_to_uri(&file);
    let edit = |line, a, b| lsp::TextEdit { range: lsp::Range::new(lsp::Position::new(line, a), lsp::Position::new(line, b)), new_text: "NEEDLE_NAME".into() };
    let ws_edit = lsp::WorkspaceEdit { changes: Some(std::collections::HashMap::from([(uri, vec![edit(3, 10, 16), edit(0, 7, 11)])])), ..Default::default() };
    let preview = crate::refactor_preview::RefactorPreview::start("Rename 'NEEDLE' to 'NEEDLE_NAME'".into(), &ws_edit, Vec::new(), rc_core::Waker::noop());
    h.state_mut().dialog = Some(crate::ui::dialogs::Dialog::RefactorPreview(Box::new(preview)));
    run_frames(&mut h, "the listing", |a| matches!(&a.dialog, Some(crate::ui::dialogs::Dialog::RefactorPreview(p)) if !p.loading()));
    h.run_steps(3);
    h.render().unwrap().save(out.join("refactor-preview.png")).unwrap();
}

#[test]
#[ignore = "Requires a GPU; writes target/snapshots/intention-preview-*.png"]
fn intention_preview_capture() {
    use rc_lsp::lsp_types as lsp;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    for dark in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let (mut h, root) = two_crates_harness(dir.path(), dark);
        let lib = root.join("alpha/src/lib.rs");
        h.state_mut().open_file_and_wait(&lib);
        h.run_steps(3);
        let edit =
            |line, a, b, text: &str| lsp::TextEdit { range: lsp::Range::new(lsp::Position::new(line, a), lsp::Position::new(line, b)), new_text: text.into() };
        let changes = [
            (rc_lsp::path_to_uri(&lib), vec![edit(2, 7, 12, "alpha_name"), edit(0, 0, 0, "use std::fmt;\n")]),
            (rc_lsp::path_to_uri(&root.join("alpha/src/alpha_util.rs")), vec![edit(0, 0, 16, "pub fn util() -> u32 {\n    1\n}")]),
        ];
        let action = lsp::CodeAction {
            title: "Rename and import".into(),
            kind: Some(lsp::CodeActionKind::QUICKFIX),
            edit: Some(lsp::WorkspaceEdit { changes: Some(std::collections::HashMap::from(changes)), ..Default::default() }),
            ..Default::default()
        };
        let other = lsp::Command { title: "Run a server command".into(), command: "server.command".into(), arguments: None };
        h.state_mut().caret_screen = Some(egui::pos2(260.0, 160.0));
        h.state_mut().dialog = Some(crate::ui::dialogs::Dialog::CodeActions {
            list: vec![lsp::CodeActionOrCommand::CodeAction(action), lsp::CodeActionOrCommand::Command(other)],
            selected: 0,
        });
        run_frames(&mut h, "the preview", |a| matches!(a.intention_preview.state, crate::intention_preview::State::Ready { .. }));
        h.run_steps(3);
        let suffix = if dark { "dark" } else { "light" };
        h.render().unwrap().save(out.join(format!("intention-preview-{suffix}.png"))).unwrap();
    }
}

#[test]
#[ignore = "Requires a GPU; writes target/snapshots/inspection-profiles-*.png"]
fn inspection_profiles_capture() {
    use rc_core::inspections::Level;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    for dark in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let (mut h, _root) = two_crates_harness(dir.path(), dark);
        let mut settings = h.state().settings.clone();
        settings.inspection_profile.set("unused_variables", Some(Level::WeakWarning));
        settings.inspection_profile.set(crate::inspection_profile::UNKNOWN_KEY, Some(Level::Off));
        settings.inspection_profile.set("clippy::needless_return", Some(Level::Error));
        h.state_mut().dialog = Some(crate::ui::dialogs::Dialog::Settings {
            draft: Box::new(settings),
            page: crate::ui::dialogs::SettingsPage::Inspections,
            browse: Default::default(),
        });
        h.run_steps(4);
        let suffix = if dark { "dark" } else { "light" };
        h.render().unwrap().save(out.join(format!("inspection-profiles-{suffix}.png"))).unwrap();
    }
}

#[test]
#[ignore = "Requires a GPU and Clippy; writes target/snapshots/inspection.png"]
fn inspection_capture() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let (mut h, root) = two_crates_harness(dir.path(), true);
    std::fs::write(root.join("beta/src/lib.rs"), "pub fn twice(x: &Vec<i32>) -> usize {\n    if x.len() == 0 { 0 } else { x.len() * 2 }\n}\n").unwrap();
    h.state_mut().inspect_code();
    run_frames(&mut h, "the inspection", |a| a.inspection.as_ref().is_some_and(|i| !i.running));
    h.run_steps(4);
    h.render().unwrap().save(out.join("inspection.png")).unwrap();
}

#[test]
#[ignore = "Requires a GPU; writes target/snapshots/attached-project.png"]
fn attached_project_capture() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let (mut h, root) = two_crates_harness(dir.path(), false);
    let gen_dir = root.join("tools/gen");
    std::fs::create_dir_all(gen_dir.join("src")).unwrap();
    std::fs::write(gen_dir.join("Cargo.toml"), "[package]\nname = \"gen\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n").unwrap();
    std::fs::write(gen_dir.join("src/main.rs"), "fn main() {}\n").unwrap();
    h.state_mut().set_attached(&gen_dir.join("Cargo.toml"), true);
    run_frames(&mut h, "the attached project", |a| a.runner.configs.iter().any(|c| c.name == "gen (gen)"));
    h.state_mut().toggle_tool(crate::toolwindow::ToolWindow::Cargo);
    h.run_steps(4);
    h.render().unwrap().save(out.join("attached-project.png")).unwrap();
}

/// A real paste in the editor records where the text went (for the imports it needs).
#[test]
fn a_paste_records_its_range() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let (mut h, root) = two_crates_harness(dir.path(), false);
    h.state_mut().open_file_and_wait(&root.join("beta/src/lib.rs"));
    h.state_mut().docs.active_doc_mut().unwrap().editor.selection = rc_text::Selection::point(0);
    h.run_steps(3);
    h.event(egui::Event::Paste("use x;\n".into()));
    h.run_steps(2);
    let doc = h.state().docs.active_doc().unwrap();
    assert_eq!(doc.editor.last_paste, Some((0, 7)));
    assert!(doc.editor.buffer.rope().to_string().starts_with("use x;\n// The beta crate."));
}

/// Settings › Keymap records a new shortcut for an action; after OK the app's keymap runs the
/// action with it, and Remove leaves the action without one.
#[test]
fn shortcuts_are_recorded_in_the_settings() {
    use crate::ui::dialogs::{Dialog, SettingsPage};
    use egui_kittest::kittest::Queryable;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let mut app = test_app(home.path(), false);
    app.open_settings(SettingsPage::Keymap);
    let mut h = harness(app, vec2(1280.0, 900.0));
    h.run_steps(3);
    let commit = format!("{:?}", rc_editor::Action::Commit);
    h.state().keymap.chord(rc_editor::Action::Commit).expect("Commit has a shortcut");
    // As the Record button of the Commit row does.
    let ctx = h.ctx.clone();
    ctx.data_mut(|d| d.insert_temp(egui::Id::unique("recording_shortcut"), commit.clone()));
    h.run_steps(1);
    h.key_press_modifiers(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::K);
    h.run_steps(3);
    let Some(Dialog::Settings { draft, .. }) = &h.state().dialog else { panic!("settings closed") };
    assert_eq!(draft.shortcuts.get(&commit).map(String::as_str), Some("ctrl+shift+K"));
    h.get_by_label(rc_kit::i18n::tr("OK")).click();
    h.run_steps(3);
    let chord = rc_editor::keymap::parse_chord("ctrl+shift+K").unwrap();
    assert_eq!(h.state().keymap.action_of(chord), Some(rc_editor::Action::Commit));
    assert_eq!(rc_core::Settings::load().shortcuts.get(&commit).map(String::as_str), Some("ctrl+shift+K"), "saved");

    let mut settings = h.state().settings.clone();
    settings.shortcuts.insert(commit, String::new());
    let ctx = h.ctx.clone();
    h.state_mut().apply_settings(&ctx, settings);
    assert_eq!(h.state().keymap.chord(rc_editor::Action::Commit), None);
}

/// [`two_crates`] where `alpha` uses `beta`, locked, with the Cargo window open.
fn dependent_crates_harness(home: &Path, dark: bool) -> (Harness<'static, CrabCraftApp>, PathBuf) {
    let root = home.join("ws");
    two_crates(&root);
    std::fs::write(root.join("alpha/Cargo.toml"), "[package]\nname = \"alpha\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nbeta = { path = \"../beta\" }\n\n[dev-dependencies]\nbeta = { path = \"../beta\" }\n").unwrap();
    let cargo = rc_cargo::Cargo::locate().expect("cargo is installed");
    assert!(cargo.std_command().args(["generate-lockfile", "--offline"]).current_dir(&root).status().unwrap().success());
    let root = root.canonicalize().unwrap();
    let mut app = test_app(&home.join("home"), dark);
    app.open_workspace_and_wait(&root);
    app.trust_project_tools_for_test();
    let mut h = harness(app, vec2(1280.0, 800.0));
    run_frames(&mut h, "the Cargo workspace", |a| a.runner.cargo_workspace.as_ref().is_some_and(|w| w.packages.len() == 2));
    h.state_mut().toggle_tool(crate::toolwindow::ToolWindow::Cargo);
    h.run_steps(3);
    (h, root)
}

/// Each member of the Cargo window has a Dependencies node; unfolding it reads `cargo tree`
/// in the background and lists the dependencies, logged in the Sync tab.
#[test]
fn the_cargo_window_shows_the_dependency_tree() {
    use crate::ui::cargo_window::DepTree;
    use egui_kittest::kittest::Queryable;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let (mut h, root) = dependent_crates_harness(dir.path(), false);
    assert!(h.state().cargo_trees.is_empty(), "nothing is read before the node is unfolded");
    let before = h.get_all_by_label("beta").count();
    // One Dependencies node per member: alpha's comes first.
    h.get_all_by_label(rc_kit::i18n::tr("Dependencies")).next().unwrap().click();
    h.run_steps(2);
    run_frames(&mut h, "the dependency tree", |a| a.cargo_trees.values().any(|t| matches!(t, DepTree::Ready(_))));
    let (key, tree) = h.state().cargo_trees.iter().next().unwrap();
    assert_eq!(key, &(root.join("Cargo.toml"), "alpha".to_string()));
    let DepTree::Ready(nodes) = tree else { panic!() };
    let names: Vec<_> = nodes.iter().map(|n| (n.depth, n.name.as_str(), n.kind)).collect();
    assert_eq!(names, vec![(0, "alpha", rc_cargo::DepKind::Normal), (1, "beta", rc_cargo::DepKind::Normal), (1, "beta", rc_cargo::DepKind::Dev)]);
    h.run_steps(2);
    assert_eq!(h.get_all_by_label("beta").count(), before + 2, "alpha's normal and dev dependency on beta");
    assert!(h.state().sync_log.entries.iter().any(|e| e.log.command.starts_with("cargo tree")));
    assert!(!root.join("target").exists());
}

#[test]
#[ignore = "Requires a GPU; writes target/snapshots/cargo-tree-*.png"]
fn cargo_tree_capture() {
    use egui_kittest::kittest::Queryable;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    for dark in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let (mut h, _) = dependent_crates_harness(dir.path(), dark);
        h.get_all_by_label(rc_kit::i18n::tr("Dependencies")).next().unwrap().click();
        h.run_steps(2);
        run_frames(&mut h, "the tree", |a| a.cargo_trees.values().any(|t| matches!(t, crate::ui::cargo_window::DepTree::Ready(_))));
        h.run_steps(3);
        h.render().unwrap().save(out.join(format!("cargo-tree-{}.png", if dark { "dark" } else { "light" }))).unwrap();
    }
}

/// A crate with `[features]`, its `Cargo.toml` open and the Cargo window shown.
fn features_harness(home: &Path, dark: bool) -> (Harness<'static, CrabCraftApp>, PathBuf) {
    let root = home.join("ws");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[features]\ndefault = [\"std\"]\nstd = []\nextra = []\n\n[workspace]\n",
    )
    .unwrap();
    std::fs::write(root.join("src/lib.rs"), "#[cfg(feature = \"extra\")]\npub fn extra() {}\n").unwrap();
    let root = root.canonicalize().unwrap();
    let mut app = test_app(&home.join("home"), dark);
    app.open_workspace_and_wait(&root);
    app.trust_project_tools_for_test();
    app.open_file_and_wait(&root.join("Cargo.toml"));
    let mut h = harness(app, vec2(1280.0, 800.0));
    run_frames(&mut h, "the Cargo workspace", |a| a.runner.cargo_workspace.is_some());
    h.state_mut().toggle_tool(crate::toolwindow::ToolWindow::Cargo);
    h.run_steps(3);
    (h, root)
}

/// The Cargo window lists each member's features with a switch; the gutter of `Cargo.toml`
/// shows the same state.
#[test]
fn the_cargo_window_turns_features_on_and_off() {
    use egui_kittest::kittest::Queryable;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let (mut h, root) = features_harness(dir.path(), false);
    let icon = |h: &Harness<'static, CrabCraftApp>, line| h.state().docs.active_doc().and_then(|d| d.editor.gutter_icons.get(&line).copied());
    assert_eq!(icon(&h, 8), Some(rc_editor::GutterIcon::FeatureOff));
    h.get_by_label(rc_kit::i18n::tr("Features")).click();
    h.run_steps(3);
    h.get_by_label("extra").click();
    h.run_steps(3);
    assert_eq!(h.state().cargo_features, vec!["demo/extra".to_string()]);
    assert_eq!(icon(&h, 8), Some(rc_editor::GutterIcon::FeatureOn));
    // std is on through default: clicking it changes nothing.
    h.get_by_label("std").click();
    h.run_steps(3);
    assert_eq!(h.state().cargo_features, vec!["demo/extra".to_string()]);
    h.get_by_label("extra").click();
    h.run_steps(3);
    assert!(h.state().cargo_features.is_empty());
    assert!(!root.join("target").exists(), "nothing was built");
}

#[test]
#[ignore = "Requires a GPU; writes target/snapshots/cargo-features-*.png"]
fn cargo_features_capture() {
    use egui_kittest::kittest::Queryable;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    for dark in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let (mut h, _) = features_harness(dir.path(), dark);
        h.get_by_label(rc_kit::i18n::tr("Features")).click();
        h.run_steps(3);
        h.state_mut().set_cargo_feature("demo", "extra", true);
        h.run_steps(3);
        h.render().unwrap().save(out.join(format!("cargo-features-{}.png", if dark { "dark" } else { "light" }))).unwrap();
    }
}

/// A crate with a crates.io dependency, a path dependency, two locked versions of a dev
/// dependency and a feature, with a hand-written `Cargo.lock`; its `Cargo.toml` open. crates.io
/// answers are given beforehand, so nothing goes to the network.
fn dep_versions_harness(home: &Path, dark: bool) -> (Harness<'static, CrabCraftApp>, PathBuf) {
    let root = home.join("ws");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("helper/src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nserde = { version = \"1\", features = [\"derive\"] }\nhelper = { path = \"helper\" }\n\n[dev-dependencies]\nrand = \"0.7\"\n\n[features]\nextra = []\n\n[workspace]\n",
    )
    .unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn f() {}\n").unwrap();
    std::fs::write(root.join("helper/Cargo.toml"), "[package]\nname = \"helper\"\nversion = \"0.2.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::write(root.join("helper/src/lib.rs"), "").unwrap();
    let reg = "source = \"registry+https://github.com/rust-lang/crates.io-index\"\n";
    std::fs::write(
        root.join("Cargo.lock"),
        format!("version = 4\n\n[[package]]\nname = \"demo\"\nversion = \"0.1.0\"\n\n[[package]]\nname = \"helper\"\nversion = \"0.2.0\"\n\n[[package]]\nname = \"rand\"\nversion = \"0.7.3\"\n{reg}\n[[package]]\nname = \"rand\"\nversion = \"0.8.5\"\n{reg}\n[[package]]\nname = \"serde\"\nversion = \"1.0.100\"\n{reg}"),
    )
    .unwrap();
    let root = root.canonicalize().unwrap();
    let mut app = test_app(&home.join("home"), dark);
    app.dep_versions.set_published("serde", rc_cargo::versions::parse_index("{\"vers\":\"1.0.229\",\"yanked\":false}\n{\"vers\":\"1.0.230\",\"yanked\":true}"));
    app.dep_versions.set_published("rand", rc_cargo::versions::parse_index("{\"vers\":\"0.7.3\",\"yanked\":false}"));
    app.open_workspace_and_wait(&root);
    app.trust_project_tools_for_test();
    app.open_file_and_wait(&root.join("Cargo.toml"));
    let mut h = harness(app, vec2(1100.0, 520.0));
    run_frames(&mut h, "the Cargo workspace and the lock", |a| {
        a.runner.cargo_workspace.is_some() && !a.dep_versions.loading() && a.docs.active_doc().is_some_and(|d| !d.editor.end_hints.is_empty())
    });
    (h, root)
}

/// Each dependency line shows its locked version, and the newest on crates.io when newer; a
/// crates.io dependency gets a docs.rs icon for its locked version. Path dependencies get
/// neither, and the feature switch keeps its line.
#[test]
fn cargo_toml_shows_locked_versions_and_docs_rs_icons() {
    use rc_editor::GutterIcon;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let (mut h, root) = dep_versions_harness(dir.path(), false);
    let hints = |h: &Harness<'static, CrabCraftApp>| h.state().docs.active_doc().unwrap().editor.end_hints.clone();
    let icon = |h: &Harness<'static, CrabCraftApp>, line| h.state().docs.active_doc().and_then(|d| d.editor.gutter_icons.get(&line).copied());
    let available = rc_kit::i18n::trf("{locked} · {latest} available", &[("locked", &"1.0.100"), ("latest", &"1.0.229")]);
    // serde (line 6) is behind crates.io; rand 0.7 picks 0.7.3 of the two locked versions.
    assert_eq!(hints(&h).into_iter().collect::<Vec<_>>(), vec![(6, available), (10, "0.7.3".to_string())]);
    assert_eq!(icon(&h, 6), Some(GutterIcon::DocsRs));
    assert_eq!(icon(&h, 7), None, "path dependency");
    assert_eq!(icon(&h, 10), Some(GutterIcon::DocsRs));
    assert_eq!(icon(&h, 13), Some(GutterIcon::FeatureOff));
    h.state_mut().open_docs_rs_at(6);
    assert_eq!(h.state().opened_url.as_deref(), Some("https://docs.rs/serde/1.0.100"));
    h.state_mut().open_docs_rs_at(10);
    assert_eq!(h.state().opened_url.as_deref(), Some("https://docs.rs/rand/0.7.3"));
    // The online check off: only the locked version.
    h.state_mut().settings.check_crates_io = false;
    h.run_steps(2);
    assert_eq!(hints(&h).get(&6).map(String::as_str), Some("1.0.100"));
    // An edited requirement is read again from the unsaved text.
    {
        let editor = &mut h.state_mut().docs.active_doc_mut().unwrap().editor;
        let text = editor.buffer.rope().to_string();
        let at = text.find("\"0.7\"").unwrap();
        editor.replace(at, at + 5, "\"0.8\"");
    }
    h.run_steps(2);
    assert_eq!(hints(&h).get(&10).map(String::as_str), Some("0.8.5"));
    assert!(!root.join("target").exists(), "nothing was built");
}

/// In a dependency table, typing a crate name lists crates.io's answers (given beforehand, so
/// nothing goes to the network); choosing one writes `name = ""` and lists its versions, newest
/// stable first with its short forms. Offline, names come from the lock and the local registry.
#[test]
fn cargo_toml_completes_crate_names_and_versions() {
    use rc_cargo::crates_io::CrateHit;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let (mut h, _root) = dep_versions_harness(dir.path(), false);
    let hits = vec![
        CrateHit { name: "tokio".into(), version: Some("1.40.0".into()), description: Some("An event-driven runtime".into()), downloads: 400_000_000 },
        CrateHit { name: "tokio-util".into(), version: Some("0.7.12".into()), description: None, downloads: 0 },
    ];
    for q in ["t", "to", "tok"] {
        h.state_mut().crate_names.set_answer(q, hits.clone());
    }
    h.state_mut()
        .dep_versions
        .set_published("tokio", rc_cargo::versions::parse_index("{\"vers\":\"1.39.0\"}\n{\"vers\":\"1.40.0\"}\n{\"vers\":\"1.41.0\",\"yanked\":true}"));
    let text = |h: &Harness<'static, CrabCraftApp>| h.state().docs.active_doc().unwrap().editor.buffer.rope().to_string();
    // The caret on a new line after `helper = { path = "helper" }`.
    {
        let editor = &mut h.state_mut().docs.active_doc_mut().unwrap().editor;
        let at = editor.buffer.rope().to_string().find("\n\n[dev-dependencies]").unwrap();
        editor.selection = rc_text::Selection::point(at);
    }
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    type_chars(&mut h, "tok");
    assert_eq!(completion_labels(&h), vec!["tokio", "tokio-util"]);
    let c = h.state().docs.active_doc().unwrap().editor.popups.completion.clone().unwrap();
    let first = c.selected_item().unwrap();
    assert_eq!((first.detail.as_str(), first.doc.as_deref()), ("1.40.0", Some("An event-driven runtime")));
    assert!(h.state().crate_names.searching().is_none(), "answered: nothing sent");
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert!(text(&h).contains("helper\" }\ntokio = \"\"\n"), "{}", text(&h));
    assert_eq!(completion_labels(&h), vec!["1.40.0", "1.40", "1", "1.39.0"]);
    h.key_press(egui::Key::ArrowDown);
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert!(text(&h).contains("\ntokio = \"1.40\"\n"), "{}", text(&h));

    // Offline: the lock's and the local registry's names, nothing asked.
    h.state_mut().settings.check_crates_io = false;
    h.state_mut().crate_names.set_local(vec!["rayon".into(), "regex".into()]);
    h.key_press(egui::Key::End);
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    type_chars(&mut h, "ra");
    assert_eq!(completion_labels(&h), vec!["rand", "rayon"]);
    assert!(h.state().crate_names.searching().is_none());
}

/// With the online check on, a dependency on a crate crates.io does not have is an error, a
/// requirement no published version matches a warning, and one the newest stable version is
/// outside a weak warning whose quick fix (Alt+Enter) updates it. Off, they are gone.
#[test]
fn cargo_toml_warns_about_unknown_and_outdated_crates() {
    use rc_editor::Severity;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let (mut h, root) = dep_versions_harness(dir.path(), false);
    h.state_mut().dep_versions.set_missing("no-such-crate");
    h.state_mut().dep_versions.set_published("anyhow", rc_cargo::versions::parse_index("{\"vers\":\"1.0.0\"}\n{\"vers\":\"2.0.1\"}"));
    h.state_mut().dep_versions.set_published("log", rc_cargo::versions::parse_index("{\"vers\":\"0.4.20\"}"));
    {
        let editor = &mut h.state_mut().docs.active_doc_mut().unwrap().editor;
        let text = editor.buffer.rope().to_string();
        let at = text.find("rand = \"0.7\"\n").unwrap() + "rand = \"0.7\"\n".len();
        editor.replace(at, at, "anyhow = \"1\"\nno-such-crate = \"1\"\nlog = { version = \"0.5\" }\n");
    }
    h.run_steps(3);
    let ours = |h: &Harness<'static, CrabCraftApp>| {
        let doc = h.state().docs.active_doc().unwrap();
        let text = doc.editor.buffer.rope().to_string();
        let mut out: Vec<(usize, String, Severity)> = doc
            .editor
            .diagnostics
            .iter()
            .filter(|d| d.code.as_deref() == Some(crate::dep_versions::DIAG_CODE))
            .map(|d| (text.chars().take(d.start).filter(|&c| c == '\n').count(), text.chars().skip(d.start).take(d.end - d.start).collect(), d.severity))
            .collect();
        out.sort_by_key(|d| d.0);
        out
    };
    assert_eq!(
        ours(&h),
        vec![(11, "1".to_string(), Severity::Info), (12, "no-such-crate".to_string(), Severity::Error), (13, "0.5".to_string(), Severity::Warning)]
    );
    let path = root.join("Cargo.toml");
    let problems = h.state().insight.problems.get(&path).cloned().unwrap_or_default();
    assert_eq!(
        problems.iter().map(|d| d.severity).collect::<Vec<_>>(),
        vec![Severity::Error, Severity::Warning],
        "the Problems window lists errors and warnings"
    );
    assert!(problems[0].message.contains("no-such-crate"), "{}", problems[0].message);

    // Alt+Enter on the outdated line offers the update.
    {
        let editor = &mut h.state_mut().docs.active_doc_mut().unwrap().editor;
        let at = editor.buffer.rope().line_to_char(11) + 2;
        editor.selection = rc_text::Selection::point(at);
    }
    h.state_mut().run_insight_action(rc_editor::Action::ShowIntentions, egui::Pos2::new(100.0, 100.0));
    h.run_steps(1);
    assert!(matches!(&h.state().dialog, Some(crate::ui::dialogs::Dialog::CodeActions { list, .. }) if list.len() == 1));
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    let text = h.state().docs.active_doc().unwrap().editor.buffer.rope().to_string();
    assert!(text.contains("\nanyhow = \"2.0.1\"\n"), "{text}");
    assert_eq!(ours(&h).iter().map(|d| d.0).collect::<Vec<_>>(), vec![12, 13]);

    // The online check off: no crates.io diagnostics.
    h.state_mut().settings.check_crates_io = false;
    h.run_steps(2);
    assert!(ours(&h).is_empty());
    assert!(h.state().insight.problems.get(&path).is_none_or(|l| l.iter().all(|d| d.code.as_deref() != Some(crate::dep_versions::DIAG_CODE))));
}

/// I-22: the open `Cargo.toml` is inspected offline, on a thread: a misspelled key and a
/// feature that names a dependency that is not optional get diagnostics (also in the Problems
/// window) whose Alt+Enter quick fixes correct them in the editor.
#[test]
fn cargo_toml_inspections_flag_mistakes_and_fix_them() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let (mut h, root) = dep_versions_harness(dir.path(), false);
    h.state_mut().settings.check_crates_io = false;
    {
        let editor = &mut h.state_mut().docs.active_doc_mut().unwrap().editor;
        let text = editor.buffer.rope().to_string();
        let at = text.find("edition = \"2021\"\n").unwrap() + "edition = \"2021\"\n".len();
        editor.replace(at, at, "descripton = \"A demo\"\n");
        let text = editor.buffer.rope().to_string();
        let at = text.find("extra = []").unwrap();
        editor.replace(at, at + "extra = []".len(), "extra = [\"dep:serde\"]");
    }
    let ours = |a: &CrabCraftApp| -> Vec<String> {
        let doc = a.docs.active_doc().unwrap();
        let mut out: Vec<String> =
            doc.editor.diagnostics.iter().filter(|d| d.code.as_deref() == Some(crate::manifest_inspect::DIAG_CODE)).map(|d| d.message.clone()).collect();
        out.sort();
        out
    };
    run_frames(&mut h, "the inspection", |a| !a.manifest_inspections.busy() && ours(a).len() == 2);
    let got = ours(h.state());
    assert_eq!(
        got,
        vec![
            "Feature `extra` includes `dep:serde`, but `serde` is not an optional dependency".to_string(),
            "Unknown key `descripton` in `[package]`; did you mean `description`?".to_string()
        ]
    );
    let path = root.join("Cargo.toml");
    assert_eq!(
        h.state().insight.problems.get(&path).map_or(0, |l| l.iter().filter(|d| d.code.as_deref() == Some(crate::manifest_inspect::DIAG_CODE)).count()),
        2,
        "listed in the Problems window"
    );

    // Alt+Enter on each line offers its fix; Enter makes it.
    for (line_text, title) in [("descripton", "Change to `description`"), ("serde = ", "Make `serde` optional")] {
        {
            let editor = &mut h.state_mut().docs.active_doc_mut().unwrap().editor;
            let text = editor.buffer.rope().to_string();
            let at = text.find(line_text).unwrap() + 1;
            editor.selection = rc_text::Selection::point(at);
        }
        h.state_mut().run_insight_action(rc_editor::Action::ShowIntentions, egui::Pos2::new(100.0, 100.0));
        h.run_steps(1);
        let titles: Vec<String> = match &h.state().dialog {
            Some(crate::ui::dialogs::Dialog::CodeActions { list, .. }) => list
                .iter()
                .map(|a| match a {
                    rc_lsp::lsp_types::CodeActionOrCommand::CodeAction(a) => a.title.clone(),
                    rc_lsp::lsp_types::CodeActionOrCommand::Command(c) => c.title.clone(),
                })
                .collect(),
            _ => Vec::new(),
        };
        assert_eq!(titles, vec![title.to_string()]);
        h.key_press(egui::Key::Enter);
        h.run_steps(3);
        run_frames(&mut h, "the next inspection", |a| !a.manifest_inspections.busy());
    }
    let text = h.state().docs.active_doc().unwrap().editor.buffer.rope().to_string();
    assert!(text.contains("\ndescription = \"A demo\"\n"), "{text}");
    assert!(text.contains("serde = { version = \"1\", features = [\"derive\"], optional = true }") || text.contains("optional = true"), "{text}");
    run_frames(&mut h, "the diagnostics to go", |a| ours(a).is_empty());
    assert!(!std::fs::read_to_string(&path).unwrap().contains("descripton"), "nothing was saved for the user");
}

#[test]
#[ignore = "Requires a GPU; writes target/snapshots/cargo-dep-versions-*.png"]
fn cargo_dep_versions_capture() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    for dark in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let (mut h, _) = dep_versions_harness(dir.path(), dark);
        h.run_steps(3);
        h.render().unwrap().save(out.join(format!("cargo-dep-versions-{}.png", if dark { "dark" } else { "light" }))).unwrap();
    }
}

#[test]
#[ignore = "Requires a GPU; opens CrabCraft's own instance.rs and writes target/snapshots/cfg-inactive.png"]
fn cfg_inactive_capture() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut app = test_app(&dir.path().join("home"), true);
    app.open_workspace_and_wait(&repo);
    let file = repo.join("crates/rc-app/src/instance.rs");
    app.open_file_and_wait(&file);
    let mut h = harness(app, vec2(1280.0, 800.0));
    run_frames(&mut h, "the cfg of the crate", |a| a.docs.active_doc().is_some_and(|d| d.editor.cfg.is_some()));
    let text = std::fs::read_to_string(&file).unwrap();
    let at = text.find("#[cfg(windows)]").expect("a Windows-only item");
    let line = text[..at].matches('\n').count();
    let doc = h.state_mut().docs.active_doc_mut().unwrap();
    doc.editor.goto(line + 12, 0);
    h.run_steps(6);
    assert!(!h.state_mut().docs.active_doc_mut().unwrap().editor.inactive().is_empty());
    h.render().unwrap().save(out.join("cfg-inactive.png")).unwrap();
}

/// C-03/C-11 on a real project: CrabCraft's own `crates/rc-cargo/Cargo.toml` against the
/// repository's `Cargo.lock` (members inherit from `[workspace.dependencies]`), with the live
/// crates.io check.
#[test]
#[ignore = "Needs the network and a GPU; writes target/snapshots/cargo-dep-versions-crabcraft.png"]
fn cargo_dep_versions_on_crabcraft_capture() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut app = test_app(&dir.path().join("home"), true);
    app.open_workspace_and_wait(&repo);
    let file = repo.join("crates/rc-cargo/Cargo.toml");
    app.open_file_and_wait(&file);
    let mut h = harness(app, vec2(1280.0, 600.0));
    let text = std::fs::read_to_string(&file).unwrap();
    let line_of = |key: &str| text.lines().position(|l| l.starts_with(key)).unwrap();
    let (anyhow, semver) = (line_of("anyhow"), line_of("semver"));
    run_frames(&mut h, "the locked versions", |a| {
        a.docs.active_doc().is_some_and(|d| d.editor.end_hints.contains_key(&anyhow) && d.editor.end_hints.contains_key(&semver))
    });
    // Give crates.io a few seconds.
    for _ in 0..100 {
        h.run_steps(1);
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let editor = &h.state().docs.active_doc().unwrap().editor;
    eprintln!("hints: {:?}", editor.end_hints);
    assert_eq!(editor.gutter_icons.get(&semver), Some(&rc_editor::GutterIcon::DocsRs));
    h.state_mut().open_docs_rs_at(semver);
    eprintln!("docs.rs: {:?}", h.state().opened_url);
    assert!(h.state().opened_url.as_deref().is_some_and(|u| u.starts_with("https://docs.rs/semver/1.")));
    h.render().unwrap().save(out.join("cargo-dep-versions-crabcraft.png")).unwrap();
}

/// I-24: rustc's explanation of an error code opens in a read-only Markdown tab, from the
/// Build window's diagnostic, the Problems window and the editor's hover of a real E0308.
#[test]
fn explains_a_rustc_error_code_from_the_build_problems_and_hover() {
    use egui_kittest::kittest::Queryable;
    use rc_kit::i18n::trf;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("demo");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n").unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {\n    let x: u32 = \"text\";\n    println!(\"{x}\");\n}\n").unwrap();
    let root = root.canonicalize().unwrap();
    let main = root.join("src/main.rs");
    let mut app = test_app(home.path(), false);
    app.settings.trusted_projects.push(root.clone());
    app.open_workspace_and_wait(&root);
    app.open_file_and_wait(&main);
    let mut h = harness(app, vec2(1280.0, 800.0));
    run_frames(&mut h, "the Cargo project", |a| a.runner.cargo_workspace.is_some());
    let target = h.state().workspace.as_ref().unwrap().ws.key_path().to_path_buf();
    let outs = h.state_mut().runner.build(&target, rc_dotnet::BuildAction::Build);
    crate::ui::run_windows::handle_outputs(h.state_mut(), outs);
    run_frames(&mut h, "the failed build", |a| a.runner.build.as_ref().is_some_and(|b| b.result.is_some() && b.diagnostics.iter().any(|d| d.code == "E0308")));
    let explain = trf("Explain {code}", &[("code", &"E0308")]);
    let explained = |a: &CrabCraftApp| a.docs.active_doc().is_some_and(|d| d.path.ends_with("E0308.md"));

    // The Build window: right-click the diagnostic.
    h.state_mut().dock.show(crate::toolwindow::ToolWindow::Build);
    h.run_steps(3);
    h.get_by_label_contains("E0308 mismatched types").click_secondary();
    h.run_steps(2);
    h.get_by_label(&explain).click();
    run_frames(&mut h, "the explanation from the Build window", explained);
    {
        let d = h.state().docs.active_doc().unwrap();
        assert!(d.editor.read_only && d.generated);
        assert!(!d.path.exists(), "generated explanation stays in memory");
        assert_eq!(d.md_view, Some(crate::docs::MdView::Preview));
        let text = d.editor.buffer.rope().to_string();
        assert!(text.starts_with("# E0308") && text.contains("Expected type did not match the received type"), "{text}");
    }
    assert!(h.state().explanations.contains_key("E0308"), "the answer is kept");
    let close = |h: &mut Harness<'static, CrabCraftApp>| {
        let i = h.state().docs.active.unwrap();
        h.state_mut().close_tab(i);
        h.run_steps(2);
    };
    close(&mut h);

    // The Problems window, with the same error from the build.
    h.state_mut().dock.show(crate::toolwindow::ToolWindow::Problems);
    run_frames(&mut h, "the problem", |a| a.insight.problems.get(&main).is_some_and(|p| p.iter().any(|d| d.code.as_deref() == Some("E0308"))));
    h.get_by_label("mismatched types").click_secondary();
    h.run_steps(2);
    h.get_by_label(&explain).click();
    run_frames(&mut h, "the explanation from the Problems window", explained);
    close(&mut h);

    // The editor's hover of the problem offers the same link.
    h.state_mut().open_file_and_wait(&main);
    {
        let d = h.state_mut().docs.active_doc_mut().unwrap();
        let pos = d.editor.buffer.rope().to_string().find("\"text\"").unwrap();
        d.editor.popups.hover = Some(rc_editor::popup::Hover {
            pos,
            text: String::new(),
            problem: Some((rc_editor::Severity::Error, "mismatched types".into())),
            explain: Some("E0308".into()),
            screen: egui::pos2(400.0, 300.0),
        });
    }
    h.run_steps(2);
    h.get_by_label(&explain).click();
    run_frames(&mut h, "the explanation from the hover", explained);
    assert!(h.state().docs.list.iter().find(|d| d.path == main).is_some_and(|d| d.editor.popups.hover.is_none()), "the hover closes");

    // A code rustc does not know is reported, and nothing else reaches it.
    h.state_mut().explain_error("E9999");
    run_frames(&mut h, "no explanation", |a| a.explain_load.is_none());
    assert!(h.state().notices.iter().any(|n| n.text == trf("rustc has no explanation for {code}", &[("code", &"E9999")])));
    h.state_mut().explain_error("clippy::len_zero");
    assert!(h.state().explain_load.is_none());
}

#[test]
#[ignore = "Requires a GPU; writes target/snapshots/explain-error-*.png"]
fn explain_error_capture() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    let home = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("demo");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n").unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {\n    let x: u32 = \"text\";\n    println!(\"{x}\");\n}\n").unwrap();
    let root = root.canonicalize().unwrap();
    let main = root.join("src/main.rs");
    let mut app = test_app(home.path(), true);
    app.open_workspace_and_wait(&root);
    app.open_file_and_wait(&main);
    let mut h = harness(app, vec2(1280.0, 800.0));
    {
        let d = h.state_mut().docs.active_doc_mut().unwrap();
        let pos = d.editor.buffer.rope().to_string().find("\"text\"").unwrap();
        d.editor.popups.hover = Some(rc_editor::popup::Hover {
            pos,
            text: String::new(),
            problem: Some((rc_editor::Severity::Error, "mismatched types: expected `u32`, found `&str`".into())),
            explain: Some("E0308".into()),
            screen: egui::pos2(420.0, 160.0),
        });
    }
    h.run_steps(4);
    h.render().unwrap().save(out.join("explain-error-hover.png")).unwrap();
    h.state_mut().docs.active_doc_mut().unwrap().editor.popups.close_all();
    h.state_mut().explain_error("E0308");
    run_frames(&mut h, "the explanation", |a| a.docs.active_doc().is_some_and(|d| d.path.ends_with("E0308.md")));
    h.run_steps(6);
    h.render().unwrap().save(out.join("explain-error-tab.png")).unwrap();
}

/// Opens this repository's `app.rs` with the caret deep in a method of `impl CrabCraftApp`.
fn context_info_harness(home: &Path, dark: bool) -> (Harness<'static, CrabCraftApp>, usize) {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/app.rs").canonicalize().unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    let at = text.find("Action::ToggleHexValues => self.toggle_hex_values()").expect("the line the test puts the caret on");
    let line = text[..at].matches('\n').count();
    let mut app = test_app(home, dark);
    app.open_file_and_wait(&file);
    let mut h = harness(app, vec2(1280.0, 800.0));
    let col = text[..at].rsplit('\n').next().map_or(0, |l| l.chars().count());
    {
        let d = h.state_mut().docs.active_doc_mut().unwrap();
        d.editor.goto(line, col);
        d.editor.request_scroll_to_caret();
    }
    run_frames(&mut h, "the tree of app.rs", |a| a.docs.active_doc().is_some_and(|d| d.editor.syntax.tree().is_some()));
    (h, line)
}

/// Alt+Q (⌃⇧Q on macOS) lists the impl and method headers scrolled out of view, outermost
/// first; a click on one goes to its line, and Escape closes the popup.
#[test]
fn alt_q_shows_the_enclosing_declarations_and_goes_to_them() {
    use egui_kittest::kittest::Queryable;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let (mut h, caret_line) = context_info_harness(dir.path(), false);
    h.state_mut().keymap = rc_editor::Keymap::new(rc_editor::KeymapKind::Rider);
    let chord = h.state().keymap.chord(rc_editor::Action::ShowContextInfo).unwrap();
    assert_eq!((chord.key, chord.mods.alt), (egui::Key::Q, true), "Alt+Q");
    h.key_press_modifiers(chord.mods, chord.key);
    h.run_steps(3);
    let rows: Vec<(usize, String)> = {
        let d = h.state().docs.active_doc().unwrap();
        let ctx = d.editor.popups.context.as_ref().expect("the popup is open");
        ctx.rows.iter().map(|r| (r.line, r.text.clone())).collect()
    };
    let texts: Vec<&str> = rows.iter().map(|(_, t)| t.as_str()).collect();
    assert_eq!(texts, ["impl CrabCraftApp", "pub fn run_ide_action(&mut self, ctx: &Context, action: Action)"]);
    assert!(rows.iter().all(|(l, _)| *l < caret_line));
    // The rows are coloured from the source: `impl` and `pub` are keywords.
    let d = h.state().docs.active_doc().unwrap();
    assert!(d.editor.popups.context.as_ref().unwrap().rows.iter().all(|r| r.roles.first() == Some(&rc_syntax::Hl::Keyword)));

    // Escape closes it; moving the caret closes it too.
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(h.state().docs.active_doc().unwrap().editor.popups.context.is_none());
    h.key_press_modifiers(chord.mods, chord.key);
    h.run_steps(2);
    assert!(h.state().docs.active_doc().unwrap().editor.popups.context.is_some());
    h.key_press(egui::Key::ArrowRight);
    h.run_steps(2);
    assert!(h.state().docs.active_doc().unwrap().editor.popups.context.is_none(), "moving the caret closes it");

    // A click on the method's header goes there.
    h.key_press_modifiers(chord.mods, chord.key);
    h.run_steps(3);
    h.get_by_label(&rows[1].1).click();
    h.run_steps(3);
    let d = h.state().docs.active_doc().unwrap();
    assert_eq!(d.editor.caret_line_col().0, rows[1].0, "the caret is on the method's line");
    assert!(d.editor.popups.context.is_none());
}

#[test]
#[ignore = "Requires a GPU; writes target/snapshots/context-info-*.png"]
fn context_info_capture() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    for dark in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let (mut h, _) = context_info_harness(dir.path(), dark);
        let chord = h.state().keymap.chord(rc_editor::Action::ShowContextInfo).unwrap();
        h.key_press_modifiers(chord.mods, chord.key);
        h.run_steps(4);
        h.render().unwrap().save(out.join(format!("context-info-{}.png", if dark { "dark" } else { "light" }))).unwrap();
    }
}

/// A crate with `#[cfg(windows)]` and `#[cfg(unix)]` code, `src/lib.rs` open, once the target
/// list is read.
fn targets_harness(home: &Path, dark: bool) -> (Harness<'static, CrabCraftApp>, PathBuf) {
    let root = home.join("ws");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n").unwrap();
    std::fs::write(root.join("src/lib.rs"), "#[cfg(windows)]\npub fn on_windows() {}\n\n#[cfg(unix)]\npub fn on_unix() {}\n").unwrap();
    let root = root.canonicalize().unwrap();
    let mut app = test_app(&home.join("home"), dark);
    app.open_workspace_and_wait(&root);
    app.trust_project_tools_for_test();
    app.open_file_and_wait(&root.join("src/lib.rs"));
    let mut h = harness(app, vec2(1280.0, 800.0));
    run_frames(&mut h, "the target list", |a| !a.triple_choices.is_empty() && a.runner.cargo_workspace.is_some());
    h.run_steps(3);
    (h, root)
}

/// The status bar shows the host triple; its list picks another target, which is saved.
#[test]
fn the_status_bar_picks_the_analysis_target() {
    use egui_kittest::kittest::Queryable;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let (mut h, root) = targets_harness(dir.path(), false);
    let host = h.state().host_triple.clone().unwrap();
    h.get_by_label(host.as_str()).click();
    h.run_steps(3);
    // rustup lists every target it knows: the list is long enough to have a filter.
    assert!(h.state().triple_choices.len() > 8);
    h.state_mut().triple_filter = "windows-msvc".into();
    h.run_steps(2);
    h.get_by_label("x86_64-pc-windows-msvc").click();
    h.run_steps(3);
    assert_eq!(h.state().cargo_target.as_deref(), Some("x86_64-pc-windows-msvc"));
    assert!(h.state().triple_filter.is_empty());
    h.state_mut().save_session();
    h.state_mut().save_all_and_wait();
    assert_eq!(rc_core::Session::load(&root).cargo_target, "x86_64-pc-windows-msvc");
    assert!(!root.join("target").exists(), "nothing was built");
}

#[test]
#[ignore = "Requires a GPU; writes target/snapshots/target-picker-*.png"]
fn target_picker_capture() {
    use egui_kittest::kittest::Queryable;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    for dark in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let (mut h, _) = targets_harness(dir.path(), dark);
        let host = h.state().host_triple.clone().unwrap();
        h.get_by_label(host.as_str()).click();
        h.run_steps(4);
        h.render().unwrap().save(out.join(format!("target-picker-{}.png", if dark { "dark" } else { "light" }))).unwrap();
        // Windows picked: the unix code is faded.
        h.state_mut().set_cargo_target(Some("x86_64-pc-windows-msvc".into()));
        egui::Popup::close_all(&h.ctx);
        run_frames(&mut h, "the cfg of Windows", |a| {
            a.docs.active_doc().is_some_and(|d| d.editor.cfg.as_ref().is_some_and(|c| c.eval("windows") == Some(true)))
        });
        h.run_steps(6);
        h.render().unwrap().save(out.join(format!("target-picker-windows-{}.png", if dark { "dark" } else { "light" }))).unwrap();
    }
}

/// A long real file of this repository, open with a problem on `error_line`.
fn lens_harness(home: &Path, dark: bool, error_line: usize) -> (Harness<'static, CrabCraftApp>, PathBuf) {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../rc-editor/src/view.rs").canonicalize().unwrap();
    let mut app = test_app(home, dark);
    app.open_file_and_wait(&file);
    let mut h = harness(app, vec2(1280.0, 800.0));
    h.run_steps(3);
    let doc = h.state_mut().docs.active_doc_mut().unwrap();
    assert!(doc.editor.buffer.len_lines() > 1000, "a long file");
    let start = doc.editor.buffer.rope().line_to_char(error_line) + 4;
    doc.editor.diagnostics.push(rc_editor::Diagnostic {
        start,
        end: start + 6,
        severity: rc_editor::Severity::Error,
        message: "lens test: mismatched types".into(),
        code: None,
        unnecessary: false,
        deprecated: false,
    });
    h.run_steps(2);
    (h, file)
}

/// Height of the scrollbar where the stripe mark of `line` is (no folds).
fn track_y_of_line(h: &Harness<'static, CrabCraftApp>, line: usize) -> egui::Pos2 {
    let ed = &h.state().docs.active_doc().unwrap().editor;
    let track = ed.scrollbar_track;
    let rows = ed.buffer.len_lines() as f32;
    let y = track.top() + line as f32 / rows.max(track.height() / ed.row_height()) * track.height();
    egui::pos2(track.center().x, y + 0.2)
}

fn hover_lens(h: &mut Harness<'static, CrabCraftApp>, at: egui::Pos2) -> Option<rc_editor::lens::ScrollLens> {
    h.event(egui::Event::PointerMoved(at));
    h.run_steps(2);
    h.state().docs.active_doc().unwrap().editor.scroll_lens.clone()
}

/// Hovering the scrollbar over a real Rust file previews the code at that height; a problem's
/// stripe mark shows its message; leaving the bar or turning the setting off hides the lens.
#[test]
fn the_scrollbar_lens_previews_the_code_under_the_pointer() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let (mut h, _) = lens_harness(dir.path(), false, 300);
    let target = {
        let ed = &h.state().docs.active_doc().unwrap().editor;
        (0..ed.buffer.len_lines()).find(|&l| ed.buffer.line_text(l).starts_with("fn paint_scrollbars(")).expect("the function is in view.rs")
    };
    let at = track_y_of_line(&h, target);
    let lens = hover_lens(&mut h, at).expect("the lens is shown");
    assert_eq!(lens.line, target);
    assert_eq!(lens.lines, (target - 4..=target + 4).collect::<Vec<_>>());
    assert_eq!(lens.message, None);
    let track = h.state().docs.active_doc().unwrap().editor.scrollbar_track;
    assert!(lens.rect.right() <= track.left(), "left of the scrollbar");
    // Hovering scrolls nothing.
    assert_eq!(h.state().docs.active_doc().unwrap().editor.scroll_offset().y, 0.0);

    // The error's stripe mark: its line in the middle and its message.
    let at = track_y_of_line(&h, 300) + egui::vec2(0.0, 0.8);
    let lens = hover_lens(&mut h, at).expect("the lens is shown");
    assert_eq!(lens.line, 300);
    assert_eq!(lens.message, Some((rc_editor::Severity::Error, "lens test: mismatched types".to_string())));

    // Away from the bar: gone.
    assert_eq!(hover_lens(&mut h, egui::pos2(500.0, 400.0)), None);

    // Turned off in the settings.
    let mut settings = h.state().settings.clone();
    settings.scrollbar_lens = false;
    let ctx = h.ctx.clone();
    h.state_mut().apply_settings(&ctx, settings);
    let at = track_y_of_line(&h, target);
    assert_eq!(hover_lens(&mut h, at), None);
}

#[test]
#[ignore = "Requires a GPU; writes target/snapshots/scrollbar-lens-*.png"]
fn scrollbar_lens_capture() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    for dark in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let (mut h, _) = lens_harness(dir.path(), dark, 900);
        let at = track_y_of_line(&h, 900) + egui::vec2(0.0, 0.8);
        hover_lens(&mut h, at).expect("the lens is shown");
        h.run_steps(2);
        h.render().unwrap().save(out.join(format!("scrollbar-lens-{}.png", if dark { "dark" } else { "light" }))).unwrap();
    }
}

/// Find Usages on a `[features]` key of `Cargo.toml` (N-13): the `cfg` uses in the Find window.
#[test]
#[ignore = "Requires a GPU; writes target/snapshots/feature-uses-*.png"]
fn feature_uses_capture() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    for dark in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("ws");
        crate::rust_tests::feature_crate(&root);
        let root = root.canonicalize().unwrap();
        let mut app = test_app(&dir.path().join("home"), dark);
        app.open_workspace_and_wait(&root);
        app.open_file_and_wait(&root.join("Cargo.toml"));
        app.docs.active_doc_mut().unwrap().editor.goto(8, 2);
        app.run_insight_action(rc_editor::Action::FindUsages, egui::Pos2::ZERO);
        let mut h = harness(app, vec2(1280.0, 800.0));
        run_frames(&mut h, "the uses of extra", |a| a.find_results.is_some());
        h.run_steps(4);
        h.render().unwrap().save(out.join(format!("feature-uses-{}.png", if dark { "dark" } else { "light" }))).unwrap();
    }
}

/// A one-request stand-in for `play.rust-lang.org/meta/gist`: returns the request it read.
fn fake_playground(listener: std::net::TcpListener, reply: &'static str) -> std::thread::JoinHandle<String> {
    use std::io::{Read, Write};
    std::thread::spawn(move || {
        listener.set_nonblocking(false).unwrap();
        let (mut s, _) = listener.accept().unwrap();
        s.set_nonblocking(false).unwrap();
        s.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
        let (mut req, mut buf) = (Vec::new(), [0u8; 4096]);
        loop {
            let n = s.read(&mut buf).unwrap_or(0);
            if n == 0 {
                break;
            }
            req.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&req);
            if let Some(end) = text.find("\r\n\r\n") {
                let len = text[..end]
                    .lines()
                    .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap()))
                    .unwrap_or(0);
                if req.len() >= end + 4 + len {
                    break;
                }
            }
        }
        let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len());
        String::from_utf8_lossy(&req).into_owned()
    })
}

/// A Rust crate (edition 2021) with `src/main.rs` open, its workspace loaded.
fn playground_harness(home: &Path, dir: &Path, dark: bool) -> Harness<'static, CrabCraftApp> {
    let root = dir.join("demo");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n").unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {\n    let x = 1;\n    println!(\"{x}\");\n}\n\nfn unused() {}\n").unwrap();
    let root = root.canonicalize().unwrap();
    let mut app = test_app(home, dark);
    app.open_workspace_and_wait(&root);
    app.trust_project_tools_for_test();
    app.open_file_and_wait(&root.join("src/main.rs"));
    let mut h = harness(app, vec2(1280.0, 800.0));
    run_frames(&mut h, "the Cargo workspace", |a| a.runner.cargo_workspace.is_some());
    h
}

/// N-18: the selection goes to the Playground only after the confirmation; the link is copied
/// and offered in the status bar. A local server stands in for play.rust-lang.org.
#[test]
fn share_in_playground_confirms_then_uploads_and_copies_the_link() {
    use crate::ui::dialogs::Dialog;
    use egui_kittest::kittest::Queryable;
    use rc_kit::i18n::{tr, trf};
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut h = playground_harness(home.path(), dir.path(), true);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    h.state_mut().playground.endpoint = format!("http://{}/meta/gist", listener.local_addr().unwrap());
    let selected = "fn main() {\n    let x = 1;\n    println!(\"{x}\");\n}";
    let select = |h: &mut Harness<'static, CrabCraftApp>| {
        h.state_mut().docs.active_doc_mut().unwrap().editor.set_selection(rc_text::Selection::single(rc_text::Range::new(0, selected.chars().count())));
        h.run_steps(2);
        h.state().caret_screen.unwrap() + vec2(-20.0, -6.0)
    };

    // From the editor's context menu: the dialog says what is sent, and where.
    let at = select(&mut h);
    right_click(&mut h, at);
    h.get_by_label(tr("Share in Playground…")).click();
    h.run_steps(4);
    match &h.state().dialog {
        Some(Dialog::SharePlayground(s)) => {
            assert_eq!(s.code, selected);
            assert_eq!(s.lines, 4);
            assert_eq!(s.preview, selected);
            assert_eq!((s.edition.as_str(), s.channel), ("2021", rc_cargo::playground::Channel::Stable), "the crate's edition is the default");
        }
        _ => panic!("no confirmation dialog"),
    }
    h.get_by_label(&trf("{n} lines will be sent", &[("n", &4)]));
    h.get_by_label(tr("The code is uploaded to play.rust-lang.org, which publishes it as a public GitHub gist. Anyone with the link can read it."));

    // Cancel: nothing reaches the server.
    h.get_by_label(tr("Cancel")).click();
    h.run_steps(4);
    assert!(h.state().dialog.is_none());
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock), "something was sent after Cancel");
    assert!(!h.state().playground.running());

    // From Find Action's list, on nightly: the upload, the link, the clipboard.
    assert!(crate::search::action_items().iter().any(|(_, a)| *a == rc_editor::Action::ShareInPlayground));
    let server = fake_playground(listener, r#"{"id":"0123456789abcdef","url":"https://gist.github.com/0123456789abcdef","code":"..."}"#);
    select(&mut h);
    h.state_mut().pending.push(rc_editor::Action::ShareInPlayground);
    h.run_steps(4);
    if let Some(Dialog::SharePlayground(s)) = &mut h.state_mut().dialog {
        s.channel = rc_cargo::playground::Channel::Nightly;
    }
    h.run_steps(1);
    h.get_by_label(tr("Upload and Copy Link")).click();
    let expected = "https://play.rust-lang.org/?version=nightly&mode=debug&edition=2021&gist=0123456789abcdef";
    let (mut copied, start) = (Vec::new(), std::time::Instant::now());
    while h.state().playground.last_url.is_none() || copied.is_empty() {
        assert!(start.elapsed() < std::time::Duration::from_secs(30), "no link from the fake server");
        h.run_steps(1);
        for c in &h.output().platform_output.commands {
            if let egui::OutputCommand::CopyText(t) = c {
                copied.push(t.clone());
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(copied, vec![expected.to_string()]);
    assert_eq!(h.state().playground.last_url.as_deref(), Some(expected));
    let req = server.join().unwrap();
    assert!(req.starts_with("POST /meta/gist "), "{req}");
    assert!(req.to_ascii_lowercase().contains("user-agent: crabcraft (https://github.com/rafaelmendes1407/crabcraft)"), "{req}");
    let body: serde_json::Value = serde_json::from_str(&req[req.find("\r\n\r\n").unwrap() + 4..]).unwrap();
    assert_eq!(body, serde_json::json!({ "code": selected }));
    assert!(h.state().notices.last().is_some_and(|n| n.url.as_deref() == Some(expected) && !n.error));

    // The status bar's Open in Browser opens the link.
    h.run_steps(2);
    h.get_by_label(tr("Open in Browser")).click();
    h.run_steps(2);
    assert_eq!(h.state().opened_url.as_deref(), Some(expected));
}

#[test]
#[ignore = "Requires a GPU; writes target/snapshots/share-playground-*.png"]
fn share_in_playground_capture() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    for dark in [true, false] {
        let home = tempfile::tempdir().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut h = playground_harness(home.path(), dir.path(), dark);
        h.state_mut().pending.push(rc_editor::Action::ShareInPlayground);
        h.run_steps(6);
        h.render().unwrap().save(out.join(format!("share-playground-{}.png", if dark { "dark" } else { "light" }))).unwrap();
    }
}

/// A Cargo project in a Git repository whose `user.name` is Ada Lovelace, with `src/old.rs`
/// starting with a header from 2020.
fn copyright_harness(home: &Path, dark: bool) -> (Harness<'static, CrabCraftApp>, PathBuf) {
    let root = home.join("ws");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"ws\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n").unwrap();
    std::fs::write(root.join("src/lib.rs"), "#![allow(dead_code)]\n\nmod old;\n").unwrap();
    std::fs::write(root.join("src/old.rs"), "// Copyright (c) 2020 Ada Lovelace\n// Part of ws.\n\npub fn old() {}\n").unwrap();
    let root = root.canonicalize().unwrap();
    for args in [&["init", "-q"][..], &["config", "user.name", "Ada Lovelace"]] {
        assert!(std::process::Command::new("git").args(args).current_dir(&root).output().unwrap().status.success());
    }
    let mut app = test_app(&home.join("home"), dark);
    app.open_workspace_and_wait(&root);
    app.trust_project_tools_for_test();
    app.copyright.open(&root, app.copyright.profile.clone(), app.waker.clone());
    let mut h = harness(app, vec2(1280.0, 800.0));
    run_frames(&mut h, "the author from Git", |a| !a.copyright.loading());
    (h, root)
}

/// Settings › Copyright sets the project's profile; a new Rust file starts with its header,
/// and Update Copyright brings the years of an old header up to this year.
#[test]
fn copyright_headers_on_new_and_old_files() {
    use egui_kittest::kittest::Queryable;
    use rc_kit::i18n::tr;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let (mut h, root) = copyright_harness(dir.path(), false);
    let year = crate::copyright::current_year();
    assert_eq!(h.state().copyright.author.as_deref(), Some("Ada Lovelace"));
    h.state_mut().open_settings(crate::ui::dialogs::SettingsPage::Copyright);
    h.run_steps(3);
    h.state_mut().copyright.draft.as_mut().unwrap().spdx = "MIT".into();
    h.get_by_label(tr("Add the header to new files")).click();
    h.run_steps(2);
    h.get_by_label(tr("OK")).click();
    h.run_steps(3);
    assert!(h.state().dialog.is_none());
    assert!(h.state().copyright.profile.on_new_files);
    h.state_mut().save_session();
    h.state_mut().save_all_and_wait();
    assert_eq!(rc_core::Session::load(&root).copyright.spdx, "MIT");

    // New › Rust File.
    h.state_mut().dialog = Some(crate::ui::dialogs::Dialog::NewFile {
        dir: root.join("src"),
        name: "net".into(),
        kind: crate::item_templates::ItemKind::RustFile,
        attach: false,
        public: false,
        error: None,
    });
    h.run_steps(2);
    h.get_by_label(tr("Create")).click();
    h.run_steps(3);
    let text = std::fs::read_to_string(root.join("src/net.rs")).unwrap();
    assert_eq!(text, format!("// Copyright (c) {year} Ada Lovelace\n// Part of ws.\n// SPDX-License-Identifier: MIT\n\n"));
    assert_eq!(h.state().docs.active_doc().map(|d| d.editor.caret()), Some(text.chars().count()));

    // Update Copyright on the old file: 2020 becomes 2020-<this year>, once.
    h.state_mut().open_file_and_wait(&root.join("src/old.rs"));
    h.run_steps(2);
    h.state_mut().pending.push(rc_editor::Action::UpdateCopyright);
    h.run_steps(3);
    let doc = |h: &Harness<'static, CrabCraftApp>| h.state().docs.active_doc().unwrap().editor.buffer.rope().to_string();
    assert_eq!(doc(&h), format!("// Copyright (c) 2020-{year} Ada Lovelace\n// Part of ws.\n\npub fn old() {{}}\n"));
    h.state_mut().pending.push(rc_editor::Action::UpdateCopyright);
    h.run_steps(3);
    assert_eq!(doc(&h), format!("// Copyright (c) 2020-{year} Ada Lovelace\n// Part of ws.\n\npub fn old() {{}}\n"));
    // A file without a header gets one at the top, before its inner attributes.
    h.state_mut().open_file_and_wait(&root.join("src/lib.rs"));
    h.run_steps(2);
    h.state_mut().pending.push(rc_editor::Action::UpdateCopyright);
    h.run_steps(3);
    assert!(doc(&h).starts_with(&format!("// Copyright (c) {year} Ada Lovelace\n// Part of ws.\n// SPDX-License-Identifier: MIT\n\n#![allow(dead_code)]\n")));
    assert!(!root.join("target").exists(), "nothing was built");
}

#[test]
#[ignore = "Requires a GPU; writes target/snapshots/copyright-*.png"]
fn copyright_capture() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    for dark in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let (mut h, _) = copyright_harness(dir.path(), dark);
        h.state_mut().open_settings(crate::ui::dialogs::SettingsPage::Copyright);
        h.run_steps(3);
        if let Some(d) = h.state_mut().copyright.draft.as_mut() {
            d.spdx = "MIT OR Apache-2.0".into();
            d.on_new_files = true;
        }
        h.run_steps(4);
        h.render().unwrap().save(out.join(format!("copyright-{}.png", if dark { "dark" } else { "light" }))).unwrap();
    }
}

#[test]
#[ignore = "Requires a GPU; writes target/snapshots/refactor-conflicts.png"]
fn refactor_conflicts_capture() {
    use crate::refactor_conflicts::{Conflict, Review};
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let (mut h, root) = two_crates_harness(dir.path(), true);
    let file = root.join("beta/src/lib.rs");
    let conflicts = vec![
        Conflict { path: Some(file.clone()), line: 3, col: 7, message: "'NEEDLE_NAME' is already used here".into() },
        Conflict { path: Some(root.join("target/debug/build/out.rs")), line: 0, col: 3, message: "This file is generated (in the build output folder)".into() },
        Conflict { path: None, line: 0, col: 0, message: "'fn' is a keyword".into() },
    ];
    let review = Review { name: "NEEDLE_NAME".into(), conflicts, edit: Some(Default::default()), preview: None };
    h.state_mut().dialog = Some(crate::ui::dialogs::Dialog::RefactorConflicts(Box::new(review)));
    h.run_steps(4);
    assert!(matches!(h.state().dialog, Some(crate::ui::dialogs::Dialog::RefactorConflicts(_))));
    h.render().unwrap().save(out.join("refactor-conflicts.png")).unwrap();
}

/// A Cargo project in git with a test, a README and an uncommitted change to `src/lib.rs`;
/// every file says "needle".
fn scopes_harness(home: &Path, dark: bool) -> (Harness<'static, CrabCraftApp>, PathBuf) {
    let root = home.join("ws");
    for (file, text) in [
        ("Cargo.toml", "[package]\nname = \"ws\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n"),
        ("src/main.rs", "// needle\nfn main() {\n    ws::hello();\n}\n"),
        ("src/lib.rs", "// needle\npub fn hello() {}\n"),
        ("src/net.rs", "// needle\npub fn net() {}\n"),
        ("tests/it.rs", "// needle\n#[test]\nfn it() {}\n"),
        ("README.md", "needle\n"),
    ] {
        std::fs::create_dir_all(root.join(file).parent().unwrap()).unwrap();
        std::fs::write(root.join(file), text).unwrap();
    }
    let root = root.canonicalize().unwrap();
    let git = |args: &[&str]| assert!(std::process::Command::new("git").args(args).current_dir(&root).output().unwrap().status.success(), "git {args:?}");
    git(&["init", "-q"]);
    git(&["config", "user.name", "Ada Lovelace"]);
    git(&["config", "user.email", "ada@example.com"]);
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "Start"]);
    std::fs::write(root.join("src/lib.rs"), "// needle\npub fn hello() {}\npub fn changed() {}\n").unwrap();
    let mut app = test_app(&home.join("home"), dark);
    app.open_workspace_and_wait(&root);
    app.trust_project_tools_for_test();
    let mut h = harness(app, vec2(1280.0, 800.0));
    run_frames(&mut h, "git's status", |a| a.vcs.status.is_some());
    (h, root)
}

/// The files Find in Files found, sorted.
fn found_in_scope(h: &mut Harness<'static, CrabCraftApp>) -> Vec<String> {
    run_frames(h, "Find in Files", |a| a.find_in_files.as_ref().is_some_and(|f| f.summary.is_some() || f.error.is_some()));
    let mut v: Vec<String> = h.state().find_in_files.as_ref().map(|f| f.results.iter().map(|r| r.rel.clone()).collect()).unwrap_or_default();
    v.sort();
    v
}

/// Settings › Scopes defines a scope and file colours (saved with the session); the colours
/// follow the scopes and git's changes; Find in Files' scope picker narrows the search, and a
/// scope whose pattern does not parse is an error, not a crash.
#[test]
fn scopes_narrow_find_in_files_and_color_files() {
    use crate::scopes::{CHANGED, TESTS};
    use egui_kittest::kittest::Queryable;
    use rc_core::session::{FileColor, NamedScope};
    use rc_kit::i18n::tr;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let (mut h, root) = scopes_harness(dir.path(), false);

    // Settings › Scopes: a new scope, renamed and given a pattern, and three colours.
    h.state_mut().open_settings(crate::ui::dialogs::SettingsPage::Scopes);
    h.run_steps(3);
    h.get_by_label(tr("Add Scope")).click();
    h.run_steps(2);
    h.get_by_label(tr("Add Color")).click();
    h.run_steps(2);
    {
        let draft = h.state_mut().explorer.scopes.draft.as_mut().unwrap();
        assert_eq!(draft.user.len(), 1);
        assert_eq!(draft.colors.len(), 1);
        draft.user[0] = NamedScope { name: "Sources".into(), pattern: "src/**/*.rs, !src/net.rs".into() };
        draft.colors = vec![
            FileColor { scope: CHANGED.into(), color: "#e08e3c".into() },
            FileColor { scope: TESTS.into(), color: "#5fb865".into() },
            FileColor { scope: "Sources".into(), color: "#4a8fe0".into() },
        ];
    }
    h.run_steps(2);
    h.get_by_label(tr("OK")).click();
    h.run_steps(3);
    assert!(h.state().dialog.is_none());
    assert_eq!(h.state().explorer.scopes.user, [NamedScope { name: "Sources".into(), pattern: "src/**/*.rs, !src/net.rs".into() }]);
    h.state_mut().save_session();
    h.state_mut().save_all_and_wait();
    let saved = rc_core::Session::load(&root);
    assert_eq!(saved.scopes.len(), 1);
    assert_eq!(saved.file_colors.len(), 3);

    // Colours: the first scope that holds the file.
    let rgb = |r, g, b| Some(egui::Color32::from_rgb(r, g, b));
    let mut color = |rel: &str| h.state_mut().explorer.scopes.color_of(&root.join(rel));
    assert_eq!(color("src/lib.rs"), rgb(0xe0, 0x8e, 0x3c), "the changed file");
    assert_eq!(color("tests/it.rs"), rgb(0x5f, 0xb8, 0x65));
    assert_eq!(color("src/main.rs"), rgb(0x4a, 0x8f, 0xe0));
    assert_eq!(color("src/net.rs"), None, "left out by the pattern");
    assert_eq!(color("README.md"), None);

    // Find in Files: every file, then each scope picked in the selector.
    h.state_mut().pending.push(rc_editor::Action::FindInFiles);
    h.run_steps(3);
    h.state_mut().find_in_files.as_mut().unwrap().query = "needle".into();
    h.state_mut().find_in_files.as_mut().unwrap().search();
    assert_eq!(found_in_scope(&mut h), ["README.md", "src/lib.rs", "src/main.rs", "src/net.rs", "tests/it.rs"]);
    let pick = |h: &mut Harness<'static, CrabCraftApp>, from: &str, to: &str| {
        // The selector shows the scope as its value.
        h.get_by(|n| n.value().as_deref() == Some(from)).click();
        h.run_steps(2);
        // The popup's entry (another window may show the same word).
        h.get_all_by_label(to).last().unwrap().click();
        h.run_steps(2);
        found_in_scope(h)
    };
    assert_eq!(pick(&mut h, tr("Project Files"), "Sources"), ["src/lib.rs", "src/main.rs"]);
    assert_eq!(pick(&mut h, "Sources", tr("Tests")), ["tests/it.rs"]);
    assert_eq!(pick(&mut h, tr("Tests"), tr("Changed Files")), ["src/lib.rs"]);

    // A pattern that does not parse: an error in the window, no results.
    h.state_mut().set_scopes(crate::scopes::Draft { user: vec![NamedScope { name: "Broken".into(), pattern: "src/[".into() }], colors: Vec::new() });
    h.run_steps(2);
    assert_eq!(pick(&mut h, tr("Changed Files"), "Broken"), Vec::<String>::new());
    let error = h.state().find_in_files.as_ref().and_then(|f| f.error.clone()).unwrap_or_default();
    assert!(error.contains("src/["), "{error}");
    assert!(h.query_by_label(&error).is_some(), "the error is shown");

    // Reopened: the scopes and colours come back from the (untrusted) session file.
    drop(h);
    std::fs::write(
        rc_core::Session::file_for(&root),
        r##"{ "scopes": [{ "name": "Sources", "pattern": "src/**" }, { "name": "builtin:x", "pattern": "*" }],
            "file_colors": [{ "scope": "Sources", "color": "#123456" }, { "scope": "Nope", "color": "#ffffff" }, { "scope": "Sources", "color": "red" }] }"##,
    )
    .unwrap();
    let mut app = test_app(&dir.path().join("home"), false);
    app.open_workspace_and_wait(&root);
    assert_eq!(app.explorer.scopes.user, [NamedScope { name: "Sources".into(), pattern: "src/**".into() }]);
    assert_eq!(app.explorer.scopes.colors, [FileColor { scope: "Sources".into(), color: "#123456".into() }]);
    assert_eq!(app.explorer.scopes.color_of(&root.join("src/net.rs")), rgb(0x12, 0x34, 0x56));
}

/// On this repository: a scope of one crate narrows Find in Files to it, and the Tests scope
/// holds the integration tests and the `*_tests.rs` modules.
#[test]
fn scopes_on_the_crabcraft_repository() {
    use rc_core::session::NamedScope;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
    let mut app = test_app(&dir.path().join("home"), false);
    app.open_workspace_and_wait(&root);
    app.set_scopes(crate::scopes::Draft {
        user: vec![NamedScope { name: "Search crate".into(), pattern: "crates/rc-search/**/*.rs".into() }],
        colors: vec![rc_core::session::FileColor { scope: crate::scopes::TESTS.into(), color: "#5fb865".into() }],
    });
    let search = |only: Option<rc_search::ScopeFilter>| {
        let mut f = crate::search::FindInFiles::new(&root, Waker::noop(), "ScopeFilter".into());
        f.only = Ok(only);
        f.search();
        let start = std::time::Instant::now();
        while f.summary.is_none() && start.elapsed() < std::time::Duration::from_secs(120) {
            std::thread::sleep(std::time::Duration::from_millis(20));
            f.poll();
        }
        let mut v: Vec<String> = f.results.iter().map(|r| r.rel.clone()).collect();
        v.sort();
        v
    };
    let everywhere = search(None);
    assert!(everywhere.iter().any(|f| f == "crates/rc-app/src/scopes.rs") && everywhere.iter().any(|f| f == "crates/rc-search/src/scope.rs"), "{everywhere:?}");
    let only = app.explorer.scopes.filter("Search crate").unwrap();
    assert_eq!(search(only), ["crates/rc-search/src/find.rs", "crates/rc-search/src/lib.rs", "crates/rc-search/src/scope.rs"]);
    let tests = app.explorer.scopes.filter(crate::scopes::TESTS).unwrap().unwrap();
    assert!(tests.matches("crates/rc-search/tests/workspace.rs") && tests.matches("crates/rc-app/src/rust_tests.rs"));
    assert!(!tests.matches("crates/rc-app/src/app.rs"));
    assert!(app.explorer.scopes.color_of(&root.join("crates/rc-app/src/vcs_tests.rs")).is_some());
    assert!(app.explorer.scopes.color_of(&root.join("crates/rc-app/src/vcs.rs")).is_none());
}

#[test]
#[ignore = "Requires a GPU; writes target/snapshots/scopes-*.png"]
fn scopes_capture() {
    use crate::scopes::{CHANGED, TESTS};
    use rc_core::session::{FileColor, NamedScope};
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    for dark in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let (mut h, root) = scopes_harness(dir.path(), dark);
        h.state_mut().set_scopes(crate::scopes::Draft {
            user: vec![NamedScope { name: "Sources".into(), pattern: "src/**/*.rs, !src/net.rs".into() }],
            colors: vec![
                FileColor { scope: CHANGED.into(), color: "#e08e3c".into() },
                FileColor { scope: TESTS.into(), color: "#5fb865".into() },
                FileColor { scope: "Sources".into(), color: "#4a8fe0".into() },
            ],
        });
        for f in ["src/main.rs", "src/net.rs", "tests/it.rs", "src/lib.rs"] {
            h.state_mut().open_file_and_wait(&root.join(f));
        }
        h.state_mut().explorer.locate(&root.join("tests/it.rs"), &root);
        h.state_mut().explorer.locate(&root.join("src/lib.rs"), &root);
        h.state_mut().pending.push(rc_editor::Action::FindInFiles);
        h.run_steps(3);
        let only = h.state().explorer.scopes.filter("Sources").map_err(|e| e.text());
        if let Some(f) = h.state_mut().find_in_files.as_mut() {
            f.query = "needle".into();
            f.scope = "Sources".into();
            f.only = only;
            f.search();
        }
        found_in_scope(&mut h);
        h.run_steps(4);
        let mode = if dark { "dark" } else { "light" };
        h.render().unwrap().save(out.join(format!("scopes-{mode}.png"))).unwrap();
        h.state_mut().open_settings(crate::ui::dialogs::SettingsPage::Scopes);
        h.run_steps(4);
        h.render().unwrap().save(out.join(format!("scopes-settings-{mode}.png"))).unwrap();
    }
}

/// B-21: run configurations go into folders (a submenu each in the selector), folders are
/// renamed with their configurations, and the folders come back with the session.
#[test]
fn run_configurations_are_kept_in_folders() {
    use egui_kittest::kittest::Queryable;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("trio");
    std::fs::create_dir_all(root.join("src/bin")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"trio\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    for bin in ["api", "worker", "cli"] {
        std::fs::write(root.join(format!("src/bin/{bin}.rs")), "fn main() {}\n").unwrap();
    }
    let root = root.canonicalize().unwrap();
    let mut app = test_app(&dir.path().join("home"), true);
    app.open_workspace_and_wait(&root);
    app.trust_project_tools_for_test();
    let mut h = harness(app, vec2(1280.0, 720.0));
    run_frames(&mut h, "the binaries", |a| a.runner.app_names().len() == 3);
    let name = |h: &Harness<'static, CrabCraftApp>, bin: &str| h.state().runner.app_names().into_iter().find(|n| n.contains(bin)).unwrap();
    let (api, worker, cli) = (name(&h, "api"), name(&h, "worker"), name(&h, "cli"));

    // New Folder… for one configuration, then another moved into it.
    h.state_mut().dialog = Some(crate::ui::dialogs::Dialog::ConfigFolder { config: api.clone(), rename: None, folder: "Servers".into() });
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(2);
    assert!(h.state().dialog.is_none());
    h.state_mut().runner.move_to_folder(&worker, Some("Servers"));
    assert_eq!(h.state().runner.folder_names(), ["Servers"]);
    // An empty name is refused by the dialog.
    h.state_mut().dialog = Some(crate::ui::dialogs::Dialog::ConfigFolder { config: cli.clone(), rename: None, folder: "   ".into() });
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(2);
    assert!(h.state().dialog.is_some());
    h.state_mut().dialog = None;

    // Renaming the folder takes both with it.
    h.state_mut().dialog = Some(crate::ui::dialogs::Dialog::ConfigFolder { config: api.clone(), rename: Some("Servers".into()), folder: "Back end".into() });
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(2);
    assert_eq!(h.state().runner.folder_of(&api), Some("Back end"));
    assert_eq!(h.state().runner.folder_of(&worker), Some("Back end"));
    assert_eq!(h.state().runner.folder_of(&cli), None);

    // The selector lists the folder as a submenu: its configurations are not at the top level.
    let selected = h.state().runner.selected_config().unwrap().name.clone();
    h.get_by_label(&selected).click();
    h.run_steps(3);
    assert!(h.query_all_by_label_contains("Back end").count() == 2, "the folder and its Rename entry are listed");
    assert_eq!(h.query_all_by_label(&cli).count(), if selected == cli { 2 } else { 1 });
    h.get_all_by_label_contains("Back end").next().unwrap().hover();
    h.run_steps(3);
    assert!(h.query_by_label(&worker).is_some(), "the submenu holds the folder's configurations");

    // Out of the folder; then the session keeps the rest.
    h.state_mut().runner.move_to_folder(&worker, None);
    h.state_mut().save_session();
    h.state_mut().save_all_and_wait();
    let mut again = test_app(&dir.path().join("home"), true);
    again.open_workspace_and_wait(&root);
    again.trust_project_tools_for_test();
    let start = std::time::Instant::now();
    while again.runner.app_names().len() < 3 && start.elapsed() < std::time::Duration::from_secs(60) {
        again.poll_cargo_workspace();
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    assert_eq!(again.runner.folder_of(&api), Some("Back end"));
    assert_eq!(again.runner.folder_of(&worker), None);
}

#[test]
#[ignore = "Requires a GPU; writes target/snapshots/run-config-folders-dark.png"]
fn run_config_folders_capture() {
    use egui_kittest::kittest::Queryable;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("trio");
    std::fs::create_dir_all(root.join("src/bin")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"trio\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    for bin in ["api", "worker", "cli"] {
        std::fs::write(root.join(format!("src/bin/{bin}.rs")), "fn main() {}\n").unwrap();
    }
    let mut app = test_app(&dir.path().join("home"), true);
    app.open_workspace_and_wait(&root.canonicalize().unwrap());
    app.trust_project_tools_for_test();
    let mut h = harness(app, vec2(1280.0, 720.0));
    run_frames(&mut h, "the binaries", |a| a.runner.app_names().len() == 3);
    for n in h.state().runner.app_names().into_iter().filter(|n| !n.contains("cli")) {
        h.state_mut().runner.move_to_folder(&n, Some("Back end"));
    }
    let selected = h.state().runner.selected_config().unwrap().name.clone();
    h.get_by_label(&selected).click();
    h.run_steps(3);
    h.get_all_by_label_contains("Back end").next().unwrap().hover();
    h.run_steps(4);
    h.render().unwrap().save(out.join("run-config-folders-dark.png")).unwrap();
}

/// D-27: captures the list of every breakpoint, with groups and a closed file.
#[test]
#[ignore = "writes target/snapshots/breakpoint-list-dark.png"]
fn breakpoint_list_capture() {
    use rc_editor::BreakpointKind;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("shop");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"shop\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::write(
        root.join("src/main.rs"),
        "mod price;\n\nfn main() {\n    let a = price::double(2);\n    let b = price::double(a);\n    println!(\"{b}\");\n}\n",
    )
    .unwrap();
    std::fs::write(root.join("src/price.rs"), "pub fn double(n: u32) -> u32 {\n    let twice = n * 2;\n    twice\n}\n").unwrap();
    let root = root.canonicalize().unwrap();
    let (main_rs, price_rs) = (root.join("src/main.rs"), root.join("src/price.rs"));
    let mut app = test_app(&dir.path().join("home"), true);
    app.open_workspace_and_wait(&root);
    app.docs.closed_breakpoints.entry(price_rs.clone()).or_default().insert(1, BreakpointKind::Normal);
    app.open_file_and_wait(&main_rs);
    for line in [3, 4, 5] {
        app.docs.active_doc_mut().unwrap().editor.breakpoints.insert(line, BreakpointKind::Normal);
    }
    app.debugger.conditions.insert((main_rs.clone(), 4), "a > 2".into());
    app.set_breakpoint_group(&main_rs, 3, Some("Calls"));
    app.set_breakpoint_group(&main_rs, 4, Some("Calls"));
    app.set_breakpoint_group(&price_rs, 1, Some("Pricing"));
    app.set_breakpoint_enabled(&main_rs, 4, false);
    app.dialog = Some(crate::ui::dialogs::Dialog::Breakpoints { selected: Some((main_rs.clone(), 3)), group: String::new() });
    let mut h = harness(app, vec2(1280.0, 720.0));
    h.run_steps(4);
    h.render().unwrap().save(out.join("breakpoint-list-dark.png")).unwrap();
}

/// D-15: captures the Attach dialog while it watches for processes by name.
#[test]
#[ignore = "writes target/snapshots/auto-attach-dark.png"]
fn auto_attach_capture() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("shop");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"shop\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    let mut app = test_app(&dir.path().join("home"), true);
    app.open_workspace_and_wait(&root.canonicalize().unwrap());
    app.trust_project_tools_for_test();
    let mut h = harness(app, vec2(1280.0, 720.0));
    run_frames(&mut h, "the Cargo project", |a| a.runner.cargo_workspace.is_some());
    h.state_mut().auto_attach_text = "shop-server*, *-worker".into();
    assert!(h.state_mut().start_auto_attach());
    let processes = vec![
        rc_core::processes::Process {
            pid: 48211,
            name: "shop-server".into(),
            exe: "shop-server".into(),
            command: "target/debug/shop-server --port 8080".into(),
        },
        rc_core::processes::Process { pid: 48190, name: "mail-worker".into(), exe: "mail-worker".into(), command: "target/debug/mail-worker".into() },
    ];
    h.state_mut().dialog = Some(crate::ui::dialogs::Dialog::Attach { filter: String::new(), processes });
    h.run_steps(4);
    h.render().unwrap().save(out.join("auto-attach-dark.png")).unwrap();
    h.state_mut().stop_auto_attach();
}

/// An `.http` file with three requests: one to `base`, one using a variable, one with a script.
fn http_harness(home: &Path, dir: &Path, base: &str, dark: bool) -> (Harness<'static, CrabCraftApp>, PathBuf) {
    let file = dir.join("api.http");
    let text = format!(
        "### Create user\nPOST {base}/users?notify=true\nContent-Type: application/json\nAccept: application/json\n\n{{\n  \"name\": \"Ferris\"\n}}\n\n###\n# @name health\nGET http://{{{{host}}}}/health\n\n### List\nGET {base}/users\n\n> {{% client.test(\"ok\", () => {{}}) %}}\n"
    );
    std::fs::write(&file, text).unwrap();
    let file = file.canonicalize().unwrap();
    let mut app = test_app(home, dark);
    app.open_file_and_wait(&file);
    let mut h = harness(app, vec2(1280.0, 800.0));
    h.run_steps(4);
    (h, file)
}

/// F-03: each request of an `.http` file gets a run icon; the one clicked reaches a local
/// server in the background and its response shows in the HTTP Client window. Variables are
/// flagged in the editor and keep their request from being sent.
#[test]
fn http_file_requests_run_from_the_gutter() {
    use egui_kittest::kittest::Queryable;
    use rc_editor::GutterIcon;
    use rc_kit::i18n::tr;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (mut h, file) = http_harness(home.path(), dir.path(), &base, true);
    {
        let doc = h.state().docs.active_doc().unwrap();
        assert_eq!(doc.editor.lang(), rc_syntax::Lang::Http);
        let icons: Vec<usize> = doc.editor.gutter_icons.iter().filter(|(_, i)| **i == GutterIcon::HttpRequest).map(|(l, _)| *l).collect();
        assert_eq!(icons, vec![1, 11, 14]);
        let messages: Vec<&str> = doc.editor.diagnostics.iter().map(|d| d.message.as_str()).collect();
        assert!(messages.iter().any(|m| m.contains("{{host}}")), "{messages:?}");
        assert!(messages.iter().any(|m| m.contains("script")), "{messages:?}");
    }

    // The variable: nothing is sent, the window says why.
    h.state_mut().send_http_at(11);
    h.run_steps(4);
    assert!(h.state().dock.is_open(crate::toolwindow::ToolWindow::Http));
    assert!(!h.state().http.running());
    h.get_by_label_contains("Undefined HTTP variable");

    // The first request: a local server answers.
    let server = fake_playground(listener, r#"{"id":42,"name":"Ferris","tags":["crab","rust"]}"#);
    h.state_mut().send_http_at(1);
    run_frames(&mut h, "the response", |a| !a.http.running());
    let seen = server.join().unwrap();
    assert!(seen.starts_with("POST /users?notify=true HTTP/1.1\r\n"), "{seen}");
    assert!(seen.ends_with("\r\n\r\n{\n  \"name\": \"Ferris\"\n}"), "{seen}");
    let e = &h.state().http.history[0];
    assert_eq!((e.title.as_str(), e.file.as_path(), e.line), ("Create user", file.as_path(), 1));
    h.get_by_label("200 OK");
    h.get_by_label_contains("\"tags\": [");
    h.get_by_label(tr("Body"));

    // The window's link goes back to the request.
    h.get_by_label("api.http:2").click();
    h.run_steps(4);
    assert_eq!(h.state().docs.active_doc().map(|d| d.editor.caret_line_col().0), Some(1));
}

#[test]
#[ignore = "Requires a GPU; writes target/snapshots/http-client-*.png"]
fn http_client_capture() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    for dark in [true, false] {
        let home = tempfile::tempdir().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (mut h, _) = http_harness(home.path(), dir.path(), &base, dark);
        let server = fake_playground(listener, r#"{"id":42,"name":"Ferris","tags":["crab","rust"],"address":{"city":"Lisbon","zip":null}}"#);
        h.state_mut().send_http_at(11);
        h.state_mut().send_http_at(1);
        run_frames(&mut h, "the response", |a| !a.http.running());
        server.join().unwrap();
        h.run_steps(4);
        let suffix = if dark { "dark" } else { "light" };
        h.render().unwrap().save(out.join(format!("http-client-{suffix}.png"))).unwrap();
        h.state_mut().http.show_headers = true;
        h.run_steps(4);
        h.render().unwrap().save(out.join(format!("http-client-headers-{suffix}.png"))).unwrap();
    }
}

/// A-01: Profile builds the selected Rust binary and records it with the real perf. Where the
/// kernel forbids recording (perf_event_paranoid above 2), the window says so and how to allow
/// it; otherwise the profile has samples of the program's own code.
#[test]
fn profile_records_the_selected_binary_or_explains_the_kernel_restriction() {
    use egui_kittest::kittest::Queryable;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if crate::profiler::perf_path().is_none() {
        eprintln!("skipped: no perf");
        return;
    }
    let home = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut h = playground_harness(home.path(), dir.path(), true);
    // A program that runs long enough to be sampled (the harness's own ends at once).
    let main_rs = dir.path().join("demo/src/main.rs");
    std::fs::write(&main_rs, "fn fib(n: u64) -> u64 { if n < 2 { n } else { fib(n - 1) + fib(n - 2) } }\n\nfn main() {\n    let t = std::time::Instant::now();\n    let mut n = 0;\n    while t.elapsed().as_millis() < 300 {\n        n += fib(std::hint::black_box(20));\n    }\n    println!(\"{n}\");\n}\n").unwrap();
    assert!(crate::search::action_items().iter().any(|(_, a)| *a == rc_editor::Action::Profile));
    h.state_mut().pending.push(rc_editor::Action::Profile);
    h.run_steps(2);
    assert!(h.state().dock.is_open(crate::toolwindow::ToolWindow::Profiler));
    assert!(h.state().profiler.busy());
    run_frames(&mut h, "the profile", |a| !a.profiler.busy());
    match h.state().profiler.result.as_ref().unwrap() {
        Ok(p) => {
            assert!(p.samples > 10, "{p:?}");
            let fib = p.frames.iter().find(|f| f.name.ends_with("demo::fib")).expect("demo::fib sampled");
            assert!(fib.own && fib.source.as_ref().is_some_and(|(path, _)| path.ends_with("src/main.rs")), "{fib:?}");
        }
        Err(crate::profiler::ProfError::Restricted(level)) => {
            assert!(level.is_some_and(|l| l > 2));
            assert!(h.get_all_by_label_contains("sudo sysctl kernel.perf_event_paranoid=2").next().is_some());
        }
        Err(e) => panic!("{e:?}\n{}", h.state().profiler.output),
    }
}

/// A profile made from `perf script` text of a small program (no recording needed).
fn sample_profile() -> crate::profiler::Profile {
    let bin = "/work/demo/target/debug/demo";
    let stacks: &[(&[&str], u64)] = &[
        (&["demo::parse::tokenize", "demo::parse::parse", "demo::main"], 40),
        (&["demo::parse::number", "demo::parse::tokenize", "demo::parse::parse", "demo::main"], 25),
        (&["demo::eval::apply", "demo::eval::eval", "demo::main"], 30),
        (&["demo::eval::apply", "demo::eval::apply", "demo::eval::eval", "demo::main"], 22),
        (&["alloc::raw_vec::RawVec<T,A>::grow_one", "demo::parse::tokenize", "demo::parse::parse", "demo::main"], 12),
        (&["__memmove_avx_unaligned_erms", "demo::report::print", "demo::main"], 9),
        (&["demo::main"], 4),
    ];
    let mut text = String::new();
    for (stack, n) in stacks {
        for i in 0..*n {
            text.push_str(&format!("demo  77 100.{i:06}:    2004008 cpu-clock:u: \n"));
            for f in stack.iter().chain(["std::rt::lang_start_internal", "__libc_start_call_main"].iter()) {
                let dso = if f.starts_with("__") { "/usr/lib/x86_64-linux-gnu/libc.so.6" } else { bin };
                text.push_str(&format!("\t    5581f0a1cc4f {f} ({dso})\n"));
            }
            text.push('\n');
        }
    }
    let mut parser = crate::profiler::ScriptParser::new(Some(Path::new(bin)));
    for l in text.split_inclusive('\n') {
        parser.line(l);
    }
    let p = parser.finish();
    assert_eq!(p.samples, 142);
    p
}

#[test]
#[ignore = "Requires a GPU; writes target/snapshots/profiler-*.png"]
fn profiler_capture() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    for dark in [true, false] {
        let suffix = if dark { "dark" } else { "light" };
        let home = tempfile::tempdir().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut h = playground_harness(home.path(), dir.path(), dark);
        let profile = sample_profile();
        {
            let app = h.state_mut();
            app.dock.show(crate::toolwindow::ToolWindow::Profiler);
            app.profiler.name = "demo".into();
            app.profiler.open = crate::profiler::hot_path(&profile);
            app.profiler.result = Some(Ok(profile));
        }
        h.run_steps(4);
        h.render().unwrap().save(out.join(format!("profiler-flame-{suffix}.png"))).unwrap();
        h.state_mut().profiler.show_tree = true;
        h.run_steps(4);
        h.render().unwrap().save(out.join(format!("profiler-tree-{suffix}.png"))).unwrap();
        h.state_mut().profiler.result = Some(Err(crate::profiler::ProfError::Restricted(Some(4))));
        h.run_steps(4);
        h.render().unwrap().save(out.join(format!("profiler-restricted-{suffix}.png"))).unwrap();
    }
}

/// V-07: a rebase stopped on conflicts (banner, Conflicts dialog), the Merge and Rebase dialogs.
#[test]
#[ignore = "Requires a GPU; writes target/snapshots/git-rebase-*.png and git-merge-dialog-dark.png"]
fn screens_git_merge_and_rebase() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    let home = tempfile::tempdir().unwrap();
    let place = tempfile::tempdir().unwrap();
    let root = place.path().canonicalize().unwrap().join("shop");
    std::fs::create_dir_all(root.join("src")).unwrap();
    let git = |args: &[&str]| assert!(std::process::Command::new("git").args(args).current_dir(&root).output().unwrap().status.success(), "{args:?}");
    git(&["init", "-q", "-b", "main"]);
    for (k, v) in [("user.name", "Ada Lovelace"), ("user.email", "ada@example.com"), ("core.hooksPath", "no-hooks"), ("commit.gpgsign", "false")] {
        git(&["config", k, v]);
    }
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"shop\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn price(n: u32) -> u32 {\n    n * 2\n}\n").unwrap();
    std::fs::write(root.join("src/tax.rs"), "pub fn tax(p: u32) -> u32 {\n    p / 5\n}\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "Initial shop"]);
    git(&["switch", "-q", "-c", "feature/discount"]);
    std::fs::write(root.join("src/lib.rs"), "pub fn price(n: u32) -> u32 {\n    n * 2 - n / 10\n}\n").unwrap();
    std::fs::write(root.join("src/tax.rs"), "pub fn tax(p: u32) -> u32 {\n    p / 4\n}\n").unwrap();
    git(&["commit", "-qam", "Give a discount"]);
    git(&["switch", "-q", "main"]);
    std::fs::write(root.join("src/lib.rs"), "pub fn price(n: u32) -> u32 {\n    n * 3\n}\n").unwrap();
    std::fs::write(root.join("src/tax.rs"), "pub fn tax(p: u32) -> u32 {\n    p / 6\n}\n").unwrap();
    git(&["commit", "-qam", "Raise prices"]);
    git(&["switch", "-q", "feature/discount"]);

    let mut app = test_app(home.path(), true);
    app.open_workspace_and_wait(&root);
    app.trust_project_tools_for_test();
    app.dock.show(crate::toolwindow::ToolWindow::Git);
    app.dock.sizes.insert(crate::toolwindow::ToolWindow::Git, 420.0);
    let mut h = harness(app, vec2(1440.0, 900.0));
    run_frames(&mut h, "branches", |a| a.vcs.branches.len() == 2 && !a.vcs.log.is_empty());
    h.state_mut().dialog = Some(crate::ui::dialogs::Dialog::merge("main".into()));
    h.run_steps(4);
    h.render().unwrap().save(out.join("git-merge-dialog-dark.png")).unwrap();
    h.state_mut().dialog = Some(crate::ui::dialogs::Dialog::Rebase { onto: "main".into(), autostash: true });
    h.run_steps(4);
    h.render().unwrap().save(out.join("git-rebase-dialog-dark.png")).unwrap();
    h.state_mut().dialog = None;
    h.state_mut().vcs.rebase("main", false);
    run_frames(&mut h, "rebase stopped", |a| a.vcs.busy.is_none() && a.vcs.operation.state.is_some() && a.vcs.conflicts().len() == 2);
    h.run_steps(4);
    h.render().unwrap().save(out.join("git-rebase-stopped-dark.png")).unwrap();
    h.state_mut().dialog = Some(crate::ui::dialogs::Dialog::Conflicts { selected: ["src/lib.rs".to_string()].into() });
    h.run_steps(4);
    h.render().unwrap().save(out.join("git-rebase-conflicts-dark.png")).unwrap();
}

/// V-08: the three-pane merge tool on a conflicted Rust file.
#[test]
#[ignore = "Requires a GPU; writes target/snapshots/merge-tool-dark.png"]
fn screens_merge_tool() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    let home = tempfile::tempdir().unwrap();
    let place = tempfile::tempdir().unwrap();
    let root = place.path().canonicalize().unwrap().join("shop");
    std::fs::create_dir_all(root.join("src")).unwrap();
    let git = |args: &[&str]| assert!(std::process::Command::new("git").args(args).current_dir(&root).output().unwrap().status.success(), "{args:?}");
    git(&["init", "-q", "-b", "main"]);
    for (k, v) in [("user.name", "Ada Lovelace"), ("user.email", "ada@example.com"), ("core.hooksPath", "no-hooks"), ("commit.gpgsign", "false")] {
        git(&["config", k, v]);
    }
    let lib = |price: &str, tax: &str, extra: &str| {
        format!(
            "/// Prices an order.\npub fn price(n: u32) -> u32 {{\n    {price}\n}}\n\n/// The tax on a price.\npub fn tax(p: u32) -> u32 {{\n    {tax}\n}}\n{extra}\n#[cfg(test)]\nmod tests {{\n    #[test]\n    fn prices() {{\n        assert!(super::price(10) > 0);\n    }}\n}}\n"
        )
    };
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"shop\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::write(root.join("src/lib.rs"), lib("n * 2", "p / 5", "")).unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "Initial shop"]);
    git(&["switch", "-q", "-c", "feature/discount"]);
    std::fs::write(root.join("src/lib.rs"), lib("n * 2 - n / 10", "p / 5", "\n/// A discount.\npub fn discount(p: u32) -> u32 {\n    p / 10\n}\n")).unwrap();
    git(&["commit", "-qam", "Give a discount"]);
    git(&["switch", "-q", "main"]);
    std::fs::write(root.join("src/lib.rs"), lib("n * 3", "p / 4", "")).unwrap();
    git(&["commit", "-qam", "Raise prices and taxes"]);
    let _ = std::process::Command::new("git").args(["merge", "feature/discount"]).current_dir(&root).output();

    let mut app = test_app(home.path(), true);
    app.open_workspace_and_wait(&root);
    app.trust_project_tools_for_test();
    let mut h = harness(app, vec2(1440.0, 900.0));
    run_frames(&mut h, "conflict", |a| a.vcs.conflicts().len() == 1 && a.vcs.operation.state.is_some());
    h.state_mut().vcs.open_merge_tool("src/lib.rs");
    run_frames(&mut h, "merge tool", |a| a.vcs.merge_tool.is_some());
    h.run_steps(6);
    h.render().unwrap().save(out.join("merge-tool-dark.png")).unwrap();
    h.state_mut().vcs.merge_tool.as_mut().unwrap().confirm_save = true;
    h.run_steps(6);
    h.render().unwrap().save(out.join("merge-tool-confirm-dark.png")).unwrap();
}

/// The bottom tool window spans the whole width, under the explorer too; a right-click in an
/// editor of a split opens its menu at the pointer.
#[test]
#[ignore = "Requires a GPU; writes target/snapshots/bottom-tool-window.png"]
fn bottom_tool_window_capture() {
    use crate::layout::{Drop, Edge};
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    let home = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut h = playground_harness(home.path(), dir.path(), true);
    let lib = dir.path().join("demo/src/lib.rs");
    std::fs::write(&lib, "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n").unwrap();
    h.state_mut().open_file_and_wait(&lib.canonicalize().unwrap());
    h.run_steps(3);
    let id = h.state().docs.active_doc().unwrap().editor.id;
    let group = h.state().layout.group_of(id).unwrap();
    assert!(h.state_mut().layout.move_doc(id, Drop::Split { group, edge: Edge::Bottom }));
    h.state_mut().dock.toggle(crate::toolwindow::ToolWindow::Terminal);
    h.run_steps(6);
    right_click(&mut h, egui::pos2(560.0, 130.0));
    h.run_steps(3);
    h.render().unwrap().save(out.join("bottom-tool-window.png")).unwrap();
}

/// With two editors side by side, Alt+Enter in the left one opens its list at that caret, and
/// clicking "Rename to …" there renames the function.
#[test]
fn context_actions_open_at_the_active_editor_of_a_split() {
    use crate::layout::{Drop, Edge};
    use egui_kittest::kittest::Queryable;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("demo");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n").unwrap();
    const MAIN: &str = "mod other;\n\nfn main() {\n    let i = 12;\n    println!(\"{i}\");\n    testeDeEscrita();\n}\n\nfn testeDeEscrita() {}\n";
    std::fs::write(root.join("src/main.rs"), MAIN).unwrap();
    std::fs::write(root.join("src/other.rs"), "pub fn other() {}\n").unwrap();
    let root = root.canonicalize().unwrap();
    let (main, other) = (root.join("src/main.rs"), root.join("src/other.rs"));
    let mut app = test_app(home.path(), true);
    app.settings.trusted_projects.push(root.clone());
    app.open_workspace_and_wait(&root);
    app.open_file_and_wait(&other);
    app.open_file_and_wait(&main);
    let mut h = harness(app, vec2(1600.0, 900.0));
    h.run_steps(3);
    // other.rs goes to a group on the right; main.rs stays on the left and is the active one.
    let other_id = h.state().docs.list[h.state().docs.index_of(&other).unwrap()].editor.id;
    let group = h.state().layout.group_of(other_id).unwrap();
    assert!(h.state_mut().layout.move_doc(other_id, Drop::Split { group, edge: Edge::Right }));
    let i = h.state().docs.index_of(&main).unwrap();
    h.state_mut().docs.activate(i);
    run_frames(&mut h, "the case warning", |a| a.docs.list[i].editor.diagnostics.iter().any(|d| d.message.contains("snake_case")));
    let at = MAIN.rfind("testeDeEscrita").unwrap() + 2;
    h.state_mut().docs.list[i].editor.set_selection(rc_text::Selection::point(at));
    h.run_steps(3);
    let caret = h.state().caret_screen.unwrap();
    assert!(caret.x < 800.0, "the caret is in the left editor: {caret:?}");
    h.state_mut().pending.push(rc_editor::Action::ShowIntentions);
    run_frames(&mut h, "the context actions", |a| matches!(&a.dialog, Some(crate::ui::dialogs::Dialog::CodeActions { list, .. }) if !list.is_empty()));
    let row = h.get_by_label("Rename to teste_de_escrita");
    let r = row.rect();
    assert!(r.left() < 800.0 && (r.top() - caret.y).abs() < 40.0, "the list opens at the caret {caret:?}, not at {r:?}");
    row.click();
    run_frames(&mut h, "the renamed function", |a| a.docs.list[i].editor.buffer.rope().to_string().contains("fn teste_de_escrita()"));
    assert!(h.state().docs.list[i].editor.buffer.rope().to_string().contains("    teste_de_escrita();\n"));
    h.state_mut().insight.stop();
}

/// Every Settings page has the same size, with a line between the menu and the page; the
/// Keymap list fills the page.
#[test]
#[ignore = "Requires a GPU; writes target/snapshots/settings-page-*.png"]
fn settings_pages_capture() {
    use crate::ui::dialogs::SettingsPage;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    let home = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut h = playground_harness(home.path(), dir.path(), true);
    let mut sizes = Vec::new();
    for (page, name) in [
        (SettingsPage::Keymap, "keymap"),
        (SettingsPage::Appearance, "appearance"),
        (SettingsPage::Editor, "editor"),
        (SettingsPage::Inspections, "inspections"),
        (SettingsPage::LiveTemplates, "live-templates"),
    ] {
        h.state_mut().open_settings(page);
        h.run_steps(4);
        let rect = h.ctx.memory(|m| m.area_rect(egui::Id::unique("settings"))).unwrap_or(egui::Rect::NOTHING);
        sizes.push((name, rect.size()));
        h.render().unwrap().save(out.join(format!("settings-page-{name}.png"))).unwrap();
        h.state_mut().dialog = None;
        h.run_steps(2);
    }
    eprintln!("{sizes:?}");
    assert!(sizes.windows(2).all(|w| (w[0].1 - w[1].1).length() < 1.0), "{sizes:?}");
}

#[test]
fn portable_preferences_are_previewed_and_applied_in_a_rust_workspace() {
    use crate::ui::dialogs::{Dialog, SettingsPage};
    use egui_kittest::kittest::Queryable;
    use rc_core::settings_transfer::Backup;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let (mut h, root) = two_crates_harness(home.path(), true);
    let original = h.state().settings.clone();
    let mut source = original.clone();
    source.editor_font_size = 19.0;
    source.trusted_projects.push(root.clone());
    let path = home.path().join("preferences.json");
    Backup::from_settings(&source).unwrap().write(&path).unwrap();
    h.state_mut().settings_backup.preview = Some(Backup::read(&path).unwrap());
    h.state_mut().open_settings(SettingsPage::Backup);
    h.run_steps(3);
    assert_eq!(h.state().settings.editor_font_size, original.editor_font_size);
    h.get_by_label(rc_kit::i18n::tr("Apply imported preferences")).click();
    h.run_steps(3);
    assert_eq!(h.state().settings.editor_font_size, original.editor_font_size);
    if let Some(Dialog::Settings { draft, .. }) = &h.state().dialog {
        assert_eq!(draft.editor_font_size, 19.0);
        assert_eq!(draft.trusted_projects, original.trusted_projects);
    } else {
        panic!("settings closed before confirmation");
    }
    h.get_by_label(rc_kit::i18n::tr("OK")).click();
    h.run_steps(3);
    assert_eq!(h.state().settings.editor_font_size, 19.0);
    assert_eq!(h.state().settings.trusted_projects, original.trusted_projects);
    assert!(h.state().runner.cargo_workspace.is_some());
}

#[test]
fn editor_font_and_ide_zoom_apply_to_a_real_rust_project() {
    let _home = HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("src")).unwrap();
    std::fs::write(root.path().join("Cargo.toml"), "[package]\nname = \"zoom_fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n").unwrap();
    let source = root.path().join("src/main.rs");
    let text = "fn main() { println!(\"zoom\"); }\n";
    std::fs::write(&source, text).unwrap();
    let mut app = test_app(home.path(), false);
    app.open_workspace_and_wait(root.path());
    app.open_file_and_wait(&source);
    let mut harness = harness(app, vec2(1280.0, 800.0));
    let ctx = harness.ctx.clone();
    harness.state_mut().run_ide_action(&ctx, rc_editor::Action::ZoomIn);
    harness.run_steps(2);
    assert!((harness.ctx.zoom_factor() - 1.1).abs() < f32::EPSILON);
    harness.state_mut().run_ide_action(&ctx, rc_editor::Action::ZoomOut);
    harness.run_steps(2);
    assert!((harness.ctx.zoom_factor() - 1.0).abs() < f32::EPSILON);
    let mut settings = harness.state().settings.clone();
    settings.editor_font_size = 21.0;
    settings.ui_zoom = 1.4;
    harness.state_mut().apply_settings(&ctx, settings);
    harness.run_steps(2);
    assert_eq!(harness.state().editor_config.font.size, 21.0);
    assert!((harness.ctx.zoom_factor() - 1.4).abs() < f32::EPSILON);
    let saved = Settings::load();
    assert_eq!(saved.editor_font_size, 21.0);
    assert_eq!(saved.ui_zoom, 1.4);
    harness.state_mut().run_ide_action(&ctx, rc_editor::Action::ZoomReset);
    harness.run_steps(2);
    assert!((harness.ctx.zoom_factor() - 1.0).abs() < f32::EPSILON);
    assert_eq!(harness.state().docs.active_doc().unwrap().editor.buffer.rope().to_string(), text);
    assert_eq!(std::fs::read_to_string(source).unwrap(), text);
}

#[test]
fn settings_keep_the_same_width_when_switching_pages() {
    use crate::ui::dialogs::SettingsPage;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let mut h = harness(test_app(home.path(), true), vec2(1280.0, 800.0));
    let mut widths = Vec::new();
    for page in [
        SettingsPage::Appearance,
        SettingsPage::Editor,
        SettingsPage::Keymap,
        SettingsPage::Tools,
        SettingsPage::ExternalTools,
        SettingsPage::LiveTemplates,
        SettingsPage::Copyright,
        SettingsPage::Scopes,
        SettingsPage::Inspections,
        SettingsPage::Backup,
        SettingsPage::Appearance,
    ] {
        h.state_mut().open_settings(page);
        h.run_steps(4);
        let initial_height = h.ctx.memory(|memory| memory.area_rect(egui::Id::unique("settings"))).unwrap().height();
        for _ in 0..30 {
            h.run_steps(1);
            let rect = h.ctx.memory(|memory| memory.area_rect(egui::Id::unique("settings"))).unwrap();
            assert!((rect.height() - initial_height).abs() < 1.0, "settings height must remain stable: {page:?}: {initial_height} -> {}", rect.height());
            assert!(rect.height() <= 760.0, "settings must stay inside the viewport: {page:?}: {rect:?}");
        }
        let rect = h.ctx.memory(|memory| memory.area_rect(egui::Id::unique("settings"))).unwrap();
        widths.push(rect.width());
    }
    assert!(widths.windows(2).all(|pair| (pair[0] - pair[1]).abs() < 1.0), "settings widths: {widths:?}");
}

#[test]
fn central_menu_commands_and_project_templates_work_in_a_rust_project() {
    use egui_kittest::kittest::Queryable;
    use rc_kit::i18n::tr;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("menu-demo");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"menu-demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n").unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    let mut app = test_app(&home.path().join("home"), true);
    app.open_workspace_and_wait(&root);
    app.open_file_and_wait(&root.join("src/main.rs"));
    let mut h = harness(app, vec2(1280.0, 800.0));
    h.get_by_label(tr("Main Menu")).click();
    h.run_steps(2);
    h.get_by_label(&format!("{} ⏵", tr("Edit"))).click();
    h.run_steps(2);
    h.get_by_label(tr("Duplicate Line or Selection")).click();
    h.run_steps(3);
    assert_eq!(h.state().docs.active_doc().unwrap().editor.buffer.rope().to_string().matches("fn main()").count(), 2);
    h.get_by_label(tr("Main Menu")).click();
    h.run_steps(2);
    h.get_by_label(&format!("{} ⏵", tr("Edit"))).click();
    h.run_steps(2);
    h.get_by_label(tr("Undo")).click();
    h.run_steps(3);
    assert_eq!(h.state().docs.active_doc().unwrap().editor.buffer.rope().to_string().matches("fn main()").count(), 1);
    h.get_by_label(tr("Main Menu")).click();
    h.run_steps(2);
    h.get_by_label(&format!("{} ⏵", tr("Tools"))).click();
    h.run_steps(2);
    h.query_all_by_label(tr("ACP Agents")).find(|node| node.rect().width() > 100.0).unwrap().click();
    h.run_steps(2);
    assert!(h.state().dock.is_open(crate::toolwindow::ToolWindow::Agents));
    h.state_mut().dock.hide_all();
    h.state_mut().dock.show(crate::toolwindow::ToolWindow::Explorer);
    let source = root.join("src").canonicalize().unwrap();
    h.state_mut().explorer.selected = Some(format!("dir:{}", source.display()));
    h.run_steps(3);
    h.get_by_label(tr("New File…")).click();
    h.run_steps(2);
    h.state_mut().explorer.new_item_filter = "Rust".into();
    h.run_steps(2);
    assert!(h.query_by_label(tr("JSON File")).is_none());
    h.get_by_label(tr("Rust File")).click();
    h.run_steps(2);
    if let Some(crate::ui::dialogs::Dialog::NewFile { dir, name, .. }) = &mut h.state_mut().dialog {
        assert_eq!(dir, &source);
        *name = "created_from_project_menu".into();
    } else {
        panic!("project template did not open the creation dialog");
    }
    h.run_steps(2);
    h.get_by_label(tr("Create")).click();
    run_frames(&mut h, "the Rust file created from the project menu", |_| source.join("created_from_project_menu.rs").is_file());
    assert!(source.join("created_from_project_menu.rs").is_file());
}

#[test]
#[ignore = "Requires a GPU; writes menu, project identity and docking screenshots"]
fn central_menus_and_docking_capture() {
    use egui_kittest::kittest::Queryable;
    use rc_kit::i18n::tr;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("project-colours");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"project-colours\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n").unwrap();
    for name in ["main.rs", "lib.rs"] {
        std::fs::write(root.join("src").join(name), "fn main() {\n    println!(\"Hello, CrabCraft!\");\n}\n").unwrap();
    }
    let mut app = test_app(&home.path().join("home"), true);
    app.open_workspace_and_wait(&root);
    for name in ["main.rs", "lib.rs"] {
        app.open_file_and_wait(&root.join("src").join(name));
    }
    let mut h = harness(app, vec2(1280.0, 800.0));
    h.get_by_label(tr("Main Menu")).click();
    h.run_steps(2);
    h.get_by_label(&format!("{} ⏵", tr("File"))).click();
    h.run_steps(3);
    h.render().unwrap().save(out.join("central-menu-file.png")).unwrap();
    h.input_mut().events.push(egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() });
    h.run_steps(3);
    h.get_by_label(tr("New File…")).click();
    h.run_steps(3);
    h.render().unwrap().save(out.join("project-new-menu.png")).unwrap();
    h.input_mut().events.push(egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() });
    h.run_steps(3);
    let from = h.query_all_by_label("lib.rs").find(|node| node.rect().top() < 130.0).unwrap().rect().center();
    h.input_mut().events.push(egui::Event::PointerMoved(from));
    h.run_steps(1);
    h.input_mut().events.push(egui::Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.run_steps(1);
    let to = egui::pos2(1050.0, 400.0);
    for step in 1..=8 {
        h.input_mut().events.push(egui::Event::PointerMoved(from + (to - from) * (step as f32 / 8.0)));
        h.run_steps(1);
    }
    h.run_steps(15);
    h.render().unwrap().save(out.join("editor-docking-overlay.png")).unwrap();
    assert!(matches!(h.state().tab_drag.as_ref().and_then(|drag| drag.target), Some(crate::layout::Drop::Split { .. })));
}

#[test]
fn editor_docking_guides_create_the_requested_split() {
    use egui_kittest::kittest::Queryable;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("dock-demo");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"dock-demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n").unwrap();
    for name in ["main.rs", "lib.rs"] {
        std::fs::write(root.join("src").join(name), "fn main() {}\n").unwrap();
    }
    let mut app = test_app(&home.path().join("home"), true);
    app.open_workspace_and_wait(&root);
    for name in ["main.rs", "lib.rs"] {
        app.open_file_and_wait(&root.join("src").join(name));
    }
    app.dock.hide_all();
    let mut h = harness(app, vec2(1280.0, 800.0));
    let from = h.query_all_by_label("lib.rs").find(|node| node.rect().top() < 130.0).unwrap().rect().center();
    // The right guide in the central docking map, away from the editor's edge region.
    drag(&mut h, from, egui::pos2(704.0, 424.0));
    assert!(matches!(h.state().layout.main, crate::layout::Node::Split { vertical: true, .. }));
    assert_eq!(h.state().layout.main.groups().len(), 2);
    assert!(h.state().tab_drag.is_none());
}

#[test]
fn tools_settings_fields_fit_the_fixed_page_width() {
    use crate::ui::dialogs::SettingsPage;
    use egui_kittest::kittest::Queryable;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let mut h = harness(test_app(home.path(), true), vec2(1280.0, 800.0));
    h.state_mut().open_settings(SettingsPage::Tools);
    h.run_steps(5);
    let modal = h.ctx.memory(|memory| memory.area_rect(egui::Id::unique("settings"))).unwrap();
    assert!(modal.width() < 900.0, "tools modal must keep its fixed width: {modal:?}");
    let fields: Vec<_> = h.query_all_by_role(egui::accesskit::Role::TextInput).map(|node| node.rect()).collect();
    assert!(fields.len() >= 10);
    for field in fields {
        assert!(field.left() >= modal.left() && field.right() <= modal.right() - 8.0, "tool field must fit inside the modal: {field:?}, {modal:?}");
    }
}

#[test]
#[ignore = "Requires a GPU; writes the Tools settings layout"]
fn tools_settings_layout_capture() {
    use crate::ui::dialogs::SettingsPage;
    let _home = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let mut h = harness(test_app(home.path(), true), vec2(1280.0, 800.0));
    h.state_mut().open_settings(SettingsPage::Tools);
    run_frames(&mut h, "the installed toolchain versions", |app| app.toolchain.is_some());
    h.run_steps(6);
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    h.render().unwrap().save(out.join("settings-tools-stacked.png")).unwrap();
}

#[test]
#[ignore = "Requires a GPU; captures completed menus, settings and docking in both themes"]
fn completed_interface_theme_and_size_capture() {
    use egui_kittest::kittest::Queryable;
    use rc_kit::i18n::tr;
    let _home = HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    std::fs::create_dir_all(&out).unwrap();
    let files = tempfile::tempdir().unwrap();
    let root = files.path().join("interface-demo");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"interface-demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n").unwrap();
    for name in ["main.rs", "lib.rs"] {
        std::fs::write(root.join("src").join(name), "fn main() {\n    println!(\"CrabCraft\");\n}\n").unwrap();
    }
    for (dark, size, label) in [
        (true, vec2(1280.0, 800.0), "dark"),
        (false, vec2(1280.0, 800.0), "light"),
        (true, vec2(640.0, 400.0), "compact-dark"),
        (false, vec2(640.0, 400.0), "compact-light"),
    ] {
        let home = tempfile::tempdir().unwrap();
        let mut app = test_app(home.path(), dark);
        app.open_workspace_and_wait(&root);
        app.trust_project_tools_for_test();
        for name in ["main.rs", "lib.rs"] {
            app.open_file_and_wait(&root.join("src").join(name));
        }
        let mut h = harness(app, size);
        h.state_mut().open_settings(crate::ui::dialogs::SettingsPage::Tools);
        run_frames(&mut h, "installed toolchain versions", |app| app.toolchain.is_some());
        h.run_steps(5);
        let modal = h.ctx.memory(|memory| memory.area_rect(egui::Id::unique("settings"))).unwrap();
        assert!(egui::Rect::from_min_size(egui::Pos2::ZERO, size).contains_rect(modal), "{label}: {modal:?}");
        h.render().unwrap().save(out.join(format!("completed-settings-{label}.png"))).unwrap();
        h.state_mut().dialog = None;
        h.run_steps(3);
        h.get_by_label(tr("Main Menu")).click();
        h.run_steps(3);
        h.get_by_label(&format!("{} ⏵", tr("File"))).click();
        h.run_steps(3);
        h.render().unwrap().save(out.join(format!("completed-menu-{label}.png"))).unwrap();
        if size.x > 1000.0 {
            h.input_mut().events.push(egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            });
            h.run_steps(3);
            let from = h.query_all_by_label("lib.rs").find(|node| node.rect().top() < 120.0).unwrap().rect().center();
            h.input_mut().events.push(egui::Event::PointerMoved(from));
            h.run_steps(1);
            h.input_mut().events.push(egui::Event::PointerButton {
                pos: from,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            });
            h.run_steps(1);
            let to = egui::pos2(size.x - 160.0, size.y * 0.5);
            for step in 1..=8 {
                h.input_mut().events.push(egui::Event::PointerMoved(from + (to - from) * (step as f32 / 8.0)));
                h.run_steps(1);
            }
            h.run_steps(15);
            assert!(matches!(h.state().tab_drag.as_ref().and_then(|drag| drag.target), Some(crate::layout::Drop::Split { .. })));
            h.render().unwrap().save(out.join(format!("completed-docking-{label}.png"))).unwrap();
            h.input_mut().events.push(egui::Event::PointerButton {
                pos: to,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            });
            h.run_steps(3);
            assert_eq!(h.state().layout.main.groups().len(), 2);
        }
    }
}

#[test]
fn untrusted_project_tool_settings_explain_why_detection_is_disabled() {
    use egui_kittest::kittest::Queryable;
    use rc_kit::i18n::tr;
    let _home = HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("untrusted-tools");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"untrusted-tools\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n").unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    let mut app = test_app(&home.path().join("home"), true);
    app.open_workspace_and_wait(&root);
    let mut h = harness(app, vec2(1280.0, 800.0));
    h.state_mut().open_settings(crate::ui::dialogs::SettingsPage::Tools);
    h.run_steps(5);
    assert!(h.query_by_label(tr("Trust this project before running project tools or code")).is_some());
    assert!(h.state().toolchain.is_none());
    assert!(h.state().settings.trusted_projects.is_empty());
}
