# Ambient for Neovim

Filetype detection, syntax highlighting, and comment settings for Ambient source (`.ab`).

## Install

Put this directory on the runtime path. From a checkout:

```lua
vim.opt.runtimepath:append('/path/to/ambient/integrations/neovim')
```

With lazy.nvim:

```lua
{ dir = '/path/to/ambient/integrations/neovim' }
```

## What it gives

- `ftdetect/`: `.ab` files have the filetype `ambient`.
- `syntax/`: highlighting by pattern, following `spec/syntax.md`.
- `ftplugin/`: `//` comments, `///` doc comments that continue, four-space indents.
