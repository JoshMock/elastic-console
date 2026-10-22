/**
 * @file Elasticsearch ES|QL
 * @author Josh Mock <joshua.mock@elastic.co>
 * @license MIT
 */

/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

// ES|QL keywords are case-insensitive: kw("from") matches FROM, from, From.
const kw = (word) =>
  alias(
    token(prec(1, new RegExp(word.split("").map((c) => (/[a-z]/.test(c) ? `[${c}${c.toUpperCase()}]` : c)).join("")))),
    word.toUpperCase(),
  );

const commaSep1 = (rule) => seq(rule, repeat(seq(",", rule)));

const PREC = {
  or: 1,
  and: 2,
  not: 3,
  compare: 4,
  predicate: 5,
  add: 6,
  mul: 7,
  unary: 8,
  cast: 9,
};

module.exports = grammar({
  name: "esql",

  extras: ($) => [/\s/, $.comment],

  word: ($) => $._unquoted_identifier,

  rules: {
    source_file: ($) => seq($._source_command, repeat(seq("|", $._processing_command))),

    // ── Source commands ──────────────────────────────────────────────────────
    _source_command: ($) => choice($.from_command, $.row_command, $.show_command, $.ts_command),

    from_command: ($) => seq(kw("from"), commaSep1($.index_pattern), optional($.metadata)),
    ts_command: ($) => seq(kw("ts"), commaSep1($.index_pattern), optional($.metadata)),
    metadata: ($) => seq(kw("metadata"), commaSep1($.identifier)),
    row_command: ($) => seq(kw("row"), commaSep1($.field)),
    show_command: ($) => seq(kw("show"), kw("info")),

    // logs-*, remote:logs, .ds-x, "quoted", ${var}
    index_pattern: ($) => choice($._index_name, $.string, $.variable),
    _index_name: (_) => token(prec(-1, /[A-Za-z0-9_.*\-:<>+{}\/]+/)),

    // ── Processing commands ──────────────────────────────────────────────────
    _processing_command: ($) =>
      choice(
        $.where_command,
        $.eval_command,
        $.stats_command,
        $.inline_stats_command,
        $.keep_command,
        $.drop_command,
        $.rename_command,
        $.sort_command,
        $.limit_command,
        $.dissect_command,
        $.grok_command,
        $.enrich_command,
        $.mv_expand_command,
        $.lookup_join_command,
        $.sample_command,
        $.change_point_command,
      ),

    where_command: ($) => seq(kw("where"), $._expression),
    eval_command: ($) => seq(kw("eval"), commaSep1($.field)),
    stats_command: ($) => seq(kw("stats"), $._stats_body),
    inline_stats_command: ($) => seq(kw("inlinestats"), $._stats_body),
    _stats_body: ($) =>
      choice(
        seq(commaSep1($.aggregate), optional($.by_clause)),
        $.by_clause,
      ),
    aggregate: ($) => seq($.field, optional(seq(kw("where"), $._expression))),
    by_clause: ($) => seq(kw("by"), commaSep1($.field)),

    keep_command: ($) => seq(kw("keep"), commaSep1($.identifier_pattern)),
    drop_command: ($) => seq(kw("drop"), commaSep1($.identifier_pattern)),
    rename_command: ($) => seq(kw("rename"), commaSep1($.rename_clause)),
    rename_clause: ($) =>
      choice(
        seq(field("old", $.identifier_pattern), kw("as"), field("new", $.identifier_pattern)),
        seq(field("new", $.identifier_pattern), "=", field("old", $.identifier_pattern)),
      ),

    sort_command: ($) => seq(kw("sort"), commaSep1($.order_expression)),
    order_expression: ($) =>
      seq(
        $._expression,
        optional(choice(kw("asc"), kw("desc"))),
        optional(seq(kw("nulls"), choice(kw("first"), kw("last")))),
      ),

    limit_command: ($) => seq(kw("limit"), choice($.integer, $.parameter, $.variable)),

    dissect_command: ($) => seq(kw("dissect"), $._primary, $.string, repeat($.command_option)),
    grok_command: ($) => seq(kw("grok"), $._primary, $.string),
    command_option: ($) => seq($.identifier, "=", $._literal),

    enrich_command: ($) =>
      seq(
        kw("enrich"),
        field("policy", $.policy_name),
        optional(seq(kw("on"), $.identifier_pattern)),
        optional(seq(kw("with"), commaSep1($.enrich_clause))),
      ),
    policy_name: (_) => token(prec(-1, /[A-Za-z0-9_.\-:*]+/)),
    enrich_clause: ($) => seq($.identifier_pattern, optional(seq("=", $.identifier_pattern))),

    mv_expand_command: ($) => seq(kw("mv_expand"), $.identifier),

    lookup_join_command: ($) =>
      seq(kw("lookup"), kw("join"), $.index_pattern, kw("on"), commaSep1($.identifier)),

    sample_command: ($) => seq(kw("sample"), $.decimal),

    change_point_command: ($) =>
      seq(
        kw("change_point"),
        $.identifier,
        optional(seq(kw("on"), $.identifier)),
        optional(seq(kw("as"), $.identifier, ",", $.identifier)),
      ),

    // name = expr, or just expr
    field: ($) => seq(optional(seq(field("name", $.identifier), "=")), field("value", $._expression)),

    // ── Expressions ──────────────────────────────────────────────────────────
    _expression: ($) =>
      choice(
        $.binary_expression,
        $.unary_expression,
        $.is_null_expression,
        $.in_expression,
        $.like_expression,
        $.match_expression,
        $._primary,
      ),

    binary_expression: ($) =>
      choice(
        ...[
          [kw("or"), PREC.or],
          [kw("and"), PREC.and],
          ["==", PREC.compare],
          ["!=", PREC.compare],
          ["<", PREC.compare],
          ["<=", PREC.compare],
          [">", PREC.compare],
          [">=", PREC.compare],
          ["=~", PREC.compare],
          ["+", PREC.add],
          ["-", PREC.add],
          ["*", PREC.mul],
          ["/", PREC.mul],
          ["%", PREC.mul],
        ].map(([op, p]) =>
          prec.left(p, seq(field("left", $._expression), field("operator", op), field("right", $._expression))),
        ),
      ),

    unary_expression: ($) =>
      choice(
        prec(PREC.not, seq(kw("not"), $._expression)),
        prec(PREC.unary, seq("-", $._expression)),
      ),

    is_null_expression: ($) =>
      prec.left(PREC.predicate, seq($._expression, kw("is"), optional(kw("not")), kw("null"))),

    in_expression: ($) =>
      prec.left(
        PREC.predicate,
        seq($._expression, optional(kw("not")), kw("in"), "(", commaSep1($._expression), ")"),
      ),

    like_expression: ($) =>
      prec.left(
        PREC.predicate,
        seq(
          $._expression,
          optional(kw("not")),
          choice(kw("like"), kw("rlike")),
          choice($.string, seq("(", commaSep1($.string), ")")),
        ),
      ),

    // field : "query" (full-text match operator)
    match_expression: ($) => prec.left(PREC.predicate, seq($._primary, ":", $._primary)),

    _primary: ($) =>
      choice(
        $._literal,
        $.identifier,
        $.function_call,
        $.cast_expression,
        $.parenthesized_expression,
      ),

    parenthesized_expression: ($) => seq("(", $._expression, ")"),

    cast_expression: ($) => prec.left(PREC.cast, seq($._primary, "::", $.type)),
    type: ($) => $._unquoted_identifier,

    function_call: ($) =>
      seq(
        field("name", alias($._unquoted_identifier, $.function_name)),
        "(",
        optional(choice("*", commaSep1($._expression))),
        ")",
      ),

    // ── Literals ─────────────────────────────────────────────────────────────
    _literal: ($) =>
      choice(
        $.string,
        $.integer,
        $.decimal,
        $.boolean,
        $.null,
        $.parameter,
        $.variable,
        $.qualified_integer,
        $.array,
      ),

    // 1 day, 15 minutes, 2h
    qualified_integer: ($) => prec(1, seq(choice($.integer, $.decimal), $.unit)),
    unit: ($) => $._unquoted_identifier,

    array: ($) => seq("[", optional(commaSep1($._literal)), "]"),

    string: (_) =>
      choice(
        token(seq('"""', /([^"]|"[^"]|""[^"])*/, '"""')),
        token(seq('"', repeat(choice(/[^"\\\n]/, /\\./)), '"')),
      ),
    integer: (_) => /\d+/,
    decimal: (_) => token(choice(/\d+\.\d*([eE][+-]?\d+)?/, /\.\d+([eE][+-]?\d+)?/, /\d+[eE][+-]?\d+/)),
    boolean: (_) => choice(kw("true"), kw("false")),
    null: (_) => kw("null"),
    // ?, ?name, ?1
    parameter: (_) => token(seq("?", optional(/[A-Za-z0-9_]+/))),
    // ${name}: substituted by the REPL before the query is sent
    variable: (_) => token(seq("${", /[A-Za-z0-9_.\-]+/, "}")),

    // ── Identifiers ──────────────────────────────────────────────────────────
    // a, a.b.c, `weird name`.b, @timestamp
    identifier: ($) => prec.right(seq($._identifier_part, repeat(seq(".", $._identifier_part)))),
    _identifier_part: ($) => choice($._unquoted_identifier, $._quoted_identifier),
    _unquoted_identifier: (_) => /[A-Za-z_@][A-Za-z0-9_@]*/,
    _quoted_identifier: (_) => token(seq("`", /([^`]|``)*/, "`")),

    // keep/drop/rename accept wildcards: host.*, *_id
    identifier_pattern: ($) => choice($.identifier, $.variable, token(/[A-Za-z0-9_@.*]*\*[A-Za-z0-9_@.*]*/)),

    comment: (_) => token(choice(seq("//", /[^\n]*/), seq("/*", /[^*]*\*+([^/*][^*]*\*+)*/, "/"))),
  },
});
