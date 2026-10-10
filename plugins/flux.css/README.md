# CSS

CSS and SCSS: highlighting, and the CSS language server with errors, completion, color and hover information, and formatting.

## Files

- **CSS**: `*.css`.
- **SCSS**: `*.scss`.

## Language servers

Flux starts the server when a file of the language opens. If it isn't on the machine, Flux
installs it into `~/Library/Application Support/flux/servers` (Settings → Language Servers shows
where each one comes from, and installs, updates or removes it).

- **vscode-css-language-server** (the npm package `vscode-langservers-extracted`) — with a Node.js of Flux's own; for CSS and SCSS.

## Highlighting

- [tree-sitter-css](https://github.com/tree-sitter/tree-sitter-css) v0.25.0, MIT — the grammar and its highlighting query, adapted to Flux's theme scopes (`licenses/`).
- [tree-sitter-scss](https://github.com/savetheclocktower/tree-sitter-scss) `f5dfc3b`, MIT — the grammar and its highlighting query, adapted to Flux's theme scopes (`licenses/`).
- Less isn't here yet: there is no maintained tree-sitter grammar for it.
