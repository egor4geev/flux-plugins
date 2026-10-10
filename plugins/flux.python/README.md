# Python

Python: highlighting, Pyright (types, completion, navigation, the project's virtual environment) and Ruff (linting and formatting).

## Files

- **Python**: `*.py`, `*.pyi`, `*.pyw`.

## Language servers

Flux starts the server when a file of the language opens. If it isn't on the machine, Flux
installs it into `~/Library/Application Support/flux/servers` (Settings → Language Servers shows
where each one comes from, and installs, updates or removes it).

- **Pyright** — from npm, with a Node.js of Flux's own. It uses the project's virtual environment (`.venv`, `venv` or `env` with a `pyvenv.cfg`, otherwise `$VIRTUAL_ENV`).
- **Ruff** — the latest release from GitHub (checked by its `.sha256`): linting and formatting (⌥⌘L).

## Highlighting

- The grammar and its highlighting query: [tree-sitter-python](https://github.com/tree-sitter/tree-sitter-python) v0.25.0, MIT — `licenses/`.
