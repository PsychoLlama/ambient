// The host runs a program built for it: the one `tests/host.rs` holds,
// which builds it with the JavaScript target and then runs these.

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { test } from "node:test";
import { pathToFileURL } from "node:url";

/** The directory the program was built into. */
const built = process.env.AMBIENT_BUILD;
// What the build wrote to load the program: the host, and `start`.
const { Fault, load, start } = await import(
  pathToFileURL(join(built, "program.js"))
);

const module = join(built, "program.wasm");
const PACKAGE = process.env.AMBIENT_PACKAGE;
/** A program code is loaded into, and code to load into it. */
const { AMBIENT_OPEN: open, AMBIENT_UNIT: unit } = process.env;

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

/** Long enough for the program to be done before it gives up. */
const PATIENT = 1000;

/** The program, over a page that keeps what it is told. */
async function page(natives = {}) {
  const said = [];
  const offered = {
    "main.said": (line) => void said.push(line),
    "main.slept": clock(),
    "main.applied": (work, to) => work(to),
    "main.echoed": (line, call) => call.perform(PACKAGE, "main.say", line),
    ...natives,
  };
  const program = await start(await readFile(module), { [PACKAGE]: offered });
  return { program, said };
}

test("a build loads its program with the native code of its package", async () => {
  const program = await start(await readFile(module));

  assert.equal(await program.run(PATIENT), 4);
});

test("an extern bound to a function of JavaScript's calls it", async () => {
  const program = await start(await readFile(module));

  // Giving up is what the larger of zero and less gives.
  assert.equal(await program.run(1), 0);
});

test("a build declares the types of its program for TypeScript", async () => {
  const declared = await readFile(join(built, "program.d.ts"), "utf8");

  assert.match(
    declared,
    /run\(a0: number \| bigint\): Promise<number \| bigint>/,
  );
  assert.match(declared, /main\.said": \(a0: string\) => null/);
});

test("native code handed to a build's loader is used over its own", async () => {
  const said = [];
  const over = { [PACKAGE]: { "main.said": (line) => void said.push(line) } };
  const program = await start(await readFile(module), over);

  assert.equal(await program.run(PATIENT), 4);
  assert.equal(said.length, 4);
});

test("a program runs, and its tasks wait on the page together", async () => {
  const { program, said } = await page();

  assert.equal(await program.run(PATIENT), 4);
  assert.deepEqual(said, ["counted 1", "counted 2", "counted 3", "still here"]);
});

test("a task that is cancelled leaves its wait on the page", async () => {
  const { program, said } = await page();

  assert.equal(await program.run(15), 0);
  assert.deepEqual(said, ["counted 1", "counted 2"]);
});

test("runs at once each wait on the page for their own answers", async () => {
  const { program } = await page();

  const gave = await Promise.all([program.run(PATIENT), program.run(15)]);
  assert.deepEqual(gave, [4, 0]);
});

test("native code that throws is a fault the page can read", async () => {
  const cause = new Error("no voice");
  const { program } = await page({
    "main.said": () => {
      throw cause;
    },
  });

  const fault = await program.run(PATIENT).then(assert.fail, (fault) => fault);
  assert.ok(fault instanceof Fault);
  assert.deepEqual([fault.kind, fault.cause], ["native", cause]);
});

test("native code calls a function only while it runs", async () => {
  let kept;
  const { program } = await page({
    "main.applied": (work, to) => (kept = work)(to),
  });

  assert.equal(await program.run(PATIENT), 4);
  assert.throws(() => kept(1), /not a function this call was handed/);
});

test("native code performs only an ability of its row", async () => {
  const { program } = await page({
    "main.echoed": (line, call) => call.perform(PACKAGE, "main.sleep", 1),
  });

  const fault = await program.run(PATIENT).then(assert.fail, (fault) => fault);
  assert.match(fault.cause.message, /does not declare the ability/);
});

test("a call back that unwinds ends the native code that made it", async () => {
  const cause = new Error("no voice");
  const echoed = (line, call) => {
    try {
      call.perform(PACKAGE, "main.say", line);
    } catch {
      // Whatever native code makes of it, the unwinding goes on.
    }
    return call.perform(PACKAGE, "main.say", "again");
  };
  const { program, said } = await page({
    "main.echoed": echoed,
    "main.said": (line) => {
      if (line === "still here") throw cause;
      said.push(line);
    },
  });

  const fault = await program.run(PATIENT).then(assert.fail, (fault) => fault);
  assert.deepEqual([fault.kind, fault.cause], ["native", cause]);
  assert.ok(!said.includes("again"));
});

test("a wait that fails is a fault", async () => {
  const { program } = await page({
    "main.slept": () => Promise.reject(new Error("awake")),
  });

  await assert.rejects(program.run(PATIENT), { kind: "native" });
});

test("a program that lacks native code is not loaded", async () => {
  await assert.rejects(
    load(await readFile(module), {}),
    /no native code for `main\./,
  );
});

test("a module is loaded from the response that carries it", async () => {
  const headers = { "content-type": "application/wasm" };
  const response = Promise.resolve(
    new Response(await readFile(module), { headers }),
  );
  const program = await start(response);

  assert.equal(await program.run(PATIENT), 4);
});

/** The program that calls what it fetches, over a page that gives `fetched`. */
async function loader(fetched) {
  const program = await load(await readFile(open), {
    [PACKAGE]: { "main.fetched": () => fetched(program) },
  });
  return program;
}

test("a program calls code its page loads into it while it runs", async () => {
  const program = await loader(async (program) => program.add(readFile(unit)));

  assert.equal(await program.run(21), 42);
  // Nothing the run left refers to what was loaded.
  assert.equal(program.units.size, 0);
});

test("a host calls a function it loaded until it lets it go", async () => {
  const program = await loader(() => null);
  const plugin = await program.add(await readFile(unit));

  assert.equal(await program.call(plugin, 4), 8);
  assert.equal(program.units.size, 1);
  plugin.release();
  assert.equal(program.units.size, 0);
});

test("nothing is loaded into a program that was not built for it", async () => {
  const { program } = await page();

  await assert.rejects(program.add(await readFile(unit)), /not built to be/);
});
