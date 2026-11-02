; Less for Flux, on the node types of tree-sitter-less by Jim Liang and Amaan Qureshi (MIT,
; licenses/tree-sitter-less.txt) and its queries/highlights.scm: the grammar is CSS with more. Less
; reads the CSS query first (languages/css/highlights.scm), then this one; the later pattern wins
; for the same text.

(js_comment) @comment

; Variables (`@name`) where they are set and where they are used, `@@name`, `@{name}`.
((property_name) @variable
  (#match? @variable "^@"))
(variable) @variable
(value_value
  "@@" @variable
  (property_name) @variable)
(interpolation
  [
    "@{"
    "}"
  ] @punctuation.special)

; Mixins and their parameters, guards, `each`, `:extend`, `@plugin`.
(mixin_definition
  (class_name) @function)
(mixin_definition
  (id_name) @function)
(mixin_statement
  (function_name) @function)
(parameter
  (variable) @variable.parameter)
(rest_parameter
  (variable) @variable.parameter)

[
  "when"
  "each"
  "@plugin"
  ":extend"
] @keyword

; `+:` and `+_:` merge values.
(merge_identifier) @operator

[
  "=="
  "!="
  "<"
  "<="
  ">="
  "..."
] @operator
