// Tree-sitter grammar for the Linkrs query language.
//
// The reserved-word rules are generated from `data/keywords.json`, which is
// itself exported from the Rust lexer. Editing keyword spellings here is not
// supported; regenerate the inventory instead.

const inv = require('./data/keywords.json');

// Relative precedence for unary operators.
const PREC = { UNARY: 10, NOT: 9 };

// Build a case-insensitive regex source for a keyword spelling.
function ci(word) {
  return word
    .split('')
    .map((ch) => (/[A-Za-z]/.test(ch) ? `[${ch.toLowerCase()}${ch.toUpperCase()}]` : ch))
    .join('');
}

function slug(word) {
  return word.toLowerCase().replace(/[^a-z0-9]+/g, '_');
}

// Spellings that are not standalone keyword tokens in the grammar.
const SKIP = new Set(['TRUE', 'FALSE', 'BALANCE']);

const generated = {};
for (const kw of inv.keywords) {
  if (kw.spelling.includes(' ') || SKIP.has(kw.spelling)) continue;
  generated['kw_' + slug(kw.spelling)] = () => prec(1, new RegExp(ci(kw.spelling)));
}

function descWord($) {
  return choice($.kw_desc, $.kw_describe);
}

function buildRules() {
  return {
    source_file: ($) => repeat($.statement),

    // Keyword rules must come before any rule that can match the same text
    // (notably the `identifier` word rule): tree-sitter only treats them as
    // extractable keywords when they are defined ahead of the word rule.
    // `source_file` stays first so it remains the start symbol.
    ...generated,

    statement: ($) => seq($.segment, repeat(seq($.pipe, $.segment)), optional(';')),

    segment: ($) =>
      choice(
        $.match_segment,
        $.optional_match_segment,
        $.go_segment,
        $.lookup_segment,
        $.fetch_segment,
        $.find_segment,
        $.get_segment,
        $.return_segment,
        $.with_segment,
        $.unwind_segment,
        $.yield_segment,
        $.where_segment,
        $.order_by_segment,
        $.group_by_segment,
        $.having_segment,
        $.skip_segment,
        $.limit_segment,
        $.insert_segment,
        $.update_segment,
        $.delete_segment,
        $.upsert_segment,
        $.merge_segment,
        $.create_segment,
        $.drop_segment,
        $.alter_segment,
        $.show_segment,
        $.describe_segment,
        $.use_segment,
      ),

    // ---- patterns -------------------------------------------------------

    match_segment: ($) => seq($.kw_match, $.match_pattern),
    optional_match_segment: ($) => seq($.kw_optional, $.kw_match, $.match_pattern),

    match_pattern: ($) => seq($.pattern_part, repeat(seq(',', $.pattern_part))),
    pattern_part: ($) =>
      seq($.node_pattern, repeat(seq($.edge_pattern, $.node_pattern))),

    node_pattern: ($) =>
      seq(
        '(',
        optional($.identifier),
        optional($.node_labels),
        optional($.node_props),
        ')',
      ),
    node_labels: ($) => repeat1(seq(':', $.identifier)),
    node_props: ($) => seq('{', optional($.prop_list), '}'),

    edge_pattern: ($) =>
      choice(
        seq('-', optional($.edge_detail), $.arrow),
        seq($.back_arrow, optional($.edge_detail), '-'),
      ),
    edge_detail: ($) =>
      seq(
        '[',
        optional($.identifier),
        optional($.edge_types),
        optional($.edge_props),
        optional($.edge_length),
        ']',
      ),
    edge_types: ($) => seq(':', $.identifier, repeat(seq($.pipe, $.identifier))),
    edge_props: ($) => seq('{', optional($.prop_list), '}'),
    edge_length: ($) =>
      seq('*', optional($.range), optional($.path_modifier)),
    range: ($) => seq(optional($.integer), '..', optional($.integer)),
    path_modifier: ($) =>
      choice(
        $.kw_trail,
        $.kw_acyclic,
        $.kw_shortest,
        $.kw_allshortestpaths,
        seq($.kw_weighted, optional(seq('(', $.identifier, ')'))),
      ),

    prop_list: ($) => seq($.prop_pair, repeat(seq(',', $.prop_pair))),
    prop_pair: ($) => seq($.identifier, ':', $._expr),

    // ---- traversal ------------------------------------------------------

    go_segment: ($) =>
      seq(
        $.kw_go,
        optional($.go_steps),
        optional($.go_from),
        optional($.go_over),
        optional($.go_direction),
      ),
    go_steps: ($) => seq(optional($.integer), choice($.kw_step, $.kw_steps)),
    go_from: ($) => seq($.kw_from, $.expr_list),
    go_over: ($) => seq($.kw_over, $.identifier_list),
    go_direction: ($) => choice($.kw_reversely, $.kw_bidirect, $.kw_in),

    lookup_segment: ($) =>
      choice(
        seq($.kw_lookup, $.kw_on, choice($.kw_tag, $.kw_edge), $.identifier, optional($.lookup_index)),
        seq($.kw_lookup, $.kw_on, $.identifier, optional($.lookup_index)),
        seq($.kw_lookup, $.identifier, optional($.lookup_index)),
        seq($.kw_lookup, optional($.lookup_index)),
      ),
    lookup_index: ($) => seq($.kw_index, optional($.identifier)),

    fetch_segment: ($) =>
      seq(
        $.kw_fetch,
        optional($.kw_prop),
        optional($.kw_on),
        $.identifier,
        choice($.star, $.expr_list),
      ),

    find_segment: ($) =>
      seq(
        $.kw_find,
        optional(choice($.kw_shortest, $.kw_allshortestpaths)),
        optional($.kw_path),
        optional(seq($.kw_from, $.vid_list)),
        optional(seq($.kw_to, $.vid_list)),
        optional(seq($.kw_over, $.identifier_list)),
        optional(seq($.kw_upto, $.integer, $.kw_steps)),
      ),

    get_segment: ($) =>
      seq(
        $.kw_get,
        $.kw_subgraph,
        optional(seq($.integer, $.kw_steps)),
        optional(seq($.kw_from, $.vid_list)),
        optional(seq($.kw_over, $.identifier_list)),
      ),

    // ---- projection -----------------------------------------------------

    return_segment: ($) =>
      seq($.kw_return, optional($.kw_distinct), $.return_items),
    with_segment: ($) =>
      seq($.kw_with, optional($.kw_distinct), $.return_items),
    yield_segment: ($) =>
      seq($.kw_yield, optional($.kw_distinct), $.return_items),
    unwind_segment: ($) =>
      seq($.kw_unwind, $._expr, $.kw_as, $.identifier),

    return_items: ($) =>
      choice(
        $.star,
        seq($.return_item, repeat(seq(',', $.return_item))),
      ),
    return_item: ($) => seq($._expr, optional(seq($.kw_as, $.identifier))),

    where_segment: ($) => seq($.kw_where, $._expr),
    order_by_segment: ($) => seq($.kw_order, $.kw_by, $.order_items),
    order_items: ($) => seq($.order_item, repeat(seq(',', $.order_item))),
    order_item: ($) =>
      seq($._expr, optional(choice($.kw_asc, descWord($), $.kw_ascending, $.kw_descending))),
    group_by_segment: ($) => seq($.kw_group, $.kw_by, $.expr_list),
    having_segment: ($) => seq($.kw_having, $._expr),
    skip_segment: ($) => seq($.kw_skip, $._expr),
    limit_segment: ($) => seq($.kw_limit, $._expr),

    // ---- DML ------------------------------------------------------------

    insert_segment: ($) =>
      seq(
        $.kw_insert,
        choice($.kw_vertex, $.kw_edge),
        $.identifier,
        optional($.prop_defs),
        $.kw_values,
        $.value_entry,
        repeat(seq(',', $.value_entry)),
      ),
    prop_defs: ($) => seq('(', $.identifier_list, ')'),
    value_entry: ($) =>
      choice(
        seq($.vid, ':', '(', optional($.expr_list), ')'),
        seq($.vid, $.arrow, $.vid, ':', '(', optional($.expr_list), ')'),
      ),

    update_segment: ($) =>
      seq(
        $.kw_update,
        choice($.kw_vertex, $.kw_edge),
        $.identifier,
        optional($.vid),
        $.kw_set,
        $.set_list,
        optional($.where_segment),
      ),
    set_list: ($) =>
      seq($.set_pair, repeat(seq(',', $.set_pair))),
    set_pair: ($) => seq($.identifier, '=', $._expr),

    delete_segment: ($) =>
      choice(
        seq($.kw_delete, $.kw_vertex, $.identifier, $.kw_from, $.vid_list, optional($.where_segment)),
        seq($.kw_delete, $.kw_edge, optional($.identifier), $.vid, $.arrow, $.vid, repeat(seq(',', $.vid, $.arrow, $.vid)), optional($.where_segment)),
      ),

    upsert_segment: ($) =>
      seq(
        $.kw_upsert,
        choice($.kw_vertex, $.kw_edge),
        $.identifier,
        optional($.prop_defs),
        $.kw_values,
        $.value_entry,
        optional($.set_list),
      ),

    merge_segment: ($) =>
      seq($.kw_merge, $.node_pattern, optional($.merge_action)),
    merge_action: ($) =>
      seq(
        optional($.kw_on),
        choice($.kw_create, $.kw_match),
        optional($.kw_set),
        optional($.set_list),
      ),

    // ---- DDL ------------------------------------------------------------

    create_segment: ($) =>
      seq(
        $.kw_create,
        choice($.kw_tag, $.kw_edge, $.kw_index, $.kw_space),
        optional($.if_not_exists),
        $.identifier,
        optional($.create_tail),
      ),
    if_not_exists: ($) => seq($.kw_if, $.kw_not, $.kw_exists),
    create_tail: ($) =>
      choice(
        seq('(', $.column_defs, ')'),
        seq($.kw_on, $.identifier, '(', $.identifier_list, ')'),
      ),
    column_defs: ($) => seq($.column_def, repeat(seq(',', $.column_def))),
    column_def: ($) => seq($.identifier, $.type_name, optional($.column_constraint)),
    column_constraint: ($) =>
      seq(choice($.kw_not, $.kw_null, $.kw_default), optional($._expr)),
    type_name: ($) =>
      choice(
        $.identifier,
        $.kw_int,
        $.kw_int8,
        $.kw_int16,
        $.kw_int32,
        $.kw_int64,
        $.kw_float,
        $.kw_double,
        $.kw_string,
        $.kw_fixed_string,
        $.kw_bool,
        $.kw_timestamp,
        $.kw_date,
        $.kw_time,
        $.kw_datetime,
      ),

    drop_segment: ($) =>
      seq(
        $.kw_drop,
        choice($.kw_tag, $.kw_edge, $.kw_index, $.kw_space),
        optional($.if_exists),
        $.identifier,
      ),
    if_exists: ($) => seq($.kw_if, $.kw_exists),

    alter_segment: ($) =>
      choice(
        seq(
          $.kw_alter,
          choice($.kw_tag, $.kw_edge, $.kw_index, $.kw_space),
          $.identifier,
          $.kw_add,
          optional($.column_defs),
        ),
        seq(
          $.kw_alter,
          choice($.kw_tag, $.kw_edge, $.kw_index, $.kw_space),
          $.identifier,
          $.kw_change,
          optional($.column_defs),
        ),
        seq(
          $.kw_alter,
          choice($.kw_tag, $.kw_edge, $.kw_index, $.kw_space),
          $.identifier,
          $.kw_drop,
          optional($.column_defs),
        ),
      ),

    show_segment: ($) =>
      choice(
        seq($.kw_show, $.kw_create, $.show_object, optional($.identifier)),
        seq($.kw_show, $.show_object, optional($.identifier)),
        seq($.kw_show, $.identifier),
        seq($.kw_show),
      ),
    show_object: ($) => choice(
      $.kw_tag, $.kw_tags, $.kw_edge, $.kw_edges, $.kw_space, $.kw_spaces,
      $.kw_index, $.kw_indexes,
    ),
    describe_segment: ($) =>
      choice(
        seq(descWord($), $.show_object, $.identifier),
        seq(descWord($), $.show_object),
        seq(descWord($), $.identifier),
        seq(descWord($)),
      ),
    use_segment: ($) => seq($.kw_use, $.identifier),

    // ---- expressions ----------------------------------------------------

    _expr: ($) => $.or_expr,

    or_expr: ($) =>
      prec.left(seq($.and_expr, repeat(seq($.kw_or, $.and_expr)))),
    and_expr: ($) =>
      prec.left(seq($.not_expr, repeat(seq($.kw_and, $.not_expr)))),
    not_expr: ($) =>
      choice($.comparison_expr, prec(PREC.NOT, seq($.kw_not, $.not_expr))),

    comparison_expr: ($) =>
      prec.left(
        seq(
          $.bitwise_expr,
          repeat(
            choice(
              seq(
                choice(
                  $.eq,
                  $.ne,
                  $.lt,
                  $.le,
                  $.gt,
                  $.ge,
                  $.regex,
                  $.kw_contains,
                  $.starts_with,
                  $.ends_with,
                  $.kw_in,
                  $.not_in,
                ),
                $.bitwise_expr,
              ),
              $.is_null,
              $.is_not_null,
              $.is_empty,
              $.is_not_empty,
              $.between_expr,
            ),
          ),
        ),
      ),
    starts_with: ($) => seq($.kw_starts, $.kw_with),
    ends_with: ($) => seq($.kw_ends, $.kw_with),
    is_null: ($) => seq($.kw_is, $.kw_null),
    is_not_null: ($) => seq($.kw_is, $.kw_not, $.kw_null),
    is_empty: ($) => seq($.kw_is, $.empty_word),
    is_not_empty: ($) => seq($.kw_is, $.kw_not, $.empty_word),
    not_in: ($) => seq($.kw_not, $.kw_in),
    empty_word: ($) => /[eE][mM][pP][tT][yY]/,
    between_expr: ($) =>
      seq($.kw_between, $.bitwise_expr, $.kw_and, $.bitwise_expr),

    bitwise_expr: ($) =>
      prec.left(
        seq(
          $.additive_expr,
          repeat(
            seq(
              choice($.pipe, $.ampersand, $.shift_left, $.shift_right),
              $.additive_expr,
            ),
          ),
        ),
      ),

    additive_expr: ($) =>
      prec.left(
        seq($.multiplicative_expr, repeat(seq(choice($.plus, $.minus), $.multiplicative_expr))),
      ),
    multiplicative_expr: ($) =>
      prec.left(
        seq($.power_expr, repeat(seq(choice($.star, $.div, $.mod), $.power_expr))),
      ),
    power_expr: ($) =>
      prec.right(seq($.unary_expr, optional(seq($.exp, $.power_expr)))),

    unary_expr: ($) =>
      choice(
        $.postfix_expr,
        prec(PREC.UNARY, seq(choice($.minus, $.not_op, $.tilde), $.unary_expr)),
      ),

    postfix_expr: ($) =>
      seq(
        $.primary_base,
        repeat(
          choice(
            seq('.', $.property_key),
            seq('[', optional($._expr), ']'),
          ),
        ),
      ),

    primary_base: ($) =>
      choice(
        $.literal,
        $.identifier,
        $.backtick_identifier,
        $.parameter,
        $.vector_literal,
        $.function_call,
        $.list_literal,
        $.map_literal,
        $.case_expr,
        seq('(', $._expr, ')'),
      ),

    // Keep the historical name as an alias so existing queries keep working.
    primary_expr: ($) => $.postfix_expr,

    literal: ($) =>
      choice(
        $.integer,
        $.float,
        $.string,
        $.boolean,
        $.kw_null,
      ),
    boolean: ($) => choice(token(new RegExp(ci('true'))), token(new RegExp(ci('false')))),

    vector_literal: ($) =>
      seq($.kw_vector, '[', optional($.vector_elements), ']'),
    vector_elements: ($) =>
      seq($.number_literal, repeat(seq(',', $.number_literal))),
    number_literal: ($) => choice($.float, $.integer),

    function_call: ($) =>
      seq(
        choice($.identifier, $.kw_count, $.kw_sum, $.kw_avg, $.kw_min, $.kw_max),
        '(',
        optional($.arg_list),
        ')',
      ),
    arg_list: ($) => seq($._expr, repeat(seq(',', $._expr))),

    list_literal: ($) => seq('[', optional($.expr_list), ']'),
    map_literal: ($) => seq('{', optional($.prop_list), '}'),
    case_expr: ($) =>
      seq(
        $.kw_case,
        optional($._expr),
        repeat1($.when_clause),
        optional(seq($.kw_else, $._expr)),
        $.kw_end,
      ),
    when_clause: ($) => seq($.kw_when, $._expr, $.kw_then, $._expr),

    parameter: ($) =>
      choice(
        seq('$', $.identifier),
        seq('@', $.identifier),
        $.src_ref,
        $.dst_ref,
        $.input_ref,
      ),
    property_key: ($) => choice($.identifier, $.backtick_identifier, $.integer),

    // ---- tokens ---------------------------------------------------------

    identifier: ($) => /[a-zA-Z_][a-zA-Z0-9_]*/,
    backtick_identifier: ($) => token(seq('`', /[^`\n]*/, '`')),
    integer: ($) => /[0-9]+/,
    float: ($) => /[0-9]+\.[0-9]+([eE][+-]?[0-9]+)?|[0-9]+[eE][+-]?[0-9]+/,
    string: ($) => token(choice(
      seq("'", /([^'\\\n]|\\.)*/, "'"),
      seq('"', /([^"\\\n]|\\.)*/, '"'),
    )),

    vid: ($) => choice($.integer, $.string, $.identifier),
    expr_list: ($) => seq($._expr, repeat(seq(',', $._expr))),
    identifier_list: ($) => seq($.identifier, repeat(seq(',', $.identifier))),
    vid_list: ($) => seq($.vid, repeat(seq(',', $.vid))),

    star: ($) => '*',
    plus: ($) => '+',
    minus: ($) => '-',
    div: ($) => '/',
    mod: ($) => '%',
    exp: ($) => '**',
    eq: ($) => '==',
    ne: ($) => choice('!=', '<>'),
    lt: ($) => '<',
    le: ($) => '<=',
    gt: ($) => '>',
    ge: ($) => '>=',
    regex: ($) => '=~',
    not_op: ($) => '!',
    tilde: ($) => '~',
    pipe: ($) => '|',
    double_pipe: ($) => '||',
    ampersand: ($) => '&',
    shift_left: ($) => '<<',
    shift_right: ($) => '>>',
    arrow: ($) => '->',
    back_arrow: ($) => '<-',
    arrow_right: ($) => '->>',
    hash_arrow: ($) => '#>',
    hash_arrow_right: ($) => '#>>',
    dollar: ($) => '$',
    at: ($) => '@',
    src_ref: ($) => '$^',
    dst_ref: ($) => '$$',
    input_ref: ($) => '$-',

    comment: ($) =>
      choice(
        token(seq('//', /[^\n]*/)),
        token(seq('--', /[^\n]*/)),
        token(seq('/*', /[^\n]*/)),
      ),
  };
}

module.exports = grammar({
  name: 'linkrs',

  extras: ($) => [$.comment, /\s/],

  word: ($) => $.identifier,

  // Bare leading keywords (SHOW, DESCRIBE, ...) can either continue the
  // current segment or start the next one when the separator is omitted, so
  // the alternatives are intentionally explored in parallel here.
  conflicts: ($) => [
    [$.show_segment],
    [$.delete_segment],
    [$.merge_segment],
    [$.order_item],
    [$.update_segment],
  ],

  rules: buildRules(),
});
