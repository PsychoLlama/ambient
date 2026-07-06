; locals.scm - Scope and definition queries for nvim-treesitter
; Used for features like highlighting definitions vs references

; -----------------------------------------------------------------------------
; Scopes
; -----------------------------------------------------------------------------

; File scope
(source_file) @local.scope

; Function bodies introduce scope
(function_definition) @local.scope

; Impl blocks introduce scope (type parameters span the methods)
(impl_definition) @local.scope

; Blocks introduce scope
(block) @local.scope

; Lambdas introduce scope
(lambda) @local.scope

; Match arms introduce scope (for pattern bindings)
(match_arm) @local.scope

; Handle expressions introduce scope
(handle_expression) @local.scope

; Handler arms introduce scope (for resume parameter)
(handler_arm) @local.scope

; -----------------------------------------------------------------------------
; Definitions
; -----------------------------------------------------------------------------

; Function definitions
(function_definition
  name: (identifier) @local.definition.function)

; Const definitions
(const_definition
  name: (identifier) @local.definition.constant)

; Type definitions
(type_definition
  name: (identifier) @local.definition.type)
(struct_definition
  name: (identifier) @local.definition.type)

; Enum definitions
(enum_definition
  name: (identifier) @local.definition.type)

; Enum variants
(enum_variant
  name: (identifier) @local.definition.enum)

; Ability definitions
(ability_definition
  name: (identifier) @local.definition.type)

; Trait definitions
(trait_definition
  name: (identifier) @local.definition.type)

; Function parameters
(parameter
  name: (identifier) @local.definition.parameter)

; Lambda parameters
(lambda_parameter
  name: (identifier) @local.definition.parameter)

; Let bindings - identifier pattern
(let_statement
  pattern: (identifier) @local.definition.var)

; Let bindings - tuple pattern
(tuple_pattern
  (identifier) @local.definition.var)

; Let bindings - record pattern
(record_pattern_field
  name: (identifier) @local.definition.var)

; Match pattern bindings
(variant_pattern
  (identifier) @local.definition.var)

; Handler arm parameters
(handler_arm
  (parameter_list
    (parameter
      name: (identifier) @local.definition.parameter)))

; -----------------------------------------------------------------------------
; References
; -----------------------------------------------------------------------------

; All other identifiers are references
(identifier) @local.reference
