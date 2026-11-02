# INI

INI files and the configs written in it — .editorconfig, .npmrc, Git's .gitconfig and .gitmodules: highlighting of sections, keys, values and comments.

## Files

- **INI**: `*.ini`, `.editorconfig`, `.npmrc`, `.gitconfig`, `.gitmodules`.

`.yarnrc` of Yarn 1 isn't INI (`key "value"`, without `=`) and stays plain text; `.yarnrc.yml` of later Yarn versions is YAML.

## Highlighting

- [tree-sitter-ini](https://github.com/justinmk/tree-sitter-ini) v1.4.0, Apache-2.0 — the grammar and its highlighting query, adapted to Flux's theme scopes (`licenses/`): sections, keys, values (numbers and booleans apart), `;` and `#` comments.
