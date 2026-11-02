# CSS

CSS, SCSS and Less: highlighting, and the CSS language server with errors, completion, color and hover information, and formatting.

## Files

- **CSS**: `*.css`.
- **SCSS**: `*.scss`.
- **Less**: `*.less`.

## Language servers

Flux starts the server when a file of the language opens. If it isn't on the machine, Flux
installs it into `~/Library/Application Support/flux/servers` (Settings → Language Servers shows
where each one comes from, and installs, updates or removes it).

- **vscode-css-language-server** (the npm package `vscode-langservers-extracted`) — with a Node.js of Flux's own; for CSS, SCSS and Less.

## Highlighting

- [tree-sitter-css](https://github.com/tree-sitter/tree-sitter-css) v0.25.0, MIT — the grammar and its highlighting query, adapted to Flux's theme scopes (`licenses/`).
- [tree-sitter-scss](https://github.com/savetheclocktower/tree-sitter-scss) `f5dfc3b`, MIT — the grammar and its highlighting query, adapted to Flux's theme scopes (`licenses/`).
- [tree-sitter-less](https://github.com/jimliang/tree-sitter-less) `e5ae624`, MIT — the grammar; the highlighting query is written for Flux on its node types and its own query (`licenses/`). Rarer forms — `~"…"` escapes, mixin arguments separated by `;`, guards with functions — aren't parsed and stay partly plain.
