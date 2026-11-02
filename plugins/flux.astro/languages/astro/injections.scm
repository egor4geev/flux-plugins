; tree-sitter-astro (MIT, licenses/tree-sitter-astro.txt): queries/injections.scm, for Flux's
; injections (plugin API 0.4) — added: <script> without `lang` is TypeScript as Astro compiles it,
; a `lang` of <style> names its language by id or alias (`scss`, `less`), style="…" is CSS.

; The frontmatter between the `---`: TypeScript.
((frontmatter
  (frontmatter_js_block) @injection.content)
  (#set! injection.language "typescript"))

; `{expression}` in attributes and in the markup: TypeScript.
((attribute_interpolation
  (attribute_js_expr) @injection.content)
  (#set! injection.language "typescript"))

((html_interpolation
  (permissible_text) @injection.content)
  (#set! injection.language "typescript"))

; <script>: Astro builds it as TypeScript.
((script_element
  (raw_text) @injection.content)
  (#set! injection.language "typescript"))

; <style>: CSS; <style lang="scss">, <style lang="less">.
((style_element
  (start_tag) @_start
  (raw_text) @injection.content)
  (#not-match? @_start "\\slang\\s*=")
  (#set! injection.language "css"))

((style_element
  (start_tag
    (attribute
      (attribute_name) @_lang
      [
        (attribute_value) @injection.language
        (quoted_attribute_value
          (attribute_value) @injection.language)
      ]))
  (raw_text) @injection.content)
  (#eq? @_lang "lang"))

; style="…": CSS declarations.
((attribute
  (attribute_name) @_style
  (quoted_attribute_value
    (attribute_value) @injection.content))
  (#eq? @_style "style")
  (#set! injection.language "css"))
