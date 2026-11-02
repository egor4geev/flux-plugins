; tree-sitter-markdown v0.5.3 (MIT, licenses/tree-sitter-markdown.txt):
; tree-sitter-markdown/queries/injections.scm, for Flux's injections (plugin API 0.4) — without the
; inline grammar (`markdown_inline`: emphasis, links and code spans inside paragraphs stay plain)
; and without the front matter of a document that begins with a thematic break.

; ```rust … ```: the language of the info string, by id or alias (`rs`, `ts`, `sh`, `py`).
(fenced_code_block
  (info_string
    (language) @injection.language)
  (code_fence_content) @injection.content)

((html_block) @injection.content
  (#set! injection.language "html"))

; Front matter: YAML between `---`, TOML between `+++`.
((minus_metadata) @injection.content
  (#set! injection.language "yaml"))

((plus_metadata) @injection.content
  (#set! injection.language "toml"))
