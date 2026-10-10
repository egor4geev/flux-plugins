# HTML

HTML: highlighting, and the HTML language server with errors, completion, hover and formatting.

## Files

- **HTML**: `*.html`, `*.htm`, `*.xhtml`.

## Language servers

Flux starts the server when a file of the language opens. If it isn't on the machine, Flux
installs it into `~/Library/Application Support/flux/servers` (Settings → Language Servers shows
where each one comes from, and installs, updates or removes it).

- **vscode-html-language-server** (the npm package `vscode-langservers-extracted`) — with a Node.js of Flux's own. It also completes and checks the CSS of `<style>` and the JavaScript of `<script>`.

## Highlighting

- [tree-sitter-html](https://github.com/tree-sitter/tree-sitter-html) v0.23.2, MIT — the grammar and its highlighting query, adapted to Flux's theme scopes (`licenses/`).
- Flux has no language injections yet: the CSS of `<style>` and the JavaScript of `<script>` are not highlighted (the language server still understands them).
