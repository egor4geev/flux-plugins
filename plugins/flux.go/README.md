# Go

Go: highlighting, and gopls with errors, completion, navigation and refactoring.

## Files

- **Go**: `*.go`.

## Language servers

Flux starts the server when a file of the language opens. If it isn't on the machine, Flux
installs it into `~/Library/Application Support/flux/servers` (Settings → Language Servers shows
where each one comes from, and installs, updates or removes it).

- **gopls** — `go install golang.org/x/tools/gopls@latest`: it needs Go on the machine.

## Highlighting

- The grammar and its highlighting query: [tree-sitter-go](https://github.com/tree-sitter/tree-sitter-go) v0.25.0, MIT — `licenses/`.
