# Markdown

Markdown: highlighting of the document's structure, and Marksman with links between files, headings and completion.

## Files

- **Markdown**: `*.md`, `*.markdown`.

## Language servers

Flux starts the server when a file of the language opens. If it isn't on the machine, Flux
installs it into `~/Library/Application Support/flux/servers` (Settings → Language Servers shows
where each one comes from, and installs, updates or removes it).

- **Marksman** — the latest release from GitHub. Links between files resolve in a Git repository or with a `.marksman.toml`.

## Highlighting

- The grammar and its highlighting query: [tree-sitter-markdown](https://github.com/tree-sitter-grammars/tree-sitter-markdown) v0.5.3 (`tree-sitter-markdown`), MIT — `licenses/`.
