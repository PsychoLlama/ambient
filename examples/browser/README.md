# A program in a page

A package with a target of its own: `src/main.ab` declares two abilities and binds them to externs, and `index.html` supplies the JavaScript for each through the host in `crates/ambient-js`.

From the root of the repository:

```sh
cargo run --release -- build examples/browser --module main.page -o examples/browser/page.wasm
python3 -m http.server
```

Then open <http://localhost:8000/examples/browser/>. The page shows each line the program says and then what it gave.

`--module main.page` names the target to build under: `page` of module `main`. The module that comes out is the program alone, and the page is its host.
