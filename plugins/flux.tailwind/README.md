# Tailwind CSS

Tailwind CSS in Flux: completion of classes with the CSS they make, the CSS of a class on hover, errors in classes and in Tailwind's at-rules, and their quick fixes — from the Tailwind CSS language server.

## Files

The files of HTML, CSS, SCSS, Less, JavaScript and TypeScript (with JSX), Vue, Svelte, Astro and Angular templates — the languages their own plugins bring. Classes are found in the `class`, `className`, `ngClass` and `class:list` attributes and in calls of the usual helpers: `cn(…)`, `clsx(…)`, `cva(…)`, `cx(…)`, `twMerge(…)`, `twJoin(…)`.

## Language servers

Flux starts the server when a file of these languages opens in a project with Tailwind CSS: a `tailwind.config.*` file (version 3) in the project's root or next to the file's `package.json`, or `tailwindcss` or a `@tailwindcss/…` package (version 4: `@tailwindcss/vite`, `@tailwindcss/postcss`) among that `package.json`'s dependencies. It runs next to the language's own server and never takes its place: the classes join the language's own completion and hover. If it isn't on the machine, Flux installs it into `~/Library/Application Support/flux/servers` (Settings → Language Servers shows where each one comes from, and installs, updates or removes it).

- **tailwindcss-language-server** (the npm package `@tailwindcss/language-server`) — with a Node.js of Flux's own. It reads the project's Tailwind CSS configuration itself: the `tailwind.config.*` of version 3, or the CSS file with `@import "tailwindcss"` of version 4. Needs Flux 0.4 or later.
