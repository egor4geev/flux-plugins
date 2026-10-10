; tree-sitter-dockerfile v0.2.0 (MIT, licenses/tree-sitter-dockerfile.txt): queries/highlights.scm,
; with additions for Flux at the end: flags of instructions, the stage's name, exposed ports.
; Flux has no language injections yet: the shell commands of RUN stay plain text.

[
	"FROM"
	"AS"
	"RUN"
	"CMD"
	"LABEL"
	"EXPOSE"
	"ENV"
	"ADD"
	"COPY"
	"ENTRYPOINT"
	"VOLUME"
	"USER"
	"WORKDIR"
	"ARG"
	"ONBUILD"
	"STOPSIGNAL"
	"HEALTHCHECK"
	"SHELL"
	"MAINTAINER"
	"CROSS_BUILD"
	(heredoc_marker)
	(heredoc_end)
] @keyword

[
	":"
	"@"
] @operator

(comment) @comment


(image_spec
	(image_tag
		":" @punctuation.special)
	(image_digest
		"@" @punctuation.special))

[
	(double_quoted_string)
	(single_quoted_string)
	(json_string)
	(heredoc_line)
] @string

(expansion
  [
	"$"
	"{"
	"}"
  ] @punctuation.special
) @none

((variable) @constant
 (#match? @constant "^[A-Z][A-Z_0-9]*$"))

; Flux: flags of instructions (`--from=build`, `--mount=…`), the stage's name, exposed ports.
[
	(param)
	(mount_param)
] @attribute
(image_alias) @label
(expose_port) @number
