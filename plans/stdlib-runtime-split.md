# Plan: Split Standard Library and Runtime into Separate Crates

## Overview

This plan separates the Ambient codebase into:
1. **`ambient-core`** - Essential types and abilities that the language depends on (Exception, future Map/List)
2. **`ambient-runtime`** - Host-provided capabilities (Console, Time, Random, Async, Log, File operations)
3. **`ambient-engine`** - Core compiler, type inference, and VM (imports core, optionally runtime)

## Current State

All abilities are hard-coded in Rust across multiple files:
- `abilities.rs` - Host handler implementations
- `infer/mod.rs` - Type signatures and ability ID mappings
- `compiler/mod.rs` - Ability/method ID resolution

There are 6 abilities today:
- **Console** (0x0001) - print, println, eprint
- **Exception** (0x0002) - throw
- **Time** (0x0003) - now, wait
- **Random** (0x0004) - seed, in_range
- **Async** (0x0005) - all, race
- **Log** (0x0006) - debug, info, warn, error

## Design Decisions

### Which abilities go where?

**`ambient-core`** (always available, language depends on these):
- **Exception** - Required for error handling semantics
- Future: Map, List, Option, Result types

**`ambient-runtime`** (host-provided, environment-specific):
- **Console** - I/O, not available in all environments
- **Time** - System clock, not available in WASM
- **Random** - PRNG, may need different implementations
- **Async** - Threading, not available in single-threaded WASM
- **Log** - Structured logging, host-specific
- Future: File operations, Network, etc.

### Key Architectural Changes

1. **AbilityRegistry trait** - Define a trait that both core and runtime implement to register their abilities with the engine
2. **Pluggable ability system** - Engine accepts ability registrations at compile-time (type info) and runtime (handlers)
3. **Ability ID namespacing** - Reserve ID ranges: 0x0000-0x00FF for core, 0x0100-0xFFFF for runtime/extensions

## Implementation Plan

### Phase 1: Create the AbilityRegistry Abstraction

**Goal**: Decouple ability definitions from the engine so they can be provided externally.

1. **Create `ambient-core` crate** (`crates/ambient-core/`)
   - Define `AbilityDescriptor` struct:
     ```rust
     pub struct AbilityDescriptor {
         pub id: u16,
         pub name: &'static str,
         pub methods: &'static [MethodDescriptor],
     }

     pub struct MethodDescriptor {
         pub id: u16,
         pub name: &'static str,
         pub param_types: fn() -> Vec<Type>,  // Deferred to avoid circular deps
         pub return_type: fn() -> Type,
     }
     ```
   - Define `AbilityProvider` trait:
     ```rust
     pub trait AbilityProvider {
         fn abilities(&self) -> &[AbilityDescriptor];
         fn register_handlers(&self, vm: &mut Vm);
     }
     ```
   - Implement Exception ability in this crate

2. **Refactor `ambient-engine` to use AbilityProvider**
   - Remove hard-coded ability mappings from `infer/mod.rs`
   - Remove hard-coded ability mappings from `compiler/mod.rs`
   - Add `AbilityResolver` that looks up abilities from registered providers
   - `Vm::new()` takes optional ability providers

### Phase 2: Create `ambient-runtime` Crate

**Goal**: Move host-provided abilities to a separate crate.

1. **Create `crates/ambient-runtime/`**
   - Move from `abilities.rs`:
     - Console ability + handlers
     - Time ability + handlers
     - Random ability + handlers
     - Async ability + handlers
     - Log ability + handlers
   - Implement `AbilityProvider` for `RuntimeAbilities`

2. **Update `ambient-cli`**
   - Import both `ambient-core` and `ambient-runtime`
   - Register both providers with the engine/VM

### Phase 3: Update Type Inference System

**Goal**: Type checker queries abilities from registered providers instead of hard-coded maps.

1. **Modify `Infer` struct**
   - Add `ability_resolver: AbilityResolver` field
   - Change `ability_name_to_id()` to query resolver
   - Change `lookup_ability_method()` to query resolver
   - Change `get_ability_method_signatures()` to query resolver

2. **Create `AbilityResolver`**
   - Holds references to all registered `AbilityDescriptor`s
   - Provides lookup by name, ID, or method
   - Built from providers at module compilation time

### Phase 4: Update Compiler

**Goal**: Compiler uses AbilityResolver instead of hard-coded mappings.

1. **Modify compiler**
   - Pass `AbilityResolver` to `Compiler::new()`
   - Change `get_method_id_for_ability()` to query resolver
   - Change `get_ability_name()` to query resolver

### Phase 5: Clean Up and Documentation

1. **Remove old code**
   - Delete hard-coded ability constants from engine
   - Remove `abilities.rs` from engine (moved to runtime)

2. **Update tests**
   - Tests need to register abilities before compiling/running
   - Create test helpers that register standard abilities

3. **Document the extension mechanism**
   - How to create a custom ability provider
   - How to add abilities for a new host environment

## File Changes Summary

### New Crates

```
crates/ambient-core/
├── Cargo.toml
└── src/
    ├── lib.rs
    ├── descriptor.rs    # AbilityDescriptor, MethodDescriptor
    ├── provider.rs      # AbilityProvider trait
    └── exception.rs     # Exception ability definition

crates/ambient-runtime/
├── Cargo.toml
└── src/
    ├── lib.rs
    ├── console.rs       # Moved from engine/abilities.rs
    ├── time.rs          # Moved from engine/abilities.rs
    ├── random.rs        # Moved from engine/abilities.rs
    ├── async_ability.rs # Moved from engine/abilities.rs
    └── log.rs           # Moved from engine/abilities.rs
```

### Modified Files

- `crates/ambient-engine/Cargo.toml` - Add `ambient-core` dependency
- `crates/ambient-engine/src/lib.rs` - Export new AbilityResolver
- `crates/ambient-engine/src/infer/mod.rs` - Use AbilityResolver
- `crates/ambient-engine/src/compiler/mod.rs` - Use AbilityResolver
- `crates/ambient-engine/src/vm/core.rs` - Accept ability providers
- `crates/ambient-cli/Cargo.toml` - Add both core and runtime deps
- `crates/ambient-cli/src/main.rs` - Register providers

### Deleted Files

- `crates/ambient-engine/src/abilities.rs` - Contents moved to runtime crate

## Dependency Graph (After)

```
ambient-core (no deps on other ambient crates)
     │
     ▼
ambient-engine (depends on core)
     │
     ▼
ambient-runtime (depends on engine for Vm type)
     │
     ▼
ambient-cli (depends on all three)
```

## Open Questions

1. **Type representation**: `MethodDescriptor` needs to express parameter/return types. Should we:
   - Use function pointers `fn() -> Type` to defer construction?
   - Use a simplified type enum that doesn't depend on engine types?
   - Accept that core must depend on engine's type module?

2. **Ability ID allocation**: Should we:
   - Reserve ranges (0x0000-0x00FF for core)?
   - Use a registry that assigns IDs dynamically?
   - Allow collisions and resolve by name?

3. **Handler registration timing**: Should handlers be registered:
   - At VM construction time?
   - Lazily on first use?
   - Separately for compile-time (types) vs runtime (handlers)?

## Success Criteria

- [ ] Can compile and run code using only core abilities (Exception)
- [ ] Can compile and run code using core + runtime abilities
- [ ] WASM target can use core but substitute/omit runtime abilities
- [ ] Adding a new ability doesn't require modifying engine code
- [ ] All existing tests pass
- [ ] No hard-coded ability IDs/names in engine crate
