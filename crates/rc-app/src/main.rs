#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod auto_attach;
mod axum_queries;
mod axum_semantic;
mod borrow_errors;
mod breakpoint_list;
mod call_tree;
mod cargo_dependencies;
mod cli;
mod clone_project;
mod compare_cli;
mod complete_statement;
mod container_workspace;
mod convert;
mod copyright;
mod coverage;
mod database;
mod database_core;
mod database_csv;
mod database_postgres;
mod debugger;
mod dep_versions;
mod deploy_ftps;
mod deploy_sftp;
mod deployment;
mod deployment_ui;
mod disassembly_view;
mod disk_open;
mod disk_revert;
mod disk_save;
mod disk_watch;
mod docs;
mod doctest_completion;
#[cfg(test)]
mod editor_rust_audit;
mod endpoint_url;
mod endpoints;
mod feature_fix;
mod feature_nav;
mod file_edit;
mod file_ops;
mod file_rename;
mod http_cli;
mod http_client;
#[cfg(test)]
mod inherited_ui_audit;
mod insight;
mod inspection_profile;
mod instance;
mod intention_preview;
mod item_templates;
mod layout;
mod local_ai;
mod local_ai_client;
mod local_ai_proposal;
mod local_history;
#[cfg(test)]
mod macro_actions_audit;
mod macro_step;
mod macro_step_engine;
mod macro_tester;
mod manifest_completion;
mod manifest_inspect;
mod memory_view;
mod merge_tool;
mod missing_crates;
mod move_items;
mod move_items_core;
mod native_menu;
mod open_files;
mod parallel_stacks;
mod playground;
mod profiler;
mod refactor_conflicts;
mod refactor_preview;
mod remote;
mod remote_development;
mod remote_development_transport;
mod remote_transfer;
mod repl;
mod repl_variables;
mod replace_files;
mod run_anything;
mod runner;
#[cfg(test)]
#[cfg(test)]
#[cfg(test)]
#[cfg(test)]
mod rust_tests;
mod rustfmt;
mod rustup_installer;
mod scopes;
#[cfg(test)]
mod screens;
mod search;
mod settings_backup;
mod settings_migration;
mod signature_backend;
mod signature_change;
mod signature_model;
mod sql_injection;
mod sync_log;
mod terminal_attach;
mod terminal_tools;
mod toolwindow;
mod type_hierarchy;
mod ui;
mod vcs;
#[cfg(test)]
mod vcs_tests;
mod web_languages;
mod workspace_open;
mod wsl_workspace;

/// Opened from Finder, the app gets a bare PATH (/usr/bin:/bin:…) without what Terminal adds
/// from /etc/paths and /etc/paths.d: Cargo and rust-analyzer may otherwise be unavailable.
#[cfg(target_os = "macos")]
fn system_path() {
    let Ok(out) = std::process::Command::new("/usr/libexec/path_helper").arg("-s").output() else { return };
    let text = String::from_utf8_lossy(&out.stdout);
    let Some(helper) = text.split("PATH=\"").nth(1).and_then(|r| r.split('"').next()) else { return };
    let current = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs: Vec<std::path::PathBuf> = std::env::split_paths(&current).collect();
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    for d in std::env::split_paths(helper) {
        // Some entries in /etc/paths.d are written with a tilde.
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
    if let Some(code) = macro_step_engine::internal() {
        std::process::exit(code);
    }
    if let Some(code) = privileged_run::internal() {
        std::process::exit(code);
    }
    if let Some(code) = privileged_debug_backend::internal() {
        std::process::exit(code);
    }
    if let Some(code) = history_edit::internal() {
        std::process::exit(code);
    }
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
        Ok(cli::Command::Http(request)) => match request.run() {
            Ok(code) => std::process::exit(code),
            Err(error) => {
                eprintln!("crabcraft: {error}");
                std::process::exit(1);
            }
        },
        Ok(cli::Command::Tool(request)) => match request.run() {
            Ok(code) => std::process::exit(code),
            Err(error) => {
                eprintln!("crabcraft: {}: {error}", rc_kit::i18n::tr("Could not start Cargo"));
                std::process::exit(1);
            }
        },
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

mod debug_sessions;
mod remote_debug;
mod settings_sync;

#[cfg(test)]
mod large_file_audit;

#[cfg(test)]
mod terminal_completion_audit;

#[cfg(test)]
mod accessibility_audit;
#[cfg(test)]
mod key_sequence_audit;

mod shelves;

mod history_edit;

mod issue_tracker;

mod hosting;

mod acp;
mod containers;
mod criticalup_provider;
mod dev_containers;
mod plugins;
mod privileged_debug;
mod privileged_debug_backend;
mod privileged_run;
mod proofreader;
mod proofreader_core;
