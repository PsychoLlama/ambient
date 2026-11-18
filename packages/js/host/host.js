// A host for a WebAssembly program built by `ambient build --target`:
// it loads the module, supplies the native code its target binds, and
// gives its tasks their turns. See "WebAssembly hosts" and "JavaScript
// hosts" in `spec/targets.md`.
//
// It has no dependencies but `constants.js`, which the build writes
// beside it, and knows of no browser and no Node: a target for either
// builds on it. What it does is what the
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
import { Units } from "./units.js";
import { Loaded } from "./values.js";

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
  const imports = { host: { native: program.calls } };
  const bytes = ArrayBuffer.isView(source) || source instanceof ArrayBuffer;
  const { instance, module } = bytes
    ? await WebAssembly.instantiate(source, imports)
    : await WebAssembly.instantiateStreaming(source, imports);
  program.begin(module, instance, natives);
  return program;
}

// What a call back says when it cannot be made: what the host in
// `crates/ambient-host/src/native/host.rs` says of the same.
const UNDECLARED_ABILITY = "the extern does not declare the ability in its row";
const UNWOUND = "the call back unwound";
const ANSWERING = "native code that answers later has nothing more to call";
const UNCALLABLE =
  "not a function this call was handed that takes as many arguments";

/**
 * One call of native code, which the native code is handed after its
 * arguments: what it calls back into the program through, while it
 * runs. A function among its arguments is called as any function is.
 */
class Call {
  /** Why it can call nothing more, once it cannot. */
  closed = null;
  /** Whether a call it made unwound: the unwinding goes on through it. */
  unwound = false;

  constructor(program, context, row, handlers) {
    this.program = program;
    this.context = context;
    /** The abilities its extern declared, each a package and a path. */
    this.row = row;
    /** The handlers bound for them where it was called, in that order. */
    this.handlers = handlers;
  }

  /**
   * Performs an ability of the extern's row with `args`, by the ID of
   * its package and its module path and name, and gives what its
   * handler gives.
   */
  perform(pkg, path, ...args) {
    const at = this.row.findIndex(
      ([of, named]) => of === pkg && named === path,
    );
    if (at < 0) throw new Error(UNDECLARED_ABILITY);
    const x = this.program.exports;
    return this.made(x.amb_word_at(this.handlers, BigInt(at)), args);
  }

  /** A function the call was handed, as one JavaScript calls. */
  handed(word) {
    return (...args) => this.made(word, args);
  }

  /** Calls `callee`, which something else holds, with nothing bound. */
  made(callee, args) {
    const { program, context } = this;
    const x = program.exports;
    if (this.unwound) throw new Error(UNWOUND);
    if (this.closed) throw new Error(this.closed);
    if (x.amb_closure_arity(callee) !== BigInt(args.length + 1)) {
      throw new Error(UNCALLABLE);
    }
    const handed = program.tuple([null, ...args]);
    // A program in which a task can suspend gives others their turns
    // beneath the call; any other has nothing that does.
    const call = x.amb_call_among ?? x.amb_call_back;
    const result = call(context, callee, handed);
    x.amb_release(handed);
    if (result === UNWIND) {
      this.unwound = true;
      throw new Error(UNWOUND);
    }
    return program.taken(result);
  }
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
    if (!this.pending.delete(ticket)) {
      // Nothing is to have what was loaded for it.
      if (answer.value instanceof Loaded) answer.value.release();
      return;
    }
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

class Program extends Units {
  /** The waits of each run that is not over, by its context. */
  waits = new Map();
  /** What native code threw, for the fault it became. */
  failure = null;
  /** What compiled code calls native code through. */
  calls = (...call) => this.native(...call);

  begin(module, instance, natives) {
    const x = (this.exports = instance.exports);
    // Where the runtime lists the units nothing refers to, in a
    // program that units are loaded into.
    if (x.amb_unit_take) this.unused = x.amb_words(1n) >>> 0;
    this.supply(module, instance, natives);
    const [entry] = this.init(module, instance, false, []);
    this.entry = new Loaded(this, entry);
  }

  /**
   * Runs the program's entry with `args` and gives what it gives. The
   * promise settles once the entry is over, and throws a `Fault` if a
   * fault ended it.
   */
  run(...args) {
    return this.call(this.entry, ...args);
  }

  /**
   * Calls a function the host holds, the entry or what a load gave,
   * with `args` and nothing bound, as `run` runs the entry.
   */
  async call(loaded, ...args) {
    const x = this.exports;
    const callee = loaded.word;
    if (x.amb_closure_arity(callee) !== BigInt(args.length + 1)) {
      throw new TypeError("the function does not take as many arguments");
    }
    const context = x.amb_context_new();
    // The entry is handed no bindings: its target is bound inside it.
    const handed = this.tuple([null, ...args]);
    this.waits.set(context, new Waits());
    const start = () => x.amb_start(context, callee, handed);
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
      this.waits.get(context).pending.clear();
      this.waits.delete(context);
      x.amb_release(handed);
      x.amb_context_free(context);
      this.heard();
      this.reclaim();
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
    const { run, row } = this.natives[Number(index)];
    const call = new Call(this, context, row, bindings);
    let gave;
    try {
      gave = run(...this.value(args, call), call);
      // What it called is unwinding, and goes on through it, whatever
      // it made of that.
      if (call.unwound) return UNWIND;
      if (typeof gave?.then !== "function") return this.word(gave);
    } catch (failure) {
      return call.unwound ? UNWIND : this.failed(context, failure);
    } finally {
      call.closed = UNCALLABLE;
    }
    // It answers later: the task waits, and what is returned here is
    // not looked at.
    call.closed = ANSWERING;
    const ticket = x.amb_await?.(context) ?? 0n;
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
