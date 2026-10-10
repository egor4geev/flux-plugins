# SQL

SQL: highlighting of PostgreSQL, MySQL, SQLite and other dialects.

## Files

- **SQL**: `*.sql`.

## Language servers

No language server: the SQL servers there are need a connection to a database and a dialect to be useful; that's for a plugin of its own.

## Highlighting

- [tree-sitter-sql](https://github.com/DerekStride/tree-sitter-sql) `39fdb00`, MIT — the grammar and its highlighting query, adapted to Flux's theme scopes (`licenses/`).
- The grammar knows many dialects at once, so it is large (11 MB): Flux compiles it once, the first time an `.sql` file opens, and keeps it in its cache.
