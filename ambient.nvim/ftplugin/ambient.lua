-- Per-buffer setup for Ambient source files. Runs on every `ambient` buffer;
-- the grammar and language server are registered once by `require('ambient').setup()`.

-- Editor options.
vim.bo.commentstring = '// %s'
vim.bo.tabstop = 2
vim.bo.shiftwidth = 2
vim.bo.expandtab = true

-- Conceal UUIDs (e.g. inside `unique(...)`) down to a single glyph, driven by
-- the `@conceal` directive in the tree-sitter highlights query. `conceallevel`
-- and `concealcursor` are window-local, so `vim.wo` scopes them to the current
-- window (equivalent to `setlocal`). Level 2 hides the text entirely;
-- `concealcursor` is left empty so the real UUID reappears whenever the cursor
-- lands on that line, keeping it editable.
vim.wo.conceallevel = 2
vim.wo.concealcursor = ''

-- Highlighting, folding, and indentation from the tree-sitter grammar. No-op
-- until `require('ambient').setup()` has registered the grammar.
require('ambient.treesitter').attach(0)
