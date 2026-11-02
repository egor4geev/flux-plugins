; tree-sitter-angular by Dennis van den Berg (MIT, licenses/tree-sitter-angular.txt):
; queries/highlights.scm, adapted for Flux. The grammar is HTML with Angular's expressions; its query
; builds on the HTML one ("inherits: html"), so the HTML part follows Flux's HTML query
; (tree-sitter-html, MIT). Mapped to Flux's scopes: binding names (`[prop]`, `(event)`,
; `[class.x]`) and template references (`#name`) are attributes and variables, `@if`/`@for`,
; structural directives (`*ngIf`) and ICU words are keywords, `?:` an operator; the `#match?` of
; template references replaces Neovim's `#lua-match?`.

(tag_name) @tag
(erroneous_end_tag_name) @tag.error
(doctype) @constant
(attribute_name) @attribute
(attribute_value) @string
(quoted_attribute_value) @string
(entity) @string.escape
(comment) @comment

"=" @punctuation.delimiter

[
  "<"
  ">"
  "</"
  "/>"
] @punctuation.bracket

; Expressions.
(identifier) @variable

(style_unit) @type

(pipe_operator) @operator

(string) @string

(number) @number

(pipe_call
  name: (identifier) @function)

(pipe_call
  arguments: (pipe_arguments
    (identifier) @variable.parameter))

(structural_directive
  "*" @keyword
  (identifier) @keyword)

((attribute
  (attribute_name) @variable)
  (#match? @variable "^#"))

(binding_name
  (identifier) @attribute)

(class_binding
  [
    (identifier)
    (class_name)
  ] @attribute)

(event_binding
  (binding_name
    (identifier) @attribute))

(event_binding
  "\"" @punctuation.delimiter)

(property_binding
  "\"" @punctuation.delimiter)

(structural_assignment
  operator: (identifier) @keyword)

(member_expression
  property: (identifier) @property)

(call_expression
  function: (identifier) @function)

(call_expression
  function: ((identifier) @function.builtin
    (#eq? @function.builtin "$any")))

(pair
  key: ((identifier) @variable.builtin
    (#eq? @variable.builtin "$implicit")))

; Control flow: `@if`, `@for … track`, `@switch`, `@defer`, `@let`.
[
  (control_keyword)
  (special_keyword)
] @keyword

(_
  "@" @keyword
  .
  (control_keyword))

((identifier) @boolean
  (#any-of? @boolean "true" "false"))

((identifier) @variable.builtin
  (#any-of? @variable.builtin "this" "$event"))

((identifier) @constant.builtin
  (#any-of? @constant.builtin "null" "undefined"))

[
  (ternary_operator)
  (conditional_operator)
] @operator

[
  "("
  ")"
  "["
  "]"
  "{"
  "}"
] @punctuation.bracket

(two_way_binding
  [
    "[("
    ")]"
  ] @punctuation.bracket)

[
  "{{"
  "}}"
] @punctuation.special

(template_substitution
  [
    "${"
    "}"
  ] @punctuation.special)

(template_chars) @string

[
  ";"
  "."
  ","
  "?."
  "!."
] @punctuation.delimiter

(nullish_coalescing_expression
  (coalescing_operator) @operator)

(concatenation_expression
  "+" @operator)

; ICU messages: `{count, plural, =0 {…} other {…}}`.
(icu_clause) @keyword

(icu_category) @constant

(binary_expression
  [
    "-"
    "&&"
    "+"
    "<"
    "<="
    "="
    "=="
    "==="
    "!="
    "!=="
    ">"
    ">="
    "*"
    "/"
    "||"
    "%"
  ] @operator)

(arrow_function
  "=>" @operator)

(object
  (spread
    "..." @operator))

(array
  (spread
    "..." @operator))

(arguments
  (spread
    "..." @operator))
