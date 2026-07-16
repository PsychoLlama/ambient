--- Default configuration and option merging for the Ambient plugin.
---
--- `setup()` takes a partial `opts` table and layers it over these defaults.
--- Each feature block (`treesitter`, `lsp`) may also be passed `false` as a
--- shorthand for `{ enable = false }`, so `setup({ lsp = false })` reads
--- naturally.

local M = {}

--- @class ambient.Config
--- @field treesitter ambient.TreesitterConfig
--- @field lsp ambient.LspConfig

--- @class ambient.TreesitterConfig
--- @field enable boolean Register the grammar and wire highlight/fold/indent.
--- @field grammar_path? string Directory holding the grammar (with `parser/ambient.so`).
--- @field parser_path? string Explicit path to the compiled parser; wins over `grammar_path`.

--- @class ambient.LspConfig
--- @field enable boolean Configure and enable the `ambient` language server.
--- @field cmd string[] Command that launches the server.
--- @field root_markers string[] Files/dirs that mark a project root.

--- @type ambient.Config
M.defaults = {
  treesitter = {
    enable = true,
    grammar_path = nil,
    parser_path = nil,
  },
  lsp = {
    enable = true,
    cmd = { 'ambient', 'lsp' },
    root_markers = { '.git' },
  },
}

-- A feature block passed as `false` means "disabled"; normalize it to a table
-- so the deep merge below always operates on tables.
local function normalize(block)
  if block == false then
    return { enable = false }
  end
  return block
end

--- Layer user `opts` over the defaults, returning a fully-populated config.
--- @param opts table|nil
--- @return ambient.Config
function M.merge(opts)
  opts = vim.deepcopy(opts or {})
  opts.treesitter = normalize(opts.treesitter)
  opts.lsp = normalize(opts.lsp)
  return vim.tbl_deep_extend('force', vim.deepcopy(M.defaults), opts)
end

return M
