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
