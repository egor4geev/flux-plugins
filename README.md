# Flux plugins

The catalog of plugins for [Flux](https://github.com/egor4geev/flux): what Settings → Plugins →
Marketplace shows. Every plugin lives here as a folder of sources; a build turns each into a
package (a `.tar.gz`, published in this repository's GitHub Releases) and writes the index Flux
reads — `index.json` at the root of `main`.

Languages, color themes and icon sets are plugins without code: a manifest, a few data files, and
for a language its tree-sitter grammar, compiled to WebAssembly by the build. Plugins with code are
WebAssembly components in Rust (see Flux's `docs/plugins.md`).

## Reporting a problem

- **A plugin doesn't work as it should**: in Flux, Settings → Plugins → the plugin → **Report an
  Issue** opens [the plugins' bug form](https://github.com/egor4geev/flux-plugins/issues/new?template=plugin_bug.yml)
  with the plugin, its version, Flux's version and macOS filled in.
- **A problem with Flux itself** goes to [Flux's issues](https://github.com/egor4geev/flux/issues/new/choose);
  **an idea, or a plugin you'd like to see** — to [Flux's Discussions](https://github.com/egor4geev/flux/discussions/categories/ideas).
- **A security problem** in a plugin: [privately](https://github.com/egor4geev/flux-plugins/security/advisories/new),
  not in a public issue.

## Adding a plugin

1. Fork the repository and add a folder `plugins/<plugin id>/` — named exactly by the plugin's id
   (`someone.hello`, lowercase words joined by `.` or `-`).
2. Put the plugin there (the layout is below). Its `flux-plugin.toml` names everything it brings:
   Flux's `docs/plugins.md`, `docs/languages.md`, `docs/themes.md` and `docs/icon-themes.md` describe
   the formats.
3. Check it builds: `python3 scripts/build.py --check --only <plugin id>`.
4. Open a pull request. CI checks it the same way; a maintainer reviews what the plugin asks for —
   the permissions of a plugin with code are read line by line — and merges. `main` takes changes
   only through pull requests.
5. On `main`, CI publishes: a release `<plugin id>-v<version>` with the package, and the index.

To publish a new version, raise `version` in the manifest: a published version is never replaced,
and Flux offers the new one to everyone who has the plugin.

## Layout

```
plugins/<plugin id>/
  flux-plugin.toml         the manifest
  README.md                shown on the plugin's page in the Marketplace
  locales/ru.toml          translations of the manifest's strings (English text = the key)
  icons/                   the plugin's icon (`icon` of the manifest), file type icons
  languages/<language>/    highlighting queries (`highlights.scm`)
  grammars.toml            where the grammars come from: the build compiles them
  grammars/<grammar>.wasm  the compiled grammars (built, not committed)
  themes/  icon-themes/    color themes, icon sets
  licenses/                the licenses of what the plugin ships (grammars, queries)
  Cargo.toml + src/        a plugin with code (built for wasm32-wasip2)
scripts/build.py           the build
dist/                      its output: packages and index.json (not committed)
```

A grammar's source is pinned to a revision:

```toml
# grammars.toml
[[grammars]]
id = "rust"                                         # the manifest's [[grammars]] id
repo = "https://github.com/tree-sitter/tree-sitter-rust"
rev = "v0.24.2"                                     # a tag or a commit
path = "."                                          # the folder with src/ in the repository
```

and compiled to the path the manifest names (`[[grammars]] wasm = "grammars/rust.wasm"`).

## Building

Needs Python 3.11+, git and the tree-sitter CLI 0.27.1 (`cargo install --locked tree-sitter-cli@0.27.1`,
or `TREE_SITTER=/path/to/tree-sitter`). The first grammar build downloads wasi-sdk into the
tree-sitter cache. A plugin with code needs Rust with `rustup target add wasm32-wasip2`.

```sh
python3 scripts/build.py --check                     # validate everything, build the grammars
python3 scripts/build.py --base-url "file://$PWD/dist"   # packages and a local index in dist/
FLUX_CATALOG_URL="file://$PWD/dist/index.json" flux  # Flux reading the local index
```

A package holds the plugin's folder without its sources and build files (`grammars.toml`,
`Cargo.toml`, `src/`, `target/` but the component). Packages are deterministic: the same files give
the same bytes, so the SHA-256 in the index identifies what Flux installs; Flux refuses a package
whose checksum doesn't match.

## Review rules

- A plugin does what its README says, and asks only for the permissions it needs.
- Grammars and queries come from their upstream repositories at a pinned revision, with their
  license in `licenses/`.
- A theme or an icon set brings only data; a language may bring a language server only by an
  install recipe Flux knows (npm, a GitHub release, `go install`, rustup).
- The plugin API version (`api`) is the one Flux runs; plugins for an older API stay until they are
  updated, but aren't offered to new installs.
