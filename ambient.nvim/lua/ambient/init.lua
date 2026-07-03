local M = {}

function M.setup(opts)
  opts = opts or {}

  -- Find the parser .so file
  local plugin_dir =
    vim.fn.fnamemodify(debug.getinfo(1, 'S').source:sub(2), ':h:h:h')
  local parser_path = opts.parser_path
    or (opts.grammar_path and opts.grammar_path .. '/parser/ambient.so')
    or (plugin_dir .. '/parser/ambient.so')

  -- Load the parser if it exists
  if vim.fn.filereadable(parser_path) == 1 then
    vim.treesitter.language.add('ambient', { path = parser_path })
  end

  -- Register filetype association
  vim.treesitter.language.register('ambient', 'ambient')

  -- Configure nvim-treesitter if available
  local ok, parser_config = pcall(function()
    return require('nvim-treesitter.parsers').get_parser_configs()
  end)
  if ok then
    parser_config.ambient = {
      install_info = {
        url = opts.grammar_path
          or vim.fn.fnamemodify(plugin_dir, ':h') .. '/tree-sitter-ambient',
        files = { 'src/parser.c' },
      },
      filetype = 'ambient',
    }
  end
end

return M
