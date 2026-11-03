; INI: the highlighting query of tree-sitter-ini (https://github.com/justinmk/tree-sitter-ini,
; Apache-2.0 — licenses/tree-sitter-ini.txt), adapted to the scopes Flux themes color: the values
; are strings (a bare number or boolean as such), `@spell` is dropped. The later pattern wins.

(section_name
  (text) @type) ; consistency with toml

(comment) @comment

[
  "["
  "]"
] @punctuation.bracket

"=" @operator

(setting
  (setting_name) @property)

(setting_value) @string

((setting_value) @number
  (#match? @number "^\\s*-?[0-9]+(\\.[0-9]+)?\\s*$"))

((setting_value) @boolean
  (#match? @boolean "^\\s*(true|false|yes|no|on|off)\\s*$"))
