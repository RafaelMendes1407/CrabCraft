//! One macro_rules layer. This parser runs only in the bounded private helper process.
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
pub const INPUT_BYTES: usize = 256 * 1024;
pub const OUTPUT_BYTES: usize = 1024 * 1024;
pub const SOURCE_BYTES: usize = 16 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Input {
    pub rules: String,
    pub arguments: String,
    pub definition_edition: u16,
    pub call_edition: u16,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Error {
    Invalid,
    NoMatch,
    Limit,
    Unsupported,
    Io,
    Cancelled,
    Timeout,
    Stale,
    Analyzer,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Reply {
    pub result: Result<String, Error>,
}
fn edition(edition: u16) -> Result<ra_ap_span::Edition, Error> {
    use ra_ap_span::Edition;
    match edition {
        2015 => Ok(Edition::Edition2015),
        2018 => Ok(Edition::Edition2018),
        2021 => Ok(Edition::Edition2021),
        2024 => Ok(Edition::Edition2024),
        _ => Err(Error::Unsupported),
    }
}
fn expand(input: Input) -> Result<String, Error> {
    use ra_ap_span::{EditionedFileId, ErasedFileAstId, FileId, Span, SpanAnchor, SyntaxContext};
    use ra_ap_tt::{TextRange, TextSize};
    if input.rules.len() > SOURCE_BYTES || input.arguments.len() > SOURCE_BYTES {
        return Err(Error::Limit);
    }
    let definition_edition = edition(input.definition_edition)?;
    let call_edition = edition(input.call_edition)?;
    let definition_anchor = SpanAnchor {
        file_id: EditionedFileId::new(FileId::from_raw(0), definition_edition),
        ast_id: ErasedFileAstId::from_raw(0),
    };
    let call_anchor = SpanAnchor {
        file_id: EditionedFileId::new(FileId::from_raw(1), call_edition),
        ast_id: ErasedFileAstId::from_raw(0),
    };
    let definition_context = SyntaxContext::root(definition_edition);
    let call_context = SyntaxContext::root(call_edition);
    let definition = ra_ap_syntax_bridge::parse_to_token_tree(
        definition_edition,
        definition_anchor,
        definition_context,
        &input.rules,
    )
    .ok_or(Error::Invalid)?;
    let arguments = ra_ap_syntax_bridge::parse_to_token_tree(
        call_edition,
        call_anchor,
        call_context,
        &input.arguments,
    )
    .ok_or(Error::Invalid)?;
    if definition.token_trees().len() > 8192 || arguments.token_trees().len() > 8192 {
        return Err(Error::Limit);
    }
    let macro_ =
        ra_ap_mbe::DeclarativeMacro::parse_macro_rules(&definition, |_| definition_edition);
    if macro_.err().is_some() {
        return Err(Error::Invalid);
    }
    let result = macro_.expand(
        &arguments,
        |_| {},
        Span {
            range: TextRange::up_to(TextSize::new(input.arguments.len() as u32)),
            anchor: call_anchor,
            ctx: call_context,
        },
        definition_edition,
    );
    if result.err.is_some() {
        return Err(Error::NoMatch);
    }
    if result.value.0.token_trees().len() > 65536 {
        return Err(Error::Limit);
    }
    // The token renderer preserves opaque expression-fragment precedence with parentheses.
    let text = result.value.0.view().to_string();
    if text.len() > OUTPUT_BYTES {
        return Err(Error::Limit);
    }
    Ok(text)
}

#[cfg(unix)]
fn memory_budget() -> Result<(), Error> {
    // A malformed/pathological macro may exhaust this child, never the desktop process.
    let bytes: u64 = 2 * 1024 * 1024 * 1024;
    let mut current = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: both syscalls access this valid local structure and only this process's limits.
    if unsafe { libc::getrlimit(libc::RLIMIT_AS, &mut current) } != 0 {
        return Err(Error::Unsupported);
    }
    let ceiling = (bytes as libc::rlim_t).min(current.rlim_max);
    let limit = libc::rlimit {
        rlim_cur: ceiling.min(current.rlim_cur),
        rlim_max: current.rlim_max,
    };
    if unsafe { libc::setrlimit(libc::RLIMIT_AS, &limit) } != 0 {
        return Err(Error::Unsupported);
    }
    let mut cpu = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    if unsafe { libc::getrlimit(libc::RLIMIT_CPU, &mut cpu) } != 0 {
        return Err(Error::Unsupported);
    }
    let ceiling = 10.min(cpu.rlim_max);
    let limit = libc::rlimit {
        rlim_cur: ceiling.min(cpu.rlim_cur),
        rlim_max: cpu.rlim_max,
    };
    if unsafe { libc::setrlimit(libc::RLIMIT_CPU, &limit) } != 0 {
        return Err(Error::Unsupported);
    }
    Ok(())
}
#[cfg(not(unix))]
fn memory_budget() -> Result<(), Error> {
    Err(Error::Unsupported)
}
fn run() -> Result<String, Error> {
    memory_budget()?;
    let mut bytes = Vec::new();
    std::io::stdin()
        .lock()
        .take((INPUT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Io)?;
    if bytes.len() > INPUT_BYTES {
        return Err(Error::Limit);
    }
    let input = serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)?;
    expand(input)
}
/// Private CLI mode is checked before settings, instance discovery or GUI initialization.
pub fn internal() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--internal-macro-step")) {
        return None;
    }
    let result = if args.next().is_some() {
        Err(Error::Invalid)
    } else {
        run()
    };
    let reply = Reply { result };
    Some(
        match serde_json::to_writer(std::io::stdout().lock(), &reply).and_then(|()| {
            std::io::stdout()
                .lock()
                .write_all(b"\n")
                .map_err(serde_json::Error::io)
        }) {
            Ok(()) => 0,
            Err(_) => 1,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn layer(rules: &str, arguments: &str) -> String {
        expand(Input {
            rules: rules.into(),
            arguments: arguments.into(),
            definition_edition: 2024,
            call_edition: 2024,
        })
        .unwrap()
    }
    #[test]
    fn one_layer_retains_nested_builtin_recursive_metavariable_and_repetition_calls() {
        assert!(layer("($e:expr)=>{add!($e,$e)}", "21").contains("add"));
        let builtin = layer("()=>{include!(\"never-read.rs\")}", "");
        assert!(builtin.contains("include"));
        assert!(builtin.contains("never-read.rs"));
        assert!(layer("($e:expr)=>{recur!($e)}", "21").contains("recur"));
        assert!(layer("($m:ident,$e:expr)=>{$m!($e)}", "inner,41").contains("inner"));
        let repeated = layer("($($e:expr),*)=>{vec![$($e),*]}", "1,2,3");
        assert!(repeated.contains("vec"));
        assert!(repeated.contains('3'));
        let precedence = layer("($e:expr)=>{$e * 2}", "1+2");
        assert!(precedence.starts_with("(1 + 2)"), "{precedence}");
        assert!(matches!(
            expand(Input {
                rules: "($x:ident)=>{$x}".into(),
                arguments: "42".into(),
                definition_edition: 2024,
                call_edition: 2024
            }),
            Err(Error::NoMatch)
        ));
        assert!(matches!(
            expand(Input {
                rules: "bad".into(),
                arguments: String::new(),
                definition_edition: 2024,
                call_edition: 2024
            }),
            Err(Error::Invalid)
        ));
    }
}
