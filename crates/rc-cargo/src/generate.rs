//! Git repository templates through cargo-generate. Hooks require explicit template trust in
//! the caller; shell commands remain disabled and stdin is closed (no invisible prompts).

use std::path::{Path, PathBuf};
use rc_dotnet::CancelToken;
use rc_dotnet::process::{StreamOpts, run_streaming};

#[derive(Clone, Debug)]
pub struct GenerateProject {
    pub repository: String,
    pub branch: String,
    pub name: String,
    pub location: PathBuf,
    pub git: bool,
    pub trusted: bool,
}

#[derive(Debug)]
pub enum GenerateError {
    Invalid(&'static str),
    Io(std::io::Error),
    Failed { code: Option<i32>, output: String },
    Cancelled,
    MissingManifest(PathBuf),
}

impl GenerateProject {
    pub fn project_dir(&self) -> PathBuf { self.location.join(&self.name) }

    pub fn problem(&self) -> Option<&'static str> {
        if !crate::new_project::valid_name(&self.name) {
            Some("Use letters, digits, - and _, starting with a letter")
        } else if !self.location.is_absolute() || !self.location.is_dir() {
            Some("The location does not exist")
        } else if self.project_dir().exists() {
            Some("That folder already exists")
        } else if self.repository.trim().is_empty() || self.repository.starts_with('-') || self.repository.chars().any(char::is_control) {
            Some("Enter a Git template repository URL or local path")
        } else if self.branch.starts_with('-') || self.branch.chars().any(char::is_control) {
            Some("Enter a valid template branch")
        } else if !self.trusted {
            Some("Trust the template before generating a project")
        } else { None }
    }

    fn command(&self, program: &Path) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new(program);
        cmd.args(["generate", "--git", &self.repository, "--name", &self.name]);
        cmd.arg("--destination").arg(&self.location);
        cmd.args(["--silent", "--force", "--skip-submodules", "--no-workspace", "--no-fmt", "--vcs", if self.git { "git" } else { "none" }]);
        if !self.branch.is_empty() { cmd.args(["--branch", &self.branch]); }
        cmd.current_dir(&self.location).env("GIT_TERMINAL_PROMPT", "0").env("RUSTUP_AUTO_INSTALL", "0");
        cmd
    }

    /// Requires cargo-generate 0.26+; older versions fail with their argument error. A failure
    /// preserves partial output for inspection and never removes an existing destination.
    pub async fn create(&self, program: &Path, cancel: &CancelToken) -> Result<PathBuf, GenerateError> {
        if let Some(problem) = self.problem() { return Err(GenerateError::Invalid(problem)); }
        let mut output = String::new();
        let result = run_streaming(self.command(program), cancel, StreamOpts::default(), &mut |_| {}, &mut |line| {
            // Keep errors useful without retaining unbounded output from an untrusted tool.
            let remaining = 64 * 1024_usize - output.len();
            for ch in line.text.chars() {
                if output.len() + ch.len_utf8() > 64 * 1024 { break; }
                output.push(ch);
            }
            if remaining > 0 && output.len() < 64 * 1024 { output.push('\n'); }
        }).await.map_err(GenerateError::Io)?;
        if result.cancelled { return Err(GenerateError::Cancelled); }
        if result.code != Some(0) { return Err(GenerateError::Failed { code: result.code, output }); }
        let dir = self.project_dir();
        if !dir.join("Cargo.toml").is_file() { return Err(GenerateError::MissingManifest(dir)); }
        Ok(dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(location: &Path) -> GenerateProject {
        GenerateProject { repository: "https://example.invalid/template".into(), branch: String::new(), name: "sample_project".into(), location: location.to_path_buf(), git: false, trusted: true }
    }

    #[test]
    fn untrusted_and_invalid_inputs_are_rejected_before_starting_a_tool() {
        let dir = tempfile::tempdir().unwrap();
        let mut req = request(dir.path());
        assert_eq!(req.problem(), None);
        req.trusted = false;
        assert!(req.problem().is_some());
        req.trusted = true;
        for source in ["", "--allow-commands", "https://example.invalid/\nfoo"] {
            req.repository = source.into();
            assert!(req.problem().is_some());
        }
        req.repository = "https://example.invalid/template".into();
        req.name = "../escape".into();
        assert!(req.problem().is_some());
    }

    #[test]
    fn command_is_noninteractive_and_never_allows_shell_commands() {
        let dir = tempfile::tempdir().unwrap();
        let req = request(dir.path());
        let command = req.command(Path::new("cargo-generate"));
        let args: Vec<_> = command.as_std().get_args().map(|s| s.to_string_lossy().into_owned()).collect();
        assert!(args.iter().any(|s| s == "--silent"));
        assert!(args.iter().any(|s| s == "--no-workspace"));
        assert!(!args.iter().any(|s| s == "--allow-commands" || s == "--overwrite"));
    }

    #[tokio::test]
    #[ignore = "requires cargo-generate 0.26+, supplied in CRABCRAFT_CARGO_GENERATE"]
    async fn real_git_template_creates_a_rust_project_without_running_its_build_script() {
        let tool = std::env::var_os("CRABCRAFT_CARGO_GENERATE").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("cargo-generate"));
        let dir = tempfile::Builder::new().prefix("crabcraft template ").tempdir().unwrap();
        let template = dir.path().join("template");
        std::fs::create_dir_all(template.join("src")).unwrap();
        std::fs::write(template.join("Cargo.toml"), "[package]\nname = \"{{project-name}}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n").unwrap();
        std::fs::write(template.join("src/main.rs"), "fn main() { println!(\"hello {{project-name}}\"); }\n").unwrap();
        std::fs::write(template.join("build.rs"), "fn main() { std::fs::write(\"build-ran\", \"yes\").unwrap(); }\n").unwrap();
        for args in [vec!["init"], vec!["add", "."], vec!["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "-m", "Template"]] {
            assert!(std::process::Command::new("git").args(args).current_dir(&template).status().unwrap().success());
        }
        let mut req = request(dir.path());
        req.repository = template.display().to_string();
        let generated = req.create(&tool, &CancelToken::new()).await.unwrap();
        assert!(generated.join("Cargo.toml").is_file());
        assert!(!generated.join("build-ran").exists());
        let manifest = std::fs::read_to_string(generated.join("Cargo.toml")).unwrap();
        assert!(manifest.contains("sample_project"), "{manifest}");
        assert!(std::process::Command::new("cargo").args(["test", "--offline"]).current_dir(&generated).status().unwrap().success());
        assert!(generated.join("build-ran").exists());
        assert!(matches!(req.create(&tool, &CancelToken::new()).await, Err(GenerateError::Invalid(_))));
        // Even a trusted template cannot bypass our disabled shell-command policy.
        std::fs::write(template.join("cargo-generate.toml"), "[hooks]\npost = [\"hook.rhai\"]\n").unwrap();
        std::fs::write(template.join("hook.rhai"), "system::command(\"cargo\", [\"--version\"]);\n").unwrap();
        assert!(std::process::Command::new("git").args(["add", "."]).current_dir(&template).status().unwrap().success());
        assert!(std::process::Command::new("git").args(["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "-m", "Command hook"]).current_dir(&template).status().unwrap().success());
        req.name = "blocked_hook".into();
        assert!(matches!(req.create(&tool, &CancelToken::new()).await, Err(GenerateError::Failed { .. })));
        assert!(!req.project_dir().join("build-ran").exists());
    }

    #[tokio::test]
    async fn cancellation_before_start_does_not_create_a_folder() {
        let dir = tempfile::tempdir().unwrap();
        let req = request(dir.path());
        let cancel = CancelToken::new();
        cancel.cancel();
        assert!(matches!(req.create(Path::new("missing-tool"), &cancel).await, Err(GenerateError::Cancelled)));
        assert!(!req.project_dir().exists());
    }
}
