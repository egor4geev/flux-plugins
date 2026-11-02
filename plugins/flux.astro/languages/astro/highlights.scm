; tree-sitter-astro by Vir Chaudhury and Amaan Qureshi (MIT, licenses/tree-sitter-astro.txt):
; queries/highlights.scm, adapted for Flux like Flux's HTML query — added: quoted attribute values
; with their quotes, the `=` of attributes, the `---` of the frontmatter and the braces of
; expressions. The frontmatter, the expressions, <script> and <style> are highlighted by their own
; languages (injections.scm).

(tag_name) @tag
(erroneous_end_tag_name) @tag.error
(doctype) @constant
(attribute_name) @attribute
(attribute_value) @string
(quoted_attribute_value) @string
(attribute_backtick_string) @string
(comment) @comment

"=" @punctuation.delimiter

[
  "<"
  ">"
  "</"
  "/>"
] @punctuation.bracket

(frontmatter "---" @punctuation.special)

(attribute_interpolation
  [
    "{"
    "}"
  ] @punctuation.special)

(html_interpolation
  [
    "{"
    "}"
  ] @punctuation.special)
