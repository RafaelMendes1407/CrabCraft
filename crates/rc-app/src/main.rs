#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod complete_statement;
mod terminal_attach;
mod database_core;
mod database_csv;
mod database;
mod remote;
mod remote_transfer;
mod database_postgres;
mod sql_injection;
mod cli;
mod endpoints;
mod clone_project;
mod compare_cli;
mod cargo_dependencies;
mod missing_crates;
mod borrow_errors;
mod repl;
mod repl_variables;
mod macro_tester;
mod macro_step_engine;
mod macro_step;
mod local_ai;
mod local_ai_client;
mod local_ai_proposal;
mod type_hierarchy;
mod doctest_completion;
mod settings_backup;
mod settings_migration;
mod instance;
mod auto_attach;
mod breakpoint_list;
mod convert;
mod copyright;
mod coverage;
mod debugger;
mod parallel_stacks;
mod dep_versions;
mod feature_fix;
mod feature_nav;
mod http_client;
mod http_cli;
mod disassembly_view;
mod disk_revert;
mod disk_watch;
mod insight;
mod web_languages;
mod inspection_profile;
mod intention_preview;
mod call_tree;
mod item_templates;
mod layout;
mod local_history;
mod manifest_completion;
mod manifest_inspect;
mod memory_view;
mod merge_tool;
mod playground;
mod profiler;
mod refactor_conflicts;
mod refactor_preview;
mod run_anything;
mod runner;
mod rustfmt;
mod rustup_installer;
mod search;
mod replace_files;
mod scopes;
mod sync_log;
mod vcs;
#[cfg(test)]
mod vcs_tests;
#[cfg(test)]
mod runner_tests;
#[cfg(test)]
mod debugger_tests;
#[cfg(test)]
mod insight_tests;
#[cfg(test)]
mod rust_tests;
#[cfg(test)]
mod screens;
#[cfg(test)]
mod editor_rust_audit;
mod docs;
mod file_ops;
mod file_rename;
mod native_menu;
mod open_files;
mod toolwindow;
mod terminal_tools;
mod ui;
#[cfg(test)]
mod inherited_ui_audit;

/// Opened from Finder, the app gets a bare PATH (/usr/bin:/bin:…) without what Terminal adds
/// from /etc/paths and /etc/paths.d, such as /usr/local/share/dotnet: the tools it starts
/// (Roslyn's project loader, `dotnet watch`, tests) would not find `dotnet`.
#[cfg(target_os = "macos")]
fn system_path() {
    let Ok(out) = std::process::Command::new("/usr/libexec/path_helper").arg("-s").output() else { return };
    let text = String::from_utf8_lossy(&out.stdout);
    let Some(helper) = text.split("PATH=\"").nth(1).and_then(|r| r.split('"').next()) else { return };
    let current = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs: Vec<std::path::PathBuf> = std::env::split_paths(&current).collect();
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    for d in std::env::split_paths(helper) {
        // /etc/paths.d entries such as `~/.dotnet/tools` are written with a tilde.
        let d = match (d.strip_prefix("~"), &home) {
            (Ok(rest), Some(h)) => h.join(rest),
            _ => d,
        };
        if !dirs.contains(&d) {
            dirs.push(d);
        }
    }
    if let Ok(joined) = std::env::join_paths(dirs) {
        // SAFETY: first thing in main, before any other thread exists.
        unsafe { std::env::set_var("PATH", joined) };
    }
}

/// The command line speaks the language chosen in the settings.
fn i18n_from_settings() {
    use rc_kit::i18n::{Lang, set_lang};
    let settings = rc_core::Settings::load();
    set_lang(settings.language.as_deref().map(Lang::from_locale).unwrap_or_else(Lang::system));
}

fn main() -> eframe::Result {
    if let Some(code) = macro_step_engine::internal() { std::process::exit(code); }
    if let Some(code) = privileged_run::internal() { std::process::exit(code); }
    if let Some(code) = history_edit::internal() { std::process::exit(code); }
    #[cfg(target_os = "macos")]
    system_path();
    let mut options = eframe::NativeOptions { viewport: rc_kit::window_chrome::viewport(), ..Default::default() };
    // Window size and position persist through eframe; everything else lives in our own files.
    options.persist_window = true;
    // `crabcraft <path>...` opens folders, projects and files; on macOS Finder's "Open With"
    // arrives through `open_files`.
    i18n_from_settings();
    let cwd = std::env::current_dir().unwrap_or_default();
    let open = match cli::parse(std::env::args_os().skip(1), &cwd) {
        Ok(cli::Command::Open(list)) => list,
        Ok(cli::Command::Compare(request)) => {
            let code = compare_cli::run(request, options)?;
            std::process::exit(code);
        }
        Ok(cli::Command::Http(request)) => {
            match request.run() {
                Ok(code) => std::process::exit(code),
                Err(error) => { eprintln!("crabcraft: {error}"); std::process::exit(1); }
            }
        }
        Ok(cli::Command::Tool(request)) => {
            match request.run() {
                Ok(code) => std::process::exit(code),
                Err(error) => {
                    eprintln!("crabcraft: {}: {error}", rc_kit::i18n::tr("Could not start Cargo"));
                    std::process::exit(1);
                }
            }
        }
        Ok(cli::Command::Help) => {
            println!("{}", cli::usage());
            return Ok(());
        }
        Ok(cli::Command::Version) => {
            println!("CrabCraft {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Err(e) => {
            eprintln!("crabcraft: {e}\n\n{}", cli::usage());
            std::process::exit(2);
        }
    };
    // Files go to the window that is already open; a project opens a window of its own.
    let endpoint = instance::endpoint();
    let only_files = !open.is_empty() && open.iter().all(|r| r.path.is_file() && r.path.file_name().is_none_or(|n| n != "Cargo.toml"));
    if only_files && instance::forward(&endpoint, &open).unwrap_or(false) {
        return Ok(());
    }
    let _listener = instance::listen(&endpoint, open_files::push);
    open_files::install();
    eframe::run_native("CrabCraft", options, Box::new(move |cc| Ok(Box::new(app::CrabCraftApp::new(cc, open)))))
}

#[cfg(test)]
mod project_trust;

mod remote_debug;
mod debug_sessions;
mod settings_sync;

#[cfg(test)]
mod large_file_audit;

#[cfg(test)]
mod terminal_completion_audit;

#[cfg(test)]
mod key_sequence_audit;
#[cfg(test)]
mod accessibility_audit;

mod shelves;

mod history_edit;

mod issue_tracker;

mod hosting;

mod acp;
mod plugins;
mod proofreader;
mod proofreader_core;
mod containers;
mod dev_containers;
mod privileged_run;
