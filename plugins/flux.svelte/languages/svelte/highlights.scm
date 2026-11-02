; Svelte for Flux, on the node types of tree-sitter-svelte by Amaan Qureshi (tree-sitter-grammars,
; MIT, licenses/tree-sitter-svelte.txt) and its queries/highlights.scm: the grammar is HTML with
; `{…}` expressions and blocks, so the HTML part follows Flux's HTML query (tree-sitter-html, MIT).
; The expressions, <script> and <style> are highlighted by their own languages (injections.scm).

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

; Blocks and tags: `{#if …}`, `{:else if …}`, `{/if}`, `{#each … as …}`, `{@html …}`, `{@render …}`.
[
  (block_start_tag)
  (block_tag)
  (block_end_tag)
  (expression_tag)
  "as"
] @keyword

(snippet_name) @function

(snippet_start
  [
    "("
    ")"
  ] @punctuation.bracket)

[
  "{"
  "}"
] @punctuation.special
