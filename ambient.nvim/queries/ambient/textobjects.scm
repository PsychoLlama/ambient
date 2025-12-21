; textobjects.scm - Text object queries for nvim-treesitter-textobjects
; See: https://github.com/nvim-treesitter/nvim-treesitter-textobjects

; -----------------------------------------------------------------------------
; Functions
; -----------------------------------------------------------------------------

; Function definitions
(function_definition
  body: (block) @function.inner) @function.outer

; Ability methods (function signatures in ability definitions)
(ability_method) @function.outer

; Lambdas
(lambda
  (_) @function.inner) @function.outer

; Handler methods
(handler_method
  (_) @function.inner) @function.outer

; -----------------------------------------------------------------------------
; Classes (using enums, abilities, and type definitions as "class-like")
; -----------------------------------------------------------------------------

; Enum definitions
(enum_definition
  (enum_variant_list) @class.inner) @class.outer

; Ability definitions
(ability_definition
  (ability_method)* @class.inner) @class.outer

; Type definitions with record body
(type_definition
  (record_type_body) @class.inner) @class.outer

; -----------------------------------------------------------------------------
; Parameters
; -----------------------------------------------------------------------------

; Function parameters
(parameter) @parameter.inner @parameter.outer

; Lambda parameters
(lambda_parameter) @parameter.inner @parameter.outer

; -----------------------------------------------------------------------------
; Arguments (function calls)
; -----------------------------------------------------------------------------

; Call expressions
(call_expression
  (argument_list) @call.inner) @call.outer

; -----------------------------------------------------------------------------
; Blocks
; -----------------------------------------------------------------------------

(block) @block.outer

(block
  (_)* @block.inner)

; -----------------------------------------------------------------------------
; Conditionals
; -----------------------------------------------------------------------------

; If expressions
(if_expression
  then: (block) @conditional.inner) @conditional.outer

; Match expressions
(match_expression) @conditional.outer

(match_arm
  body: (_) @conditional.inner)

; -----------------------------------------------------------------------------
; Assignments (let statements)
; -----------------------------------------------------------------------------

(let_statement
  pattern: (_) @assignment.lhs
  value: (_) @assignment.rhs @assignment.inner) @assignment.outer

(const_definition
  name: (_) @assignment.lhs
  value: (_) @assignment.rhs @assignment.inner) @assignment.outer

; -----------------------------------------------------------------------------
; Comments
; -----------------------------------------------------------------------------

(comment) @comment.outer @comment.inner

; -----------------------------------------------------------------------------
; Statements
; -----------------------------------------------------------------------------

(let_statement) @statement.outer
(expression_statement) @statement.outer

; -----------------------------------------------------------------------------
; Numbers
; -----------------------------------------------------------------------------

(number) @number.inner
