# JSON

JSON and JSON with Comments: highlighting, and the JSON language server with errors, completion and formatting.

## Files

- **JSON**: `*.json`, `flake.lock`.
- **JSON with Comments**: `*.jsonc`.

## Language servers

Flux starts the server when a file of the language opens. If it isn't on the machine, Flux
installs it into `~/Library/Application Support/flux/servers` (Settings → Language Servers shows
where each one comes from, and installs, updates or removes it).

- **vscode-json-language-server** (the npm package `vscode-langservers-extracted`) — with a Node.js of Flux's own.

## Highlighting

- The grammar and its highlighting query: [tree-sitter-json](https://github.com/tree-sitter/tree-sitter-json) v0.24.8, MIT — `licenses/`.
