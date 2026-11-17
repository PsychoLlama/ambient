// A host for a WebAssembly program built by `ambient build --module`:
// it loads the module, supplies the native code its target binds, and
// gives its tasks their turns. See "WebAssembly hosts" and "JavaScript
// hosts" in `spec/targets.md`.
//
// It has no dependencies but `constants.js`, and knows of no browser and
// no Node: a target for either builds on it. What it does is what the
// host in `crates/ambient/src/backend/wasm` does; where the two
// differ, that one is right.

import {
  FORSAKEN,
  NATIVE,
  NULL,
  SUSPENDING,
  UNWIND,
  WAITING,
} from "./constants.js";
import { Values } from "./values.js";

export { Float, Handle, Tag, Tuple } from "./values.js";

/** A fault that ended a program: its kind, a tag's name, and message. */
export class Fault extends Error {
  constructor(kind, message, trace) {
    super(message);
    this.name = "Fault";
    this.kind = kind;
    this.trace = trace;
  }
}

/**
 * Loads a program: its module, and the native code for each extern it
 * reaches, as `{ [package id]: { [path]: function } }`. A function
 * that gives a promise is one that answers later, which its extern
 * must be declared `suspending` to do.
 *
 * The module is its bytes, or the response that carries them (or a
 * promise of one, as `fetch` gives), which is compiled as it arrives.
 */
export async function load(source, natives = {}) {
  const program = new Program();
  const imports = { host: { native: (...call) => program.native(...call) } };
  const bytes = ArrayBuffer.isView(source) || source instanceof ArrayBuffer;
  const { instance, module } = bytes
    ? await WebAssembly.instantiate(source, imports)
    : await WebAssembly.instantiateStreaming(source, imports);
  program.begin(module, instance, natives);
  return program;
}

/** What one run's tasks wait on the world for. */
class Waits {
  /** Answers that came and were not yet handed over. */
  answers = [];
  /** The tickets of the waits no answer has come to. */
  pending = new Set();
  /** What wakes the run's loop, while it waits for an answer. */
  arrived = null;

  /** Takes the answer to `ticket`, unless its task has left the wait. */
  answer(ticket, answer) {
    if (!this.pending.delete(ticket)) return;
    this.answers.push({ ticket, ...answer });
    this.arrived?.();
    this.arrived = null;
  }

  /** Whether no answer is here and none is coming. */
  get stranded() {
    return this.answers.length === 0 && this.pending.size === 0;
  }

  /** The next answer, once one is here. */
  async next() {
    if (this.answers.length === 0) {
      await new Promise((wake) => (this.arrived = wake));
    }
    return this.answers.shift();
  }
}

class Program extends Values {
  /** The native code for each extern, by its position. */
  natives = [];
  /** The waits of each run that is not over, by its context. */
  waits = new Map();
  /** What native code threw, for the fault it became. */
  failure = null;

  begin(module, instance, natives) {
    this.exports = instance.exports;
    for (const { name, kind } of WebAssembly.Module.exports(module)) {
      if (kind !== "global" || !name.startsWith("native:")) continue;
      const [pkg, path] = name.slice("native:".length).split("\t");
      const native = natives[pkg]?.[path];
      if (typeof native !== "function") {
        throw new Error(`no native code for \`${path}\` of package \`${pkg}\``);
      }
      this.natives[Number(this.exports[name].value)] = native;
    }
    this.entry = this.exports.init();
  }

  /**
   * Runs the program's entry with `args` and gives what it gives. The
   * promise settles once the entry is over, and throws a `Fault` if a
   * fault ended it.
   */
  async run(...args) {
    const x = this.exports;
    const context = x.amb_context_new();
    // The entry is handed no bindings: its target is bound inside it.
    const handed = this.tuple([null, ...args]);
    this.waits.set(context, new Waits());
    const start = () => x.amb_start(context, this.entry, handed);
    try {
      // Unwinding in this mode is the root task suspending: the others
      // take their turns, and the entry runs again to resume it.
      let result = start();
      while (result === UNWIND && x.amb_context_mode(context) === SUSPENDING) {
        await this.schedule(context);
        result = start();
      }
      if (result !== UNWIND) return this.taken(result);
      throw this.fault(context);
    } finally {
      // An answer that comes after this finds no wait, and is dropped.
      this.waits.delete(context);
      x.amb_release(handed);
      x.amb_context_free(context);
      this.heard();
    }
  }

  /**
   * Gives suspended tasks their turns until the root can go on, taking
   * an answer from the world whenever none can run without one. The
   * program runs until every task is suspended before this waits.
   */
  async schedule(context) {
    const x = this.exports;
    const waits = this.waits.get(context);
    for (;;) {
      x.amb_schedule(context);
      this.heard();
      if (x.amb_context_mode(context) !== WAITING) return;
      if (waits.stranded) {
        x.amb_strand(context);
        continue;
      }
      const { ticket, value, failed } = await waits.next();
      if (failed) {
        x.amb_refuse(context, ticket);
      } else {
        x.amb_answer(context, ticket, this.word(value));
      }
    }
  }

  /** The host's side of `amb_native`: runs the extern at `index`. */
  native(context, index, bindings, args) {
    const x = this.exports;
    this.heard();
    const waits = this.waits.get(context);
    if (index === FORSAKEN) {
      // A task left a wait, whose ticket is where the bindings would
      // be. Its answer, if one comes, is dropped.
      waits.pending.delete(bindings);
      return NULL;
    }
    let gave;
    try {
      gave = this.natives[Number(index)](...this.value(args));
    } catch (failure) {
      return this.failed(context, failure);
    }
    if (typeof gave?.then !== "function") return this.word(gave);
    // It answers later: the task waits, and what is returned here is
    // not looked at.
    const ticket = x.amb_await(context);
    if (ticket === 0n) {
      return this.failed(
        context,
        new Error("the extern is not declared `suspending`"),
      );
    }
    waits.pending.add(ticket);
    gave.then(
      (value) => waits.answer(ticket, { value }),
      (failure) => waits.answer(ticket, { failed: failure ?? true }),
    );
    return NULL;
  }

  failed(context, failure) {
    this.failure = failure;
    return this.exports.amb_fault(context, NATIVE);
  }

  /** The fault that ended a call, as the error a boundary makes of it. */
  fault(context) {
    const x = this.exports;
    const error = x.amb_context_error(context);
    if (error === NULL) {
      return new Fault(
        "cancelled",
        "every task waited on what would never happen",
      );
    }
    const { kind, message, trace } = this.taken(error);
    const fault = new Fault(kind.name, message, trace);
    if (this.failure) fault.cause = this.failure;
    this.failure = null;
    return fault;
  }

  /** Lets go of what each handle the program dropped stood for. */
  heard() {
    const x = this.exports;
    while (x.amb_handle_next() !== 0n) {
      this.handles.delete(x.amb_handle_number());
    }
  }
}
