; Ignore files: the highlighting query written for Flux on the node types of tree-sitter-gitignore
; (https://github.com/shunsambongi/tree-sitter-gitignore, MIT). Nested captures win: a pattern is a
; string, its wildcards, separators and escapes stand out in it.

(comment) @comment

(pattern) @string

(negation) @operator

[
  (directory_separator)
  (directory_separator_escaped)
] @punctuation.delimiter

[
  (wildcard_char_single)
  (wildcard_chars)
  (wildcard_chars_allow_slash)
] @punctuation.special

[
  (pattern_char_escaped)
  (bracket_char_escaped)
  (directory_separator_escaped)
] @string.escape

(bracket_expr
  [
    "["
    "]"
  ] @punctuation.bracket)

(bracket_negation) @operator

(bracket_range
  "-" @operator)

(bracket_char_class) @constant
