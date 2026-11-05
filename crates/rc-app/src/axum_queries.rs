//! Conservative query-shape syntax. Callers must confirm all returned identity sites through RA.
use rc_syntax::tree_sitter::Node;
use rc_text::Rope;
#[derive(Clone, Debug)]
pub struct Parameter {
    pub extractor: usize,
    pub schema: Option<usize>,
}
#[derive(Clone, Debug)]
pub struct Schema {
    pub derives: Vec<usize>,
    pub names: Vec<String>,
}
#[derive(Clone, Debug)]
struct Token {
    value: String,
    at: usize,
    string: bool,
}
fn children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}
fn text<'a>(node: Node<'_>, source: &'a str) -> Option<&'a str> {
    source.get(node.byte_range())
}
fn last_name(node: Node<'_>) -> Node<'_> {
    node.child_by_field_name("name").unwrap_or(node)
}
fn tokens(source: &str, start: usize) -> Option<Vec<Token>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < source.len() {
        if out.len() > 512 {
            return None;
        }
        let c = source[at..].chars().next()?;
        if c.is_whitespace() {
            at += c.len_utf8();
            continue;
        }
        let begin = at;
        if c == '"' {
            at += 1;
            let mut escaped = false;
            let mut closed = false;
            while at < source.len() {
                let next = source[at..].chars().next()?;
                at += next.len_utf8();
                if !escaped && next == '"' {
                    closed = true;
                    break;
                }
                if !escaped && next == '\\' {
                    escaped = true;
                } else {
                    escaped = false;
                }
            }
            if !closed {
                return None;
            }
            out.push(Token {
                value: rc_syntax::axum::string_value(&source[begin..at], false)?,
                at: start + begin,
                string: true,
            });
            continue;
        }
        if c == '_' || c.is_alphabetic() {
            at += c.len_utf8();
            while at < source.len() {
                let c = source[at..].chars().next()?;
                if c == '_' || c.is_alphanumeric() {
                    at += c.len_utf8();
                } else {
                    break;
                }
            }
        } else {
            at += c.len_utf8();
        }
        out.push(Token {
            value: source[begin..at].into(),
            at: start + begin,
            string: false,
        });
    }
    Some(out)
}
fn attribute(node: Node<'_>, source: &str) -> Option<Vec<Token>> {
    let raw = text(node, source)?;
    if raw.len() > 8192 {
        return None;
    }
    let value = tokens(raw, node.start_byte())?;
    if value.first()?.value != "#" || value.get(1)?.value != "[" || value.last()?.value != "]" {
        return None;
    }
    Some(value[2..value.len() - 1].to_vec())
}
fn attached(mut node: Node<'_>) -> Vec<Node<'_>> {
    let mut out = Vec::new();
    while let Some(previous) = node.prev_named_sibling() {
        if previous.kind() == "attribute_item" {
            out.push(previous);
            node = previous;
        } else if matches!(previous.kind(), "line_comment" | "block_comment") {
            node = previous;
        } else {
            break;
        }
    }
    out.reverse();
    out
}
fn node_at<'a>(tree: &'a rc_syntax::tree_sitter::Tree, at: usize, kind: &str) -> Option<Node<'a>> {
    let mut node = tree.root_node().descendant_for_byte_range(at, at)?;
    loop {
        if node.kind() == kind {
            return Some(node);
        }
        node = node.parent()?;
    }
}
pub fn parameters(rope: &Rope, function_at: usize) -> Option<Vec<Parameter>> {
    if rope.len_bytes() > 2 * 1024 * 1024 {
        return None;
    }
    let tree = rc_syntax::parse_now(rc_syntax::Lang::Rust, rope, None)?;
    let function = node_at(&tree, function_at, "function_item")?;
    let parameters = function.child_by_field_name("parameters")?;
    let mut out = Vec::new();
    for parameter in children(parameters) {
        if parameter.kind() != "parameter" {
            continue;
        }
        let Some(ty) = parameter.child_by_field_name("type") else {
            continue;
        };
        if ty.kind() != "generic_type" {
            continue;
        }
        let base = last_name(ty.child_by_field_name("type")?);
        let arguments = ty.child_by_field_name("type_arguments")?;
        let args = children(arguments);
        let schema = if args.len() == 1
            && matches!(args[0].kind(), "type_identifier" | "scoped_type_identifier")
        {
            Some(last_name(args[0]).start_byte())
        } else {
            None
        };
        out.push(Parameter {
            extractor: base.start_byte(),
            schema,
        });
        if out.len() > 32 {
            return None;
        }
    }
    Some(out)
}
pub fn schema(rope: &Rope, struct_at: usize) -> Option<Schema> {
    if rope.len_bytes() > 2 * 1024 * 1024 {
        return None;
    }
    let source = rope.to_string();
    let tree = rc_syntax::parse_now(rc_syntax::Lang::Rust, rope, None)?;
    let node = node_at(&tree, struct_at, "struct_item")?;
    if node.child_by_field_name("type_parameters").is_some() {
        return None;
    }
    let mut derives = Vec::new();
    for attr in attached(node) {
        let values = attribute(attr, &source)?;
        if values.first()?.value == "derive"
            && values.get(1)?.value == "("
            && values.last()?.value == ")"
        {
            for token in &values[2..values.len() - 1] {
                if token.value == "Deserialize" && !token.string {
                    derives.push(token.at);
                }
            }
        } else if !(values.len() == 4
            && values[0].value == "serde"
            && values[1].value == "("
            && matches!(values[2].value.as_str(), "default" | "deny_unknown_fields")
            && values[3].value == ")")
        {
            return None;
        }
    }
    if derives.is_empty() {
        return None;
    }
    let body = node.child_by_field_name("body")?;
    if body.kind() != "field_declaration_list" {
        return None;
    }
    let mut names = Vec::new();
    for field in children(body) {
        if field.kind() != "field_declaration" {
            continue;
        }
        let name = field.child_by_field_name("name")?;
        let mut wire = text(name, &source)?.trim_start_matches("r#").to_string();
        let mut aliases = Vec::new();
        let mut skip = false;
        for attr in attached(field) {
            let values = attribute(attr, &source)?;
            if values.first()?.value != "serde"
                || values.get(1)?.value != "("
                || values.last()?.value != ")"
            {
                return None;
            }
            let inner = &values[2..values.len() - 1];
            let mut at = 0;
            while at < inner.len() {
                let flag = &inner[at].value;
                at += 1;
                match flag.as_str() {
                    "rename" | "alias" => {
                        if inner.get(at)?.value != "=" || !inner.get(at + 1)?.string {
                            return None;
                        }
                        let value = inner[at + 1].value.clone();
                        at += 2;
                        if flag == "rename" {
                            wire = value;
                        } else {
                            aliases.push(value);
                        }
                    }
                    "skip" | "skip_deserializing" => skip = true,
                    "default" | "skip_serializing" => {}
                    _ => return None,
                }
                if at < inner.len() {
                    if inner[at].value != "," {
                        return None;
                    }
                    at += 1;
                }
            }
        }
        if !skip {
            aliases.insert(0, wire);
            for name in aliases {
                if name.is_empty()
                    || name.len() > 128
                    || !name
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
                {
                    return None;
                }
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
        if names.len() > 128 {
            return None;
        }
    }
    Some(Schema { derives, names })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extracts_only_explicit_deserialize_shapes_and_query_identity_sites() {
        let source = "#[derive(serde::Deserialize)]pub struct Search{pub q:Option<String>,#[serde(rename=\"page_size\",alias=\"page\")]pub size:usize,#[serde(skip)]pub hidden:bool}pub fn show(query: axum::extract::Query<Search>){}";
        let rope = Rope::from_str(source);
        let shape = schema(&rope, source.find("Search").unwrap()).unwrap();
        assert_eq!(shape.names, ["q", "page_size", "page"]);
        assert_eq!(
            &source[shape.derives[0]..shape.derives[0] + 11],
            "Deserialize"
        );
        let parameters = parameters(&rope, source.find("show").unwrap()).unwrap();
        assert_eq!(parameters.len(), 1);
        assert!(source[parameters[0].extractor..].starts_with("Query"));
        assert!(source[parameters[0].schema.unwrap()..].starts_with("Search"));
        for attr in [
            "#[serde(flatten)]",
            "#[serde(deserialize_with=\"custom\")]",
            "#[cfg(unix)]",
        ] {
            let source =
                format!("#[derive(serde::Deserialize)] struct Search{{{attr} field: String}}");
            assert!(schema(&Rope::from_str(&source), source.find("Search").unwrap()).is_none());
        }
        let dynamic =
            "fn show(query: axum::extract::Query<std::collections::HashMap<String,String>>){}";
        assert!(
            super::parameters(&Rope::from_str(dynamic), dynamic.find("show").unwrap()).unwrap()[0]
                .schema
                .is_none()
        );
        let custom = "struct Search{field:String}";
        assert!(schema(&Rope::from_str(custom), custom.find("Search").unwrap()).is_none());
    }
}
