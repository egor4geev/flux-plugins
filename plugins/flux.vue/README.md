# Vue

Vue single-file components: highlighting of the template, scripts and styles, and the Vue language server with errors, completion, hover and navigation.

## Files

- **Vue**: `*.vue`.

## Language servers

Flux starts the server when a file of the language opens. If it isn't on the machine, Flux
installs it into `~/Library/Application Support/flux/servers` (Settings → Language Servers shows
where each one comes from, and installs, updates or removes it).

- **vue-language-server** (the npm package `@vue/language-server` 2.2, with TypeScript 5.9) — with a Node.js of Flux's own. It checks, completes and explains the template and the TypeScript or JavaScript of a component by itself. Imports of `.vue` files in `.ts` files are not resolved by the TypeScript server: a `declare module '*.vue'` in the project helps.

## Highlighting

- [tree-sitter-vue](https://github.com/tree-sitter-grammars/tree-sitter-vue) `ce8011a`, MIT — the grammar; the highlighting query is written for Flux on its node types and its own query (`licenses/`).
- The languages inside a component are highlighted by their own plugins: `<script>` as JavaScript, or the language its `lang` names (`ts`, `tsx`); `<style>` as CSS, or `scss`, `less`, `postcss`; `{{ }}` and the values of directives (`v-if`, `:prop`, `@click`) as TypeScript. Pug templates and Sass or Stylus styles stay plain. Needs Flux 0.4 or later.
