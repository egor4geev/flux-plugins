# Dotenv

.env files: highlighting of keys, values, references and comments, and checks — keys set twice, files with values Git doesn't ignore, keys missing from .env.example.

## Files

- **Dotenv**: `.env`, `.env.*` (`.env.local`, `.env.production`, `.env.example`), `*.env` (`production.env`). Needs Flux 0.4 or later.

## Highlighting

`KEY=value` and `export KEY=value`; values unquoted, in double quotes (over several lines, with escapes like `\n` and references), in single quotes or backticks (as they are); references to other variables — `$NAME`, `${NAME}`, `${NAME:-default}` — in double quotes and unquoted values; `# comments`, after a value too. Values are read as the `dotenv` package of Node.js reads them: an unquoted value ends at a `#`.

- The grammar is written for the catalog — [`grammars/tree-sitter-dotenv`](https://github.com/egor4geev/flux-plugins/tree/main/grammars/tree-sitter-dotenv), MIT — and so is the highlighting query (`licenses/`).

## Checks

The plugin's problems show as a language server's do: underlined, on hover, in the Problems window (F2) — for the open files as they are typed, for the project's other `.env` files as they are on disk (the project's folder and two levels below it, without `node_modules`).

- **A key set twice** — a warning on the later lines. The `dotenv` package keeps the first value, a shell the last one.
- **A file Git doesn't ignore** — a warning on its first line, when the file has values: the next commit may take it. A file Git already has gets a warning too. Templates — `.env.example`, `.env.sample`, `.env.template`, `.env.dist`, `example.env` — are meant to be committed and are not checked. Some frameworks keep defaults in a committed `.env`: turn the check off in Settings → Plugins → Dotenv.
- **A file against its template** — hints: the keys of `.env.example` (or `.env.sample`, `.env.template`) of the same folder that a file doesn't set, on its first line; the keys a file sets that the template doesn't list, on their lines.

## Permissions

- **The project's files, to read**: the `.env` files and their templates.
- **Programs: `git`**: `git check-ignore` and `git ls-files` tell which `.env` files Git ignores and which it has. Without Git, or outside a repository, this check is skipped.
