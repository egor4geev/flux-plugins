# YAML

YAML: highlighting, and the YAML language server with errors, completion and schemas of well-known files.

## Files

- **YAML**: `*.yaml`, `*.yml`, `.clang-format`, `.clang-tidy`.

## Language servers

Flux starts the server when a file of the language opens. If it isn't on the machine, Flux
installs it into `~/Library/Application Support/flux/servers` (Settings → Language Servers shows
where each one comes from, and installs, updates or removes it).

- **yaml-language-server** — from npm, with a Node.js of Flux's own.

## Highlighting

- The grammar and its highlighting query: [tree-sitter-yaml](https://github.com/tree-sitter-grammars/tree-sitter-yaml) v0.7.2, MIT — `licenses/`.
