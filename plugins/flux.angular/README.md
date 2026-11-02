# Angular

Angular templates: highlighting of bindings, pipes and control flow blocks, and the Angular language server with errors, completion, hover and navigation between templates and components.

## Files

- **Angular Template**: `*.component.html`. An Angular template is HTML by its extension, so Flux tells it apart by the name the Angular style guide gave templates before version 20; a template named otherwise (`app.html` next to `app.ts`) opens as HTML.

## Language servers

Flux starts the server when a file of the language opens. If it isn't on the machine, Flux
installs it into `~/Library/Application Support/flux/servers` (Settings → Language Servers shows
where each one comes from, and installs, updates or removes it).

- **ngserver** (the npm package `@angular/language-server`, with TypeScript and Angular's language service) — with a Node.js of Flux's own. It takes TypeScript and Angular's language service from the project when it has them, otherwise the ones installed with it. It serves the templates; the TypeScript of components stays with the TypeScript server.

## Highlighting

- [tree-sitter-angular](https://github.com/dlvandenberg/tree-sitter-angular) v0.9.2, MIT — the grammar and its highlighting query, adapted to Flux's theme scopes (`licenses/`). The grammar reads Angular's expressions itself: bindings (`[prop]`, `(event)`, `[(ngModel)]`), `{{ }}` with pipes, structural directives (`*ngFor`), `@if`, `@for`, `@switch`, `@defer` and `@let`, ICU messages.
- `<style>` and `style="…"` are highlighted as CSS, `<script>` as JavaScript. Needs Flux 0.4 or later.
