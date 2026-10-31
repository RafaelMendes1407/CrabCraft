; Smaller nodes win, then later patterns, so the general patterns come first.
(field_identifier) @field
(shorthand_field_identifier) @field
(type_identifier) @type
(primitive_type) @type.struct

; Items and uses.
(struct_item name: (type_identifier) @type.struct)
(union_item name: (type_identifier) @type.struct)
(enum_item name: (type_identifier) @type.enum)
(trait_item name: (type_identifier) @type.interface)
(type_item name: (type_identifier) @type)
(type_parameter name: (type_identifier) @type.parameter)
(lifetime (identifier) @type.parameter)
(enum_variant name: (identifier) @constant.enum)
(mod_item name: (identifier) @namespace)
(scoped_identifier path: (identifier) @namespace)
(scoped_type_identifier path: (identifier) @namespace)
(use_declaration argument: (identifier) @namespace)
(crate) @keyword
(self) @keyword
(super) @keyword
(const_item name: (identifier) @constant)
(static_item name: (identifier) @constant)
((identifier) @constant
 (#match? @constant "^[A-Z][A-Z0-9_]+$"))

; Functions and macros.
(function_item name: (identifier) @function)
(function_signature_item name: (identifier) @function)
(call_expression function: (identifier) @function.call)
(call_expression function: (field_expression field: (field_identifier) @function.call))
(call_expression function: (scoped_identifier name: (identifier) @function.call))
(generic_function function: (identifier) @function.call)
(macro_definition name: (identifier) @preproc)
(parameter pattern: (identifier) @variable.parameter)
(closure_parameters (identifier) @variable.parameter)

; Attributes.
(attribute_item) @attribute
(inner_attribute_item) @attribute

; Literals.
(string_literal) @string
(raw_string_literal) @string
(char_literal) @string
(escape_sequence) @string.escape
(integer_literal) @number
(float_literal) @number
(boolean_literal) @keyword

; Keywords.
[
  "as" "async" "await" "break" "const" "continue" "default" "dyn" "else" "enum" "extern" "fn"
  "for" "gen" "if" "impl" "in" "let" "loop" "macro_rules!" "match" "mod" "move" "pub" "raw"
  "ref" "return" "static" "struct" "trait" "type" "union" "unsafe" "use" "where" "while" "yield"
] @keyword
(mutable_specifier) @keyword

; Operators and punctuation.
["(" ")" "[" "]" "{" "}"] @punctuation
["," ";" "." ":" "::"] @punctuation
[
  "+" "-" "*" "/" "%" "=" "==" "!=" "<=" ">=" "&&" "||" "!" "&" "|" "^" "<<" ">>" "+=" "-="
  "*=" "/=" "%=" "&=" "|=" "^=" "<<=" ">>=" ".." "..=" "..." "->" "=>" "?"
] @operator

; Comments come last: the markers of doc comments contain the same tokens as the operators.
(line_comment) @comment
(block_comment) @comment
(line_comment (doc_comment)) @comment.doc
(block_comment (doc_comment)) @comment.doc
(outer_doc_comment_marker) @comment.doc
(inner_doc_comment_marker) @comment.doc

; After the operators: the `!` of a macro call belongs to its name.
(macro_invocation macro: (identifier) @preproc "!" @preproc)
