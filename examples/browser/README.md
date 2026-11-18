# A program in a page

A package with a target of its own: `src/main.ab` declares two abilities and binds them to externs, `js/main.js` is the JavaScript of each, and the target extends the JavaScript target (`packages/js`), which says how a program under it is built.

From the root of the repository:

```sh
cargo run --release -- build examples/browser --target main.page
python3 -m http.server
```

Then open <http://localhost:8000/examples/browser/>. The page shows each line the program says and then what it gave.

`--target main.page` names the target to build under: `page` of module `main`. The build is written to `examples/browser/build`: the module, the host that loads it, and `program.js`, which the page imports. It holds the JavaScript of the externs the program reaches and no others.
