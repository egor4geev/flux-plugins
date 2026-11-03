# Svelte

Svelte components: highlighting of the markup, blocks, scripts and styles, and the Svelte language server with errors, completion, hover, navigation and formatting.

## Files

- **Svelte**: `*.svelte`.

## Language servers

Flux starts the server when a file of the language opens. If it isn't on the machine, Flux
installs it into `~/Library/Application Support/flux/servers` (Settings → Language Servers shows
where each one comes from, and installs, updates or removes it).

- **svelte-language-server** (the npm package, with TypeScript 6) — with a Node.js of Flux's own. It uses the project's own Svelte when the project has one.

## Highlighting

- [tree-sitter-svelte](https://github.com/tree-sitter-grammars/tree-sitter-svelte) v1.0.2, MIT — the grammar; the highlighting query is written for Flux on its node types and its own query (`licenses/`).
- The languages inside a component are highlighted by their own plugins: `<script>` as JavaScript, or the language its `lang` names (`ts`); `<style>` as CSS, or `scss`, `less`, `postcss`; the `{…}` expressions, the conditions and lists of `{#if}`, `{#each}`, `{#await}`, and `{@html}`, `{@const}`, `{@render}` as TypeScript. Needs Flux 0.4 or later.
