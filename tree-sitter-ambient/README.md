# tree-sitter-ambient

Tree-sitter grammar for the [Ambient](../) programming language.

## Features

- Full syntax support for Ambient language constructs
- Syntax highlighting queries for editors
- Code folding support
- Structural editing support

## Installation

### Prerequisites

- Node.js and pnpm
- tree-sitter CLI

```bash
pnpm install
pnpm run build
```

### Usage with Neovim

Use the [`ambient.nvim`](../ambient.nvim) plugin, which registers this grammar
with Neovim core and wires up highlighting, folding, and indentation from the
queries here:

```lua
require('ambient').setup({
  treesitter = {
    -- Path to the built parser. Defaults to the copy bundled with the plugin,
    -- so this is only needed to point at a parser built elsewhere.
    parser_path = '/path/to/tree-sitter-ambient/parser/ambient.so',
  },
})
```

Highlighting and folding come from Neovim core (`vim.treesitter`); indentation
uses `nvim-treesitter`'s indent expression. No `:TSInstall` step is needed —
the plugin loads the compiled parser directly.

### Usage with Helix

Copy the grammar and queries to your Helix runtime directory.

## Development

Generate the parser:

```bash
pnpm run build
```

Run tests:

```bash
pnpm test
```

Parse a file:

```bash
pnpm run parse -- path/to/file.ab
```

## License

MIT
