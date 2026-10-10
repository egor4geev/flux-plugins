; SCSS for Flux, on the node types of tree-sitter-scss by Andrew Dupont (savetheclocktower, MIT,
; licenses/tree-sitter-scss.txt): the grammar's own query covers only CSS. SCSS reads the CSS query
; first (languages/css/highlights.scm), then this one; the later pattern wins for the same text.

(single_line_comment) @comment

; Directives and their words.
[
  "@use"
  "@forward"
  "@mixin"
  "@include"
  "@function"
  "@return"
  "@extend"
  "@at-root"
  "@debug"
  "@warn"
  "@error"
  "@if"
  "@else"
  "@each"
  "@for"
  "@while"
  "as"
  "with"
  "show"
  "hide"
  "in"
] @keyword

[
  (through)
  (default)
  (global)
] @keyword

; Variables (`$name`) where they are set and where they are used, loop variables.
(variable_name) @variable
(variable_value) @variable
(each_statement (key) @variable)
(each_statement (value) @variable)

; Parameters of mixins and functions, named arguments.
(parameter (variable_name) @variable.parameter)
(argument_name) @variable.parameter

; Mixins and functions: their names where they are made and used.
(mixin_statement (name) @function)
(function_statement (name) @function)
(mixin_name) @function

; Placeholder selectors (`%name`), as class names.
(placeholder_selector "%" @punctuation.delimiter)
(placeholder_name) @property

; Values.
(boolean_value) @boolean
(null_value) @constant.builtin
(map_pair key: (plain_value) @property)

[
  "=="
  "!="
  "<"
  "<="
  ">="
  "%"
  "..."
] @operator

(interpolation
  [
    "#{"
    "}"
  ] @punctuation.special)
