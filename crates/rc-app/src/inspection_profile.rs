//! Inspection profiles (I-18): the Default profile (in the settings) and the project's own
//! (in its session), which the project may use instead. A profile turns inspections off or
//! changes their level; it is applied wherever findings become diagnostics, so the editor's
//! underlines, its scroll-bar marks and the Problems window all follow it:
//!
//! - CrabCraft's inspections of `Cargo.toml` (I-22) and its crates.io checks (C-02, C-04), each
//!   with its own id;
//! - rust-analyzer's diagnostics, which include rustc's and Clippy's (`cargo check` or Clippy on
//!   save), matched by their code;
//! - Inspect Code's Clippy findings (I-17), matched by lint.

use rc_core::inspections::{InspectionProfile, Level};
use rc_editor::{Diagnostic, Severity};
use rc_kit::i18n::tr;

/// Where an inspection comes from, for grouping in Settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    CargoToml,
    CratesIo,
    Rust,
}

impl Group {
    pub fn title(self) -> &'static str {
        match self {
            Group::CargoToml => "Cargo.toml",
            Group::CratesIo => "crates.io",
            Group::Rust => tr("Rust (rust-analyzer, compiler and Clippy)"),
        }
    }
}

/// The inspections listed in Settings › Inspections. Other diagnostic codes can be added there.
pub const KNOWN: &[(&str, Group)] = &[
    (UNKNOWN_KEY, Group::CargoToml),
    (INCOMPATIBLE_DEV_DEPENDENCY, Group::CargoToml),
    (NO_SOURCE, Group::CargoToml),
    (FEATURE_VALUE, Group::CargoToml),
    (EDITION, Group::CargoToml),
    (MISSING_PACKAGE, Group::CargoToml),
    (UNKNOWN_CRATE, Group::CratesIo),
    (NO_MATCHING_VERSION, Group::CratesIo),
    (NEWER_VERSION, Group::CratesIo),
    ("unused_variables", Group::Rust),
    ("unused_imports", Group::Rust),
    ("unused_mut", Group::Rust),
    ("dead_code", Group::Rust),
    ("non_snake_case", Group::Rust),
    ("inactive-code", Group::Rust),
    ("unlinked-file", Group::Rust),
    ("unresolved-proc-macro", Group::Rust),
    ("clippy::*", Group::Rust),
];

pub const UNKNOWN_KEY: &str = "cargo-toml:unknown-key";
pub const INCOMPATIBLE_DEV_DEPENDENCY: &str = "cargo-toml:incompatible-dev-dependency";
pub const NO_SOURCE: &str = "cargo-toml:no-source";
pub const FEATURE_VALUE: &str = "cargo-toml:feature-value";
pub const EDITION: &str = "cargo-toml:edition";
pub const MISSING_PACKAGE: &str = "cargo-toml:missing-package";
pub const UNKNOWN_CRATE: &str = "crates-io:unknown-crate";
pub const NO_MATCHING_VERSION: &str = "crates-io:no-matching-version";
pub const NEWER_VERSION: &str = "crates-io:newer-version";

/// What Settings shows for an inspection: CrabCraft's own get a description, diagnostic codes
/// are shown as they are.
pub fn title(id: &str) -> String {
    match id {
        UNKNOWN_KEY => tr("Unknown key").into(),
        INCOMPATIBLE_DEV_DEPENDENCY => tr("Dev-dependency incompatible with the regular one").into(),
        NO_SOURCE => tr("Dependency without version, path, git or workspace source").into(),
        FEATURE_VALUE => tr("Feature that names a missing feature or dependency").into(),
        EDITION => tr("Unknown edition").into(),
        MISSING_PACKAGE => tr("Workspace member or path dependency without a package").into(),
        UNKNOWN_CRATE => tr("Crate not found on crates.io").into(),
        NO_MATCHING_VERSION => tr("No published version matches the requirement").into(),
        NEWER_VERSION => tr("Newer version outside the requirement").into(),
        "clippy::*" => tr("Every Clippy lint (clippy::*)").into(),
        other => other.to_string(),
    }
}

/// The name of a level in Settings.
pub fn level_name(level: Option<Level>) -> &'static str {
    match level {
        None => tr("Default"),
        Some(Level::Error) => tr("Error"),
        Some(Level::Warning) => tr("Warning"),
        Some(Level::WeakWarning) => tr("Weak Warning"),
        Some(Level::Info) => tr("Info"),
        Some(Level::Off) => tr("Off"),
    }
}

/// The severity a finding of inspection `id` gets under `profile` (`None`: turned off).
/// A hint stays a hint (rustc's "help" notes carry the code of the finding they explain) and
/// goes only when the inspection is off.
pub fn severity(profile: &InspectionProfile, id: Option<&str>, own: Severity) -> Option<Severity> {
    match id.and_then(|id| profile.level(id)) {
        None => Some(own),
        Some(Level::Off) => None,
        Some(_) if own == Severity::Hint => Some(Severity::Hint),
        Some(Level::Error) => Some(Severity::Error),
        Some(Level::Warning) => Some(Severity::Warning),
        Some(Level::WeakWarning) => Some(Severity::Info),
        Some(Level::Info) => Some(Severity::Hint),
    }
}

/// `list` under `profile`, each diagnostic judged by its code.
pub fn apply(profile: &InspectionProfile, list: Vec<Diagnostic>) -> Vec<Diagnostic> {
    if profile.levels.is_empty() {
        return list;
    }
    list.into_iter()
        .filter_map(|mut d| {
            d.severity = severity(profile, d.code.as_deref(), d.severity)?;
            Some(d)
        })
        .collect()
}

/// Diagnostics made by CrabCraft with their inspection id, under `profile`.
pub fn apply_own(profile: &InspectionProfile, list: &[(Diagnostic, &'static str)]) -> Vec<Diagnostic> {
    list.iter()
        .filter_map(|(d, id)| {
            let severity = severity(profile, Some(id), d.severity)?;
            Some(Diagnostic { severity, ..d.clone() })
        })
        .collect()
}

/// The inspection id of a `Cargo.toml` problem (I-22).
pub fn manifest_id(problem: &rc_cargo::inspect::Problem) -> &'static str {
    use rc_cargo::inspect::Problem as P;
    match problem {
        P::UnknownKey { .. } => UNKNOWN_KEY,
        P::DuplicateDependency { .. } => INCOMPATIBLE_DEV_DEPENDENCY,
        P::NoSource { .. } => NO_SOURCE,
        P::UnknownFeature { .. } | P::NotADependency { .. } | P::NotOptional { .. } | P::NoImplicitFeature { .. } => FEATURE_VALUE,
        P::InvalidEdition { .. } | P::EditionNotString => EDITION,
        P::MissingMember { .. } | P::MissingPathDependency { .. } => MISSING_PACKAGE,
    }
}

/// The project's choice of profile, and the drafts edited in Settings.
#[derive(Default)]
pub struct Profiles {
    /// The project's own profile (from its session, cleaned).
    pub project: InspectionProfile,
    /// The project uses `project` instead of the Default profile.
    pub use_project: bool,
    /// Settings › Inspections while it is open: (use the project's, the project's profile).
    pub draft: Option<(bool, InspectionProfile)>,
    /// The diagnostic code being typed in Settings › Inspections.
    pub new_code: String,
}

impl Profiles {
    /// The profile in force: the project's or the Default one.
    pub fn effective<'a>(&'a self, default: &'a InspectionProfile) -> &'a InspectionProfile {
        if self.use_project { &self.project } else { default }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diag(code: &str, severity: Severity) -> Diagnostic {
        Diagnostic { start: 0, end: 1, severity, message: code.into(), code: Some(code.into()), unnecessary: false, deprecated: false }
    }

    #[test]
    fn a_profile_changes_or_drops_findings_by_code() {
        let mut p = InspectionProfile::default();
        p.set("unused_variables", Some(Level::Off));
        p.set("dead_code", Some(Level::Error));
        p.set("clippy::*", Some(Level::WeakWarning));
        p.set("E0425", Some(Level::Info));
        let list = vec![
            diag("unused_variables", Severity::Warning),
            diag("unused_variables", Severity::Hint),
            diag("dead_code", Severity::Hint),
            diag("dead_code", Severity::Warning),
            diag("clippy::len_zero", Severity::Warning),
            diag("E0425", Severity::Error),
            diag("other", Severity::Warning),
        ];
        let out: Vec<(String, Severity)> = apply(&p, list).into_iter().map(|d| (d.message, d.severity)).collect();
        assert_eq!(
            out,
            [
                ("dead_code".to_string(), Severity::Hint),
                ("dead_code".to_string(), Severity::Error),
                ("clippy::len_zero".to_string(), Severity::Info),
                ("E0425".to_string(), Severity::Hint),
                ("other".to_string(), Severity::Warning)
            ]
        );
        // CrabCraft's own findings go by their inspection id, not their code.
        p.set(UNKNOWN_KEY, Some(Level::Off));
        p.set(NO_SOURCE, Some(Level::Error));
        let own = [
            (diag("Cargo.toml", Severity::Warning), UNKNOWN_KEY),
            (diag("Cargo.toml", Severity::Warning), NO_SOURCE),
            (diag("Cargo.toml", Severity::Warning), EDITION),
        ];
        let out: Vec<Severity> = apply_own(&p, &own).into_iter().map(|d| d.severity).collect();
        assert_eq!(out, [Severity::Error, Severity::Warning]);
    }

    #[test]
    fn the_project_profile_replaces_the_default_one_when_chosen() {
        let mut default = InspectionProfile::default();
        default.set("dead_code", Some(Level::Off));
        let mut profiles = Profiles::default();
        profiles.project.set("dead_code", Some(Level::Error));
        assert_eq!(profiles.effective(&default).level("dead_code"), Some(Level::Off));
        profiles.use_project = true;
        assert_eq!(profiles.effective(&default).level("dead_code"), Some(Level::Error));
    }
}
