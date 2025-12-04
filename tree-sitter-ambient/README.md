# tree-sitter-ambient

Tree-sitter grammar for the [Ambient](../) programming language.

## Features

- Full syntax support for Ambient language constructs
- Syntax highlighting queries for editors
- Code folding support
- Structural editing support

## Installation

### Prerequisites

- Node.js (for npm)
- tree-sitter CLI

```bash
npm install
npm run build
```

### Usage with Neovim

Add to your nvim-treesitter configuration:

```lua
local parser_config = require("nvim-treesitter.parsers").get_parser_configs()
parser_config.ambient = {
  install_info = {
    url = "/path/to/tree-sitter-ambient",
    files = { "src/parser.c" },
  },
  filetype = "ab",
}
```

Then install the parser:

```vim
:TSInstall ambient
```

### Usage with Helix

Copy the grammar and queries to your Helix runtime directory.

## Development

Generate the parser:

```bash
npm run build
```

Run tests:

```bash
npm test
```

Parse a file:

```bash
npm run parse -- path/to/file.ab
```

## License

MIT
