; tree-sitter-make (MIT, licenses/tree-sitter-make.txt): queries/highlights.scm, adapted for
; Flux — a variable has one color where it is set and where it is used (`@constant`; upstream:
; `@string.special.symbol` and `@string`); automatic variables (`$@`, `$<`) are `@variable.builtin`
; and the silent `@` of a recipe line `@punctuation.special` (upstream: `@character.special`);
; `@spell` dropped. `#set! priority` has no effect in Flux. Flux has no language injections yet:
; recipes stay plain text.

(comment) @comment

(conditional
  (_
    [
      "ifeq"
      "else"
      "ifneq"
      "ifdef"
      "ifndef"
    ] @keyword.conditional)
  "endif" @keyword.conditional)

(rule
  (targets
    (word) @function))

(rule
  (targets) @_target
  (prerequisites
    (word) @function
    (#eq? @_target ".PHONY")))

(rule
  (targets
    (word) @function.builtin
    (#any-of? @function.builtin
      ".DEFAULT" ".SUFFIXES" ".DELETE_ON_ERROR" ".EXPORT_ALL_VARIABLES" ".IGNORE" ".INTERMEDIATE"
      ".LOW_RESOLUTION_TIME" ".NOTPARALLEL" ".ONESHELL" ".PHONY" ".POSIX" ".PRECIOUS" ".SECONDARY"
      ".SECONDEXPANSION" ".SILENT" ".SUFFIXES")))

(rule
  [
    "&:"
    ":"
    "::"
    "|"
  ] @operator)

[
  "export"
  "unexport"
] @keyword.import

(override_directive
  "override" @keyword)

(include_directive
  [
    "include"
    "-include"
  ] @keyword.import
  filenames: (list
    (word) @string.special.path))

(variable_assignment
  name: (word) @constant
  [
    "?="
    ":="
    "::="
    ; ":::="
    "+="
    "="
  ] @operator)

(shell_assignment
  name: (word) @constant
  "!=" @operator)

(define_directive
  "define" @keyword
  name: (word) @constant
  [
    "="
    ":="
    "::="
    ; ":::="
    "?="
    "!="
  ]? @operator
  "endef" @keyword)

(variable_assignment
  (word) @variable.builtin
  (#any-of? @variable.builtin
    ".DEFAULT_GOAL" ".EXTRA_PREREQS" ".FEATURES" ".INCLUDE_DIRS" ".RECIPEPREFIX" ".SHELLFLAGS"
    ".VARIABLES" "MAKEARGS" "MAKEFILE_LIST" "MAKEFLAGS" "MAKE_RESTARTS" "MAKE_TERMERR"
    "MAKE_TERMOUT" "SHELL"))

(variable_reference
  (word) @constant) @operator

(shell_function
  [
    "$"
    "("
    ")"
  ] @operator
  "shell" @function.builtin)

(function_call
  [
    "$"
    "("
    ")"
  ] @operator)

(substitution_reference
  [
    "$"
    "("
    ")"
  ] @operator)

(automatic_variable
  "$"
  _ @variable.builtin
  (#set! priority 105))

(automatic_variable
  [
    "$"
    "("
    ")"
  ] @operator
  (#set! priority 105))

(recipe_line
  "@" @punctuation.special)

(function_call
  [
    "subst"
    "patsubst"
    "strip"
    "findstring"
    "filter"
    "filter-out"
    "sort"
    "word"
    "words"
    "wordlist"
    "firstword"
    "lastword"
    "dir"
    "notdir"
    "suffix"
    "basename"
    "addsuffix"
    "addprefix"
    "join"
    "wildcard"
    "realpath"
    "abspath"
    "error"
    "warning"
    "info"
    "origin"
    "flavor"
    "foreach"
    "if"
    "or"
    "and"
    "call"
    "eval"
    "file"
    "value"
  ] @function.builtin)

"\\" @punctuation.special
