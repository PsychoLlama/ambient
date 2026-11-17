// How a host sees a program's values, and makes them. See "JavaScript
// hosts" in `spec/targets.md`.

import { FALSE, Kind, NULL, TRUE } from "./constants.js";

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

const utf8 = new TextDecoder("utf-8", { fatal: false });
const text = new TextEncoder();

/** What reads and makes values in a program's memory. */
export class Values {
  /** What the module exports: its memory and the runtime it carries. */
  exports = null;
  /** What each handle the host minted stands for, by its number. */
  handles = new Map();
  minted = 0;

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
