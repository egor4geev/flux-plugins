# ESLint

ESLint in Flux: the problems of the project's ESLint rules as you type, and their quick fixes — fix a problem, all of its kind, or turn the rule off for a line.

## Files

The files of JavaScript and TypeScript (with JSX), Vue, Svelte and Astro — the languages their own plugins bring. A file is checked when the project's ESLint config covers it: Vue, Svelte and Astro files need the config's plugin for them.

## Language servers

Flux starts the server when a file of these languages opens, next to the language's own server. If it isn't on the machine, Flux installs it into `~/Library/Application Support/flux/servers` (Settings → Language Servers shows where each one comes from, and installs, updates or removes it).

- **vscode-eslint-language-server** (the npm package `vscode-langservers-extracted`) — with a Node.js of Flux's own. It runs the project's own ESLint (`npm install` first): `eslint.config.*` of ESLint 9 and later, and `.eslintrc.*` of older versions. In a project without ESLint it stays idle. Quick fixes (⌥↵): fix this problem, fix all of the rule's problems, fix everything that can be fixed, turn the rule off for this line or the file. Needs Flux 0.4 or later.
- With an `.eslintrc.*` config (ESLint 8 and older) every file of these languages is checked: a Vue, Svelte or Astro file without the config's plugin for it shows a parsing error. With `"settings": { "validate": "probe" }` of the server (Settings → Language Servers → the server → Configure…) only the files the config has a parser for are checked.
