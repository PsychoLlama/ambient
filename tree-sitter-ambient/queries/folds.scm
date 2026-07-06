; folds.scm - Code folding queries for nvim-treesitter

; Functions
(function_definition) @fold

; Const definitions (multiline)
(const_definition) @fold

; Type definitions
(type_definition) @fold
(struct_definition) @fold

; Enum definitions
(enum_definition) @fold

; Ability definitions
(ability_definition) @fold

; Trait definitions
(trait_definition) @fold

; Impl blocks
(impl_definition) @fold

; Blocks
(block) @fold

; If expressions
(if_expression) @fold

; Match expressions
(match_expression) @fold

; Handle expressions
(handle_expression) @fold

; Sandbox expressions
(sandbox_expression) @fold

; Handler literals
(handler_literal) @fold

; Lambdas (multiline)
(lambda) @fold

; Record literals
(record_literal) @fold

; List literals
(list_literal) @fold

; Comments (for folding consecutive comments)
(comment) @fold
