//! Bounded embedded web highlighting computed on the parser worker, never while painting.
use crate::{Hl, Lang, Span};
use rc_text::Rope;
use tree_sitter::Tree;
#[derive(Clone, Debug)]
pub struct EmbeddedSpan {
    pub start: usize,
    pub end: usize,
    pub role: Hl,
}
pub fn collect(lang: Lang, tree: &Tree, rope: &Rope) -> Vec<EmbeddedSpan> {
    if lang != Lang::Html {
        return Vec::new();
    }
    let mut stack = vec![tree.root_node()];
    let mut result = Vec::new();
    let mut bytes = 0;
    let mut regions = 0;
    let mut nodes = 0;
    while let Some(node) = stack.pop() {
        nodes += 1;
        if nodes > 100_000 {
            break;
        }
        if node.kind() == "raw_text" {
            let Some(parent) = node.parent() else {
                continue;
            };
            let embedded = match parent.kind() {
                "style_element" => Lang::Css,
                "script_element" => Lang::JavaScript,
                _ => continue,
            };
            let start = node.start_byte();
            let end = node.end_byte();
            if end > rope.len_bytes() || end - start > 64 * 1024 {
                continue;
            }
            bytes += end - start;
            regions += 1;
            if bytes > 256 * 1024 || regions > 32 {
                break;
            }
            let prefix = rope
                .byte_slice(parent.start_byte()..start)
                .chars()
                .take(4096)
                .collect::<String>()
                .to_ascii_lowercase();
            if prefix.contains("application/json")
                || prefix.contains("application/ld+json")
                || prefix.contains("text/template")
            {
                continue;
            }
            let embedded = if embedded == Lang::JavaScript
                && (prefix.contains("typescript")
                    || prefix.contains("lang=\"ts\"")
                    || prefix.contains("lang='ts'"))
            {
                Lang::TypeScript
            } else {
                embedded
            };
            let body = Rope::from_str(&rope.byte_slice(start..end).to_string());
            let Some(tree) = crate::parse_now(embedded, &body, None) else {
                continue;
            };
            let offset = rope.byte_to_char(start);
            let lines = crate::highlight_lines(embedded, &tree, &body, 0, body.len_lines());
            for (line, spans) in lines.into_iter().enumerate() {
                let base = offset + body.line_to_char(line);
                for span in spans {
                    if result.len() >= 16384 {
                        return result;
                    }
                    result.push(EmbeddedSpan {
                        start: base + span.start,
                        end: base + span.end,
                        role: span.hl,
                    });
                }
            }
            continue;
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            stack.push(child);
        }
    }
    result.sort_by_key(|span| span.start);
    result
}
pub fn overlay(embedded: &[EmbeddedSpan], rope: &Rope, first: usize, lines: &mut [Vec<Span>]) {
    if embedded.is_empty() {
        return;
    }
    for (index, spans) in lines.iter_mut().enumerate() {
        let line = first + index;
        if line >= rope.len_lines() {
            break;
        }
        let base = rope.line_to_char(line);
        let end = base + rope.line(line).len_chars();
        for item in embedded
            .iter()
            .filter(|span| span.start < end && span.end > base)
        {
            spans.push(Span {
                start: item.start.saturating_sub(base),
                end: item.end.min(end) - base,
                hl: item.role,
            });
        }
        spans.sort_by_key(|span| span.start);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_rust_web_assets_embed_css_javascript_on_background_parser_and_discard_stale_layers() {
        let css = include_str!("../../../tests/fixtures/RustWeb/static/app.css");
        let js = include_str!("../../../tests/fixtures/RustWeb/static/app.js");
        let text = format!(
            "<main>🦀</main>\n<style>{css}</style>\n<script>{js}</script>\n<script type=\"application/json\">{{\"color\":1}}</script>"
        );
        let rope = Rope::from_str(&text);
        let pool = crate::ParserPool::new(std::sync::Arc::new(|| {}));
        let mut syntax = crate::Syntax::new(Lang::Html);
        syntax.request_parse(&pool, 1, rope.clone(), 1);
        let result = pool
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
        assert!(!result.embedded.is_empty());
        assert!(syntax.accept(result));
        let lines = syntax.highlight(&rope, 0, rope.len_lines());
        let mut roles = Vec::new();
        for (line, spans) in lines.iter().enumerate() {
            let source = rope.line(line);
            for span in spans {
                roles.push((source.slice(span.start..span.end).to_string(), span.hl));
            }
        }
        assert!(
            roles
                .iter()
                .any(|(text, role)| text == "color" && *role == Hl::Property),
            "{roles:?}"
        );
        assert!(
            roles
                .iter()
                .any(|(text, role)| text == "const" && *role == Hl::Keyword),
            "{roles:?}"
        );
        syntax.edit(&[], 2);
        assert!(
            !syntax
                .highlight(&rope, 0, rope.len_lines())
                .iter()
                .flatten()
                .any(|span| span.hl == Hl::Keyword)
        );
        pool.submit(1, Lang::Html, rope.clone(), 1, None);
        assert!(
            !syntax.accept(
                pool.recv_timeout(std::time::Duration::from_secs(3))
                    .unwrap()
            )
        );
    }
}
