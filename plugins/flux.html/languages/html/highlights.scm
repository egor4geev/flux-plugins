; tree-sitter-html v0.23.2 (MIT, licenses/tree-sitter-html.txt): queries/highlights.scm, adapted for
; Flux — added: quoted attribute values with their quotes, character references (`&amp;`), the `=`
; of attributes. The JavaScript of <script> and the CSS of <style> are highlighted by their own
; languages (injections.scm).

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
