// Loading code into a program that is running, and dropping it again:
// what `crates/ambient/src/backend/wasm/units.rs` does. See "Loading"
// in `docs/spec/linking.md`.

import { Loaded, Values } from "./values.js";

/** A constant global of a module to instantiate. */
const constant = (value, as) =>
  new WebAssembly.Global({ value: as, mutable: false }, value);

/**
 * What a module exports as globals whose names begin `prefix`: the
 * rest of each name, and the number it holds.
 */
function exported(module, instance, prefix) {
  const found = [];
  for (const { name, kind } of WebAssembly.Module.exports(module)) {
    if (kind !== "global" || !name.startsWith(prefix)) continue;
    const held = Number(instance.exports[name].value);
    found.push([name.slice(prefix.length), held]);
  }
  return found;
}

/** A program as what is loaded into it sees it. */
export class Units extends Values {
  /** The native code and row of each extern, by its position. */
  natives = [];
  /** Where the program has each definition, by its hash. */
  known = new Map();
  /** What each load brought, by the address of its block. */
  units = new Map();
  /** What the program has of the runtime: functions and states, by hash. */
  pieces = new Map();
  states = new Map();
  /** Where the runtime lists the units nothing refers to, if it does. */
  unused = 0;
  /** The load under way, which the next waits for. */
  loading = Promise.resolve();
  adding = false;

  /**
   * Takes the native code for each extern a module reaches, as
   * `{ [package id]: { [path]: function } }`, and gives the positions
   * they have from then on.
   */
  supply(module, instance, natives) {
    const externs = exported(module, instance, "native:");
    externs.sort(([, at], [, other]) => at - other);
    const supplied = externs.map(([name]) => {
      // Its package and path, and then those of each ability of its row.
      const [pkg, path, ...names] = name.split("\t");
      const run = natives[pkg]?.[path];
      if (typeof run !== "function") {
        throw new Error(`no native code for \`${path}\` of package \`${pkg}\``);
      }
      const row = [];
      for (let at = 0; at < names.length; at += 2) {
        row.push([names[at], names[at + 1]]);
      }
      return { run, row };
    });
    const start = this.natives.length;
    this.natives.push(...supplied);
    return [start, this.natives.length];
  }

  /**
   * Runs a module's `init`, and notes what it brought of the runtime
   * and where it put each definition that is not among `had`. Gives
   * the entry, and the address of the module's block, or zero for a
   * module that does not say.
   */
  init(module, instance, loaded, had) {
    const entry = instance.exports.init();
    for (const [hash, address] of exported(module, instance, "state:")) {
      if (!this.states.has(hash)) this.states.set(hash, address);
    }
    for (const { name, kind } of WebAssembly.Module.exports(module)) {
      if (kind !== "function" || !name.startsWith("piece:")) continue;
      const hash = name.slice("piece:".length);
      if (!this.pieces.has(hash)) this.pieces.set(hash, instance.exports[name]);
    }
    // Nothing can be loaded into a program that does not say where it is.
    if (!instance.exports.base) return [entry, 0];
    const base = instance.exports.base.value >>> 0;
    const definitions = new Map(exported(module, instance, "definition:"));
    for (const [hash, root] of exported(module, instance, "root:")) {
      if (had.includes(hash)) continue;
      const definition = definitions.get(hash) || 0;
      this.known.set(hash, {
        root: base + root,
        definition: definition && base + definition,
        unit: loaded ? base : 0,
      });
    }
    return [entry, base];
  }

  /**
   * Loads more into the program: a unit, as bytes, a compiled module,
   * or the response that carries one, with the native code for the
   * externs it reaches. Gives its function, which the caller holds.
   *
   * What the program has already, by hash, the unit uses; the rest it
   * brings. It is kept while a value refers to it.
   */
  add(source, natives = {}) {
    const added = this.loading.then(() => this.added(source, natives));
    this.loading = added.catch(() => {});
    return added;
  }

  async added(source, natives) {
    const x = this.exports;
    if (this.unused === 0) {
      throw new Error("the program was not built to be loaded into");
    }
    const module = await compiled(source);
    this.reclaim();
    const imports = {
      rt: x,
      host: {
        native: this.calls,
        unused: constant(BigInt(this.unused), "i64"),
        natives: constant(BigInt(this.natives.length), "i64"),
      },
      known: {},
      piece: {},
      state: {},
    };
    // The units it refers into, and the definitions it was told of.
    const [holds, hashes] = [[], []];
    for (const { module: from, name } of WebAssembly.Module.imports(module)) {
      if (from === "piece") {
        const piece = this.pieces.get(name);
        if (!piece) {
          throw new Error("a unit was told of a piece the program lacks");
        }
        imports.piece[name] = piece;
      } else if (from === "state") {
        // Zero for one the program lacks, which the unit then brings.
        imports.state[name] = constant(this.states.get(name) ?? 0, "i32");
      } else if (from === "known") {
        const [what, hash] = name.split(":");
        const known = this.known.get(hash);
        const address = what === "root" ? known?.root : known?.definition;
        imports.known[name] = constant(BigInt(address ?? 0), "i64");
        if (!known) continue;
        hashes.push(hash);
        if (known.unit !== 0 && !holds.includes(known.unit)) {
          holds.push(known.unit);
        }
      }
    }
    // Nothing is dropped while the unit is instantiated: it was told
    // where things are, and how many externs the program has.
    this.adding = true;
    let instance;
    try {
      instance = await WebAssembly.instantiate(module, imports);
    } finally {
      this.adding = false;
    }
    const supplied = this.supply(module, instance, natives);
    for (const unit of holds) x.amb_unit_retain(BigInt(unit));
    const [entry, base] = this.init(module, instance, true, hashes);
    const brought = [...this.known].filter(([, known]) => known.unit === base);
    this.units.set(base, {
      instance,
      holds,
      hashes: brought.map(([hash]) => hash),
      natives: supplied,
    });
    return new Loaded(this, entry);
  }

  /**
   * Drops what was loaded and nothing refers to any more, and whatever
   * only that referred to. Every load begins with this and every call
   * ends with it.
   */
  reclaim() {
    const x = this.exports;
    // A program nothing is loaded into has nothing to drop.
    if (this.unused === 0 || this.adding) return;
    for (;;) {
      const unit = Number(x.amb_unit_take(this.unused));
      const gone = this.units.get(unit);
      if (!gone) return;
      this.units.delete(unit);
      const [start, end] = gone.natives;
      this.natives.fill(null, start, end);
      while (this.natives.length > 0 && this.natives.at(-1) === null) {
        this.natives.pop();
      }
      // A later unit may have brought a definition again.
      for (const hash of gone.hashes) {
        if (this.known.get(hash)?.unit === unit) this.known.delete(hash);
      }
      gone.instance.exports.drop?.();
      for (const held of gone.holds) x.amb_unit_release(BigInt(held));
    }
  }
}

/** A module of what carries one: itself, its bytes, or a response. */
async function compiled(source) {
  const from = await source;
  if (from instanceof WebAssembly.Module) return from;
  const bytes = ArrayBuffer.isView(from) || from instanceof ArrayBuffer;
  return bytes ? WebAssembly.compile(from) : WebAssembly.compileStreaming(from);
}
