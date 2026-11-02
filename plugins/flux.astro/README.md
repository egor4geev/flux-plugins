# Astro

Astro components: highlighting of the frontmatter, markup, expressions, scripts and styles, and the Astro language server with errors, completion, hover and navigation.

## Files

- **Astro**: `*.astro`.

## Language servers

Flux starts the server when a file of the language opens. If it isn't on the machine, Flux
installs it into `~/Library/Application Support/flux/servers` (Settings → Language Servers shows
where each one comes from, and installs, updates or removes it).

- **astro-ls** (the npm package `@astrojs/language-server`, with TypeScript 6) — with a Node.js of Flux's own.

## Highlighting

- [tree-sitter-astro](https://github.com/virchau13/tree-sitter-astro) `213f6e6`, MIT — the grammar and its highlighting and injection queries, adapted to Flux's theme scopes (`licenses/`).
- The languages inside a component are highlighted by their own plugins: the frontmatter between the `---`, the `{…}` expressions and `<script>` as TypeScript; `<style>` as CSS, or the language its `lang` names (`scss`, `less`). Needs Flux 0.4 or later.
