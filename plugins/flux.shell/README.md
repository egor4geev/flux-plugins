# Shell Script

Shell scripts (sh, bash, zsh): highlighting, and the Bash language server with ShellCheck.

## Files

- **Shell**: `*.sh`, `*.bash`, `*.zsh`, `.bashrc`, `.bash_profile`, `.bash_aliases`, `.bash_logout`, `.profile`, `.zshrc`, `.zshenv`, `.zprofile`, `.zlogin`, `.zlogout`, `PKGBUILD`.

## Language servers

Flux starts the server when a file of the language opens. If it isn't on the machine, Flux
installs it into `~/Library/Application Support/flux/servers` (Settings → Language Servers shows
where each one comes from, and installs, updates or removes it).

- **bash-language-server** — from npm, with a Node.js of Flux's own; Flux adds ShellCheck (a release from GitHub) for its diagnostics. Zsh files are highlighted, but the Bash server doesn't check them.

## Highlighting

- The grammar and its highlighting query: [tree-sitter-bash](https://github.com/tree-sitter/tree-sitter-bash) v0.25.1, MIT — `licenses/`.
