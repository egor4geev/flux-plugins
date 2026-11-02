; Vue for Flux, on the node types of tree-sitter-vue by Amaan Qureshi (tree-sitter-grammars, MIT,
; licenses/tree-sitter-vue.txt) and its queries/vue/highlights.scm: the grammar is HTML with
; templates, `{{ }}` interpolations and directives, so the HTML part follows Flux's HTML query
; (tree-sitter-html, MIT). The TypeScript and CSS of a component — <script>, <style>, `{{ }}` and
; the values of directives — are highlighted by their own languages (injections.scm).

(tag_name) @tag
(erroneous_end_tag_name) @tag.error
(doctype) @constant
(attribute_name) @attribute
(attribute (attribute_value) @string)
(attribute (quoted_attribute_value) @string)
(entity) @string.escape
(comment) @comment

"=" @punctuation.delimiter

[
  "<"
  ">"
  "</"
  "/>"
] @punctuation.bracket

; Directives: `v-if`, `v-for`, and the short forms `:prop` (v-bind), `@event` (v-on), `#slot`
; (v-slot); their arguments (`:href`, `v-on:click`, `:[key]`) and modifiers (`.prevent`).
(directive_name) @keyword
(directive_attribute
  [
    ":"
    "@"
    "#"
  ] @keyword)
(directive_value) @attribute
(dynamic_directive_value
  [
    "["
    "]"
  ] @punctuation.bracket)
(dynamic_directive_inner_value) @variable
(directive_modifiers "." @punctuation.delimiter)
(directive_modifier) @attribute

; The value of a directive is an expression: only its quotes are punctuation.
(directive_attribute
  (quoted_attribute_value
    [
      "\""
      "'"
    ] @punctuation.special))

(interpolation
  [
    "{{"
    "}}"
  ] @punctuation.special)
