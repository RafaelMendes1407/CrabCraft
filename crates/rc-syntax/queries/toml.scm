(comment) @comment
(string) @string
(escape_sequence) @string.escape
(integer) @number
(float) @number
(boolean) @keyword
(offset_date_time) @number
(local_date_time) @number
(local_date) @number
(local_time) @number
(bare_key) @field
(quoted_key) @field
(table (bare_key) @type.struct)
(table (dotted_key (bare_key) @type.struct))
(table (quoted_key) @type.struct)
(table_array_element (bare_key) @type.struct)
(table_array_element (dotted_key (bare_key) @type.struct))
["[" "]" "[[" "]]" "{" "}"] @punctuation
["=" "." ","] @punctuation
