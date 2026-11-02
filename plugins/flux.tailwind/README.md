# Tailwind CSS

Tailwind CSS in Flux: completion of classes with the CSS they make, the CSS of a class on hover, errors in classes and in Tailwind's at-rules, and their quick fixes — from the Tailwind CSS language server.

## Files

The files of HTML, CSS, SCSS, Less, JavaScript and TypeScript (with JSX), Vue, Svelte, Astro and Angular templates — the languages their own plugins bring. Classes are found in the `class`, `className`, `ngClass` and `class:list` attributes and in calls of the usual helpers: `cn(…)`, `clsx(…)`, `cva(…)`, `cx(…)`, `twMerge(…)`, `twJoin(…)`.

## Language servers

Flux starts the server when a file of these languages opens, next to the language's own server. If it isn't on the machine, Flux installs it into `~/Library/Application Support/flux/servers` (Settings → Language Servers shows where each one comes from, and installs, updates or removes it).

- **tailwindcss-language-server** (the npm package `@tailwindcss/language-server`) — with a Node.js of Flux's own. It finds the project's Tailwind CSS itself: a `tailwind.config.*` file of version 3, or a CSS file with `@import "tailwindcss"` of version 4; in a project without one it stays idle. Needs Flux 0.4 or later.
