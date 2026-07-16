--- Tree-sitter integration.
---
--- `setup()` registers the compiled grammar with Neovim core (no dependency on
--- `nvim-treesitter` for parsing). `attach()` wires the per-buffer editor
--- features from the grammar's queries:
---
---   * highlighting — `vim.treesitter.start`, core (highlights.scm),
---   * folding      — `vim.treesitter.foldexpr`, core (folds.scm),
---   * indentation  — `nvim_treesitter#indent()` (indents.scm).
---
--- Core has no tree-sitter indenter, so indentation is the one feature still
--- borrowed from `nvim-treesitter`. Its indent expression only needs the
--- grammar registered (which we do above) and the query on the runtimepath; it
--- does not need the parser registered through `nvim-treesitter` itself, so no
--- `nvim-treesitter.parsers` config and no `pcall` guard around it.

local M = {
  --- Set once the grammar is registered; `attach()` is a no-op until then.
  registered = false,
}

-- Plugin root: `.../ambient.nvim/lua/ambient/treesitter.lua` → three parents up.
local function plugin_root()
  return vim.fn.fnamemodify(debug.getinfo(1, 'S').source:sub(2), ':h:h:h')
end

--- Resolve the compiled parser (`ambient.so`) from the config.
--- @param cfg ambient.TreesitterConfig
--- @return string
local function resolve_parser(cfg)
  if cfg.parser_path then
    return cfg.parser_path
  end
  if cfg.grammar_path then
    return cfg.grammar_path .. '/parser/ambient.so'
  end
  return plugin_root() .. '/parser/ambient.so'
end

--- @return boolean
local function has_nvim_treesitter()
  return pcall(require, 'nvim-treesitter')
end

--- Register the grammar with Neovim so parsers, queries, highlighting, folding,
--- and indentation resolve for the `ambient` filetype.
--- @param cfg ambient.TreesitterConfig
function M.setup(cfg)
  if not cfg.enable then
    return
  end

  local parser = resolve_parser(cfg)
  if vim.fn.filereadable(parser) == 1 then
    -- `add` is a no-op (returns true) if the grammar is already loaded.
    M.registered = vim.treesitter.language.add('ambient', { path = parser })
      == true
  end

  -- Map the `ambient` filetype to the `ambient` language. Harmless to repeat,
  -- and lets queries resolve even when the parser lives elsewhere on the rtp.
  vim.treesitter.language.register('ambient', 'ambient')
end

--- Turn on tree-sitter editor features for a buffer. Called from the ftplugin.
--- @param bufnr integer
function M.attach(bufnr)
  if not M.registered then
    return
  end

  -- Highlighting (core).
  vim.treesitter.start(bufnr, 'ambient')

  -- Folding (core). Window-local, so scope it to the buffer's current window.
  vim.wo.foldmethod = 'expr'
  vim.wo.foldexpr = 'v:lua.vim.treesitter.foldexpr()'

  -- Indentation (nvim-treesitter). Skip gracefully when it is not installed;
  -- highlighting and folding still work from core.
  if has_nvim_treesitter() then
    vim.bo[bufnr].indentexpr = 'nvim_treesitter#indent()'
  end
end

return M
