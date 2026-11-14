//! Handwritten bounded SQL token colors and Rust string injections. No database connection,
//! macros, project code or interpolated expression is evaluated by this module.
use crate::{Hl, Span};
use rc_text::Rope;
use std::ops::Range;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Node, Query, QueryCursor, Tree};
pub fn injected_role(role: Hl) -> Hl {
    match role {
        Hl::Keyword => Hl::SqlKeyword,
        Hl::String => Hl::SqlString,
        Hl::Number => Hl::SqlNumber,
        Hl::Comment => Hl::SqlComment,
        Hl::Parameter => Hl::SqlParameter,
        Hl::Punct | Hl::Operator => Hl::SqlOperator,
        Hl::Escape => Hl::SqlEscape,
        _ => Hl::SqlIdentifier,
    }
}
pub fn is_injected(role: Hl) -> bool {
    matches!(role, Hl::SqlKeyword | Hl::SqlString | Hl::SqlNumber | Hl::SqlComment | Hl::SqlIdentifier | Hl::SqlParameter | Hl::SqlOperator | Hl::SqlEscape)
}
pub const KEYWORDS: &[&str] = &[
    "SELECT",
    "FROM",
    "WHERE",
    "JOIN",
    "LEFT",
    "RIGHT",
    "INNER",
    "OUTER",
    "CROSS",
    "ON",
    "AS",
    "WITH",
    "RECURSIVE",
    "INSERT",
    "INTO",
    "VALUES",
    "UPDATE",
    "SET",
    "DELETE",
    "RETURNING",
    "CREATE",
    "TABLE",
    "VIEW",
    "INDEX",
    "TRIGGER",
    "ALTER",
    "DROP",
    "IF",
    "EXISTS",
    "NOT",
    "NULL",
    "IS",
    "AND",
    "OR",
    "IN",
    "BETWEEN",
    "LIKE",
    "ILIKE",
    "ESCAPE",
    "DISTINCT",
    "ALL",
    "GROUP",
    "BY",
    "ORDER",
    "HAVING",
    "LIMIT",
    "OFFSET",
    "ASC",
    "DESC",
    "UNION",
    "EXCEPT",
    "INTERSECT",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "PRIMARY",
    "KEY",
    "FOREIGN",
    "REFERENCES",
    "UNIQUE",
    "CHECK",
    "DEFAULT",
    "CASCADE",
    "CONFLICT",
    "DO",
    "NOTHING",
    "EXPLAIN",
    "ANALYZE",
    "PRAGMA",
    "BEGIN",
    "COMMIT",
    "ROLLBACK",
    "SAVEPOINT",
    "TRUE",
    "FALSE",
    "CAST",
    "OVER",
    "PARTITION",
    "WINDOW",
    "FILTER",
    "INTEGER",
    "REAL",
    "TEXT",
    "BLOB",
    "BOOLEAN",
    "VARCHAR",
    "JSON",
    "JSONB",
    "TIMESTAMP",
];
#[derive(Clone, Debug)]
pub struct Token {
    pub range: Range<usize>,
    pub role: Hl,
}
pub fn tokens(sql: &str) -> Vec<Token> {
    if sql.len() > 64 * 1024 {
        return Vec::new();
    }
    let bytes = sql.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() && out.len() < 16_384 {
        let start = i;
        let role = match bytes[i] {
            b if b.is_ascii_whitespace() => {
                i += 1;
                continue;
            }
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                Hl::Comment
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                let mut depth = 1_usize;
                while i < bytes.len() && depth > 0 {
                    if bytes.get(i..i + 2) == Some(b"/*") {
                        depth += 1;
                        i += 2;
                    } else if bytes.get(i..i + 2) == Some(b"*/") {
                        depth -= 1;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                Hl::Comment
            }
            b'\'' | b'"' | b'`' => {
                let quote = bytes[i];
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == quote {
                        if bytes.get(i + 1) == Some(&quote) {
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    i += 1;
                }
                if quote == b'\'' { Hl::String } else { Hl::Property }
            }
            b'$' => {
                let mut end = i + 1;
                while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
                    end += 1;
                }
                if bytes.get(end) == Some(&b'$') && (end == i + 1 || !bytes[i + 1].is_ascii_digit()) {
                    end += 1;
                    let tag = &sql[i..end];
                    i = sql[end..].find(tag).map_or(bytes.len(), |at| end + at + tag.len());
                    Hl::String
                } else {
                    i = end;
                    Hl::Parameter
                }
            }
            b'?' | b':' | b'@' => {
                i += 1;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                Hl::Parameter
            }
            b'0'..=b'9' => {
                i += 1;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || matches!(bytes[i], b'.' | b'_')) {
                    i += 1;
                }
                Hl::Number
            }
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                i += 1;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] >= 128) {
                    i += 1;
                }
                if KEYWORDS.iter().any(|word| word.eq_ignore_ascii_case(&sql[start..i])) { Hl::Keyword } else { Hl::Field }
            }
            b'(' | b')' | b',' | b';' | b'.' | b'[' | b']' => {
                i += 1;
                Hl::Punct
            }
            b'=' | b'>' | b'<' | b'!' | b'+' | b'-' | b'*' | b'/' | b'|' | b'&' | b'~' => {
                i += 1;
                Hl::Operator
            }
            _ => {
                let width = sql[i..].chars().next().map_or(1, char::len_utf8);
                i += width;
                Hl::Field
            }
        };
        out.push(Token { range: start..i, role });
    }
    out
}
#[derive(Clone, Debug)]
pub struct Injection {
    pub literal: Range<usize>,
    pub content: Range<usize>,
    pub sql: String,
    map: Vec<Range<usize>>,
}
impl Injection {
    pub fn source_range(&self, range: Range<usize>) -> Option<Range<usize>> {
        if range.start >= range.end {
            return None;
        }
        Some(self.map.get(range.start)?.start..self.map.get(range.end - 1)?.end)
    }
    pub fn sql_offset(&self, source: usize) -> Option<usize> {
        if source == self.content.end {
            return Some(self.sql.len());
        }
        self.map.iter().position(|range| range.start == source)
    }
}
fn node_text<'a>(node: Node<'_>, source: &'a str) -> Option<&'a str> {
    source.get(node.byte_range())
}
fn marker(node: Node<'_>, source: &str) -> bool {
    let mut current = Some(node);
    for _ in 0..8 {
        let Some(node) = current else { break };
        if let Some(previous) = node.prev_named_sibling()
            && matches!(previous.kind(), "line_comment" | "block_comment")
            && node_text(previous, source).is_some_and(|text| matches!(text.trim(), "// language=SQL" | "/* language=SQL */"))
        {
            return true;
        }
        if matches!(node.kind(), "block" | "function_item" | "source_file") {
            break;
        }
        current = node.parent();
    }
    false
}
fn context(node: Node<'_>, source: &str) -> bool {
    if marker(node, source) {
        return true;
    }
    let mut parent = node.parent();
    if parent.is_some_and(|node| node.kind() == "reference_expression") {
        parent = parent.and_then(|node| node.parent());
    }
    let Some(arguments) = parent else {
        return false;
    };
    if arguments.kind() == "token_tree" {
        let Some(invocation) = arguments.parent().filter(|node| node.kind() == "macro_invocation") else {
            return false;
        };
        let Some(name) = invocation.child_by_field_name("macro").and_then(|node| node_text(node, source)) else {
            return false;
        };
        let mut cursor = arguments.walk();
        let first = arguments.named_children(&mut cursor).find(|child| !matches!(child.kind(), "line_comment" | "block_comment"));
        return first.is_some_and(|first| first.id() == node.id()) && matches!(name, "sqlx::query" | "sqlx::query_as" | "sqlx::query_scalar");
    }
    if arguments.kind() != "arguments" {
        return false;
    }
    let mut cursor = arguments.walk();
    if arguments
        .named_children(&mut cursor)
        .find(|child| !matches!(child.kind(), "line_comment" | "block_comment"))
        .is_none_or(|first| first.id() != node.id() && first.child(0).is_none_or(|child| child.id() != node.id()))
    {
        return false;
    }
    let Some(function) = arguments.parent().and_then(|node| node.child_by_field_name("function")) else {
        return false;
    };
    if let Some(name) = node_text(function, source)
        && matches!(name, "sqlx::query" | "sqlx::query_as" | "sqlx::query_scalar" | "diesel::sql_query")
    {
        return true;
    }
    if function.kind() != "field_expression" {
        return false;
    }
    let method = function.child_by_field_name("field").and_then(|node| node_text(node, source));
    let receiver = function.child_by_field_name("value").and_then(|node| node_text(node, source));
    method.is_some_and(|method| {
        matches!(
            method,
            "prepare"
                | "prepare_cached"
                | "query"
                | "query_row"
                | "query_map"
                | "query_one"
                | "query_opt"
                | "query_raw"
                | "execute"
                | "execute_batch"
                | "batch_execute"
                | "simple_query"
        )
    }) && receiver.is_some_and(|receiver| matches!(receiver, "db" | "conn" | "connection" | "client" | "pool" | "tx" | "transaction"))
}
fn decode(node: Node<'_>, source: &str) -> Option<Injection> {
    if node.end_byte().saturating_sub(node.start_byte()) > 64 * 1024 || node.has_error() || !context(node, source) {
        return None;
    }
    let literal = node_text(node, source)?;
    let raw = literal.starts_with('r');
    if !raw && !literal.starts_with('"') {
        return None;
    }
    let first = literal.find('"')? + 1;
    let last = literal.rfind('"')?;
    if first > last {
        return None;
    }
    let content = node.start_byte() + first..node.start_byte() + last;
    let mut sql = String::new();
    let mut map = Vec::new();
    let mut i = content.start;
    while i < content.end {
        let start = i;
        let mut ch = source.get(i..content.end)?.chars().next()?;
        i += ch.len_utf8();
        if !raw && ch == '\\' {
            let escape = *source.as_bytes().get(i)?;
            i += 1;
            ch = match escape {
                b'n' => '\n',
                b'r' => '\r',
                b't' => '\t',
                b'0' => '\0',
                b'\\' => '\\',
                b'"' => '"',
                b'\'' => '\'',
                b'x' => {
                    let end = i.checked_add(2)?;
                    if end > content.end {
                        return None;
                    }
                    let value = u8::from_str_radix(source.get(i..end)?, 16).ok()?;
                    if value >= 128 {
                        return None;
                    }
                    i = end;
                    value as char
                }
                b'u' => {
                    if source.as_bytes().get(i) != Some(&b'{') {
                        return None;
                    }
                    i += 1;
                    let end = source.get(i..content.end)?.find('}')? + i;
                    let value: String = source[i..end].chars().filter(|ch| *ch != '_').collect();
                    if value.len() > 6 {
                        return None;
                    }
                    let value = u32::from_str_radix(&value, 16).ok()?;
                    i = end + 1;
                    char::from_u32(value)?
                }
                b'\n' => {
                    while i < content.end && source[i..].chars().next().is_some_and(char::is_whitespace) {
                        i += source[i..].chars().next()?.len_utf8();
                    }
                    continue;
                }
                b'\r' if source.as_bytes().get(i) == Some(&b'\n') => {
                    i += 1;
                    while i < content.end && source[i..].chars().next().is_some_and(char::is_whitespace) {
                        i += source[i..].chars().next()?.len_utf8();
                    }
                    continue;
                }
                _ => return None,
            };
        }
        sql.push(ch);
        for _ in 0..ch.len_utf8() {
            map.push(start..i);
        }
    }
    // Context heuristics never classify an ordinary non-SQL string as SQL solely by API name.
    if !marker(node, source) && !sql.trim().is_empty() {
        let first = tokens(&sql).into_iter().find(|token| token.role != Hl::Comment)?;
        let keyword = sql.get(first.range)?;
        if !KEYWORDS.iter().any(|word| word.eq_ignore_ascii_case(keyword) || (keyword.len() >= 2 && word.starts_with(&keyword.to_ascii_uppercase()))) {
            return None;
        }
    }
    Some(Injection { literal: node.byte_range(), content, sql, map })
}
pub fn at(tree: &Tree, rope: &Rope, byte: usize) -> Option<Injection> {
    if byte > rope.len_bytes() || rope.len_bytes() > 2 * 1024 * 1024 {
        return None;
    }
    let mut node = tree.root_node().descendant_for_byte_range(byte, byte)?;
    for _ in 0..16 {
        if matches!(node.kind(), "string_literal" | "raw_string_literal") {
            let source = rope.to_string();
            return decode(node, &source);
        }
        node = node.parent()?;
    }
    None
}
/// SQL roles mapped to original Rust source bytes. Work is restricted to visible nodes.
pub fn injected(tree: &Tree, rope: &Rope, range: Range<usize>) -> Vec<Token> {
    if rope.len_bytes() > 2 * 1024 * 1024 {
        return Vec::new();
    }
    static QUERY: std::sync::OnceLock<Option<Query>> = std::sync::OnceLock::new();
    let Some(query) = QUERY
        .get_or_init(|| crate::Lang::Rust.language().and_then(|language| Query::new(&language, "[(string_literal) (raw_string_literal)] @sql").ok()))
        .as_ref()
    else {
        return Vec::new();
    };
    let source = rope.to_string();
    let mut cursor = QueryCursor::new();
    cursor.set_byte_range(range.clone());
    let mut captures = cursor.captures(query, tree.root_node(), source.as_bytes());
    let mut out = Vec::new();
    let mut strings = 0;
    while let Some((found, index)) = captures.next() {
        if strings >= 16 {
            break;
        }
        let node = found.captures[*index].node;
        if let Some(injection) = decode(node, &source) {
            strings += 1;
            out.push(Token { range: injection.content.clone(), role: Hl::Plain });
            for token in tokens(&injection.sql) {
                if let Some(mapped) = injection.source_range(token.range)
                    && mapped.start < range.end
                    && mapped.end > range.start
                {
                    out.push(Token { range: mapped, role: token.role });
                }
            }
        }
    }
    out
}
/// Encode a completion for this original Rust literal; never close raw-string delimiters.
pub fn encode(injection: &Injection, rope: &Rope, value: &str) -> Option<String> {
    let prefix = rope.byte_slice(injection.literal.start..injection.content.start).to_string();
    if prefix.starts_with('r') {
        let closing = rope.byte_slice(injection.content.end..injection.literal.end).to_string();
        (!value.contains(&closing)).then(|| value.to_string())
    } else {
        let mut escaped = String::new();
        for ch in value.chars() {
            match ch {
                '\\' => escaped.push_str("\\\\"),
                '"' => escaped.push_str("\\\""),
                '\n' => escaped.push_str("\\n"),
                '\r' => escaped.push_str("\\r"),
                '\t' => escaped.push_str("\\t"),
                '\0' => escaped.push_str("\\0"),
                ch if ch.is_control() => escaped.push_str(&format!("\\u{{{:x}}}", ch as u32)),
                ch => escaped.push(ch),
            }
        }
        Some(escaped)
    }
}
/// Standalone console colors. Ranges use character columns, matching the native editor.
pub fn lines(rope: &Rope, first: usize, count: usize) -> Vec<Vec<Span>> {
    let last = first.saturating_add(count).min(rope.len_lines());
    let source = rope.to_string();
    let tokens = tokens(&source);
    let mut out = Vec::new();
    for line in first..last {
        let start = rope.line_to_byte(line);
        let end = if line + 1 < rope.len_lines() { rope.line_to_byte(line + 1) } else { rope.len_bytes() };
        out.push(
            tokens
                .iter()
                .filter_map(|token| {
                    let a = token.range.start.max(start);
                    let b = token.range.end.min(end);
                    (a < b).then(|| Span {
                        start: rope.byte_to_char(a) - rope.line_to_char(line),
                        end: rope.byte_to_char(b) - rope.line_to_char(line),
                        hl: token.role,
                    })
                })
                .collect(),
        );
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Lang, parse_now};
    #[test]
    fn raw_and_escaped_rust_sql_map_back_without_touching_other_strings() {
        let source = r###"// language=SQL
const Q:&str=r##"SELECT "🦀" FROM users WHERE id=$1 AND name='O''Brien';"##;
fn f(){conn.query_row("SELECT name\nFROM users WHERE name=\"value\"",[]);println!("SELECT ordinary prose");conn.execute("GET /path",[]);}
"###;
        let rope = Rope::from_str(source);
        let tree = parse_now(Lang::Rust, &rope, None).unwrap();
        let raw = at(&tree, &rope, source.find("FROM users").unwrap()).unwrap();
        assert!(raw.sql.contains("🦀"));
        let normal = at(&tree, &rope, source.find("name\\n").unwrap()).unwrap();
        assert!(normal.sql.contains("name\nFROM"));
        assert!(normal.sql.contains("name=\"value\""));
        let mapped = injected(&tree, &rope, 0..source.len());
        assert!(mapped.iter().any(|token| token.role == Hl::Keyword && source.get(token.range.clone()) == Some("SELECT")));
        let prose = source.find("SELECT ordinary").unwrap();
        assert!(at(&tree, &rope, prose).is_none());
        assert!(at(&tree, &rope, source.find("GET /path").unwrap()).is_none());
    }
    #[test]
    fn sql_tokens_keep_comments_strings_parameters_and_keywords_separate() {
        let sql = "-- SELECT hidden\nSELECT $tag$FROM$tag$, 'JOIN', :name, $1, ?2 FROM users /* WHERE /* nested */ x */";
        let list = tokens(sql);
        assert_eq!(list.iter().filter(|token| token.role == Hl::Keyword).map(|token| &sql[token.range.clone()]).collect::<Vec<_>>(), ["SELECT", "FROM"]);
        assert_eq!(list.iter().filter(|token| token.role == Hl::Parameter).count(), 3);
        assert!(tokens(&"x".repeat(64 * 1024 + 1)).is_empty());
    }
}

#[cfg(test)]
mod literal_encoding_tests {
    use super::*;
    #[test]
    fn completion_encoding_keeps_rust_delimiters_and_utf8_valid() {
        for source in ["// language=SQL\nconst Q:&str=r#\"SELECT x\"#;", "// language=SQL\nconst Q:&str=\"SELECT x\";"] {
            let rope = Rope::from_str(source);
            let tree = crate::parse_now(crate::Lang::Rust, &rope, None).unwrap();
            let injection = at(&tree, &rope, source.find("SELECT").unwrap()).unwrap();
            if source.contains("r#\"") {
                assert!(encode(&injection, &rope, "danger\"#name").is_none());
            } else {
                assert_eq!(encode(&injection, &rope, "\"🦀\"\n").unwrap(), "\\\"🦀\\\"\\n");
            }
        }
        for text in ["🦀 SELECT café FROM ü", "SELECT $тег$é$тег$", "'unterminated 🦀", "/* nested /* 🦀 */", "SELECT :नाम,?1,$1"] {
            for token in tokens(text) {
                assert!(text.get(token.range).is_some());
            }
        }
    }
}
