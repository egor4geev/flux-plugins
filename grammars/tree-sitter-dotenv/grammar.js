/**
 * A tree-sitter grammar of dotenv files (`.env`, `.env.local`, `production.env`), written for the
 * Flux plugin catalog (`plugins/flux.dotenv`). It reads them as the Node.js `dotenv` package and
 * `dotenv-expand` do — the loaders of most JavaScript tools:
 *
 * - `KEY=value`, `export KEY=value`, spaces around `=`; keys of letters, digits, `_`, `.`, `-`;
 * - values unquoted (to the end of the line or a `#`), in double quotes (several lines, escapes,
 *   `$VAR` and `${VAR}`, `${VAR:-default}`), in single quotes or backticks (literal, several lines);
 * - `$VAR` and `${VAR}` in unquoted values too, `\$` for a literal dollar;
 * - `# comments` on their own lines and after values.
 *
 * Generated with tree-sitter CLI 0.27.1: `tree-sitter generate` in this folder.
 */

const NAME = /[A-Za-z_][A-Za-z0-9_]*/;

module.exports = grammar({
  name: 'dotenv',

  // Spaces and tabs separate the parts of a line; a line ends at `\n`, which the rules name.
  extras: _ => [/[ \t\r\f\v\uFEFF]/],

  word: $ => $.key,

  rules: {
    document: $ => seq(
      repeat(seq(optional($._line), $._newline)),
      optional($._line),
    ),

    _line: $ => choice(
      $.comment,
      seq($.assignment, optional($.comment)),
    ),

    _newline: _ => /\n/,

    comment: _ => token(seq('#', /[^\n]*/)),

    // `export=1` is a key named `export`, `export KEY=1` an exported KEY.
    assignment: $ => seq(
      optional('export'),
      field('key', choice($.key, alias('export', $.key))),
      '=',
      optional(field('value', $._value)),
    ),

    key: _ => /[A-Za-z0-9_][A-Za-z0-9_.\-]*/,

    _value: $ => choice(
      $.double_quoted_string,
      $.single_quoted_string,
      $.backtick_string,
      $.unquoted_value,
    ),

    // dotenv expands `\n` and `\r` in double quotes; `\"`, `\\` and `\$` keep the character.
    double_quoted_string: $ => seq(
      '"',
      repeat(choice(
        alias(token.immediate(prec(1, /[^"\\$]+/)), $.string_content),
        $.escape_sequence,
        $.simple_expansion,
        $.expansion,
        alias(token.immediate('$'), $.string_content),
      )),
      token.immediate('"'),
    ),

    single_quoted_string: $ => seq(
      '\'',
      optional(alias(token.immediate(prec(1, /[^']+/)), $.string_content)),
      token.immediate('\''),
    ),

    backtick_string: $ => seq(
      '`',
      optional(alias(token.immediate(prec(1, /[^`]+/)), $.string_content)),
      token.immediate('`'),
    ),

    // To the end of the line or a `#`; the spaces between the words belong to the value, those at
    // its ends don't. A quote inside (`it's`) is a character of the value.
    unquoted_value: $ => prec.right(seq(
      choice($._unquoted_start, $.escape_sequence, $.simple_expansion, $.expansion, $._dollar),
      repeat(choice(
        $._unquoted_text,
        $.escape_sequence,
        $.simple_expansion,
        $.expansion,
        $._dollar,
      )),
    )),

    _unquoted_start: $ => alias(/([^\s#$\\"'`]|\\[^\s$])([^\s#$\\]|\\[^\s$])*/, $.text),
    _unquoted_text: $ => alias(/([^\s#$\\]|\\[^\s$])+/, $.text),
    // A `$` that starts no reference: `PRICE=5$`, `A=$ 1`.
    _dollar: $ => alias(/\$/, $.text),

    escape_sequence: _ => token(choice(
      /\\[^\n]/,
      /\\\n/,
    )),

    simple_expansion: _ => token(seq('$', NAME)),

    // `${NAME}`, `${NAME:-default}`, `${NAME-default}`.
    expansion: $ => seq(
      '${',
      field('name', alias(token.immediate(NAME), $.variable_name)),
      optional(seq(
        field('operator', alias(token.immediate(choice(':-', '-', ':=', '=', ':?', '?', ':+', '+')), $.operator)),
        optional(field('default', alias(token.immediate(/[^}\n]+/), $.default_value))),
      )),
      token.immediate('}'),
    ),
  },
});
