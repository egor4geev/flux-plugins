# Dockerfile

Dockerfiles: highlighting, and the Dockerfile language server with errors, completion, hover and formatting.

## Files

- **Dockerfile**: `*.dockerfile`, `Dockerfile`, `dockerfile`, `Containerfile`, `Dockerfile.dev`, `Dockerfile.prod`, `Dockerfile.production`, `Dockerfile.test`, `Dockerfile.local`, `Dockerfile.ci`, `Dockerfile.base`, `Dockerfile.build`, `Dockerfile.debug`, `Dockerfile.release`.

## Language servers

Flux starts the server when a file of the language opens. If it isn't on the machine, Flux
installs it into `~/Library/Application Support/flux/servers` (Settings → Language Servers shows
where each one comes from, and installs, updates or removes it).

- **docker-langserver** (the npm package `dockerfile-language-server-nodejs`) — with a Node.js of Flux's own.

## Highlighting

- [tree-sitter-dockerfile](https://github.com/camdencheek/tree-sitter-dockerfile) v0.2.0, MIT — the grammar and its highlighting query, adapted to Flux's theme scopes (`licenses/`).
- Flux has no language injections yet: the shell commands of `RUN` are not highlighted.
