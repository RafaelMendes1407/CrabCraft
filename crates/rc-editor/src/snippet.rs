//! Snippets with tab stops: live templates and the language server's completion snippets.
//!
//! The syntax is LSP's: `$1`, `${2:default}`, `$0` for the final caret, `\$` for a dollar.
//! After insertion Tab moves to the next stop and Shift+Tab back; Escape, or Tab past the last
//! stop, ends it.

/// A parsed snippet: the text to insert and its stops, in visiting order (`$0` last), as
/// `(start, end)` char offsets into the text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snippet {
    pub text: String,
    pub stops: Vec<(usize, usize)>,
}

/// The stops of a snippet being filled in, as offsets into the document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnippetSession {
    pub stops: Vec<(usize, usize)>,
    pub current: usize,
}

pub fn parse(s: &str) -> Snippet {
    let mut out: Vec<char> = Vec::new();
    // (number, start, end); a number seen again (a mirror) keeps its first place.
    let mut found: Vec<(u32, usize, usize)> = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let number = |i: &mut usize| {
        let mut n = 0u32;
        while *i < chars.len() && chars[*i].is_ascii_digit() {
            n = n.saturating_mul(10).saturating_add(chars[*i].to_digit(10).unwrap_or(0));
            *i += 1;
        }
        n
    };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() && matches!(chars[i + 1], '$' | '}' | '\\') {
            out.push(chars[i + 1]);
            i += 2;
            continue;
        }
        if c == '$' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit() {
            i += 1;
            let n = number(&mut i);
            found.push((n, out.len(), out.len()));
            continue;
        }
        if c == '$' && i + 2 < chars.len() && chars[i + 1] == '{' && chars[i + 2].is_ascii_digit() {
            i += 2;
            let n = number(&mut i);
            let start = out.len();
            if i < chars.len() && chars[i] == ':' {
                i += 1;
                let mut depth = 1;
                while i < chars.len() {
                    match chars[i] {
                        '\\' if i + 1 < chars.len() => {
                            out.push(chars[i + 1]);
                            i += 2;
                            continue;
                        }
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    out.push(chars[i]);
                    i += 1;
                }
            }
            // Past the closing brace.
            i += 1;
            found.push((n, start, out.len()));
            continue;
        }
        out.push(c);
        i += 1;
    }
    let mut stops: Vec<(u32, usize, usize)> = Vec::new();
    for f in found {
        if !stops.iter().any(|s| s.0 == f.0) {
            stops.push(f);
        }
    }
    // 1, 2, 3… then 0.
    stops.sort_by_key(|s| if s.0 == 0 { u32::MAX } else { s.0 });
    Snippet { text: out.into_iter().collect(), stops: stops.into_iter().map(|(_, s, e)| (s, e)).collect() }
}

/// Fits a multi-line template to where it goes: lines after the first get the indentation of
/// the line it is inserted on, and each leading group of four spaces becomes `unit`.
pub fn reindent(body: &str, line_indent: &str, unit: &str) -> String {
    let mut out = String::with_capacity(body.len());
    for (i, line) in body.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
            let spaces = line.len() - line.trim_start_matches(' ').len();
            if !line.trim().is_empty() {
                out.push_str(line_indent);
            }
            out.push_str(&unit.repeat(spaces / 4));
            out.push_str(&" ".repeat(spaces % 4));
            out.push_str(&line[spaces..]);
        } else {
            out.push_str(line);
        }
    }
    out
}

/// The live template whose abbreviation ends right before `caret` on its line, as
/// `(start of the abbreviation, body)`. The abbreviation must be a whole word.
pub fn template_at<'a>(line_before_caret: &str, caret: usize, templates: &'a [(String, String)]) -> Option<(usize, &'a str)> {
    let word: String = line_before_caret.chars().rev().take_while(|c| c.is_alphanumeric() || *c == '_').collect::<Vec<_>>().into_iter().rev().collect();
    if word.is_empty() {
        return None;
    }
    let (_, body) = templates.iter().find(|(abbr, _)| *abbr == word)?;
    Some((caret - word.chars().count(), body.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stops_are_found_in_order_with_the_final_one_last() {
        let s = parse("fn ${1:name}($2) {\n    $0\n}");
        assert_eq!(s.text, "fn name() {\n    \n}");
        assert_eq!(s.stops, vec![(3, 7), (8, 8), (16, 16)]);
        let s = parse("println!(\"${2:x}\", ${1:a});$0");
        assert_eq!(s.text, "println!(\"x\", a);");
        assert_eq!(s.stops, vec![(14, 15), (10, 11), (17, 17)]);
    }

    #[test]
    fn escapes_mirrors_and_nested_braces() {
        let s = parse("\\$HOME ${1:a{b}} $1 \\}");
        assert_eq!(s.text, "$HOME a{b}  }");
        assert_eq!(s.stops, vec![(6, 10)], "a mirror keeps the first place");
        assert_eq!(parse("plain $ text").stops, Vec::<(usize, usize)>::new());
        assert_eq!(parse("${").text, "${");
        assert_eq!(parse("unclosed ${1:abc").text, "unclosed abc");
    }

    #[test]
    fn templates_follow_the_indentation_where_they_go() {
        assert_eq!(reindent("if x {\n    y\n}", "    ", "\t"), "if x {\n    \ty\n    }");
        assert_eq!(reindent("a\n\n  b", "  ", "  "), "a\n\n    b");
    }

    #[test]
    fn a_template_is_found_by_the_word_before_the_caret() {
        let t = vec![("p".to_string(), "println!($1);".to_string()), ("test".to_string(), "#[test]".to_string())];
        assert_eq!(template_at("    p", 9, &t), Some((8, "println!($1);")));
        assert_eq!(template_at("x.test", 6, &t), Some((2, "#[test]")));
        assert_eq!(template_at("    ap", 10, &t), None, "only a whole word");
        assert_eq!(template_at("    ", 4, &t), None);
    }
}
