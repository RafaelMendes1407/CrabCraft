//! Validated arguments for the visual dependency manager. Cargo edits the manifest and lock;
//! no shell is involved. Reading installed dependencies does not invoke Cargo or the network.
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scope {
    #[default]
    Normal,
    Dev,
    Build,
}
impl Scope {
    pub fn table(self) -> &'static str {
        match self {
            Self::Normal => "dependencies",
            Self::Dev => "dev-dependencies",
            Self::Build => "build-dependencies",
        }
    }
    fn flag(self) -> Option<&'static str> {
        match self {
            Self::Normal => None,
            Self::Dev => Some("--dev"),
            Self::Build => Some("--build"),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    pub name: String,
    pub scope: Scope,
    pub version: String,
    pub source: String,
    pub features: Vec<String>,
    pub path: Option<PathBuf>,
    pub editable: bool,
    pub optional: bool,
    pub default_features: bool,
}
#[derive(Clone, Debug, Default)]
pub struct Add {
    pub name: String,
    pub version: String,
    pub features: Vec<String>,
    pub optional: bool,
    pub no_default_features: bool,
    pub path: Option<PathBuf>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DependencyError {
    Name,
    Version,
    Features,
    Manifest,
    TooLarge,
    OptionalDev,
}

fn name_ok(name: &str) -> bool {
    crate::valid_crate_name(name) && !name.starts_with('-')
}

pub fn add_args(manifest: &Path, scope: Scope, add: &Add, offline: bool) -> Result<Vec<String>, DependencyError> {
    if scope == Scope::Dev && add.optional {
        return Err(DependencyError::OptionalDev);
    }
    if !name_ok(&add.name) {
        return Err(DependencyError::Name);
    }
    if !add.version.is_empty() && semver::VersionReq::parse(&add.version).is_err() {
        return Err(DependencyError::Version);
    }
    if add.features.len() > 100
        || add.features.iter().any(|f| f.is_empty() || f.len() > 128 || !f.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+')))
    {
        return Err(DependencyError::Features);
    }
    let mut args = vec!["add".into(), "--manifest-path".into(), manifest.to_string_lossy().into_owned()];
    args.extend(scope.flag().map(str::to_string));
    if offline {
        args.push("--offline".into());
    }
    if scope != Scope::Dev {
        args.push(if add.optional { "--optional" } else { "--no-optional" }.into());
    }
    args.push(if add.no_default_features { "--no-default-features" } else { "--default-features" }.into());
    if !add.features.is_empty() {
        args.extend(["--features".into(), add.features.join(",")]);
    }
    if let Some(path) = &add.path {
        args.extend(["--path".into(), path.to_string_lossy().into_owned()]);
    }
    args.push("--".into());
    args.push(if add.version.is_empty() { add.name.clone() } else { format!("{}@{}", add.name, add.version) });
    Ok(args)
}
pub fn remove_args(manifest: &Path, scope: Scope, name: &str, offline: bool) -> Result<Vec<String>, DependencyError> {
    if !name_ok(name) {
        return Err(DependencyError::Name);
    }
    let mut args = vec!["remove".into(), "--manifest-path".into(), manifest.to_string_lossy().into_owned()];
    args.extend(scope.flag().map(str::to_string));
    if offline {
        args.push("--offline".into());
    }
    args.extend(["--".into(), name.into()]);
    Ok(args)
}

/// Direct, dev and build entries of this package. Target-specific and workspace-level
/// dependency tables are deliberately outside this editor's scope.
pub fn installed(text: &str) -> Result<Vec<Installed>, DependencyError> {
    if text.len() > 4 * 1024 * 1024 {
        return Err(DependencyError::TooLarge);
    }
    let doc: toml::Value = toml::from_str(text).map_err(|_| DependencyError::Manifest)?;
    let mut deps = Vec::new();
    for scope in [Scope::Normal, Scope::Dev, Scope::Build] {
        let Some(table) = doc.get(scope.table()).and_then(toml::Value::as_table) else { continue };
        for (name, value) in table.iter().take(10_000) {
            if !name_ok(name) {
                continue;
            }
            let get = |key: &str| value.get(key).and_then(toml::Value::as_str).unwrap_or("").to_string();
            let source = if let Some(s) = value.get("path").and_then(toml::Value::as_str) {
                s.to_string()
            } else if let Some(s) = value.get("git").and_then(toml::Value::as_str) {
                s.to_string()
            } else if value.get("workspace").and_then(toml::Value::as_bool) == Some(true) {
                "workspace".to_string()
            } else {
                get("registry")
            };
            deps.push(Installed {
                name: name.clone(),
                scope,
                version: value.as_str().map(str::to_string).unwrap_or_else(|| get("version")),
                source: source.clone(),
                path: value.get("path").and_then(toml::Value::as_str).map(PathBuf::from),
                editable: source.is_empty() || value.get("path").is_some(),
                features: value
                    .get("features")
                    .and_then(toml::Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(toml::Value::as_str)
                    .map(str::to_string)
                    .take(100)
                    .collect(),
                optional: value.get("optional").and_then(toml::Value::as_bool).unwrap_or(false),
                default_features: value.get("default-features").and_then(toml::Value::as_bool).unwrap_or(true),
            });
        }
    }
    Ok(deps)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_options_and_invalid_requirements() {
        let manifest = Path::new("/tmp/project/Cargo.toml");
        assert_eq!(add_args(manifest, Scope::Normal, &Add { name: "--help".into(), ..Default::default() }, true), Err(DependencyError::Name));
        assert_eq!(
            add_args(manifest, Scope::Normal, &Add { name: "serde".into(), version: "oops".into(), ..Default::default() }, true),
            Err(DependencyError::Version)
        );
        let deps = installed("[dependencies]\nserde = { version = \"1\", features = [\"derive\"], default-features = false }\n[dev-dependencies]\nlocal = { path = \"../local\" }\n[target.'cfg(unix)'.dependencies]\nother = \"1\"\n").unwrap();
        assert_eq!(deps.len(), 2);
        assert_eq!(deps[0].features, ["derive"]);
        assert!(!deps[0].default_features);
        assert_eq!(deps[1].scope, Scope::Dev);
    }
    #[test]
    fn cargo_edits_real_workspace_packages_and_scopes_without_running_build_scripts() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = [\"app\", \"local\"]\nresolver = \"2\"\n").unwrap();
        for name in ["app", "local"] {
            std::fs::create_dir_all(root.join(name).join("src")).unwrap();
            std::fs::write(
                root.join(name).join("Cargo.toml"),
                format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[features]\nextra = []\n"),
            )
            .unwrap();
            std::fs::write(root.join(name).join("src/lib.rs"), "pub fn value() -> u32 { 42 }\n").unwrap();
            std::fs::write(root.join(name).join("build.rs"), "fn main() { std::fs::write(\"build-ran\", \"yes\").unwrap(); }\n").unwrap();
        }
        let cargo = crate::Cargo::locate().unwrap();
        let manifest = root.join("app/Cargo.toml");
        for scope in [Scope::Normal, Scope::Dev, Scope::Build] {
            let add =
                Add { name: "local".into(), features: vec!["extra".into()], no_default_features: true, path: Some(root.join("local")), ..Default::default() };
            let output = cargo.std_command().args(add_args(&manifest, scope, &add, true).unwrap()).current_dir(root).output().unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            let deps = installed(&std::fs::read_to_string(&manifest).unwrap()).unwrap();
            assert!(deps.iter().any(|dep| dep.name == "local" && dep.scope == scope && dep.features == ["extra"] && !dep.default_features));
            assert!(!root.join("app/build-ran").exists());
            assert!(!root.join("local/build-ran").exists());
            assert!(installed(&std::fs::read_to_string(root.join("local/Cargo.toml")).unwrap()).unwrap().is_empty());
            let mut updated = add.clone();
            updated.no_default_features = false;
            updated.optional = scope != Scope::Dev;
            let output = cargo.std_command().args(add_args(&manifest, scope, &updated, true).unwrap()).current_dir(root).output().unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            let deps = installed(&std::fs::read_to_string(&manifest).unwrap()).unwrap();
            assert!(deps.iter().any(|dep| dep.default_features && dep.optional == updated.optional));
            updated.optional = false;
            let output = cargo.std_command().args(add_args(&manifest, scope, &updated, true).unwrap()).current_dir(root).output().unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            assert!(installed(&std::fs::read_to_string(&manifest).unwrap()).unwrap().iter().all(|dep| !dep.optional));
            let output = cargo.std_command().args(remove_args(&manifest, scope, "local", true).unwrap()).current_dir(root).output().unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            assert!(installed(&std::fs::read_to_string(&manifest).unwrap()).unwrap().is_empty());
        }
    }
}
