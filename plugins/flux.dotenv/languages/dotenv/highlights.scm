; dotenv: the highlighting query of Flux's grammar (grammars/tree-sitter-dotenv in the catalog's
; repository), written for the scopes Flux themes color. The later pattern wins.

(comment) @comment

"export" @keyword

(assignment
  key: (key) @property)

"=" @operator

; Values: strings, whatever the quotes; a bare number or boolean as such.
[
  (unquoted_value)
  (double_quoted_string)
  (single_quoted_string)
  (backtick_string)
] @string

((unquoted_value) @number
  (#match? @number "^-?[0-9]+(\\.[0-9]+)?$"))

((unquoted_value) @boolean
  (#any-of? @boolean "true" "false" "TRUE" "FALSE" "True" "False"))

(escape_sequence) @string.escape

; References to other variables, expanded when the file is read.
(simple_expansion) @variable

(expansion
  [
    "${"
    "}"
  ] @punctuation.special)

(expansion
  name: (variable_name) @variable)

(expansion
  operator: (operator) @operator)

(expansion
  default: (default_value) @string)
