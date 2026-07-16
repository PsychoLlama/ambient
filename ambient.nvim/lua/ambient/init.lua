--- Ambient language support for Neovim.
---
--- Public entry point. `setup()` registers the tree-sitter grammar and the
--- language server; per-buffer editor features (highlight, fold, indent,
--- conceal, options) are wired by `ftplugin/ambient.lua` as each buffer opens.
---
--- Usage:
---
---   require('ambient').setup({
---     treesitter = { parser_path = '/path/to/tree-sitter-ambient/parser/ambient.so' },
---   })
---
--- See `lua/ambient/config.lua` for the full set of options.

local config = require('ambient.config')

local M = {}

--- Resolved configuration; the ftplugin reads this to know what is enabled.
--- @type ambient.Config|nil
M.config = nil

--- @param opts table|nil
function M.setup(opts)
  M.config = config.merge(opts)

  require('ambient.treesitter').setup(M.config.treesitter)
  require('ambient.lsp').setup(M.config.lsp)
end

return M
