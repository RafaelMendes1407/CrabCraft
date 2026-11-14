//! Pure Rust syntax and byte-range operations for guarded signature changes.
type ParameterSlots = (Vec<Range<usize>>, bool, String, String);

use rc_syntax::tree_sitter::Node;
use std::{collections::BTreeSet, ops::Range};
#[derive(Clone, Debug)]
pub enum Error {
    Syntax,
    Unsupported,
    Input,
    Limit,
    Overlap,
    Conflict,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parameter {
    pub original: Option<usize>,
    pub name: String,
    pub ty: String,
    pub argument: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Input {
    pub parameters: Vec<Parameter>,
    pub return_type: String,
}
#[derive(Clone, Debug)]
pub struct OriginalParameter {
    pub name: String,
    pub ty: String,
    pub binding: Range<usize>,
    pub type_range: Range<usize>,
    pub slot: Range<usize>,
    pub scalar: bool,
}
#[derive(Clone, Debug)]
pub struct Function {
    pub name: String,
    pub params: Range<usize>,
    pub body: Range<usize>,
    pub parameters: Vec<OriginalParameter>,
    pub return_type: String,
    pub return_range: Option<Range<usize>>,
    pub trailing_comma: bool,
    pub tail: String,
    pub prefix: String,
}
#[derive(Clone, Debug)]
pub struct Edit {
    pub range: Range<usize>,
    pub text: String,
}
#[derive(Clone, Debug)]
pub struct Argument {
    pub range: Range<usize>,
    pub expression: Range<usize>,
    pub literal: bool,
    pub identifier: bool,
}
#[derive(Clone, Debug)]
pub struct Call {
    pub arguments: Range<usize>,
    pub values: Vec<Argument>,
    pub trailing_comma: bool,
    pub tail: String,
    pub prefix: String,
}
fn tree(text: &str) -> Result<rc_syntax::tree_sitter::Tree, Error> {
    if text.contains('\0') {
        return Err(Error::Syntax);
    }
    let rope = rc_text::Rope::from_str(text);
    let tree = rc_syntax::parse_now(rc_syntax::Lang::Rust, &rope, None).ok_or(Error::Syntax)?;
    if tree.root_node().has_error() {
        return Err(Error::Syntax);
    }
    Ok(tree)
}
fn text<'a>(source: &'a str, node: Node<'_>) -> Result<&'a str, Error> {
    source.get(node.byte_range()).ok_or(Error::Input)
}
fn comments(node: Node<'_>) -> bool {
    matches!(node.kind(), "line_comment" | "block_comment")
}
fn attributes(mut node: Node<'_>) -> bool {
    loop {
        let mut previous = node.prev_named_sibling();
        while let Some(p) = previous {
            if comments(p) {
                previous = p.prev_named_sibling();
                continue;
            }
            if p.kind() == "attribute_item" {
                return true;
            }
            break;
        }
        let Some(parent) = node.parent() else {
            return false;
        };
        if parent.kind() == "mod_item" {
            node = parent;
        } else if parent.kind() == "declaration_list" {
            node = parent;
            continue;
        } else {
            return false;
        }
    }
}
fn slots(source: &str, node: Node<'_>, entries: &[Node<'_>]) -> Result<ParameterSlots, Error> {
    if node.end_byte() < node.start_byte() + 2 {
        return Err(Error::Input);
    }
    let begin = node.start_byte() + 1;
    let end = node.end_byte() - 1;
    let mut boundaries = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "," {
            boundaries.push(child.byte_range());
        }
    }
    let mut start = begin;
    let mut output = Vec::new();
    let mut tail = String::new();
    let mut trailing = false;
    for comma in boundaries.iter().cloned().chain(std::iter::once(end..end)) {
        let range = start..comma.start;
        let count = entries.iter().filter(|entry| entry.start_byte() >= range.start && entry.end_byte() <= range.end).count();
        if count == 1 {
            output.push(range.clone());
            trailing = comma.start < end;
        } else if count == 0 && comma.start == end {
            tail = source.get(range).ok_or(Error::Input)?.into();
        } else {
            return Err(Error::Unsupported);
        }
        start = comma.end;
    }
    if output.len() != entries.len() {
        return Err(Error::Unsupported);
    }
    let prefix =
        output.first().and_then(|range| source.get(range.clone())).map(|slot| slot.chars().take_while(|c| c.is_whitespace()).collect()).unwrap_or_default();
    Ok((output, trailing, tail, prefix))
}
pub fn function(source: &str, position: usize) -> Result<Function, Error> {
    if position > source.len() || !source.is_char_boundary(position) {
        return Err(Error::Input);
    }
    let tree = tree(source)?;
    let mut node = tree.root_node().descendant_for_byte_range(position, position).ok_or(Error::Input)?;
    while node.kind() != "function_item" {
        node = node.parent().ok_or(Error::Unsupported)?;
    }
    let name = node.child_by_field_name("name").ok_or(Error::Unsupported)?;
    if position < name.start_byte() || position > name.end_byte() {
        return Err(Error::Unsupported);
    }
    let mut ancestor = node.parent();
    while let Some(parent) = ancestor {
        if matches!(parent.kind(), "impl_item" | "trait_item" | "foreign_mod_item" | "function_item" | "block") {
            return Err(Error::Unsupported);
        }
        ancestor = parent.parent();
    }
    if attributes(node) || text(source, name)? == "main" {
        return Err(Error::Unsupported);
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "function_modifiers" && text(source, child)?.trim() != "async" {
            return Err(Error::Unsupported);
        }
    }
    let parameters = node.child_by_field_name("parameters").ok_or(Error::Unsupported)?;
    let body = node.child_by_field_name("body").ok_or(Error::Unsupported)?;
    let mut cursor = parameters.walk();
    let entries: Vec<_> = parameters.named_children(&mut cursor).filter(|child| !comments(*child)).collect();
    if entries.len() > 32 {
        return Err(Error::Limit);
    }
    if entries.iter().any(|entry| entry.kind() != "parameter") {
        return Err(Error::Unsupported);
    }
    let (ranges, trailing_comma, tail, prefix) = slots(source, parameters, &entries)?;
    let mut result = Vec::new();
    for (entry, slot) in entries.into_iter().zip(ranges) {
        let pattern = entry.child_by_field_name("pattern").ok_or(Error::Unsupported)?;
        let ty = entry.child_by_field_name("type").ok_or(Error::Unsupported)?;
        if !matches!(pattern.kind(), "identifier" | "_") {
            return Err(Error::Unsupported);
        }
        let scalar = matches!(
            text(source, ty)?.trim(),
            "u8" | "u16" | "u32" | "u64" | "u128" | "usize" | "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "f32" | "f64" | "bool" | "char" | "()"
        );
        result.push(OriginalParameter {
            name: text(source, pattern)?.into(),
            ty: text(source, ty)?.into(),
            binding: pattern.byte_range(),
            type_range: ty.byte_range(),
            slot,
            scalar,
        });
    }
    let returned = node.child_by_field_name("return_type");
    Ok(Function {
        name: text(source, name)?.into(),
        params: parameters.byte_range(),
        body: body.byte_range(),
        parameters: result,
        return_type: returned.map(|node| text(source, node)).transpose()?.unwrap_or("()").into(),
        return_range: returned.map(|node| node.byte_range()),
        trailing_comma,
        tail,
        prefix,
    })
}
impl Function {
    pub fn input(&self) -> Input {
        Input {
            parameters: self
                .parameters
                .iter()
                .enumerate()
                .map(|(index, param)| Parameter { original: Some(index), name: param.name.clone(), ty: param.ty.clone(), argument: String::new() })
                .collect(),
            return_type: self.return_type.clone(),
        }
    }
}
fn name_valid(name: &str) -> Result<(), Error> {
    let source = format!("fn __cc({name}:u8){{}}");
    let f = function(&source, 3)?;
    if f.parameters.len() != 1 || f.parameters[0].name != name {
        return Err(Error::Input);
    }
    Ok(())
}
fn type_valid(ty: &str) -> Result<(), Error> {
    let source = format!("type __Cc = {ty};");
    let tree = tree(&source)?;
    let root = tree.root_node();
    let mut cursor = root.walk();
    let nodes: Vec<_> = root.named_children(&mut cursor).collect();
    if nodes.len() != 1 || nodes[0].kind() != "type_item" {
        return Err(Error::Input);
    }
    let node = nodes[0].child_by_field_name("type").ok_or(Error::Input)?;
    if text(&source, node)? != ty || ty == "_" {
        return Err(Error::Input);
    }
    Ok(())
}
pub fn validate(function: &Function, input: &Input) -> Result<(), Error> {
    if input.parameters.len() > 32 {
        return Err(Error::Limit);
    }
    let mut originals = BTreeSet::new();
    let mut names = BTreeSet::new();
    for parameter in &input.parameters {
        if parameter.name.len() > 128 || parameter.ty.len() > 2048 || parameter.argument.len() > 2048 {
            return Err(Error::Limit);
        }
        name_valid(&parameter.name)?;
        type_valid(&parameter.ty)?;
        if parameter.name != "_" && !names.insert(parameter.name.strip_prefix("r#").unwrap_or(&parameter.name)) {
            return Err(Error::Conflict);
        }
        if let Some(original) = parameter.original {
            if original >= function.parameters.len() || !originals.insert(original) {
                return Err(Error::Input);
            }
        } else {
            literal(&parameter.argument)?;
        }
    }
    if input.return_type.len() > 2048 {
        return Err(Error::Limit);
    }
    type_valid(&input.return_type)
}
pub fn literal(source: &str) -> Result<(), Error> {
    let prefix = "fn __cc(){let _ = ";
    let text = format!("{prefix}{source};}}");
    let tree = tree(&text)?;
    let mut stack = vec![tree.root_node()];
    let mut expression = None;
    while let Some(node) = stack.pop() {
        if node.kind() == "let_declaration" {
            expression = node.child_by_field_name("value");
            break;
        }
        let mut cursor = node.walk();
        stack.extend(node.named_children(&mut cursor));
    }
    let node = expression.ok_or(Error::Input)?;
    if text.get(node.byte_range()) != Some(source) || !pure_literal(node) {
        return Err(Error::Unsupported);
    }
    Ok(())
}
fn pure_literal(node: Node<'_>) -> bool {
    match node.kind() {
        "integer_literal" | "float_literal" | "char_literal" | "string_literal" | "raw_string_literal" | "boolean_literal" | "unit_expression" => true,
        "unary_expression" | "parenthesized_expression" | "tuple_expression" | "array_expression" | "reference_expression" => {
            let mut cursor = node.walk();
            let children: Vec<_> = node.named_children(&mut cursor).filter(|n| !comments(*n)).collect();
            !children.is_empty() && children.into_iter().all(pure_literal)
        }
        _ => false,
    }
}
pub fn render(source: &str, range: Range<usize>, edits: &[Edit]) -> Result<String, Error> {
    let mut included: Vec<_> = edits.iter().filter(|edit| edit.range.start >= range.start && edit.range.end <= range.end).collect();
    included.sort_by_key(|edit| edit.range.start);
    let mut result = String::new();
    let mut cursor = range.start;
    for edit in included {
        if edit.range.start < cursor {
            return Err(Error::Overlap);
        }
        result.push_str(source.get(cursor..edit.range.start).ok_or(Error::Input)?);
        result.push_str(&edit.text);
        cursor = edit.range.end;
    }
    result.push_str(source.get(cursor..range.end).ok_or(Error::Input)?);
    Ok(result)
}
pub fn signature(source: &str, function: &Function, input: &Input) -> Result<Vec<Edit>, Error> {
    validate(function, input)?;
    let mut parameters = Vec::new();
    for row in &input.parameters {
        if let Some(index) = row.original {
            let old = function.parameters.get(index).ok_or(Error::Input)?;
            let edits = vec![Edit { range: old.binding.clone(), text: row.name.clone() }, Edit { range: old.type_range.clone(), text: row.ty.clone() }];
            parameters.push(render(source, old.slot.clone(), &edits)?);
        } else {
            parameters.push(format!("{}{}: {}", function.prefix, row.name, row.ty));
        }
    }
    let suffix = if parameters.is_empty() {
        function.tail.clone()
    } else if function.trailing_comma {
        format!(",{}", function.tail)
    } else {
        String::new()
    };
    let mut result = vec![Edit { range: function.params.clone(), text: format!("({}{suffix})", parameters.join(",")) }];
    if input.return_type != function.return_type {
        match &function.return_range {
            Some(range) => result.push(Edit { range: range.clone(), text: input.return_type.clone() }),
            None => result.push(Edit { range: function.params.end..function.params.end, text: format!(" -> {}", input.return_type) }),
        }
    }
    Ok(result)
}

fn direct_call(source: &str, tree: &rc_syntax::tree_sitter::Tree, position: usize) -> Result<Option<Call>, Error> {
    let mut current = tree.root_node().descendant_for_byte_range(position, position);
    while let Some(node) = current {
        if node.kind() == "macro_definition" {
            return Err(Error::Unsupported);
        }
        if node.kind() == "call_expression" {
            let mut callee = node.child_by_field_name("function").ok_or(Error::Input)?;
            loop {
                match callee.kind() {
                    "generic_function" => callee = callee.child_by_field_name("function").ok_or(Error::Input)?,
                    "parenthesized_expression" => {
                        let mut cursor = callee.walk();
                        callee = callee.named_children(&mut cursor).find(|n| !comments(*n)).ok_or(Error::Input)?;
                    }
                    _ => break,
                }
            }
            if !matches!(callee.kind(), "identifier" | "scoped_identifier") {
                return Ok(None);
            }
            while let Some(name) = callee.child_by_field_name("name") {
                callee = name;
            }
            if position >= callee.start_byte() && position < callee.end_byte() {
                let arguments = node.child_by_field_name("arguments").ok_or(Error::Input)?;
                let mut cursor = arguments.walk();
                let values: Vec<_> = arguments.named_children(&mut cursor).filter(|node| !comments(*node)).collect();
                if values.len() > 32 || values.iter().any(|node| node.kind() == "attribute_item") {
                    return Err(Error::Unsupported);
                }
                let (ranges, trailing_comma, tail, prefix) = slots(source, arguments, &values)?;
                let values = values
                    .into_iter()
                    .zip(ranges)
                    .map(|(node, range)| Argument {
                        range,
                        expression: node.byte_range(),
                        literal: pure_literal(node),
                        identifier: node.kind() == "identifier",
                    })
                    .collect();
                return Ok(Some(Call { arguments: arguments.byte_range(), values, trailing_comma, tail, prefix }));
            }
        }
        current = node.parent();
    }
    Ok(None)
}
pub fn call(source: &str, position: usize) -> Result<Call, Error> {
    if position > source.len() || !source.is_char_boundary(position) {
        return Err(Error::Input);
    }
    let tree = tree(source)?;
    if let Some(call) = direct_call(source, &tree, position)? {
        return Ok(call);
    }
    let mut node = tree.root_node().descendant_for_byte_range(position, position).ok_or(Error::Input)?;
    let mut tokens = None;
    loop {
        if node.kind() == "macro_definition" {
            return Err(Error::Unsupported);
        }
        if node.kind() == "token_tree" && tokens.is_none() {
            tokens = Some(node.byte_range());
        }
        match node.parent() {
            Some(parent) => node = parent,
            None => break,
        }
    }
    let tokens = tokens.ok_or(Error::Unsupported)?;
    let begin = tokens.start + 1;
    let end = tokens.end.checked_sub(1).ok_or(Error::Input)?;
    let content = source.get(begin..end).ok_or(Error::Input)?;
    for (prefix, suffix) in [("fn __cc(){let _ = (", ");}"), ("fn __cc(){let _ = [", "]; }"), ("fn __cc(){", "\n}")] {
        let projected = format!("{prefix}{content}{suffix}");
        let Ok(tree) = self::tree(&projected) else {
            continue;
        };
        let projected_position = position.checked_sub(begin).ok_or(Error::Input)? + prefix.len();
        if let Some(mut call) = direct_call(&projected, &tree, projected_position)? {
            fn map(range: &mut Range<usize>, begin: usize, prefix: usize, end: usize) -> Result<(), Error> {
                range.start = range.start.checked_sub(prefix).ok_or(Error::Input)? + begin;
                range.end = range.end.checked_sub(prefix).ok_or(Error::Input)? + begin;
                if range.end > end {
                    return Err(Error::Unsupported);
                }
                Ok(())
            }
            map(&mut call.arguments, begin, prefix.len(), end)?;
            for value in &mut call.values {
                map(&mut value.range, begin, prefix.len(), end)?;
                map(&mut value.expression, begin, prefix.len(), end)?;
            }
            return Ok(call);
        }
    }
    Err(Error::Unsupported)
}
pub fn arguments(source: &str, call: &Call, function: &Function, input: &Input, nested: &[Edit]) -> Result<String, Error> {
    if call.values.len() != function.parameters.len() {
        return Err(Error::Unsupported);
    }
    let reordered = input.parameters.iter().filter_map(|row| row.original).collect::<Vec<_>>();
    let old = (0..function.parameters.len()).collect::<Vec<_>>();
    if reordered != old {
        for (index, value) in call.values.iter().enumerate() {
            let new_position = input.parameters.iter().position(|row| row.original == Some(index));
            if new_position != Some(index) && !value.literal && !(value.identifier && function.parameters[index].scalar) {
                return Err(Error::Unsupported);
            }
        }
    }
    let mut values = Vec::new();
    for row in &input.parameters {
        match row.original {
            Some(index) => values.push(render(source, call.values.get(index).ok_or(Error::Input)?.range.clone(), nested)?),
            None => values.push(format!("{}{}", call.prefix, row.argument)),
        }
    }
    let suffix = if values.is_empty() {
        call.tail.clone()
    } else if call.trailing_comma {
        format!(",{}", call.tail)
    } else {
        String::new()
    };
    Ok(format!("({}{suffix})", values.join(",")))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signatures_preserve_comments_mutability_and_validate_new_fields() {
        let source = "pub fn sum(mut left: i32, /* right */ right: i32,) -> i32 {left+=right;left}";
        let function = function(source, source.find("sum").unwrap()).unwrap();
        let mut input = function.input();
        input.parameters.swap(0, 1);
        input.parameters[0].name = "rhs".into();
        input.parameters.push(Parameter { original: None, name: "bonus".into(), ty: "u32".into(), argument: "0".into() });
        let edits = signature(source, &function, &input).unwrap();
        let changed = render(source, 0..source.len(), &edits).unwrap();
        assert!(changed.contains("/* right */ rhs: i32,mut left: i32"), "{changed}");
        assert!(changed.contains("bonus: u32"));
        input.parameters[0].name = "left".into();
        assert!(validate(&function, &input).is_err());
        assert!(name_valid("x) {} fn evil(").is_err());
        assert!(type_valid("u32; fn evil() {}").is_err());
        assert!(literal("std::process::exit(1)").is_err());
    }
    #[test]
    fn calls_inside_standard_macro_tokens_are_mapped_and_side_effects_refused() {
        let declaration = "fn sum(left:i32,right:i32)->i32{left+right}";
        let function = function(declaration, 3).unwrap();
        let mut input = function.input();
        input.parameters.swap(0, 1);
        let source = "fn main(){assert_eq!(sum(40, /*keep*/ 2),42);}";
        let call = super::call(source, source.find("sum").unwrap() + 1).unwrap();
        let replacement = arguments(source, &call, &function, &input, &[]).unwrap();
        assert_eq!(replacement, "( /*keep*/ 2,40)");
        let source = "fn main(){sum(side_effect(),2);}";
        let call = super::call(source, source.find("sum").unwrap() + 1).unwrap();
        assert!(arguments(source, &call, &function, &input, &[]).is_err());
        assert!(super::function("impl A {fn sum(a:i32,b:i32){}}", 12).is_err());
    }
}
