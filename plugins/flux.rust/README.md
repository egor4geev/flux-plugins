# Rust

Rust: highlighting, and rust-analyzer with errors, completion, navigation and refactoring.

## Files

- **Rust**: `*.rs`.

## Language servers

Flux starts the server when a file of the language opens. If it isn't on the machine, Flux
installs it into `~/Library/Application Support/flux/servers` (Settings → Language Servers shows
where each one comes from, and installs, updates or removes it).

- **rust-analyzer** — `rustup component add rust-analyzer` when rustup is there, otherwise the latest release from GitHub.

## Highlighting

- The grammar and its highlighting query: [tree-sitter-rust](https://github.com/tree-sitter/tree-sitter-rust) v0.24.2, MIT — `licenses/`.
