// The host runs a program built for it. The module is built by the
// command line before these run: see `web/README.md`.

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";

import { Fault, load } from "../host.js";

const module = process.env.AMBIENT_MODULE;
const PACKAGE = process.env.AMBIENT_PACKAGE;

/**
 * A clock that passes only when asked: each wait is over in the order
 * it is due, one at a time, once the program has nothing else to do.
 * No test waits on a real timer, so a run is the same every time.
 */
function clock() {
  const waits = [];
  let now = 0;
  let ticking = false;
  const tick = () => {
    ticking = false;
    waits.sort((a, b) => a.due - b.due);
    const next = waits.shift();
    if (!next) return;
    now = next.due;
    next.wake(null);
    if (waits.length > 0) wind();
  };
  const wind = () => {
    if (!ticking) setImmediate(tick);
    ticking = true;
  };
  return (ms) =>
    new Promise((wake) => {
      waits.push({ due: now + ms, wake });
      wind();
    });
}

/** The example's program, over a page that keeps what it is told. */
async function page(natives = {}) {
  const said = [];
  const offered = {
    "main.said": (line) => void said.push(line),
    "main.slept": clock(),
    ...natives,
  };
  const program = await load(await readFile(module), { [PACKAGE]: offered });
  return { program, said };
}

test("a program runs, and its tasks wait on the page together", async () => {
  const { program, said } = await page();

  assert.equal(await program.run(), 4);
  assert.deepEqual(said, ["counted 1", "counted 2", "counted 3", "still here"]);
});

test("native code that throws is a fault the page can read", async () => {
  const cause = new Error("no voice");
  const { program } = await page({
    "main.said": () => {
      throw cause;
    },
  });

  const fault = await program.run().then(assert.fail, (fault) => fault);
  assert.ok(fault instanceof Fault);
  assert.deepEqual([fault.kind, fault.cause], ["native", cause]);
});

test("a wait that fails is a fault", async () => {
  const { program } = await page({
    "main.slept": () => Promise.reject(new Error("awake")),
  });

  await assert.rejects(program.run(), { kind: "native" });
});

test("a program that lacks native code is not loaded", async () => {
  await assert.rejects(
    load(await readFile(module), {}),
    /no native code for `main\.s/,
  );
});
