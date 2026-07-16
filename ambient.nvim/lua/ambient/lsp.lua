--- Language server integration.
---
--- Uses Neovim's built-in LSP configuration framework (`vim.lsp.config` /
--- `vim.lsp.enable`, Neovim 0.11+): the plugin owns the `ambient` server
--- definition so a user's config need only call `require('ambient').setup()`
--- rather than repeat the `cmd` / `root_markers` boilerplate.

local M = {}

--- Register and enable the `ambient` language server.
--- @param cfg ambient.LspConfig
function M.setup(cfg)
  if not cfg.enable then
    return
  end

  -- `enable` is ours; everything else is passed through to the server config.
  local server = vim.deepcopy(cfg)
  server.enable = nil
  server.filetypes = server.filetypes or { 'ambient' }

  vim.lsp.config('ambient', server)
  vim.lsp.enable('ambient')
end

return M
