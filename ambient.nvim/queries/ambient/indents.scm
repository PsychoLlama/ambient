; indents.scm - Indentation queries for nvim-treesitter

; -----------------------------------------------------------------------------
; Indent increases (after opening brace/bracket)
; -----------------------------------------------------------------------------

; Blocks
(block) @indent.begin

; Function definitions (for the body)
(function_definition
  body: (block) @indent.begin)

; Enum definitions
(enum_definition) @indent.begin

; Ability definitions
(ability_definition) @indent.begin

; Type definitions with record body
(type_definition
  (record_type_body) @indent.begin)

; If expressions
(if_expression
  then: (block) @indent.begin)
(if_expression
  else: (block) @indent.begin)

; Match expressions
(match_expression) @indent.begin

; Match arms (for the body)
(match_arm) @indent.begin

; Handle expressions
(handle_expression) @indent.begin
(handler_block) @indent.begin

; Sandbox expressions
(sandbox_expression) @indent.begin

; Handler literals
(handler_literal) @indent.begin

; Record literals
(record_literal) @indent.begin

; List literals
(list_literal) @indent.begin

; Tuples (multiline)
(tuple) @indent.begin

; Parameter lists (multiline)
(parameter_list) @indent.begin
(argument_list) @indent.begin

; Lambda parameters
(lambda_parameters) @indent.begin

; -----------------------------------------------------------------------------
; Indent ends (closing braces/brackets)
; -----------------------------------------------------------------------------

"}" @indent.end
"]" @indent.end
")" @indent.end

; -----------------------------------------------------------------------------
; Branch points (same indent level as parent)
; -----------------------------------------------------------------------------

; Else branches should align with if
"else" @indent.branch

; Match arms should align with each other
(match_arm) @indent.branch

; Handler arms should align with each other
(handler_arm) @indent.branch

; Enum variants should align with each other
(enum_variant) @indent.branch

; -----------------------------------------------------------------------------
; Ignore certain nodes
; -----------------------------------------------------------------------------

; Don't auto-indent inside strings
(string) @indent.ignore

; Don't auto-indent comments
(comment) @indent.ignore
