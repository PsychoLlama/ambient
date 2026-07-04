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
  "ability"
  "trait"
  "impl"
  "for"
  "use"
  "with"
  "handle"
  "sandbox"
  "unique"
] @keyword

(visibility) @keyword

; Literals
(boolean) @boolean
(number) @number
(uuid) @constant
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

(handler_arm
  ability: (identifier) @type
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

; Type definitions
(type_definition
  name: (identifier) @type)

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
