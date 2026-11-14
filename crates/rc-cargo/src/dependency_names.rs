//! Dependency import roots for conservative missing-crate suggestions. Target tables are
//! included without evaluating cfg, so inactive/optional entries are never duplicated.
use crate::dependencies::{DependencyError, Scope};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default)]
pub struct Names {
    pub roots: BTreeSet<String>,
    /// Canonical package name -> the dependency import names available in this scope.
    pub packages: BTreeMap<String, BTreeSet<String>>,
}
#[derive(Clone, Debug, Default)]
pub struct DependencyNames {
    pub normal: Names,
    pub dev: Names,
    pub build: Names,
}
impl DependencyNames {
    pub fn scope(&self, scope: Scope) -> &Names {
        match scope {
            Scope::Normal => &self.normal,
            Scope::Dev => &self.dev,
            Scope::Build => &self.build,
        }
    }
}
fn normalized(name: &str) -> String {
    name.replace('-', "_")
}
fn table(names: &mut Names, table: Option<&toml::value::Table>, workspace: Option<&toml::value::Table>, budget: &mut usize) -> Result<(), DependencyError> {
    for (name, value) in table.into_iter().flatten() {
        if *budget == 0 {
            return Err(DependencyError::TooLarge);
        }
        *budget -= 1;
        if !crate::valid_crate_name(name) {
            continue;
        }
        let inherited = value.get("workspace").and_then(toml::Value::as_bool) == Some(true);
        let definition = if inherited { workspace.and_then(|table| table.get(name)).ok_or(DependencyError::Manifest)? } else { value };
        let package = definition.get("package").and_then(toml::Value::as_str).unwrap_or(name);
        if !crate::valid_crate_name(package) {
            continue;
        }
        let import = normalized(name);
        names.roots.insert(import.clone());
        names.packages.entry(normalized(package)).or_default().insert(import);
    }
    Ok(())
}

pub fn dependency_names(member: &str, workspace: Option<&str>) -> Result<DependencyNames, DependencyError> {
    if member.len() > 4 * 1024 * 1024 || workspace.is_some_and(|source| source.len() > 4 * 1024 * 1024) {
        return Err(DependencyError::TooLarge);
    }
    let member: toml::Value = toml::from_str(member).map_err(|_| DependencyError::Manifest)?;
    let workspace: Option<toml::Value> = workspace.map(toml::from_str).transpose().map_err(|_| DependencyError::Manifest)?;
    let shared = workspace.as_ref().and_then(|value| value.get("workspace")).and_then(|value| value.get("dependencies")).and_then(toml::Value::as_table);
    let mut result = DependencyNames::default();
    let mut budget = 12_000;
    for (scope, names) in [(Scope::Normal, &mut result.normal), (Scope::Dev, &mut result.dev), (Scope::Build, &mut result.build)] {
        table(names, member.get(scope.table()).and_then(toml::Value::as_table), shared, &mut budget)?;
        if let Some(targets) = member.get("target").and_then(toml::Value::as_table) {
            if targets.len() > 256 {
                return Err(DependencyError::TooLarge);
            }
            for target in targets.values() {
                table(names, target.get(scope.table()).and_then(toml::Value::as_table), shared, &mut budget)?;
            }
        }
    }
    result.dev.roots.extend(result.normal.roots.iter().cloned());
    for (package, aliases) in &result.normal.packages {
        result.dev.packages.entry(package.clone()).or_default().extend(aliases.iter().cloned());
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aliases_workspace_targets_optional_and_dependency_scopes_are_distinct() {
        let workspace = r#"[workspace.dependencies]
serde_alias = { package = "serde", version = "1" }
"#;
        let member = r#"[dependencies]
serde_alias = { workspace = true }
tokio = { version = "1", optional = true }
[dev-dependencies]
tempfile = "3"
[build-dependencies]
regex = "1"
[target.'cfg(windows)'.dependencies]
winapi = "0.3"
"#;
        let names = dependency_names(&member, Some(&workspace)).unwrap();
        assert!(names.normal.roots.contains("serde_alias") && !names.normal.roots.contains("serde"));
        assert!(names.normal.packages["serde"].contains("serde_alias"));
        assert!(names.normal.roots.contains("tokio") && names.normal.roots.contains("winapi"));
        assert!(!names.normal.roots.contains("tempfile") && names.dev.roots.contains("tempfile"));
        assert!(names.dev.roots.contains("serde_alias") && !names.build.roots.contains("serde_alias"));
        assert!(names.build.roots.contains("regex") && !names.normal.roots.contains("regex"));
    }
}
