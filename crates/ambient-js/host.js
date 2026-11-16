// A host for a WebAssembly program built by `ambient build --module`:
// it loads the module, supplies the native code its target binds, and
// gives its tasks their turns. See "WebAssembly hosts" and "JavaScript
// hosts" in `spec/targets.md`.
//
// It is one module with no dependencies, and knows of no browser and
// no Node: a target for either builds on it. What it does is what the
// host in `crates/ambient/src/backend/wasm` does; where the two
// differ, that one is right.

// Words the runtime gives a meaning of their own.
const UNWIND = 0n;
const NULL = 0b0010n;
const FALSE = 0b0110n;
const TRUE = 0b1010n;

// The modes of a context, as `amb_context_mode` gives them.
const SUSPENDING = 1n;
const WAITING = 3n;

// The fault native code that fails is.
const NATIVE = 5n;

// The index native code is called with to be told a task left a wait.
const FORSAKEN = -1n;

// What kind of value a word is, as `amb_kind` gives it.
const Kind = {
  null: 0n,
  bool: 1n,
  int: 2n,
  float: 3n,
  string: 4n,
  tuple: 5n,
  list: 6n,
  record: 7n,
  function: 8n,
  tag: 10n,
  bytes: 15n,
  handle: 16n,
};

/** A tag: a name that is a value, told apart by the package it is of. */
export class Tag {
  constructor(pkg, name) {
    this.package = pkg;
    this.name = name;
  }
}

/** A tuple, which an array alone would be read as a list. */
export class Tuple extends Array {}

/** A number that is a float even when it is whole. */
export class Float {
  constructor(value) {
    this.value = value;
  }
}

/**
 * A handle to mint: `value` under a brand of a package. The program
 * holds a number, and the host keeps `value` until the program lets
 * the handle go.
 */
export class Handle {
  constructor(pkg, brand, value) {
    this.package = pkg;
    this.brand = brand;
    this.value = value;
  }
}

/** A fault that ended a program: its kind, a tag's name, and message. */
export class Fault extends Error {
  constructor(kind, message, trace) {
    super(message);
    this.name = "Fault";
    this.kind = kind;
    this.trace = trace;
  }
}

const utf8 = new TextDecoder("utf-8", { fatal: false });
const text = new TextEncoder();

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

class Program {
  /** What the module exports: its memory and the runtime it carries. */
  exports = null;
  /** The native code for each extern, by its position. */
  natives = [];
  /** What each handle the host minted stands for, by its number. */
  handles = new Map();
  minted = 0;
  /** Answers that came and were not yet handed over. */
  answers = [];
  /** How many tasks wait on an answer, and what wakes the loop. */
  pending = 0;
  arrived = null;
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
    const start = () => x.amb_start(context, this.entry, handed, 0n);
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
    for (;;) {
      x.amb_schedule(context);
      this.heard();
      if (x.amb_context_mode(context) !== WAITING) return;
      if (this.answers.length === 0 && this.pending === 0) {
        x.amb_strand(context);
        continue;
      }
      if (this.answers.length === 0) {
        await new Promise((wake) => (this.arrived = wake));
      }
      const { ticket, value, failed } = this.answers.shift();
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
    if (index === FORSAKEN) {
      // A task left a wait. Its answer, if one comes, is dropped.
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
    this.pending += 1;
    const answer = (answer) => {
      this.pending -= 1;
      this.answers.push({ ticket, ...answer });
      this.arrived?.();
      this.arrived = null;
    };
    gave.then(
      (value) => answer({ value }),
      (failure) => answer({ failed: failure ?? true }),
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

  /** The bytes of memory, which move whenever memory grows. */
  get memory() {
    return new Uint8Array(this.exports.memory.buffer);
  }

  /** The value of a word the caller owns, which is then let go. */
  taken(word) {
    const value = this.value(word);
    this.exports.amb_release(word);
    return value;
  }

  /** The value a word stands for, as JavaScript has it: a copy. */
  value(word) {
    const x = this.exports;
    const members = (from, step) => {
      const found = [];
      const length = x.amb_length(word) * BigInt(step);
      for (let at = BigInt(from); at < length; at += BigInt(step)) {
        found.push(this.value(x.amb_word_at(word, at)));
      }
      return found;
    };
    switch (x.amb_kind(word)) {
      case Kind.null:
        return null;
      case Kind.bool:
        return word === TRUE;
      case Kind.int: {
        const value = x.amb_int_value(word);
        const exact = BigInt.asIntN(53, value) === value;
        return exact ? Number(value) : value;
      }
      case Kind.float:
        return new Float64Array(
          new BigUint64Array([x.amb_float_bits(word)]).buffer,
        )[0];
      case Kind.string:
        return utf8.decode(this.bytes(word));
      case Kind.bytes:
        return this.bytes(word).slice();
      case Kind.tuple:
        return Tuple.from(members(0, 1));
      case Kind.list:
        return members(0, 1);
      case Kind.record: {
        const keys = members(0, 2);
        const values = members(1, 2);
        return Object.fromEntries(keys.map((key, at) => [key, values[at]]));
      }
      case Kind.tag: {
        const [pkg, name] = utf8.decode(this.bytes(word)).split("\0");
        return new Tag(pkg, name);
      }
      case Kind.handle:
        return this.handles.get(x.amb_word_at(word, 1n));
      default:
        // A function, a cell, and what else is the program's alone.
        return undefined;
    }
  }

  /** The bytes of a string, a tag, or a bytes value, where they lie. */
  bytes(word) {
    const start = this.exports.amb_string_bytes(word) >>> 0;
    return this.memory.subarray(
      start,
      start + Number(this.exports.amb_length(word)),
    );
  }

  /** A word for a value, which the caller owns. */
  word(value) {
    const x = this.exports;
    if (value === null || value === undefined) return NULL;
    if (value === true) return TRUE;
    if (value === false) return FALSE;
    if (typeof value === "bigint") return x.amb_int_new(value);
    if (typeof value === "number") {
      return Number.isInteger(value)
        ? x.amb_int_new(BigInt(value))
        : this.float(value);
    }
    if (value instanceof Float) return this.float(value.value);
    if (typeof value === "string")
      return this.made(x.amb_string_new, text.encode(value));
    if (value instanceof Uint8Array) return this.made(x.amb_bytes_new, value);
    if (value instanceof Tag) {
      return this.made(
        x.amb_tag,
        text.encode(`${value.package}\0${value.name}`),
      );
    }
    if (value instanceof Handle) {
      const brand = this.made(
        x.amb_tag,
        text.encode(`${value.package}\0${value.brand}`),
      );
      const number = BigInt((this.minted += 1));
      this.handles.set(number, value.value);
      return x.amb_handle_new(brand, number);
    }
    if (value instanceof Tuple) return this.tuple(value);
    if (Array.isArray(value)) return this.sequence(1n, value);
    return this.record(value);
  }

  float(value) {
    const bits = new BigUint64Array(new Float64Array([value]).buffer)[0];
    return this.exports.amb_float_new(bits);
  }

  tuple(members) {
    return this.sequence(0n, members);
  }

  sequence(list, members) {
    const x = this.exports;
    const made = x.amb_sequence_new(list, BigInt(members.length));
    members.forEach((member, at) => {
      const word = this.word(member);
      x.amb_sequence_set(made, BigInt(at), word);
      x.amb_release(word);
    });
    return made;
  }

  record(fields) {
    const x = this.exports;
    const entries = Object.entries(fields);
    const made = x.amb_record_new(BigInt(entries.length));
    for (const [name, value] of entries) {
      const word = this.word(value);
      x.amb_record_set(made, this.made(x.amb_intern, text.encode(name)), word);
      x.amb_release(word);
    }
    return made;
  }

  /** What `make` makes of `bytes`, which it reads from memory. */
  made(make, bytes) {
    const x = this.exports;
    const room = BigInt(Math.max(1, Math.ceil(bytes.length / 8)));
    const address = x.amb_words(room);
    this.memory.set(bytes, address >>> 0);
    const made = make(address, BigInt(bytes.length));
    x.amb_words_free(address, room);
    return made;
  }
}
