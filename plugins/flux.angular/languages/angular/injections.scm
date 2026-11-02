; Angular templates for Flux (plugin API 0.4). The grammar parses Angular's expressions itself, so
; only what is another language goes to it: <script>, <style> and style="…", as in HTML.

((script_element
  (raw_text) @injection.content)
  (#set! injection.language "javascript"))

((style_element
  (raw_text) @injection.content)
  (#set! injection.language "css"))

((attribute
  (attribute_name) @_style
  (quoted_attribute_value
    (attribute_value) @injection.content))
  (#eq? @_style "style")
  (#set! injection.language "css"))
