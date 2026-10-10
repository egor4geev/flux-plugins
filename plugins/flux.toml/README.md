# TOML

TOML: highlighting, and Taplo with errors, completion and formatting.

## Files

- **TOML**: `*.toml`, `Cargo.lock`, `Pipfile`, `poetry.lock`, `uv.lock`.

## Language servers

Flux starts the server when a file of the language opens. If it isn't on the machine, Flux
installs it into `~/Library/Application Support/flux/servers` (Settings → Language Servers shows
where each one comes from, and installs, updates or removes it).

- **Taplo** — the latest release from GitHub.

## Highlighting

- The grammar and its highlighting query: [tree-sitter-toml](https://github.com/tree-sitter-grammars/tree-sitter-toml) v0.7.0, MIT — `licenses/`.
