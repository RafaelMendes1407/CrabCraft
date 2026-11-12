//! Highlighting of a range of lines from a syntax tree.

use crate::lang::Lang;
use rc_text::Rope;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Node, QueryCursor, Tree};

/// Colour role of a piece of text. The editor theme maps each to a colour and style; LSP
/// semantic tokens map onto the same roles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum Hl {
    #[default]
    Plain,
    Keyword,
    String,
    Escape,
    Number,
    Comment,
    DocComment,
    Class,
    Interface,
    Struct,
    Enum,
    Delegate,
    TypeParam,
    Namespace,
    Method,
    Field,
    Property,
    Event,
    Constant,
    EnumMember,
    Parameter,
    Local,
    Operator,
    Punct,
    Attribute,
    Preproc,
    Tag,
    XmlAttr,
    Entity,
    Heading,
    Code,
    Link,
    /// Text the language server reports as unused or inactive.
    Dimmed,
    /// SQL injected in a Rust literal; these roles outrank whole-string LSP tokens.
    SqlKeyword, SqlString, SqlNumber, SqlComment, SqlIdentifier, SqlParameter, SqlOperator, SqlEscape,
}

impl Hl {
    /// Role for a capture name of the highlight queries.
    pub fn from_capture(name: &str) -> Option<Hl> {
        Some(match name {
            "none" => Hl::Plain,
            "variable" | "variable.local" | "field.maybe" => return None,
            "keyword" => Hl::Keyword,
            "string" | "string.brace" => Hl::String,
            "string.escape" => Hl::Escape,
            "number" => Hl::Number,
            "comment" => Hl::Comment,
            "comment.doc" => Hl::DocComment,
            "type" | "type.class" => Hl::Class,
            "type.interface" => Hl::Interface,
            "type.struct" => Hl::Struct,
            "type.enum" => Hl::Enum,
            "type.delegate" => Hl::Delegate,
            "type.parameter" => Hl::TypeParam,
            "namespace" => Hl::Namespace,
            "function" | "function.call" => Hl::Method,
            "field" => Hl::Field,
            "property" => Hl::Property,
            "event" => Hl::Event,
            "constant" => Hl::Constant,
            "constant.enum" => Hl::EnumMember,
            "variable.parameter" => Hl::Parameter,
            "operator" => Hl::Operator,
            "punctuation" | "punctuation.bracket" => Hl::Punct,
            "attribute" => Hl::Attribute,
            "xml.attribute" => Hl::XmlAttr,
            "preproc" | "preproc.arg" => Hl::Preproc,
            "tag" => Hl::Tag,
            "entity" => Hl::Entity,
            "heading" => Hl::Heading,
            "code" => Hl::Code,
            "link" => Hl::Link,
            _ => return None,
        })
    }
}

/// A coloured run within one line, in *char* columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub hl: Hl,
}

/// Highlights lines `first..first + count` of `rope`: one span list per line, covering only
/// the coloured parts (the rest is `Plain`).
pub fn highlight_lines(lang: Lang, tree: &Tree, rope: &Rope, first: usize, count: usize) -> Vec<Vec<Span>> {
    let last = (first + count).min(rope.len_lines());
    if first >= last {
        return Vec::new();
    }
    let Some(query) = lang.query() else { return vec![Vec::new(); last - first] };
    let start_byte = rope.line_to_byte(first);
    let end_byte = if last < rope.len_lines() { rope.line_to_byte(last) } else { rope.len_bytes() };
    let mut roles = vec![None::<(usize, u32, Hl)>; end_byte - start_byte];
    let names = query.capture_names();

    let mut cursor = QueryCursor::new();
    cursor.set_byte_range(start_byte..end_byte);
    let text = |node: Node| {
        let (s, e) = (node.start_byte().min(rope.len_bytes()), node.end_byte().min(rope.len_bytes()));
        rope.byte_slice(s..e).chunks().map(str::as_bytes)
    };
    let mut captures = cursor.captures(query, tree.root_node(), text);
    // (byte len, pattern) decide precedence: smaller nodes, then later patterns win.
    while let Some((m, index)) = captures.next() {
        let cap = m.captures[*index];
        let Some(hl) = Hl::from_capture(names[cap.index as usize]) else { continue };
        let node = cap.node;
        let (s, e) = (node.start_byte().max(start_byte), node.end_byte().min(end_byte));
        if s >= e {
            continue;
        }
        let size = node.end_byte() - node.start_byte();
        let pattern = m.pattern_index as u32;
        for slot in &mut roles[s - start_byte..e - start_byte] {
            let wins = match slot {
                None => true,
                Some((sz, pat, _)) => size < *sz || (size == *sz && pattern >= *pat),
            };
            if wins {
                *slot = Some((size, pattern, hl));
            }
        }
    }

    if lang == Lang::Rust {
        for token in crate::sql::injected(tree, rope, start_byte..end_byte) {
            let start=token.range.start.max(start_byte)-start_byte;
            let end=token.range.end.min(end_byte)-start_byte;
            for slot in &mut roles[start..end] {
                if token.role==Hl::Plain { if slot.is_some_and(|(_,_,role)|role==Hl::Escape){*slot=Some((0,u32::MAX,Hl::SqlEscape));}continue; }
                let role=if slot.is_some_and(|(_,_,role)| matches!(role,Hl::Escape|Hl::SqlEscape)){Hl::SqlEscape}else{crate::sql::injected_role(token.role)}; *slot=Some((0,u32::MAX,role));
            }
        }
    }

    // Byte roles → char spans per line.
    let mut out = Vec::with_capacity(last - first);
    for line in first..last {
        let lb = rope.line_to_byte(line) - start_byte;
        let slice = rope.line(line);
        let mut spans: Vec<Span> = Vec::new();
        let mut byte = lb;
        for (col, ch) in slice.chars().enumerate() {
            if ch == '\n' {
                break;
            }
            let hl = roles.get(byte).copied().flatten().map_or(Hl::Plain, |r| r.2);
            match spans.last_mut() {
                Some(sp) if sp.hl == hl && sp.end == col => sp.end = col + 1,
                _ => spans.push(Span { start: col, end: col + 1, hl }),
            }
            byte += ch.len_utf8();
        }
        spans.retain(|s| s.hl != Hl::Plain);
        out.push(spans);
    }
    out
}

/// Highlighting of HTTP request files; `None` for grammar-backed languages.
pub fn highlight_lexical(lang: Lang, rope: &Rope, first: usize, count: usize) -> Option<Vec<Vec<Span>>> {
    if lang == Lang::Http {
        // Lines above `first` tell whether it is a header or the body.
        let last = (first + count).min(rope.len_lines());
        let floor = first.saturating_sub(crate::http::MAX_LOOK_BACK);
        let from = (floor..=first.min(last)).rev().find(|&l| l < rope.len_lines() && rope.line(l).chars().take(3).eq("###".chars())).unwrap_or(floor);
        let lines: Vec<String> = (from..last).map(|l| rope.line(l).to_string()).collect();
        return Some(crate::http::highlight(&lines, first - from, last.saturating_sub(first)));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_now;

    fn roles_of(lang: Lang, src: &str) -> Vec<(String, Hl)> {
        let rope = Rope::from_str(src);
        let tree = parse_now(lang, &rope, None).unwrap();
        let lines = highlight_lines(lang, &tree, &rope, 0, rope.len_lines());
        let mut out = Vec::new();
        for (i, spans) in lines.iter().enumerate() {
            let line: Vec<char> = rope.line(i).chars().collect();
            for s in spans {
                out.push((line[s.start..s.end].iter().collect(), s.hl));
            }
        }
        out
    }

    #[test]
    fn web_assets_in_a_rust_project_parse_and_highlight() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/RustWeb");
        let assets = [
            ("static/index.html", Lang::Html, "main", Hl::Tag),
            ("static/app.css", Lang::Css, "color", Hl::Property),
            ("static/app.js", Lang::JavaScript, "const", Hl::Keyword),
            ("static/app.ts", Lang::TypeScript, "interface", Hl::Keyword),
            ("static/view.tsx", Lang::Tsx, "string", Hl::Class),
        ];
        for (file, lang, token, role) in assets {
            let path = root.join(file);
            assert_eq!(Lang::from_path(&path), lang);
            let src = std::fs::read_to_string(path).unwrap();
            let rope = Rope::from_str(&src);
            let tree = crate::parse_now(lang, &rope, None).unwrap();
            assert!(!tree.root_node().has_error(), "{file}: {}", tree.root_node().to_sexp());
            assert!(lang.query().is_some(), "invalid query for {lang:?}");
            let roles = roles_of(lang, &src);
            assert!(roles.iter().any(|(text, hl)| text == token && *hl == role), "{file}: {roles:?}");
            assert!(!crate::folding_ranges(lang, &tree, &rope).is_empty(), "no folds for {file}");
        }
    }

    #[test]
    fn rust_roles() {
        let src = "use std::fmt;\n/// Doc\n#[derive(Debug)]\npub struct Point<'a, T> { x: &'a T }\nenum Kind { One }\ntrait Shape { fn area(&self) -> f64; }\nconst MAX: u32 = 1;\nfn main() {\n    let mut p = make(1.5);\n    println!(\"hi\\n\");\n    p.area();\n    // done\n}\n";
        let roles = roles_of(Lang::Rust, src);
        let has = |text: &str, hl: Hl| roles.iter().any(|(t, h)| t == text && *h == hl);
        assert!(has("use", Hl::Keyword), "{roles:?}");
        assert!(has("std", Hl::Namespace), "{roles:?}");
        assert!(roles.iter().any(|(t, h)| t.starts_with("/// Doc") && *h == Hl::DocComment), "{roles:?}");
        assert!(roles.iter().any(|(t, h)| t.starts_with('#') && *h == Hl::Attribute), "{roles:?}");
        assert!(has("Point", Hl::Struct), "{roles:?}");
        assert!(has("Kind", Hl::Enum), "{roles:?}");
        assert!(has("One", Hl::EnumMember), "{roles:?}");
        assert!(has("Shape", Hl::Interface), "{roles:?}");
        assert!(has("T", Hl::TypeParam), "{roles:?}");
        assert!(has("MAX", Hl::Constant), "{roles:?}");
        assert!(has("main", Hl::Method) && has("make", Hl::Method) && has("area", Hl::Method), "{roles:?}");
        assert!(has("println!", Hl::Preproc), "{roles:?}");
        assert!(has("mut", Hl::Keyword), "{roles:?}");
        assert!(has("1.5", Hl::Number), "{roles:?}");
        assert!(has("\\n", Hl::Escape), "{roles:?}");
        assert!(roles.iter().any(|(t, h)| t.starts_with("// done") && *h == Hl::Comment), "{roles:?}");
    }

    #[test]
    fn toml_roles() {
        let src = "# comment\n[package]\nname = \"demo\"\nedition = 2024\n\n[dependencies.serde]\noptional = true\n";
        let roles = roles_of(Lang::Toml, src);
        let has = |text: &str, hl: Hl| roles.iter().any(|(t, h)| t == text && *h == hl);
        assert!(has("# comment", Hl::Comment), "{roles:?}");
        assert!(has("package", Hl::Struct), "{roles:?}");
        assert!(has("name", Hl::Field), "{roles:?}");
        assert!(has("\"demo\"", Hl::String), "{roles:?}");
        assert!(has("2024", Hl::Number), "{roles:?}");
        assert!(has("true", Hl::Keyword), "{roles:?}");
        assert!(has("dependencies", Hl::Struct) && has("serde", Hl::Struct), "{roles:?}");
    }

    #[test]
    fn xml_roles() {
        let roles = roles_of(Lang::Xml, "<project name=\"sample\">\n  <!-- c -->\n</project>\n");
        assert!(roles.iter().any(|(t, h)| t == "name" && *h == Hl::XmlAttr), "{roles:?}");
        assert!(roles.iter().any(|(t, h)| t == "\"sample\"" && *h == Hl::String), "{roles:?}");
        assert!(roles.iter().any(|(t, h)| t == "<!-- c -->" && *h == Hl::Comment), "{roles:?}");
    }

}
