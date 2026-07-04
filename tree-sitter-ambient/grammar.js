/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

/**
 * Tree-sitter grammar for the Ambient programming language.
 *
 * Ambient is a content-addressed, ability-based programming language
 * inspired by Unison, Rust, and TypeScript.
 */

const PREC = {
  COMMENT: 0,
  OR: 1,
  AND: 2,
  COMPARE: 3,
  ADD: 4,
  MULT: 5,
  UNARY: 6,
  PERFORM: 7, // ! operator (lower than call so it wraps the call)
  CALL: 8,
  MEMBER: 9,
};

module.exports = grammar({
  name: "ambient",

  extras: ($) => [/\s/, $.comment, $.doc_comment, $.inner_doc_comment],

  word: ($) => $.identifier,

  conflicts: ($) => [
    // Brace-delimited constructs: {} could be empty block, record, or handler
    [$.handler_literal, $.record_literal, $.block],
    // Lambda parameters vs parenthesized expression: (x) could be either
    [$.lambda_parameter, $._expression],
    // Handler method starts with identifier, conflicts with expression
    [$.handler_method, $._expression],
    // Pattern ambiguities: `x` could be binding or variant pattern
    [$.variant_pattern, $._pattern],
    // Match guard with () could be lambda params or unit
    [$.lambda_parameters, $.unit],
  ],

  rules: {
    // ─────────────────────────────────────────────────────────────────────────
    // Top-level
    // ─────────────────────────────────────────────────────────────────────────

    source_file: ($) => repeat($._item),

    _item: ($) =>
      choice(
        $.function_definition,
        $.const_definition,
        $.type_definition,
        $.enum_definition,
        $.ability_definition,
        $.use_declaration
      ),

    // ─────────────────────────────────────────────────────────────────────────
    // Declarations
    // ─────────────────────────────────────────────────────────────────────────

    function_definition: ($) =>
      seq(
        optional($.visibility),
        "fn",
        field("name", $.identifier),
        optional($.type_parameters),
        $.parameter_list,
        optional(seq(":", field("return_type", $._type))),
        optional($.ability_clause),
        field("body", $.block)
      ),

    const_definition: ($) =>
      seq(
        optional($.visibility),
        "const",
        field("name", $.identifier),
        ":",
        field("type", $._type),
        "=",
        field("value", $._expression),
        ";"
      ),

    type_definition: ($) =>
      seq(
        optional($.visibility),
        optional($.unique_modifier),
        "type",
        field("name", $.identifier),
        optional($.type_parameters),
        optional(seq("=", field("type", $._type))),
        optional($.record_type_body),
        optional(";")
      ),

    unique_modifier: ($) => seq("unique", "(", $.uuid, ")"),

    // UUID literals are uppercase-only (canonical 8-4-4-4-12 hex). Lowercase
    // hex is reserved for identifiers/numbers, matching the compiler's lexer.
    uuid: ($) =>
      /[0-9A-F]{8}-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{12}/,

    enum_definition: ($) =>
      seq(
        optional($.visibility),
        optional($.unique_modifier),
        "enum",
        field("name", $.identifier),
        optional($.type_parameters),
        "{",
        optional($.enum_variant_list),
        "}"
      ),

    enum_variant_list: ($) =>
      seq($.enum_variant, repeat(seq(",", $.enum_variant)), optional(",")),

    enum_variant: ($) =>
      seq(
        field("name", $.identifier),
        optional(seq("(", optional($._type_list), ")"))
      ),

    ability_definition: ($) =>
      seq(
        optional($.visibility),
        "ability",
        field("name", $.identifier),
        optional($.type_parameters),
        optional($.ability_dependency),
        "{",
        repeat($.ability_method),
        "}"
      ),

    ability_dependency: ($) => seq("with", $.identifier_list),

    ability_method: ($) =>
      seq(
        "fn",
        field("name", $.identifier),
        optional($.type_parameters),
        $.parameter_list,
        ":",
        field("return_type", $._type),
        ";"
      ),

    use_declaration: ($) => seq(optional($.visibility), "use", $.use_path, ";"),

    use_path: ($) =>
      seq(
        $.identifier,
        repeat(seq("::", $.identifier)),
        optional(choice(seq("::", "*"), seq("::", $.use_group)))
      ),

    use_group: ($) => seq("{", $.identifier_list, "}"),

    // ─────────────────────────────────────────────────────────────────────────
    // Parameters and Types
    // ─────────────────────────────────────────────────────────────────────────

    visibility: ($) => "pub",

    parameter_list: ($) =>
      seq("(", optional(seq($.parameter, repeat(seq(",", $.parameter)))), ")"),

    parameter: ($) =>
      seq(field("name", $.identifier), optional(seq(":", field("type", $._type)))),

    type_parameters: ($) =>
      seq("<", $.type_parameter, repeat(seq(",", $.type_parameter)), ">"),

    type_parameter: ($) => seq($.identifier, optional("!")),

    ability_clause: ($) => seq("with", $._ability_list),

    _ability_list: ($) =>
      prec.left(seq($._ability_ref, repeat(seq(",", $._ability_ref)))),

    _ability_ref: ($) => choice($.identifier, "_"),

    _type: ($) =>
      choice(
        $.identifier,
        $.generic_type,
        $.function_type,
        $.tuple_type,
        $.record_type,
        $.ability_type,
        $.handler_type,
        $.unit_type,
        $.never_type,
        $.infer_type
      ),

    never_type: ($) => "!",

    infer_type: ($) => "_",

    generic_type: ($) =>
      seq($.identifier, "<", $._type_list, ">"),

    function_type: ($) =>
      prec.left(
        seq(
          "(",
          optional($._type_list),
          ")",
          "->",
          $._type,
          optional($.ability_clause)
        )
      ),

    tuple_type: ($) => seq("(", $._type_list, ")"),

    record_type: ($) => seq("{", optional($.record_type_fields), "}"),

    record_type_body: ($) => seq("{", optional($.record_type_fields), "}"),

    record_type_fields: ($) =>
      seq($.record_type_field, repeat(seq(",", $.record_type_field)), optional(",")),

    record_type_field: ($) =>
      seq(field("name", $.identifier), ":", field("type", $._type)),

    ability_type: ($) =>
      seq("Ability", "<", $._type, ",", $.identifier, "!", ">"),

    handler_type: ($) => seq("Handler", "<", $.identifier, ">"),

    unit_type: ($) => seq("(", ")"),

    _type_list: ($) => seq($._type, repeat(seq(",", $._type))),

    identifier_list: ($) => seq($.identifier, repeat(seq(",", $.identifier))),

    // ─────────────────────────────────────────────────────────────────────────
    // Expressions
    // ─────────────────────────────────────────────────────────────────────────

    _expression: ($) =>
      choice(
        $.identifier,
        $.number,
        $.string,
        $.boolean,
        $.unit,
        $.binary_expression,
        $.unary_expression,
        $.call_expression,
        $.perform_expression,
        $.scoped_identifier,
        $.member_expression,
        $.tuple_index_expression,
        $.index_expression,
        $.if_expression,
        $.match_expression,
        $.block,
        $.lambda,
        $.tuple,
        $.list_literal,
        $.record_literal,
        $.handle_expression,
        $.sandbox_expression,
        $.handler_literal,
        $.parenthesized_expression
      ),

    parenthesized_expression: ($) => seq("(", $._expression, ")"),

    binary_expression: ($) =>
      choice(
        prec.left(PREC.OR, seq($._expression, "||", $._expression)),
        prec.left(PREC.AND, seq($._expression, "&&", $._expression)),
        prec.left(
          PREC.COMPARE,
          seq(
            $._expression,
            choice("==", "!=", "<", "<=", ">", ">="),
            $._expression
          )
        ),
        prec.left(PREC.ADD, seq($._expression, choice("+", "-"), $._expression)),
        prec.left(
          PREC.MULT,
          seq($._expression, choice("*", "/", "%"), $._expression)
        )
      ),

    unary_expression: ($) =>
      prec(PREC.UNARY, seq(choice("-", "!"), $._expression)),

    call_expression: ($) =>
      prec(PREC.CALL, seq($._expression, $.argument_list)),

    argument_list: ($) =>
      seq(
        "(",
        optional(seq($._expression, repeat(seq(",", $._expression)))),
        ")"
      ),

    perform_expression: ($) => prec(PREC.PERFORM, seq($._expression, "!")),

    // Namespace / path access uses `::` (`core::math::abs`, `platform::Fs`).
    scoped_identifier: ($) =>
      prec(
        PREC.MEMBER,
        seq(
          field("path", choice($.identifier, $.scoped_identifier)),
          "::",
          field("name", $.identifier)
        )
      ),

    // Value member access uses `.` (`record.field`).
    member_expression: ($) =>
      prec(PREC.MEMBER, seq($._expression, ".", $.identifier)),

    tuple_index_expression: ($) =>
      prec(PREC.MEMBER, seq($._expression, ".", $.number)),

    index_expression: ($) =>
      prec(PREC.MEMBER, seq($._expression, "[", $._expression, "]")),

    if_expression: ($) =>
      seq(
        "if",
        field("condition", $._expression),
        field("then", $.block),
        optional(seq("else", field("else", choice($.block, $.if_expression))))
      ),

    match_expression: ($) =>
      seq("match", field("value", $._expression), "{", repeat($.match_arm), "}"),

    match_arm: ($) =>
      seq(
        field("pattern", $._pattern),
        optional(seq("if", field("guard", $._expression))),
        "=>",
        field("body", $._expression),
        optional(",")
      ),

    _pattern: ($) =>
      choice(
        $.identifier,
        $.number,
        $.string,
        $.boolean,
        $.wildcard_pattern,
        $.tuple_pattern,
        $.variant_pattern,
        $.record_pattern
      ),

    wildcard_pattern: ($) => "_",

    tuple_pattern: ($) =>
      seq("(", $._pattern, repeat(seq(",", $._pattern)), ")"),

    variant_pattern: ($) =>
      seq(
        field("name", $.identifier),
        optional(seq("(", optional(seq($._pattern, repeat(seq(",", $._pattern)))), ")"))
      ),

    record_pattern: ($) =>
      seq("{", optional($.record_pattern_fields), "}"),

    record_pattern_fields: ($) =>
      seq(
        $.record_pattern_field,
        repeat(seq(",", $.record_pattern_field)),
        optional(",")
      ),

    record_pattern_field: ($) =>
      seq(field("name", $.identifier), optional(seq(":", $._pattern))),

    block: ($) => seq("{", repeat($._statement), optional($._expression), "}"),

    _statement: ($) => choice($.let_statement, $.expression_statement),

    let_statement: ($) =>
      seq(
        "let",
        field("pattern", $._pattern),
        optional(seq(":", field("type", $._type))),
        "=",
        field("value", $._expression),
        ";"
      ),

    expression_statement: ($) => seq($._expression, ";"),

    lambda: ($) =>
      prec.right(seq($.lambda_parameters, "=>", $._expression)),

    lambda_parameters: ($) =>
      seq(
        "(",
        optional(seq($.lambda_parameter, repeat(seq(",", $.lambda_parameter)))),
        ")"
      ),

    lambda_parameter: ($) =>
      seq(field("name", $.identifier), optional(seq(":", field("type", $._type)))),

    tuple: ($) =>
      seq("(", $._expression, ",", $._expression, repeat(seq(",", $._expression)), ")"),

    list_literal: ($) =>
      seq(
        "[",
        optional(seq($._expression, repeat(seq(",", $._expression)), optional(","))),
        "]"
      ),

    record_literal: ($) =>
      seq(
        optional(seq($.identifier, "::")),
        "{",
        optional($.record_fields),
        "}"
      ),

    record_fields: ($) =>
      seq($.record_field, repeat(seq(",", $.record_field)), optional(",")),

    record_field: ($) =>
      seq(field("name", $.identifier), ":", field("value", $._expression)),

    handle_expression: ($) =>
      seq(
        "handle",
        field("body", $._expression),
        optional(seq("with", $.handler_refs)),
        $.handler_block
      ),

    handler_refs: ($) =>
      seq($._expression, repeat(seq(",", $._expression))),

    handler_block: ($) =>
      seq(
        "{",
        repeat($.handler_arm),
        optional(seq("else", "{", $._expression, "}")),
        "}"
      ),

    handler_arm: ($) =>
      seq(
        field("ability", $.identifier),
        "::",
        field("method", $.identifier),
        $.parameter_list,
        "=>",
        $.block
      ),

    handler_literal: ($) =>
      seq("{", repeat($.handler_method), "}"),

    handler_method: ($) =>
      seq(
        field("name", $.identifier),
        $.parameter_list,
        "=>",
        choice($.block, seq($._expression, ",")),
      ),

    sandbox_expression: ($) =>
      seq(
        "sandbox",
        optional(seq("with", $._ability_list)),
        $.block
      ),

    unit: ($) => seq("(", ")"),

    // ─────────────────────────────────────────────────────────────────────────
    // Literals
    // ─────────────────────────────────────────────────────────────────────────

    identifier: ($) => /[a-zA-Z_][a-zA-Z0-9_]*/,

    number: ($) => /\d+(\.\d+)?/,

    string: ($) =>
      seq(
        '"',
        repeat(
          choice(
            $.string_content,
            $.escape_sequence,
            $.string_interpolation
          )
        ),
        '"'
      ),

    string_content: ($) => /[^"\\$]+|(\$[^{])/,

    escape_sequence: ($) => /\\[nrt\\"$]/,

    string_interpolation: ($) =>
      seq("${", $._expression, "}"),

    boolean: ($) => choice("true", "false"),

    // Doc comments (/// for items, //! for modules)
    doc_comment: ($) => token(seq("///", /.*/)),

    inner_doc_comment: ($) => token(seq("//!", /.*/)),

    // Regular comments (explicitly exclude /// and //!)
    comment: ($) => token(seq("//", /([^\/!\n].*)?/)),
  },
});
