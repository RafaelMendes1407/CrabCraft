//! `cargo new`: the New Project dialog's templates.

use crate::toolchain::Cargo;
use std::path::{Path, PathBuf};
use tokio::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Template {
    Binary,
    Library,
    /// A `proc-macro = true` library with a derive macro and an integration test.
    ProcMacro,
    /// A `cdylib` + `rlib` library with a `#[wasm_bindgen]` function.
    WebAssembly,
}

/// The target WebAssembly libraries are built for.
pub const WASM_TARGET: &str = "wasm32-unknown-unknown";

/// What the WebAssembly template needs that is not part of Cargo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WasmTools {
    /// The standard library for [`WASM_TARGET`] is installed.
    pub target: bool,
    pub wasm_pack: bool,
}

impl WasmTools {
    /// Looks for the target (from the home folder, so no project's `rust-toolchain` applies) and
    /// for `wasm-pack`. Installs nothing. This blocks.
    pub fn detect(cargo: &Cargo) -> WasmTools {
        let dir = crate::toolchain::home().filter(|h| h.is_dir()).unwrap_or_else(std::env::temp_dir);
        WasmTools { target: crate::toolchain::has_target(cargo, &dir, WASM_TARGET), wasm_pack: crate::toolchain::find_tool("wasm-pack").is_some() }
    }
}

/// What to create.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewProject {
    pub template: Template,
    pub name: String,
    /// The folder that receives the project's folder.
    pub location: PathBuf,
    pub git: bool,
}

/// What Cargo accepts as a new package name, and is not a Rust keyword.
pub fn valid_name(name: &str) -> bool {
    const RESERVED: [&str; 12] = ["as", "crate", "fn", "impl", "let", "mod", "self", "struct", "super", "test", "type", "use"];
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic()) && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') && !RESERVED.contains(&name)
}

impl NewProject {
    pub fn project_dir(&self) -> PathBuf {
        self.location.join(&self.name)
    }

    /// Why the project cannot be created, as a text to translate.
    pub fn problem(&self) -> Option<&'static str> {
        if !valid_name(&self.name) {
            Some("Use letters, digits, - and _, starting with a letter")
        } else if !self.location.is_dir() {
            Some("The location does not exist")
        } else if self.project_dir().exists() {
            Some("That folder already exists")
        } else {
            None
        }
    }

    /// Arguments after `cargo`.
    pub fn args(&self) -> Vec<String> {
        let kind = match self.template {
            Template::Binary => "--bin",
            Template::Library | Template::ProcMacro | Template::WebAssembly => "--lib",
        };
        vec![
            "new".into(),
            kind.into(),
            "--vcs".into(),
            if self.git { "git" } else { "none" }.into(),
            "--name".into(),
            self.name.clone(),
            self.project_dir().to_string_lossy().into_owned(),
        ]
    }

    /// Creates the project and returns its folder. The folder is removed if Cargo fails half way.
    pub async fn create(&self, cargo: &Cargo) -> anyhow::Result<PathBuf> {
        if let Some(p) = self.problem() {
            anyhow::bail!("{p}");
        }
        let dir = self.project_dir();
        let mut cmd: Command = cargo.command();
        let out = cmd.args(self.args()).current_dir(&self.location).output().await?;
        if !out.status.success() {
            if dir.is_dir() && !was_left_with_content(&dir) {
                let _ = std::fs::remove_dir(&dir);
            }
            anyhow::bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
        }
        self.scaffold(&dir).map_err(|e| anyhow::anyhow!("{}: {e}", dir.display()))?;
        Ok(dir)
    }

    /// Turns the library `cargo new` made into the template: the manifest's `[lib]` and
    /// dependencies, and the sample sources.
    fn scaffold(&self, dir: &Path) -> std::io::Result<()> {
        let crate_name = self.name.replace('-', "_");
        let (lib, deps, files): (&str, &[&str], Vec<(&str, String)>) = match self.template {
            Template::Binary | Template::Library => return Ok(()),
            Template::ProcMacro => (
                "proc-macro = true",
                &["proc-macro2 = \"1\"", "quote = \"1\"", "syn = \"2\""],
                vec![("src/lib.rs", PROC_MACRO_LIB.to_string()), ("tests/describe.rs", PROC_MACRO_TEST.replace("{crate}", &crate_name))],
            ),
            Template::WebAssembly => ("crate-type = [\"cdylib\", \"rlib\"]", &["wasm-bindgen = \"0.2\""], vec![("src/lib.rs", WASM_LIB.to_string())]),
        };
        let manifest = dir.join("Cargo.toml");
        let text = std::fs::read_to_string(&manifest)?;
        std::fs::write(&manifest, with_lib_and_dependencies(&text, lib, deps))?;
        for (path, content) in files {
            let path = dir.join(path);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, content)?;
        }
        Ok(())
    }
}

/// `manifest` with a `[lib]` section holding `lib`, and `deps` under `[dependencies]` (the
/// section `cargo new` leaves empty at the end; it is added when missing).
fn with_lib_and_dependencies(manifest: &str, lib: &str, deps: &[&str]) -> String {
    let deps = deps.iter().map(|d| format!("{d}\n")).collect::<String>();
    let lib = format!("[lib]\n{lib}\n\n");
    let mut out = String::with_capacity(manifest.len() + lib.len() + deps.len());
    let mut done = false;
    for line in manifest.split_inclusive('\n') {
        if !done && line.trim() == "[dependencies]" {
            out.push_str(&lib);
            out.push_str(line);
            if !line.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&deps);
            done = true;
        } else {
            out.push_str(line);
        }
    }
    if !done {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
        out.push_str(&lib);
        out.push_str("[dependencies]\n");
        out.push_str(&deps);
    }
    out
}

const PROC_MACRO_LIB: &str = r#"//! Procedural macros. `#[derive(Describe)]` gives a struct `type_name()` and `field_names()`.
//!
//! A proc-macro crate can only export macros: use them from another crate, as the test in
//! `tests/` does.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Data, DeriveInput, Fields, parse_macro_input};

/// Adds `type_name()` and `field_names()` to a struct.
#[proc_macro_derive(Describe)]
pub fn derive_describe(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand(&input).unwrap_or_else(syn::Error::into_compile_error).into()
}

/// The code the derive adds, or an error pointing at what it cannot handle.
fn expand(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(&input.ident, "Describe can only be derived for structs"));
    };
    let fields: Vec<String> = match &data.fields {
        Fields::Named(named) => named.named.iter().filter_map(|f| f.ident.as_ref()).map(|i| i.to_string()).collect(),
        Fields::Unnamed(unnamed) => (0..unnamed.unnamed.len()).map(|i| i.to_string()).collect(),
        Fields::Unit => Vec::new(),
    };
    let name = &input.ident;
    let type_name = name.to_string();
    let (impl_generics, type_generics, where_clause) = input.generics.split_for_impl();
    Ok(quote! {
        impl #impl_generics #name #type_generics #where_clause {
            /// The name of the type, as written in the source.
            pub fn type_name() -> &'static str {
                #type_name
            }

            /// The names of the fields, in order; tuple fields are numbered.
            pub fn field_names() -> &'static [&'static str] {
                &[#(#fields),*]
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enums_are_rejected() {
        let input: DeriveInput = syn::parse_quote!(
            enum Direction {
                Up,
                Down,
            }
        );
        assert!(expand(&input).is_err());
    }
}
"#;

const PROC_MACRO_TEST: &str = r#"//! Uses the derive macro from another crate, as its users will.

use {crate}::Describe;

#[derive(Describe)]
struct Point {
    x: i32,
    y: i32,
}

#[derive(Describe)]
struct Pair<T>(T, T);

#[derive(Describe)]
struct Empty;

#[test]
fn describes_named_fields() {
    let p = Point { x: 1, y: 2 };
    assert_eq!(p.x + p.y, 3);
    assert_eq!(Point::type_name(), "Point");
    assert_eq!(Point::field_names(), ["x", "y"]);
}

#[test]
fn describes_tuple_and_unit_structs() {
    let pair = Pair(1, 2);
    assert_eq!(pair.0 + pair.1, 3);
    assert_eq!(Pair::<u8>::field_names(), ["0", "1"]);
    let _ = Empty;
    assert!(Empty::field_names().is_empty());
}
"#;

const WASM_LIB: &str = r#"//! A library for WebAssembly. `#[wasm_bindgen]` items can be called from JavaScript.
//!
//! Check it with `cargo check --target wasm32-unknown-unknown` (after
//! `rustup target add wasm32-unknown-unknown`), and package it for the web with
//! `wasm-pack build --target web`. The tests run natively with `cargo test`.

use wasm_bindgen::prelude::*;

/// A greeting for `name`; from JavaScript: `greet("Ferris")`.
#[wasm_bindgen]
pub fn greet(name: &str) -> String {
    format!("Hello, {name}!")
}

/// The sum of two numbers, without overflow.
#[wasm_bindgen]
pub fn add(left: u32, right: u32) -> u64 {
    u64::from(left) + u64::from(right)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greets() {
        assert_eq!(greet("Ferris"), "Hello, Ferris!");
    }

    #[test]
    fn adds_without_overflow() {
        assert_eq!(add(u32::MAX, 1), u64::from(u32::MAX) + 1);
    }
}
"#;

fn was_left_with_content(dir: &Path) -> bool {
    std::fs::read_dir(dir).map_or(true, |mut e| e.next().is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(location: &Path, name: &str) -> NewProject {
        NewProject { template: Template::Library, name: name.into(), location: location.to_path_buf(), git: false }
    }

    #[test]
    fn names_follow_cargo() {
        for ok in ["demo", "my-app", "my_app2", "a"] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in ["", "2fast", "-a", "a b", "café", "self", "../x", "a/b"] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn problems_are_found_before_creating() {
        let dir = tempfile::tempdir().unwrap();
        assert!(request(dir.path(), "1bad").problem().is_some());
        assert!(request(&dir.path().join("missing"), "ok").problem().is_some());
        std::fs::create_dir(dir.path().join("taken")).unwrap();
        assert!(request(dir.path(), "taken").problem().is_some());
        assert_eq!(request(dir.path(), "free").problem(), None);
    }

    #[test]
    fn creates_a_real_project() {
        let dir = tempfile::tempdir().unwrap();
        let cargo = Cargo::locate().expect("cargo is installed");
        let lib = request(dir.path(), "made_here");
        let path = rc_core::Runtime::block_on(lib.create(&cargo)).unwrap();
        assert!(path.join("src/lib.rs").is_file() && path.join("Cargo.toml").is_file());
        assert!(!path.join(".git").exists());
        let bin = NewProject { template: Template::Binary, name: "made_too".into(), location: dir.path().to_path_buf(), git: true };
        let path = rc_core::Runtime::block_on(bin.create(&cargo)).unwrap();
        assert!(path.join("src/main.rs").is_file() && path.join(".git").exists());
        assert!(rc_core::Runtime::block_on(bin.create(&cargo)).is_err(), "the folder exists now");
    }

    #[test]
    fn the_manifest_gets_lib_and_dependencies() {
        let made = "[package]\nname = \"m\"\nedition = \"2024\"\n\n[dependencies]\n";
        let out = with_lib_and_dependencies(made, "proc-macro = true", &["syn = \"2\"", "quote = \"1\""]);
        assert_eq!(out, "[package]\nname = \"m\"\nedition = \"2024\"\n\n[lib]\nproc-macro = true\n\n[dependencies]\nsyn = \"2\"\nquote = \"1\"\n");
        let out = with_lib_and_dependencies("[package]\nname = \"m\"", "crate-type = [\"cdylib\"]", &["wasm-bindgen = \"0.2\""]);
        assert_eq!(out, "[package]\nname = \"m\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n\n[dependencies]\nwasm-bindgen = \"0.2\"\n");
        let parsed: toml::Table = toml::from_str(&out).unwrap();
        assert!(parsed["lib"]["crate-type"].is_array() && parsed["dependencies"]["wasm-bindgen"].is_str());
    }

    /// Runs `cargo <args>` in `dir`, offline when the crates are already downloaded. `None` when
    /// it could not run because a crate is missing from the local registry.
    fn cargo_in(cargo: &Cargo, dir: &Path, args: &[&str]) -> Option<std::process::Output> {
        let out = cargo.std_command().args(args).arg("--offline").current_dir(dir).output().unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        if !out.status.success() && (err.contains("offline") || err.contains("no matching package")) {
            eprintln!("skipped: the dependencies are not in the local Cargo registry: {err}");
            return None;
        }
        assert!(out.status.success(), "cargo {args:?}: {err}");
        Some(out)
    }

    #[test]
    fn creates_a_procedural_macro_that_builds_and_passes_its_test() {
        let dir = tempfile::tempdir().unwrap();
        let cargo = Cargo::locate().expect("cargo is installed");
        let req = NewProject { template: Template::ProcMacro, name: "my-derive".into(), location: dir.path().to_path_buf(), git: false };
        let path = rc_core::Runtime::block_on(req.create(&cargo)).unwrap();
        let manifest: toml::Table = toml::from_str(&std::fs::read_to_string(path.join("Cargo.toml")).unwrap()).unwrap();
        assert_eq!(manifest["lib"]["proc-macro"].as_bool(), Some(true));
        for dep in ["syn", "quote", "proc-macro2"] {
            assert!(manifest["dependencies"].get(dep).is_some(), "{dep}");
        }
        assert!(std::fs::read_to_string(path.join("tests/describe.rs")).unwrap().contains("use my_derive::Describe;"));
        if cargo_in(&cargo, &path, &["check", "--all-targets"]).is_none() {
            return;
        }
        let out = cargo_in(&cargo, &path, &["test"]).unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("describes_named_fields ... ok") && text.contains("enums_are_rejected ... ok"), "{text}");
    }

    /// Strict validation for P-08, unlike the optional target probe in the native test.
    #[test]
    #[ignore = "Requires wasm32-unknown-unknown and cached wasm-bindgen dependencies"]
    fn webassembly_template_checks_on_the_real_wasm_target() {
        let dir = tempfile::tempdir().unwrap();
        let cargo = Cargo::locate().expect("cargo is installed");
        assert!(WasmTools::detect(&cargo).target, "install wasm32-unknown-unknown first");
        let req = NewProject { template: Template::WebAssembly, name: "web_verified".into(), location: dir.path().to_path_buf(), git: false };
        let path = rc_core::Runtime::block_on(req.create(&cargo)).unwrap();
        cargo_in(&cargo, &path, &["check", "--target", WASM_TARGET]).expect("wasm-bindgen dependencies must be cached");
    }

    #[test]
    fn creates_a_webassembly_library_that_builds() {
        let dir = tempfile::tempdir().unwrap();
        let cargo = Cargo::locate().expect("cargo is installed");
        let req = NewProject { template: Template::WebAssembly, name: "web_lib".into(), location: dir.path().to_path_buf(), git: false };
        let path = rc_core::Runtime::block_on(req.create(&cargo)).unwrap();
        let manifest: toml::Table = toml::from_str(&std::fs::read_to_string(path.join("Cargo.toml")).unwrap()).unwrap();
        let kinds: Vec<_> = manifest["lib"]["crate-type"].as_array().unwrap().iter().filter_map(|v| v.as_str()).collect();
        assert_eq!(kinds, ["cdylib", "rlib"]);
        assert!(manifest["dependencies"].get("wasm-bindgen").is_some());
        if cargo_in(&cargo, &path, &["test"]).is_none() {
            return;
        }
        let tools = WasmTools::detect(&cargo);
        if tools.target {
            cargo_in(&cargo, &path, &["check", "--target", WASM_TARGET]);
        } else {
            eprintln!("skipped: the {WASM_TARGET} target is not installed");
        }
    }
}
