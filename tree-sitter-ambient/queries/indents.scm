; indents.scm - Indentation queries for nvim-treesitter's indent module.
;
; The model (see nvim-treesitter's `indent.lua`): a line's indent is the number
; of enclosing `@indent.begin` nodes that open on an *earlier* line, minus one
; level for a leading closing delimiter (`@indent.branch`). So we mark every
; multi-line container as `@indent.begin` and let the closing `}`/`)`/`]` pull
; its own line back out. One indent per source row is applied, so a container
; and a body node that open on the same line (`match x {`, `let y = with {`)
; only count once.

; -----------------------------------------------------------------------------
; Containers — their inner lines indent one level.
; -----------------------------------------------------------------------------

; Brace bodies. `block` covers every `{ ... }` statement body (functions, `if`
; / `else`, `handle`, `sandbox`, handler-method arms); the rest open their
; braces directly on the definition/expression node, with no inner body node.
[
  (block)
  (record_type_body)
  (record_type)
  (record_literal)
  (record_pattern)
  (enum_definition)
  (ability_definition)
  (trait_definition)
  (impl_definition)
  (match_expression)
  (handler_literal)
  (use_group)
] @indent.begin

; Bracket / paren bodies.
[
  (list_literal)
  (tuple)
  (tuple_type)
  (tuple_pattern)
  (parameter_list)
  (argument_list)
  (lambda_parameters)
  (parenthesized_expression)
] @indent.begin

; Method chains — each `.method(...)` on its own line aligns one level under
; the receiver. The nested `member_expression`s all open on the receiver's
; row, so the one-indent-per-row rule keeps the whole chain at a single level.
(member_expression) @indent.begin

; Continuations — a construct whose value/body wraps onto later lines. Each
; opens on its keyword line, so a wrapped tail indents one level under it.
[
  (let_statement)
  (const_definition)
  (match_arm)
  (handler_method)
  (with_handle_expression)
] @indent.begin

; A wrapped signature clause (`with ...`, `where ...` dropped onto its own line
; below the parameter list) indents one level under `fn`. These clauses sit on
; a single line, so `immediate` lets them count and `start_at_same_line` lets
; the clause's own line pick up the level; because the clause is a sibling of
; the body block — not an ancestor of it — the body is left untouched.
((ability_clause) @indent.begin
  (#set! indent.immediate)
  (#set! indent.start_at_same_line))
((where_clause) @indent.begin
  (#set! indent.immediate)
  (#set! indent.start_at_same_line))

; -----------------------------------------------------------------------------
; Closing delimiters — a line that opens with one dedents back a level.
; -----------------------------------------------------------------------------

[
  "}"
  ")"
  "]"
] @indent.branch

; -----------------------------------------------------------------------------
; Ignore — never reflow the insides of strings or comments.
; -----------------------------------------------------------------------------

[
  (string)
  (comment)
  (doc_comment)
  (inner_doc_comment)
] @indent.ignore
