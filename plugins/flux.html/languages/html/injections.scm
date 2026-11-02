; tree-sitter-html v0.23.2 (MIT, licenses/tree-sitter-html.txt): queries/injections.scm, for Flux's
; injections (plugin API 0.4) — added: the `type` of <script> (modules, JSON), the `lang` of
; <script> and <style> (by a language's id or alias), event handlers and style="…".

; <script>: JavaScript.
((script_element
  (start_tag) @_start
  (raw_text) @injection.content)
  (#not-match? @_start "\\s(type|lang)\\s*=")
  (#set! injection.language "javascript"))

; <script type="module">, type="text/javascript".
((script_element
  (start_tag
    (attribute
      (attribute_name) @_type
      [
        (attribute_value) @_value
        (quoted_attribute_value
          (attribute_value) @_value)
      ]))
  (raw_text) @injection.content)
  (#eq? @_type "type")
  (#match? @_value "^(module|(text|application)/(javascript|ecmascript|babel|jsx))$")
  (#set! injection.language "javascript"))

; <script type="importmap">, type="application/ld+json": JSON.
((script_element
  (start_tag
    (attribute
      (attribute_name) @_type
      [
        (attribute_value) @_value
        (quoted_attribute_value
          (attribute_value) @_value)
      ]))
  (raw_text) @injection.content)
  (#eq? @_type "type")
  (#match? @_value "^(importmap|speculationrules|application/(ld\\+)?json)$")
  (#set! injection.language "json"))

; <style>: CSS.
((style_element
  (start_tag) @_start
  (raw_text) @injection.content)
  (#not-match? @_start "\\slang\\s*=")
  (#set! injection.language "css"))

; <script lang="ts">, <style lang="scss">.
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

; onclick="…": JavaScript; style="…": CSS declarations.
((attribute
  (attribute_name) @_handler
  (quoted_attribute_value
    (attribute_value) @injection.content))
  (#match? @_handler "^on[a-z]+$")
  (#set! injection.language "javascript"))

((attribute
  (attribute_name) @_style
  (quoted_attribute_value
    (attribute_value) @injection.content))
  (#eq? @_style "style")
  (#set! injection.language "css"))
