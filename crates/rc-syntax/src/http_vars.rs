//! Bounded HTTP file-variable substitution, without environment access or script evaluation.
use std::collections::BTreeMap;
pub const MAX_EXPANDED: usize = 1024 * 1024;
const MAX_VARIABLES: usize = 256;
const MAX_DEPTH: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VariableError {
    Undefined(String),
    Cycle(String),
    Limit,
    Unclosed,
}

pub struct Resolver<'a> {
    variables: &'a BTreeMap<String, String>,
    cache: BTreeMap<String, String>,
    stack: Vec<String>,
    cached_bytes: usize,
}
impl<'a> Resolver<'a> {
    pub fn new(variables: &'a BTreeMap<String, String>) -> Self {
        Self { variables, cache: BTreeMap::new(), stack: Vec::new(), cached_bytes: 0 }
    }
    pub fn variable(&mut self, name: &str) -> Result<String, VariableError> {
        if self.variables.len() > MAX_VARIABLES || self.stack.len() >= MAX_DEPTH {
            return Err(VariableError::Limit);
        }
        if let Some(value) = self.cache.get(name) {
            return Ok(value.clone());
        }
        if self.stack.iter().any(|item| item == name) {
            return Err(VariableError::Cycle(name.into()));
        }
        let raw = self.variables.get(name).ok_or_else(|| VariableError::Undefined(name.into()))?;
        self.stack.push(name.into());
        let result = self.expand(raw);
        self.stack.pop();
        let expanded = result?;
        if expanded.len() > MAX_EXPANDED.saturating_sub(self.cached_bytes) {
            return Err(VariableError::Limit);
        }
        self.cached_bytes += expanded.len();
        self.cache.insert(name.into(), expanded.clone());
        Ok(expanded)
    }
    pub fn expand(&mut self, text: &str) -> Result<String, VariableError> {
        let mut rest = text;
        let mut output = String::new();
        let push = |out: &mut String, value: &str| {
            if value.len() > MAX_EXPANDED.saturating_sub(out.len()) {
                return Err(VariableError::Limit);
            }
            out.push_str(value);
            Ok(())
        };
        while let Some(start) = rest.find("{{") {
            push(&mut output, &rest[..start])?;
            let after = &rest[start + 2..];
            let end = after.find("}}").ok_or(VariableError::Unclosed)?;
            let name = after[..end].trim();
            let value = self.variable(name)?;
            push(&mut output, &value)?;
            rest = &after[end + 2..];
        }
        push(&mut output, rest)?;
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nested_unicode_variables_are_literal_and_whitespace_is_allowed() {
        let vars = BTreeMap::from([("host".into(), "localhost".into()), ("url".into(), "http://{{host}}/café".into())]);
        let mut resolver = Resolver::new(&vars);
        assert_eq!(resolver.expand("{{ url }}?q={{host}}").unwrap(), "http://localhost/café?q=localhost");
        assert_eq!(resolver.expand("$HOME").unwrap(), "$HOME");
        assert!(matches!(resolver.expand("{{unknown}}"), Err(VariableError::Undefined(_))));
    }
    #[test]
    fn cycles_depth_and_exponential_expansion_are_bounded() {
        let mut vars = BTreeMap::from([("a".into(), "{{b}}".into()), ("b".into(), "{{a}}".into())]);
        assert!(matches!(Resolver::new(&vars).expand("{{a}}"), Err(VariableError::Cycle(_))));
        vars.clear();
        vars.insert("v0".into(), "x".repeat(1024));
        for n in 1..20 {
            vars.insert(format!("v{n}"), format!("{{{{v{}}}}}{{{{v{}}}}}", n - 1, n - 1));
        }
        assert_eq!(Resolver::new(&vars).expand("{{v19}}"), Err(VariableError::Limit));
        assert_eq!(Resolver::new(&vars).expand("{{unclosed"), Err(VariableError::Unclosed));
        assert!(Resolver::new(&BTreeMap::new()).expand(&"x".repeat(MAX_EXPANDED + 1)).is_err());
    }
}
