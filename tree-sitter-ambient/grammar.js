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
    // Brace-delimited constructs: `{ x ...` could open a handler or a block
    // until the token after the first identifier decides (a record is split
    // off separately below, since it also carries an optional type prefix).
    [$.handler_literal, $.block],
    // `Name {` could be a typed record or a bare name followed by a block
    // (as in `if cond { ... }`). The record needs `field:` inside the brace,
    // so the block reading wins wherever the brace isn't `{ ident: ... }`.
    [$._expression, $.record_literal],
    // Lambda parameters vs parenthesized expression: (x) could be either
    [$.lambda_parameter, $._expression],
    // `Ability::method(...)` prefix of a handler arm looks exactly like a
    // `scoped_identifier` call until the `=>` (or a param that isn't an
    // expression) settles it. Equal precedence + this conflict keeps the
    // handler branch alive under GLR instead of committing to the call.
    [$.handler_method, $.scoped_identifier],
    // Pattern ambiguities: `x` could be binding or variant pattern
    [$.variant_pattern, $._pattern],
    // Match guard with () could be lambda params or unit
    [$.lambda_parameters, $.unit],
    // `use a::b::…`: after each segment a `::` could extend the path or
    // introduce a `::{group}` / `::*` tail — the token after `::` decides,
    // so keep both readings alive under GLR.
    [$.use_path],
  ],

  rules: {
    // ─────────────────────────────────────────────────────────────────────────
    // Top-level
    // ─────────────────────────────────────────────────────────────────────────

    source_file: ($) => repeat($._item),

    _item: ($) =>
      choice(
        $.function_definition,
        $.extern_function_definition,
        $.const_definition,
        $.type_definition,
        $.struct_definition,
        $.enum_definition,
        $.ability_definition,
        $.trait_definition,
        $.impl_definition,
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
        optional($.where_clause),
        optional($.ability_clause),
        field("body", $.block)
      ),

    // `extern fn` declares a body-less signature implemented by the host;
    // it ends with `;` and takes no ability clause (extern fns are pure).
    extern_function_definition: ($) =>
      seq(
        optional($.visibility),
        "extern",
        "fn",
        field("name", $.identifier),
        optional($.type_parameters),
        $.parameter_list,
        optional(seq(":", field("return_type", $._type))),
        ";"
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
        "type",
        field("name", $.identifier),
        optional($.type_parameters),
        "=",
        field("type", $._type),
        ";"
      ),

    struct_definition: ($) =>
      seq(
        optional($.visibility),
        // `extern` marks an engine-provided type: nameable and readable, but
        // not constructable by user code. It requires `unique(...)`; the
        // lowering pass enforces that.
        optional("extern"),
        optional($.unique_modifier),
        "struct",
        field("name", $.identifier),
        optional($.type_parameters),
        // A record body `{ ... }`, or `;` for a unit struct. The compiler's
        // lowering pass enforces the `unique(...)`-required and non-empty rules.
        choice($.record_type_body, ";")
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

    // Abilities are nominal: the `unique(<uuid>)` prefix is the identity
    // (mandatory, like `enum`; the lowering pass enforces it). Every method
    // carries a default implementation body; only the reserved Exception
    // carve-out stays abstract with a `;`.
    ability_definition: ($) =>
      seq(
        optional($.visibility),
        optional($.unique_modifier),
        "ability",
        field("name", $.identifier),
        optional($.type_parameters),
        optional($.ability_dependency),
        "{",
        repeat($.ability_method),
        "}"
      ),

    // Dependencies are abilities, so they take the same references as any
    // other `with` clause — bare (`Stdio`) or fully-qualified
    // (`core::system::Stdio`).
    ability_dependency: ($) => seq("with", $._ability_list),

    ability_method: ($) =>
      seq(
        "fn",
        field("name", $.identifier),
        optional($.type_parameters),
        $.parameter_list,
        ":",
        field("return_type", $._type),
        optional($.where_clause),
        // A block is the method's default implementation; a `;` leaves it
        // abstract (the Exception carve-out).
        choice(field("body", $.block), ";")
      ),

    // Traits declare shared behavior as a set of method signatures. Method
    // bodies live in `impl` blocks, so a trait method looks like an ability
    // method: `fn name(self, other: Self): Ret;`.
    trait_definition: ($) =>
      seq(
        optional($.visibility),
        // Traits are nominal: `unique(<uuid>)` is the identity (mandatory in
        // practice; the lowering pass enforces it, like `ability`).
        optional($.unique_modifier),
        "trait",
        field("name", $.identifier),
        optional($.type_parameters),
        "{",
        repeat(choice($.associated_type, $.trait_method)),
        "}"
      ),

    // An associated type item: declared in a trait body (`type Error;`),
    // bound in an impl body (`type Error = String;`). One rule covers both
    // (the value is optional); the compiler enforces which form is legal
    // where.
    associated_type: ($) =>
      seq(
        "type",
        field("name", $.identifier),
        optional(seq("=", field("type", $._type))),
        ";"
      ),

    trait_method: ($) =>
      seq(
        "fn",
        field("name", $.identifier),
        optional($.type_parameters),
        $.parameter_list,
        ":",
        field("return_type", $._type),
        optional($.where_clause),
        ";"
      ),

    // `impl Trait for Type { ... }` provides a trait's methods; `impl Type
    // { ... }` (no `for`) attaches inherent methods directly. Both may be
    // generic over the target's type parameters (`impl<T> Option<T> { ... }`).
    impl_definition: ($) =>
      seq(
        "impl",
        optional($.type_parameters),
        optional(seq(field("trait", $._type), "for")),
        field("type", $._type),
        optional($.where_clause),
        "{",
        repeat(choice($.associated_type, $.function_definition)),
        "}"
      ),

    // Rust-style use trees: a path may end in `::*` (glob), `::{ ... }` (a
    // nestable brace group), or `as alias`. Path-root keywords (`pkg`,
    // `core`, `self`, `super`) are ordinary identifiers to the grammar; the
    // compiler validates their placement during lowering.
    use_declaration: ($) => seq(optional($.visibility), "use", $.use_tree, ";"),

    use_tree: ($) =>
      choice(
        $.use_group,
        seq(
          field("path", $.use_path),
          optional(
            choice(
              seq("::", "*"),
              seq("::", $.use_group),
              seq("as", field("alias", $.identifier))
            )
          )
        )
      ),

    use_path: ($) => seq($.identifier, repeat(seq("::", $.identifier))),

    use_group: ($) =>
      seq(
        "{",
        optional(seq($.use_tree, repeat(seq(",", $.use_tree)), optional(","))),
        "}"
      ),

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

    // A type parameter is a name (`T`), an ability variable (`E!`), or a
    // bounded parameter (`T: Eq + Into<Money>`). Bounds are `+`-separated
    // trait references, the same grammar the `where` clause inlines here.
    type_parameter: ($) =>
      seq(
        field("name", $.identifier),
        optional("!"),
        optional(seq(":", $.trait_bounds))
      ),

    // `where` moves a type parameter's bounds off the `<...>` list:
    // `where T: From<String>, U: Eq + Ord`.
    where_clause: ($) =>
      prec.right(
        seq(
          "where",
          $.where_predicate,
          repeat(seq(",", $.where_predicate)),
          optional(",")
        )
      ),

    where_predicate: ($) =>
      seq(field("type", $._type), ":", $.trait_bounds),

    trait_bounds: ($) =>
      seq($.trait_bound, repeat(seq("+", $.trait_bound))),

    // A trait reference in bound position: a bare or qualified name with
    // optional type arguments (`Eq`, `Into<Money>`, `m::Convert<Number>`).
    trait_bound: ($) =>
      seq(
        field("name", choice($.identifier, $.scoped_identifier)),
        optional(seq("<", $._type_list, ">"))
      ),

    ability_clause: ($) => seq("with", $._ability_list),

    _ability_list: ($) =>
      prec.left(seq($._ability_ref, repeat(seq(",", $._ability_ref)))),

    // An ability reference: bare (`Stdio`), fully-qualified
    // (`core::system::Stdio`), or `_` (ability-polymorphic).
    _ability_ref: ($) => choice($.identifier, $.scoped_identifier, "_"),

    _type: ($) =>
      choice(
        $.identifier,
        // Qualified type heads: a module path (`core::time::Duration`) or an
        // associated-type projection (`Self::Error`).
        $.scoped_identifier,
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

    // `Handler` is type syntax, not a name: the first argument is an ability
    // reference (bare or `::`-qualified), the optional second the answer type.
    handler_type: ($) =>
      seq(
        "Handler",
        "<",
        field("ability", choice($.identifier, $.scoped_identifier)),
        optional(seq(",", field("answer", $._type))),
        ">",
      ),

    unit_type: ($) => seq("(", ")"),

    _type_list: ($) => seq($._type, repeat(seq(",", $._type))),


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
        $.with_handle_expression,
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
        optional(seq($._expression, repeat(seq(",", $._expression)), optional(","))),
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
      prec.right(
        seq(
          "if",
          field("condition", $._expression),
          field("then", $.block),
          optional(seq("else", field("else", choice($.block, $.if_expression))))
        )
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

    // A variant pattern names a constructor by a bare or qualified path,
    // matching expression position: `Circle(r)`, `Shape::Circle(r)`, and
    // path-root-keyword heads `pkg::shapes::Shape::Circle`, `core::…`. Path-root
    // keywords are ordinary identifiers to the grammar, so `scoped_identifier`
    // already admits them.
    variant_pattern: ($) =>
      seq(
        field("name", choice($.identifier, $.scoped_identifier)),
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

    // A block may open with `use` imports scoped to the rest of the block.
    _statement: ($) =>
      choice($.let_statement, $.use_declaration, $.expression_statement),

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

    // Record construction. `TypeName { field: value }` (typed) or a bare
    // `{ field: value }` (structural). The type prefix is a plain or
    // `::`-qualified name. At least one `field:` is required — an empty `{}`
    // is always a block, never a record — which is what disambiguates a bare
    // record from a block and a typed record from `expr { block }`.
    record_literal: ($) =>
      seq(
        optional(field("type", choice($.identifier, $.scoped_identifier))),
        "{",
        $.record_fields,
        "}"
      ),

    record_fields: ($) =>
      seq($.record_field, repeat(seq(",", $.record_field)), optional(",")),

    record_field: ($) =>
      seq(field("name", $.identifier), ":", field("value", $._expression)),

    with_handle_expression: ($) =>
      prec.right(
        seq(
          "with",
          $.handler_list,
          "handle",
          field("body", $._expression),
          optional(seq("else", $._expression))
        )
      ),

    handler_list: ($) =>
      seq($._expression, repeat(seq(",", $._expression))),

    handler_literal: ($) =>
      seq("{", repeat($.handler_method), "}"),

    handler_method: ($) =>
      prec(
        PREC.MEMBER,
        seq(
          // The ability may be bare (`Stdio`) or fully-qualified
          // (`core::system::Stdio`); the trailing `::method` is split off.
          field("ability", choice($.identifier, $.scoped_identifier)),
          "::",
          field("method", $.identifier),
          $.parameter_list,
          "=>",
          choice($.block, seq($._expression, optional(",")))
        )
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

    // A run of ordinary characters, or a lone `$` that does not open an
    // interpolation. The `${` case is claimed by string_interpolation, which
    // wins by longest-match, so a bare `$` (even right before the closing
    // quote, as in "$") stays literal text.
    string_content: ($) => /[^"\\$]+|\$/,

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
