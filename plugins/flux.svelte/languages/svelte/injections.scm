; Svelte for Flux: the languages inside a component (plugin API 0.4). A `lang` names a language by
; its id or alias, as Markdown code fences do: `ts`, `scss`, `less`, `postcss`…; a language Flux
; doesn't have (`pug`, `sass`, `stylus`) stays plain text.

; <script>, <script module>: JavaScript.
((script_element
  (start_tag) @_start
  (raw_text) @injection.content)
  (#not-match? @_start "\\slang\\s*=")
  (#set! injection.language "javascript"))

; <script lang="ts">.
((script_element
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

; <style>: CSS.
((style_element
  (start_tag) @_start
  (raw_text) @injection.content)
  (#not-match? @_start "\\slang\\s*=")
  (#set! injection.language "css"))

; <style lang="scss">.
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

; `{expression}` in the markup and in attributes, the conditions and lists of blocks, the values of
; `{@html …}`, `{@const …}`, `{@render …}`: TypeScript, which reads JavaScript too.
((svelte_raw_text) @injection.content
  (#set! injection.language "typescript"))

; style="…": CSS declarations.
((attribute
  (attribute_name) @_style
  (quoted_attribute_value
    (attribute_value) @injection.content))
  (#eq? @_style "style")
  (#not-match? @injection.content "\\{")
  (#set! injection.language "css"))
