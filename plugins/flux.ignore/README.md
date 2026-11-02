# Ignore Files

Ignore files of Git and of the tools that read the same patterns — .gitignore, .dockerignore, .npmignore, .prettierignore, .eslintignore and others: highlighting of patterns, wildcards, negations and comments.

## Files

- **Ignore File**: `.gitignore`, `.dockerignore`, `.npmignore`, `.prettierignore`, `.eslintignore`, `.stylelintignore`, `.vercelignore`, `.gcloudignore`, `.hgignore`, `.ignore`.

`.hgignore` is read as Git's patterns: its `syntax: glob` lines are highlighted as they mean, regular expressions as plain patterns.

## Highlighting

- [tree-sitter-gitignore](https://github.com/shunsambongi/tree-sitter-gitignore) `f4685bf`, MIT — the grammar (`licenses/`); the highlighting query is written for Flux on its node types: `!` negations, `*`, `**` and `?`, `/`, `[…]` sets and escapes.
