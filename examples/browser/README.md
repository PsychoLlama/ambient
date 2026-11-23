# A program in a page

A package built under the JavaScript target (`packages/js`), which says how a program under it is built. `src/main.ab` reaches the page through two native handlers bound to what JavaScript already has (`document.getElementById`, and `append` of what that gives), whose JavaScript the build writes. Waiting answers later, so it is the one function written by hand (`js/main.js`).

From the root of the repository:

```sh
(cd examples/browser && cargo run --release -- build)
python3 -m http.server
```

Then open <http://localhost:8000/examples/browser/>. The page shows each line the program says and then what it gave.

The manifest names the target `main` is built under (`target = "js.host"`), and each native handler of the package says it answers there (`in host`). The build is written to `examples/browser/build/browser`, named for the binary: the module, the host that loads it, and `program.js`, which the page imports. It holds the JavaScript of the externs the program reaches and no others.
