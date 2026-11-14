//! wasm-pack builds reuse the Run window's streaming output and cancellation.
use rc_core::session::RunOptions;
use rc_kit::i18n::tr;
use std::path::{Path, PathBuf};

pub const TARGETS: &[&str] = &["bundler", "web", "nodejs", "no-modules", "deno"];

#[derive(Debug, PartialEq, Eq)]
pub enum PlanError {
    Target,
    Manifest,
    Features,
    Environment,
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Target => tr("Choose a supported wasm-pack target"),
            Self::Manifest => tr("Specify an existing Cargo.toml or project folder"),
            Self::Features => tr("Invalid Cargo feature names"),
            Self::Environment => tr("One NAME=value per line."),
        })
    }
}
impl std::error::Error for PlanError {}

pub fn plan(manifest: &Path, options: &RunOptions, mut env: Vec<(String, String)>) -> Result<rc_dotnet::LaunchPlan, PlanError> {
    let dir = manifest.parent().filter(|_| manifest.file_name().is_some_and(|name| name == "Cargo.toml")).ok_or(PlanError::Manifest)?;
    let target = if options.wasm_target.is_empty() { "bundler" } else { options.wasm_target.as_str() };
    if !TARGETS.contains(&target) {
        return Err(PlanError::Target);
    }
    let mut args: Vec<String> = ["build", ".", "--target", target, "--mode", "no-install", "--no-opt"].into_iter().map(str::to_string).collect();
    args.push(if options.wasm_dev { "--dev" } else { "--release" }.into());
    if !options.wasm_out_dir.trim().is_empty() {
        args.extend(["--out-dir".into(), options.wasm_out_dir.trim().into()]);
    }
    let features: Vec<&str> = options.features.split([',', ' ']).filter(|s| !s.is_empty()).collect();
    if features.iter().any(|s| !s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+' | '.' | '/'))) {
        return Err(PlanError::Features);
    }
    if !features.is_empty() {
        args.extend(["--features".into(), features.join(",")]);
    }
    env.extend(rc_core::session::parse_env(&options.env).map_err(|_| PlanError::Environment)?);
    env.push(("RUSTUP_AUTO_INSTALL".into(), "0".into()));
    Ok(rc_dotnet::LaunchPlan { program: PathBuf::from("wasm-pack"), args, env, working_dir: dir.to_path_buf(), urls: Vec::new() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_rejects_untrusted_configuration_values_and_never_installs_tools() {
        let manifest = Path::new("/workspace/demo/Cargo.toml");
        let options = RunOptions {
            wasm_target: "web".into(),
            wasm_dev: true,
            wasm_out_dir: "web assets".into(),
            features: "small,fast".into(),
            env: "MODE=test".into(),
            ..Default::default()
        };
        let p = plan(manifest, &options, vec![("MODE".into(), "project".into())]).unwrap();
        assert_eq!(p.working_dir, Path::new("/workspace/demo"));
        assert_eq!(
            p.args,
            ["build", ".", "--target", "web", "--mode", "no-install", "--no-opt", "--dev", "--out-dir", "web assets", "--features", "small,fast"]
        );
        assert_eq!(p.env.iter().rfind(|(key, _)| key == "MODE").unwrap().1, "test");
        assert!(p.env.contains(&("RUSTUP_AUTO_INSTALL".into(), "0".into())));
        assert_eq!(plan(manifest, &RunOptions { wasm_target: "--publish".into(), ..Default::default() }, Vec::new()).unwrap_err(), PlanError::Target);
        assert_eq!(plan(manifest, &RunOptions { features: "foo; rm".into(), ..Default::default() }, Vec::new()).unwrap_err(), PlanError::Features);
        assert_eq!(plan(manifest, &RunOptions { env: "invalid".into(), ..Default::default() }, Vec::new()).unwrap_err(), PlanError::Environment);
        let saved = serde_json::to_string(&options).unwrap();
        assert_eq!(serde_json::from_str::<RunOptions>(&saved).unwrap(), options);
    }

    #[test]
    #[ignore = "Requires wasm-pack, matching wasm-bindgen CLI and wasm32 target; builds real wasm"]
    fn builds_real_wasm_from_a_trusted_run_configuration() {
        use crate::runner::{ConfigKind, Runner, RunnerOutput};
        use rc_core::Waker;
        use std::time::{Duration, Instant};
        let package = tempfile::Builder::new().prefix("crabcraft wasm ").tempdir().unwrap();
        std::fs::create_dir(package.path().join("src")).unwrap();
        std::fs::write(package.path().join("Cargo.toml"), "[package]\nname = \"wasm_fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[lib]\ncrate-type = [\"cdylib\", \"rlib\"]\n[dependencies]\nwasm-bindgen = \"=0.2.130\"\n[features]\nfast = []\n[workspace]\n").unwrap();
        std::fs::write(package.path().join("src/lib.rs"), "use wasm_bindgen::prelude::*;\n#[wasm_bindgen]\npub fn answer() -> i32 { 42 }\n").unwrap();
        std::fs::write(package.path().join("build.rs"), "fn main() { std::fs::write(\"build-ran\", \"yes\").unwrap(); }\n").unwrap();
        let cargo = rc_cargo::Cargo::locate().unwrap().with_env(vec![("CARGO_NET_OFFLINE".into(), "true".into())]);
        let model = rc_cargo::load_workspace(&cargo, &package.path().join("Cargo.toml")).unwrap();
        assert!(model.packages[0].cdylib);
        let mut runner = Runner::new(Waker::noop());
        runner.set_cargo_workspace(model, None);
        runner.selected = runner.configs.iter().position(|c| matches!(c.kind, ConfigKind::WasmPack { .. })).unwrap();
        let config = runner.selected_config().unwrap().name.clone();
        assert!(matches!(runner.run_selected().as_slice(), [RunnerOutput::Notify(_, true)]));
        assert!(runner.runs.is_empty());
        assert!(!package.path().join("build-ran").exists());
        runner.trusted_project = Some(package.path().canonicalize().unwrap());
        // The test caller puts installed test tools on PATH; the shipped command stays exact.
        let options = RunOptions {
            wasm_target: "web".into(),
            wasm_dev: true,
            wasm_out_dir: "web assets".into(),
            features: "fast".into(),
            env: "CARGO_NET_OFFLINE=true".into(),
            ..Default::default()
        };
        runner.options.insert(config, options);
        assert!(matches!(runner.run_selected().as_slice(), [RunnerOutput::ShowTool(_)]));
        let deadline = Instant::now() + Duration::from_secs(60);
        while runner.runs[0].running() {
            runner.update();
            assert!(Instant::now() < deadline, "wasm-pack timed out: {:?}", runner.runs[0].console.term.screen_lines());
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(runner.runs[0].exit, Some(Some(0)), "{:?}", runner.runs[0].console.term.screen_lines());
        assert!(package.path().join("build-ran").exists());
        assert!(package.path().join("web assets/wasm_fixture_bg.wasm").is_file());
        assert!(package.path().join("web assets/wasm_fixture.js").is_file());
        // Rerun starts a fresh wasm-pack build; Stop must terminate its Cargo/build-script tree.
        std::fs::write(package.path().join("build.rs"), "fn main() { std::fs::write(\"build-started\", std::process::id().to_string()).unwrap(); std::thread::sleep(std::time::Duration::from_secs(20)); std::fs::write(\"build-finished\", \"yes\").unwrap(); }\n").unwrap();
        runner.run_selected();
        let deadline = Instant::now() + Duration::from_secs(20);
        while !package.path().join("build-started").exists() {
            runner.update();
            assert!(Instant::now() < deadline, "build script did not start: {:?}", runner.runs[0].console.term.screen_lines());
            std::thread::sleep(Duration::from_millis(10));
        }
        runner.runs[0].stop();
        while runner.runs[0].running() {
            runner.update();
            assert!(Instant::now() < deadline, "Stop did not finish wasm-pack");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!package.path().join("build-finished").exists());
    }
}
