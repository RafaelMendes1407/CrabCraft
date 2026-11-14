//! Semantic tokens: decoding the server's relative encoding into per-line colour spans with
//! the editor's roles for standard LSP and rust-analyzer token types.

use rc_syntax::{Hl, Span};
use rc_text::Rope;

pub const CLIENT_TOKEN_TYPES: &[&str] = &[
    "namespace",
    "type",
    "class",
    "enum",
    "interface",
    "struct",
    "typeParameter",
    "parameter",
    "variable",
    "property",
    "enumMember",
    "event",
    "function",
    "method",
    "macro",
    "keyword",
    "modifier",
    "comment",
    "string",
    "number",
    "regexp",
    "operator",
    "decorator",
];
pub const CLIENT_TOKEN_MODIFIERS: &[&str] =
    &["declaration", "definition", "readonly", "static", "deprecated", "abstract", "async", "modification", "documentation", "defaultLibrary"];

/// Colour role of a token type name (`None`: leave the syntax colour).
pub fn role(token_type: &str) -> Option<Hl> {
    Some(match token_type {
        "class" | "type" => Hl::Class,
        "struct" => Hl::Struct,
        "enum" => Hl::Enum,
        "interface" => Hl::Interface,
        "typeParameter" => Hl::TypeParam,
        "namespace" => Hl::Namespace,
        "method" | "function" => Hl::Method,
        // rust-analyzer.
        "macro" | "derive" => Hl::Preproc,
        "builtinType" => Hl::Struct,
        "typeAlias" | "associatedType" => Hl::Class,
        "lifetime" | "constParameter" => Hl::TypeParam,
        "union" => Hl::Struct,
        "static" | "const" => Hl::Constant,
        "attribute" | "builtinAttribute" => Hl::Attribute,
        "property" => Hl::Property,
        "field" => Hl::Field,
        "constant" => Hl::Constant,
        "event" => Hl::Event,
        "enumMember" => Hl::EnumMember,
        "variable" => Hl::Local,
        "parameter" => Hl::Parameter,
        "string" => Hl::String,
        _ => return None,
    })
}

/// Decodes `data` (5 integers per token, positions relative to the previous token, columns in
/// UTF-16 units) into spans for each line of `rope`.
pub fn decode(data: &[u32], legend: &[String], rope: &Rope) -> Vec<Vec<Span>> {
    let lines = rope.len_lines();
    let mut out: Vec<Vec<Span>> = vec![Vec::new(); lines];
    let (mut line, mut start) = (0u32, 0u32);
    for t in data.as_chunks::<5>().0 {
        let (dl, ds, len, ty) = (t[0], t[1], t[2], t[3] as usize);
        line += dl;
        start = if dl == 0 { start + ds } else { ds };
        let l = line as usize;
        if l >= lines {
            break;
        }
        let Some(hl) = legend.get(ty).and_then(|n| role(n)) else { continue };
        let line_start = rope.line_to_char(l);
        let base = rope.char_to_utf16_cu(line_start);
        let line_len = rc_text::buffer::line_len(rope, l);
        let max_cu = rope.char_to_utf16_cu(line_start + line_len) - base;
        let a = (start as usize).min(max_cu);
        let b = (start as usize + len as usize).min(max_cu);
        let ca = rope.utf16_cu_to_char(base + a) - line_start;
        let cb = rope.utf16_cu_to_char(base + b) - line_start;
        if cb > ca {
            out[l].push(Span { start: ca, end: cb, hl });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_tokens_decode_to_char_spans() {
        let rope = Rope::from_str("var 😀x = Foo.Bar;\nint y;");
        let legend: Vec<String> = ["keyword", "variable", "class", "property"].iter().map(|s| s.to_string()).collect();
        // "Foo" at utf16 col 10 (the emoji is 2 units), "Bar" 4 later; line 1 "y" at col 4.
        let data = [0, 10, 3, 2, 0, 0, 4, 3, 3, 0, 1, 4, 1, 1, 0];
        let spans = decode(&data, &legend, &rope);
        assert_eq!(spans[0], vec![Span { start: 9, end: 12, hl: Hl::Class }, Span { start: 13, end: 16, hl: Hl::Property }]);
        assert_eq!(spans[1], vec![Span { start: 4, end: 5, hl: Hl::Local }]);
    }
}
