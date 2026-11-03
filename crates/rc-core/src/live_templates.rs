//! Live templates: an abbreviation typed in a Rust file and expanded with Tab into a snippet
//! (`$1`, `${2:default}`, `$0`), editable in Settings › Live Templates. The default
//! descriptions are code, not prose, so they need no translation.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LiveTemplate {
    pub abbreviation: String,
    pub description: String,
    /// The snippet; four leading spaces are one indentation level.
    pub body: String,
}

fn t(abbreviation: &str, description: &str, body: &str) -> LiveTemplate {
    LiveTemplate { abbreviation: abbreviation.into(), description: description.into(), body: body.into() }
}

/// The templates a new installation starts with.
pub fn defaults() -> Vec<LiveTemplate> {
    vec![
        t("p", "println!", "println!(\"$1\");$0"),
        t("pd", "println!(\"{:?}\")", "println!(\"{:?}\", $1);$0"),
        t("main", "fn main", "fn main() {\n    $0\n}"),
        t("fn", "fn", "fn ${1:name}($2) {\n    $0\n}"),
        t("test", "#[test] fn", "#[test]\nfn ${1:name}() {\n    $0\n}"),
        t("tmod", "#[cfg(test)] mod tests", "#[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    fn ${1:name}() {\n        $0\n    }\n}"),
        t("st", "struct", "struct ${1:Name} {\n    $0\n}"),
        t("en", "enum", "enum ${1:Name} {\n    $0\n}"),
        t("impl", "impl", "impl ${1:Type} {\n    $0\n}"),
        t("ifl", "if let", "if let ${1:Some(value)} = ${2:option} {\n    $0\n}"),
        t("matchr", "match Ok/Err", "match ${1:result} {\n    Ok(${2:value}) => $3,\n    Err(${4:error}) => $0,\n}"),
    ]
}

/// Templates as the editor uses them: those with an abbreviation and a body.
pub fn for_editor(list: &[LiveTemplate]) -> Vec<(String, String)> {
    list.iter().filter(|t| !t.abbreviation.trim().is_empty() && !t.body.is_empty()).map(|t| (t.abbreviation.trim().to_string(), t.body.clone())).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn defaults_have_unique_abbreviations_and_bodies() {
        let list = super::defaults();
        let mut names: Vec<&str> = list.iter().map(|t| t.abbreviation.as_str()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), list.len());
        assert_eq!(super::for_editor(&list).len(), list.len());
        let mut blank = list.clone();
        blank[0].abbreviation = "  ".into();
        assert_eq!(super::for_editor(&blank).len(), list.len() - 1);
    }
}
