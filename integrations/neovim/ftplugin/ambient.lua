if vim.b.did_ftplugin then
  return
end
vim.b.did_ftplugin = true

vim.bo.commentstring = '// %s'
-- `///` before `//`, so a continued doc comment stays one.
vim.bo.comments = ':///,://'
vim.bo.shiftwidth = 4
vim.bo.softtabstop = 4
vim.bo.expandtab = true

vim.b.undo_ftplugin = 'setlocal commentstring< comments< shiftwidth< softtabstop< expandtab<'
