; Vue for Flux: the languages inside a component (plugin API 0.4). A `lang` names a language by
; its id or alias, as Markdown code fences do: `ts`, `tsx`, `jsx`, `scss`, `less`, `postcss`…; a
; language Flux doesn't have (`pug`, `sass`, `stylus`) stays plain text.

; <script>, <script setup>: JavaScript.
((script_element
  (start_tag) @_start
  (raw_text) @injection.content)
  (#not-match? @_start "\\slang\\s*=")
  (#set! injection.language "javascript"))

; <script lang="ts">, <script setup lang="tsx">.
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

; <style>, <style scoped>: CSS.
((style_element
  (start_tag) @_start
  (raw_text) @injection.content)
  (#not-match? @_start "\\slang\\s*=")
  (#set! injection.language "css"))

; <style lang="scss">, <style lang="less">.
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

; `{{ expression }}` and the values of directives (`v-if="…"`, `:prop="…"`, `@click="…"`):
; TypeScript, which reads JavaScript too.
((interpolation
  (raw_text) @injection.content)
  (#set! injection.language "typescript"))

((directive_attribute
  (quoted_attribute_value
    (attribute_value) @injection.content))
  (#set! injection.language "typescript"))

; style="…": CSS declarations.
((attribute
  (attribute_name) @_style
  (quoted_attribute_value
    (attribute_value) @injection.content))
  (#eq? @_style "style")
  (#set! injection.language "css"))
