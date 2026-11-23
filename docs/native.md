# Writing a native bundle

This is for someone who wants a package to reach code that is not Ambient: a C library, a socket, a device. The [spec](spec/externs.md) says what an extern is and why; this page is how to make one work. The [guide](guide.md) covers the language itself.

## What a package declares

An extern is the handler of an ability whose body is native code. Native code is reached only by performing an ability, even code that touches no world, so a program's row says all the native code it reaches:

```
type(nominal) Stream;

pub ability open(address: string): Stream;
extern handler open in os;

pub ability line(of: Stream): string;
extern handler(suspending) line in os;
```

- An `extern handler` answers an ability, and takes its name unless it is given one (`extern handler reads for line`). `in os` makes it the ability's binding under `system.os`, where `ambient run` runs a program, so a program reaches it by having `line` in its row.
- Only the package that declares an ability says so of it. Any other binds it in a target of its own (`pub target world extends os { line: reads }`), which a binary names in the manifest: `target = "pkg.main.world"` under `[bin.main]`.
- `(suspending)` says the handler may answer later, while other tasks run.
- A `type(nominal)` is a brand: the type of a handle the bundle mints.

## What a bundle is

A native bundle is an object file or an archive, for one platform, with a function for each extern a program may reach.

```
ambient header > native/ambient.h   # in the package
```

The header is everything a bundle is written against. It has:

- A prototype for each extern. Its name is `amb_x_`, the package's ID, and the extern's module path and name, with whatever a C name cannot hold made `_`: `double` in `src/main.ab` of the package `7c1e…` is `amb_x_7c1e…_main_double`.
- `amb_built_<id>_<layout>`, which the header defines. A bundle built against another layout of values does not link, and is built again against a new header.
- Each entry point of the runtime, with who owns what it takes and gives.
- `AMB_TAG_` constants: the text of each tag and brand of the package, as `amb_tag` takes it.
- The platform this `ambient` links for, which is the name the manifest wants.

Every function has one shape. It takes the call it is in and its arguments, a tuple it does not own, and gives a value its caller owns:

```c
#include "ambient.h"

uint64_t amb_x_7c1e_main_double(struct amb_call *call, uint64_t args) {
  return amb_int_new(amb_int_value(amb_word_at(args, 0)) * 2);
}
```

A bundle in Rust is a `staticlib` over the `ambient-native` crate, whose `natives!` names each function for its extern. It may use `std`, threads, and any crate: a bundle is sealed when it is linked, so what it brings is its own, and a program carries only what its functions reach.

A bundle never brings a runtime. One that defines an entry point itself is refused.

## Where the manifest says it is

```toml
[native.x86_64-unknown-linux-gnu]
path = "native/bundle.o"
libraries = ["-lm"]
```

- The platform is named as the compiler names the machine `ambient` was built for. A package with no bundle for it says which platforms it has.
- `path` is for a package in the workspace. A published one gives a `digest`, and a `url` to fetch it from.
- `libraries` is what the machine's linker is told to link besides.
- A package outside the workspace brings its bundle only where what depends on it says `native = true` beside where it is. No grant holds native code, so that trusts the package with the machine.

Linking uses the machine's `cc`, or `$CC`.

## Answering later

A suspending handler takes a ticket and returns at once. Any thread answers by the ticket:

```c
uint64_t amb_x_7c1e_main_reads(struct amb_call *call, uint64_t args) {
  struct wait *wait = begin(amb_word_at(args, 0));
  wait->ticket = amb_bundle_later(call);
  start_thread(reading, wait);
  return AMB_NULL;
}
```

- Only the program's thread touches a value. A thread hands over what makes the answer (`amb_bundle_answer`), and the function it names is called on the program's thread.
- A task may be cancelled while it waits. `amb_bundle_answer` then gives zero, and what the thread took of the world is the bundle's to keep for the next task that asks.
- `amb_bundle_waiting` says whether the task waits still. A thread that reads in pieces asks before each.

## Handles

A handle is a brand and a number the bundle chose. The program cannot read the number or make a handle; the bundle gets it back with `amb_word_at(handle, 1)`.

```c
uint64_t brand = amb_tag((const uint8_t *)AMB_TAG_main_Stream, sizeof AMB_TAG_main_Stream - 1);
amb_bundle_dropping(call, brand, closed);
return amb_handle_new(brand, number);
```

`amb_bundle_dropping` has `closed` called with the number of each handle of the brand the program holds no longer. It is how a native resource is released without the program saying so.

## What does not work yet

- A test and a comptime call reach no bundle. Test the Ambient code around an extern by providing its ability.
- A program that packs or compiles code reaches no bundle.
- Nothing wakes a thread that is blocked in the world when its task is cancelled.
