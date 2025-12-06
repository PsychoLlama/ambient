require('core.lsp').add({
  name = 'ambient',
  command = { 'ambient', 'lsp' },
  filetypes = { 'ambient' },
  root = { patterns = { 'ambient.toml', '.git' } },
  settings = {},
})
