local project_root = vim.fn.expand('<sfile>:h')

vim.lsp.config('ambient', {
  cmd = { 'ambient', 'lsp' },
  filetypes = { 'ambient' },
  root_markers = { '.git' },
})

vim.lsp.enable('ambient')

require('core.pkg').add_hook(function(plugins)
  return vim.list_extend(plugins, {
    {
      name = 'ambient.nvim',
      type = 'path',
      source = project_root .. '/ambient.nvim',
      config = function()
        require('ambient').setup({
          grammar_path = vim.env.TREE_SITTER_AMBIENT
            or (project_root .. '/tree-sitter-ambient'),
        })
      end,
    },
  })
end)
