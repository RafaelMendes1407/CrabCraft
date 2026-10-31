//! Run Anything (Ctrl, Ctrl): what the text typed so far can run.

/// Commands offered before the user types anything of their own.
const COMMON: [&str; 8] = ["cargo build", "cargo check", "cargo test", "cargo clippy", "cargo run", "cargo fmt", "cargo doc", "cargo update"];

/// How many commands are remembered.
pub const HISTORY: usize = 10;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    /// A run configuration, by its index.
    Config(usize),
    /// A Cargo command line.
    Command(String),
}

/// Everything that matches `text`: what was typed, recent commands, run configurations and common
/// commands, without repeating a command.
pub fn items(text: &str, history: &[String], configs: &[String]) -> Vec<Item> {
    let typed = text.trim();
    let needle = typed.to_lowercase();
    let matches = |candidate: &str| candidate.to_lowercase().contains(&needle);
    let mut out: Vec<Item> = Vec::new();
    let push = |item: Item, out: &mut Vec<Item>| {
        if !out.contains(&item) {
            out.push(item);
        }
    };
    if !typed.is_empty() {
        push(Item::Command(typed.to_string()), &mut out);
    }
    for command in history.iter().filter(|c| matches(c)) {
        push(Item::Command(command.clone()), &mut out);
    }
    for (i, _) in configs.iter().enumerate().filter(|(_, name)| matches(name)) {
        push(Item::Config(i), &mut out);
    }
    for command in COMMON.iter().filter(|c| matches(c)) {
        push(Item::Command((*command).to_string()), &mut out);
    }
    out
}

/// `history` with `command` first, without duplicates and within the limit.
pub fn remember(history: &mut Vec<String>, command: &str) {
    history.retain(|c| c != command);
    history.insert(0, command.to_string());
    history.truncate(HISTORY);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(text: &str) -> Item {
        Item::Command(text.to_string())
    }

    #[test]
    fn empty_text_offers_recent_commands_configurations_and_common_ones() {
        let list = items("", &["cargo tree".to_string()], &["app".to_string()]);
        assert_eq!(list[..3], [command("cargo tree"), Item::Config(0), command("cargo build")]);
        assert_eq!(list.len(), 2 + COMMON.len());
    }

    #[test]
    fn typed_text_runs_as_it_is_and_filters_the_rest() {
        let list = items(" TES ", &["cargo test --doc".to_string()], &["tester".to_string(), "app".to_string()]);
        assert_eq!(list, [command("TES"), command("cargo test --doc"), Item::Config(0), command("cargo test")]);
    }

    #[test]
    fn a_command_is_not_offered_twice() {
        let list = items("cargo test", &["cargo test".to_string()], &[]);
        assert_eq!(list, [command("cargo test")]);
    }

    #[test]
    fn history_keeps_the_latest_commands_first() {
        let mut history: Vec<String> = (0..HISTORY).map(|i| format!("c{i}")).collect();
        remember(&mut history, "c3");
        assert_eq!((history[0].as_str(), history.len()), ("c3", HISTORY));
        remember(&mut history, "new");
        assert_eq!((history[0].as_str(), history[1].as_str(), history.len()), ("new", "c3", HISTORY));
    }
}
