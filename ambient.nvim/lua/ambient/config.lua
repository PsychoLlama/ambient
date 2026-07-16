--- Default configuration and option merging for the Ambient plugin.
---
--- `setup()` takes a partial `opts` table and layers it over these defaults.
--- To turn a feature off, set its `enable` field: `setup({ lsp = { enable =
--- false } })`.

local M = {}

--- @class ambient.Config
--- @field treesitter ambient.TreesitterConfig
--- @field lsp ambient.LspConfig

--- @class ambient.TreesitterConfig
--- @field enable boolean Register the grammar and wire highlight/fold/indent.
--- @field parser_path? string Path to the compiled parser (`ambient.so`); defaults to the copy bundled with the plugin.

--- @class ambient.LspConfig
--- @field enable boolean Configure and enable the `ambient` language server.
--- @field cmd string[] Command that launches the server.
--- @field root_markers string[] Files/dirs that mark a project root.

--- @type ambient.Config
M.defaults = {
  treesitter = {
    enable = true,
    parser_path = nil,
  },
  lsp = {
    enable = true,
    cmd = { 'ambient', 'lsp' },
    root_markers = { '.git' },
  },
}

--- Layer user `opts` over the defaults, returning a fully-populated config.
--- @param opts table|nil
--- @return ambient.Config
function M.merge(opts)
  return vim.tbl_deep_extend('force', vim.deepcopy(M.defaults), opts or {})
end

return M
