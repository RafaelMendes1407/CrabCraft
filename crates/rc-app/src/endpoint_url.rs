//! Pure, bounded URL completion contexts. No source reads, analysis requests or network I/O.
use std::ops::Range;
#[derive(Debug, Clone)]
pub enum Kind {
    Path {
        prefix: String,
    },
    Query {
        path: String,
        prefix: String,
        used: Vec<String>,
        has_equal: bool,
    },
}
#[derive(Debug, Clone)]
pub struct Context {
    pub range: Range<usize>,
    pub kind: Kind,
}
pub fn context(value: &str, caret: usize) -> Option<Context> {
    if value.len() > 8192
        || caret > value.len()
        || !value.is_char_boundary(caret)
        || value.chars().any(char::is_control)
    {
        return None;
    }
    let start = if value.starts_with('/') || value.is_empty() {
        0
    } else {
        let Some(scheme) = value.find("://") else {
            return relative_context(value, caret);
        };
        if !matches!(&value[..scheme], "http" | "https") {
            return None;
        }
        let authority = scheme + 3;
        let end = value[authority..]
            .find(['/', '?', '#'])
            .map_or(value.len(), |offset| authority + offset);
        if authority == end {
            return None;
        }
        end
    };
    let path_end = value[start..]
        .find(['?', '#'])
        .map_or(value.len(), |offset| start + offset);
    if caret >= start && caret <= path_end {
        return Some(Context {
            range: start..path_end,
            kind: Kind::Path {
                prefix: value[start..caret].into(),
            },
        });
    }
    if value.as_bytes().get(path_end) != Some(&b'?') {
        return None;
    }
    let query_start = path_end + 1;
    let query_end = value[query_start..]
        .find('#')
        .map_or(value.len(), |offset| query_start + offset);
    if caret < query_start || caret > query_end {
        return None;
    }
    let name_start = value[query_start..caret]
        .rfind('&')
        .map_or(query_start, |offset| query_start + offset + 1);
    let pair_end = value[caret..query_end]
        .find('&')
        .map_or(query_end, |offset| caret + offset);
    let name_end = value[name_start..pair_end]
        .find('=')
        .map_or(pair_end, |offset| name_start + offset);
    if caret > name_end {
        return None;
    }
    let prefix = &value[name_start..caret];
    if !prefix
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return None;
    }
    let other = format!(
        "{}{}",
        &value[query_start..name_start],
        &value[pair_end..query_end]
    );
    let parsed = reqwest::Url::parse(&format!("http://localhost/?{other}")).ok()?;
    let used = parsed
        .query_pairs()
        .map(|(name, _)| name.into_owned())
        .take(128)
        .collect();
    Some(Context {
        range: name_start..name_end,
        kind: Kind::Query {
            path: value[start..path_end].into(),
            prefix: prefix.into(),
            used,
            has_equal: name_end < pair_end,
        },
    })
}
fn relative_context(value: &str, caret: usize) -> Option<Context> {
    let start = value.find(['/', '?', '#']).unwrap_or(value.len());
    let mut result = context(&value[start..], caret.checked_sub(start)?)?;
    result.range.start += start;
    result.range.end += start;
    Some(result)
}
pub fn matches(route: &str, path: &str) -> bool {
    if route.len() > 4096 || path.len() > 4096 {
        return false;
    }
    let mut actual = path.split('/');
    for part in route.split('/') {
        if part.starts_with("{*") && part.ends_with('}') || part.starts_with('*') {
            return actual.next().is_some();
        }
        let Some(value) = actual.next() else {
            return false;
        };
        if (part.starts_with('{') && part.ends_with('}')) || part.starts_with(':') {
            if value.is_empty() {
                return false;
            }
        } else if part != value {
            return false;
        }
    }
    actual.next().is_none()
}
pub struct Literal {
    pub value: String,
    map: Vec<Range<usize>>,
    pub content: Range<usize>,
}
impl Literal {
    pub fn offset(&self, source: usize) -> Option<usize> {
        if source == self.content.end {
            return Some(self.value.len());
        }
        self.map.iter().position(|range| range.start == source)
    }
    pub fn source_range(&self, range: Range<usize>) -> Option<Range<usize>> {
        if range.start > range.end {
            return None;
        }
        let start = if range.start == self.value.len() {
            self.content.end
        } else {
            self.map.get(range.start)?.start
        };
        let end = if range.start == range.end {
            start
        } else {
            self.map.get(range.end.checked_sub(1)?)?.end
        };
        Some(start..end)
    }
}
pub fn literal(
    rope: &rc_text::Rope,
    tree: &rc_syntax::tree_sitter::Tree,
    at: usize,
) -> Option<Literal> {
    if rope.len_bytes() > 2 * 1024 * 1024 || at > rope.len_bytes() {
        return None;
    }
    let mut node = tree.root_node().descendant_for_byte_range(at, at)?;
    for _ in 0..16 {
        if matches!(node.kind(), "string_literal" | "raw_string_literal") {
            break;
        }
        node = node.parent()?;
    }
    if !matches!(node.kind(), "string_literal" | "raw_string_literal")
        || node.has_error()
        || node.end_byte() - node.start_byte() > 8192
    {
        return None;
    }
    let source = rope.to_string();
    let token = source.get(node.byte_range())?;
    let raw = node.kind() == "raw_string_literal";
    if !raw && !token.starts_with('"') {
        return None;
    }
    let first = token.find('"')? + 1;
    let last = token.rfind('"')?;
    if first > last {
        return None;
    }
    let content = node.start_byte() + first..node.start_byte() + last;
    if at < content.start || at > content.end {
        return None;
    }
    let mut value = String::new();
    let mut map = Vec::new();
    let mut cursor = content.start;
    while cursor < content.end {
        let start = cursor;
        let ch = source[cursor..content.end].chars().next()?;
        cursor += ch.len_utf8();
        let decoded = if !raw && ch == '\\' {
            let escape = *source.as_bytes().get(cursor)?;
            cursor += 1;
            match escape {
                b'x' => cursor = cursor.checked_add(2)?,
                b'u' => {
                    if source.as_bytes().get(cursor) != Some(&b'{') {
                        return None;
                    }
                    cursor = source[cursor..content.end].find('}')? + cursor + 1;
                }
                b'\n' | b'\r' => return None,
                _ => {}
            }
            if cursor > content.end {
                return None;
            }
            rc_syntax::axum::string_value(&format!("\"{}\"", source.get(start..cursor)?), false)?
        } else {
            ch.to_string()
        };
        for _ in 0..decoded.len() {
            map.push(start..cursor);
        }
        value.push_str(&decoded);
    }
    if !value.starts_with("http://") && !value.starts_with("https://") {
        return None;
    }
    Some(Literal {
        value,
        map,
        content,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn query_context_refuses_values_fragments_and_keeps_other_parameters() {
        let c = context(
            "https://host/users/42?q=hello&pa=9#fragment",
            "https://host/users/42?q=hello&pa".len(),
        )
        .unwrap();
        match c.kind {
            Kind::Query {
                path,
                prefix,
                used,
                has_equal,
            } => {
                assert_eq!(path, "/users/42");
                assert_eq!(prefix, "pa");
                assert_eq!(used, ["q"]);
                assert!(has_equal);
            }
            _ => panic!(),
        };
        assert!(context("/users?q=hello", 12).is_none());
        assert!(context("/users?q=1#fragment", 16).is_none());
        assert!(matches("/users/{id}", "/users/42"));
        assert!(!matches("/users/{id}", "/users/42/extra"));
    }
    #[test]
    fn rust_url_maps_unicode_and_escapes_without_touching_delimiters() {
        let source = "fn main(){let préfix=\"https://host/\\u{7528}户?pa\";}";
        let rope = rc_text::Rope::from_str(source);
        let tree = rc_syntax::parse_now(rc_syntax::Lang::Rust, &rope, None).unwrap();
        let at = source.find("?pa").unwrap() + 3;
        let literal = literal(&rope, &tree, at).unwrap();
        let caret = literal.offset(at).unwrap();
        let c = context(&literal.value, caret).unwrap();
        assert_eq!(&source[literal.source_range(c.range).unwrap()], "pa");
        assert!(literal.value.contains("用户"));
        let raw = "fn main(){let x=r##\"http://host/us\"##;}";
        let rope = rc_text::Rope::from_str(raw);
        let tree = rc_syntax::parse_now(rc_syntax::Lang::Rust, &rope, None).unwrap();
        assert!(super::literal(&rope, &tree, raw.find("/us").unwrap() + 3).is_some());
    }
}
