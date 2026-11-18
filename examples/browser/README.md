# A program in a page

A package with a target of its own, which extends the JavaScript target (`packages/js`): that says how a program under it is built. `src/main.ab` reaches the page through two externs bound to what JavaScript already has (`document.getElementById`, and `append` of what that gives), whose JavaScript the build writes. Waiting answers later, so it is the one function written by hand (`js/main.js`).

From the root of the repository:

```sh
cargo run --release -- build examples/browser --target main.page
python3 -m http.server
```

Then open <http://localhost:8000/examples/browser/>. The page shows each line the program says and then what it gave.

`--target main.page` names the target to build under: `page` of module `main`. The build is written to `examples/browser/build`: the module, the host that loads it, and `program.js`, which the page imports. It holds the JavaScript of the externs the program reaches and no others.
