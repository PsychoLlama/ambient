; Keywords
[
  "fn"
  "let"
  "const"
  "if"
  "else"
  "match"
  "enum"
  "type"
  "struct"
  "ability"
  "trait"
  "impl"
  "for"
  "use"
  "as"
  "with"
  "handle"
  "sandbox"
  "unique"
  "extern"
] @keyword

(visibility) @keyword

; Literals
(boolean) @boolean
(number) @number

; Collapse UUIDs to a single glyph inline; the real text is revealed when the
; cursor sits on the line (see `concealcursor` in ftplugin/ambient.lua).
((uuid) @constant
  (#set! conceal "…"))
(string) @string
(string_content) @string
(escape_sequence) @string.escape
(comment) @comment
(doc_comment) @comment.documentation
(inner_doc_comment) @comment.documentation

; Types
(type_parameter) @type.parameter
(generic_type (identifier) @type)
(handler_type "Handler" @type.builtin)
(ability_type "Ability" @type.builtin)

; Functions
(function_definition
  name: (identifier) @function)

(extern_function_definition
  name: (identifier) @function)

(ability_method
  name: (identifier) @function.method)

(trait_method
  name: (identifier) @function.method)

(call_expression
  (identifier) @function.call)

(call_expression
  (member_expression
    (identifier) @function.call .))

; Namespace-qualified call: `core::math::abs(...)`
(call_expression
  (scoped_identifier
    name: (identifier) @function.call))

; Path segments of a namespace path read as namespaces/modules.
(scoped_identifier
  path: (identifier) @namespace)
(scoped_identifier
  path: (scoped_identifier
    name: (identifier) @namespace))

; Abilities
(ability_definition
  name: (identifier) @type)

(handler_method
  ability: (identifier) @type
  method: (identifier) @function.method)

; Fully-qualified handler arm (`core::system::Stdio::out(...) => ...`): the
; final path segment is the ability name.
(handler_method
  ability: (scoped_identifier
    name: (identifier) @type)
  method: (identifier) @function.method)

; Traits and impls
(trait_definition
  name: (identifier) @type)

(impl_definition
  trait: (identifier) @type)

(impl_definition
  type: (identifier) @type)

; Enums
(enum_definition
  name: (identifier) @type)

(enum_variant
  name: (identifier) @constructor)

(variant_pattern
  name: (identifier) @constructor)
; Qualified variant pattern (`shapes::Shape::Circle`, `pkg::…`): the final
; path segment is the constructor; leading segments read as namespaces above.
(variant_pattern
  name: (scoped_identifier
    name: (identifier) @constructor))

; Type definitions
(type_definition
  name: (identifier) @type)
(struct_definition
  name: (identifier) @type)

; Record construction: the `TypeName` in `TypeName { ... }`
(record_literal
  type: (identifier) @type)
(record_literal
  type: (scoped_identifier
    name: (identifier) @type))

(record_field
  name: (identifier) @property)

; Parameters
(parameter
  name: (identifier) @variable.parameter)

(lambda_parameter
  name: (identifier) @variable.parameter)

; Variables
(let_statement
  pattern: (identifier) @variable)

(identifier) @variable

; Operators
[
  "+"
  "-"
  "*"
  "/"
  "%"
  "=="
  "!="
  "<"
  "<="
  ">"
  ">="
  "&&"
  "||"
  "!"
  "="
  "=>"
  "->"
] @operator

; Punctuation
[
  "("
  ")"
  "{"
  "}"
  "["
  "]"
] @punctuation.bracket

[
  ","
  ";"
  ":"
  "::"
  "."
] @punctuation.delimiter

; Special
(perform_expression "!" @operator.special)
(wildcard_pattern) @variable.builtin
(unit) @constant.builtin
