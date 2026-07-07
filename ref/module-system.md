# Module System Design

A module system for Ambient that enables code organization across files with explicit imports and exports.

## Design Goals

1. **Filesystem-based**: Module paths mirror directory structure
2. **Explicit imports**: All dependencies declared via `use` statements
3. **Clear namespacing**: `pkg` for local package, `core` for standard library
4. **Cyclic imports supported**: Modules can reference each other
5. **Lazy loading**: Only parse/compile what's actually imported
6. **Incremental compilation**: Cache results, only recompile changed files
7. **Workspace-ready**: Design supports multi-package workspaces

## Syntax

### Import Statements

```ambient
// Import a module (brings the module name into scope)
use pkg::utils;                    // utils::helper()

// Import specific items
use pkg::utils::helper;             // helper()
use pkg::utils::{helper, format};   // helper(), format()

// Relative imports (within the same package)
use self::sibling;                 // Import from ./sibling.ab
use super::parent;                 // Import from ../parent.ab
use super::super::grandparent;      // Import from ../../grandparent.ab

// Standard library
use core::collections::List;                    // List::map(), List::filter()

// Re-exports (make imported items part of this module's public API)
pub use pkg::other::Thing;          // Re-export Thing
pub use pkg::utils::{helper, format}; // Re-export specific items
```

### Visibility

```ambient
// Public items are exported (visible outside the module)
pub fn hello(): string { "Hello" }
pub const PI: number = 3.14159;
pub enum Option<T> { Some(T), None }
pub ability Logger { fn log(msg: string): (); }

// Private items are internal (default)
fn internal_helper(): number { 42 }
```

### Module Structure

```
my_project/
  ambient.toml          # Package manifest
  src/
    main.ab             # Entry point (must exist, exports pub fn main)
    utils.ab            # pkg::utils
    utils/
      format.ab         # pkg::utils::format
      parse.ab          # pkg::utils::parse
  build/                # Build cache (gitignored)
    cache/
      ...
```

Modules map directly to files. There are no special filenames (`mod.ab`, `index.ab`, etc.) - every `.ab` file is its own module named after the file.

## Package Manifest (ambient.toml)

```toml
[package]
name = "my_project"
version = "0.1.0"

[build]
src = "src"             # Source directory (default: "src")
# Entry point is always {src}/main.ab - not configurable
```

### Future: Workspace Support

The manifest format is designed to support workspaces in the future:

```toml
# Workspace root ambient.toml
[workspace]
members = [
    "packages/*",       # Glob patterns supported
    "tools/cli",        # Or explicit paths
]

# Individual packages still have their own ambient.toml
```

This is not implemented in the initial version but the design does not preclude it.

## Namespaces

| Prefix | Meaning |
|--------|---------|
| `pkg`  | Current package (relative to `build.src`) |
| `core` | Standard library |
| `self` | Current module's directory |
| `super`| Parent directory (can chain: `super::super::x`) |

These are **reserved keywords** and cannot be used as identifiers:
- `pkg`, `core`, `self`, `super` - module path prefixes
- `use`, `pub` - already reserved

This is a breaking change if existing code uses these as names.

## Resolution Rules

1. **Qualified names** (`pkg::utils::helper`) are resolved from the package root
2. **Relative imports** (`self::sibling`) are resolved from the current file's location
3. **Chained super** (`super::super::x`) walks up multiple directory levels
4. **Imported names** shadow local definitions (with a warning)
5. **Ambiguous imports** are an error (must qualify)
6. **Boundary guard**: Imports cannot escape above `build.src` (e.g., `super` from `src/main.ab` is an error)
7. **Re-export chains**: `pub use` chains are followed transitively (with cycle detection)

## Entry Point

The entry point must be `pub fn run()` in `main.ab`:

```ambient
// src/main.ab
pub fn run(): () {
    // ...
}

// With abilities (optional)
pub fn run(): () with platform::Stdio, platform::FileSystem {
    // ...
}
```

**Requirements:**
- Must be named `run`
- Must be `pub`
- Return type is flexible (any type)
- Abilities are optional (default: none)
- No parameters

The platform provides handlers for core abilities (`Stdio`, `Filesystem`, `Time`, `Random`, etc.) automatically.

## Visibility Semantics

### Private Items

Private items (without `pub`) are only accessible within their own module:

```ambient
// utils.ab
fn helper(): number { 42 }        // Private
pub fn public_fn(): number {
    helper()                       // OK: same module
}

// main.ab
use pkg::utils;
utils::helper()                     // ERROR: helper is private
```

## Cyclic Imports

Cyclic imports between modules are supported:

```ambient
// a.ab
use pkg::b::B_Type;
pub type A_Type = { b: B_Type }
pub fn make_a(b: B_Type): A_Type { { b: b } }

// b.ab
use pkg::a::A_Type;
pub type B_Type = { value: number }
pub fn use_a(a: A_Type): number { a.b.value }
```

### How Cycles Are Resolved

Resolution happens in multiple passes:

1. **Parse imported modules** - Only parse files that are transitively imported
2. **Collect signatures** - Extract all type/function/ability signatures from parsed modules
3. **Build symbol table** - Map all qualified names to their definitions
4. **Resolve imports** - Verify all `use` statements point to valid, visible symbols
5. **Type check bodies** - Check function bodies with full type information available

This two-phase approach (signatures first, bodies second) allows cycles at the signature level.

### Restrictions on Cycles

- **Type alias cycles must be finite**: `type A = B; type B = A;` is an error
- **Enum cycles through indirection only**: `enum A { X(B) }` OK if B contains A via reference
- **Const cycles forbidden**: Constants must be acyclic (evaluated at compile time)

## Lazy Module Loading

For performance with large codebases, modules are loaded lazily based on what's actually imported.

### Loading Strategy

```
Entry: src/main.ab
         │
         ├─ use pkg::utils::format     → Parse src/utils/format.ab
         │       │
         │       └─ use pkg::common   → Parse src/common.ab
         │
         └─ use pkg::api              → Parse src/api.ab

Files NOT parsed: src/legacy.ab, src/unused/*.ab, etc.
```

1. Start from `main.ab`
2. Parse and extract `use` statements
3. For each import, resolve the file path and parse it
4. Recursively process imports from those files
5. Continue until all transitive imports are resolved

This means a package with 10,000 files but an entry point that only uses 100 will only parse those 100 files.

### Module Discovery

When resolving `use pkg::utils::format`:

1. Check if `src/utils/format.ab` exists → parse it
2. If not, check if `src/utils.ab` exists and exports `format` (re-export or inline module)
3. If neither, error: module not found

## Incremental Compilation

Build results are cached in `build/` to avoid redundant work.

### Cache Structure

```
build/
  cache/
    manifest.json           # Maps file paths to cache entries
    modules/
      {hash}.json           # Cached module data (AST, types, bytecode)
```

### Cache Key

Each module's cache key is computed from:
- File content hash (blake3)
- Hashes of all direct imports (transitive invalidation)
- Compiler version

### Invalidation

When a file changes:
1. Recompute its content hash
2. Invalidate its cache entry
3. Invalidate all modules that import it (transitively)
4. On next build, only recompile invalidated modules

### Cache Entries

```rust
/// A cached module entry.
struct CacheEntry {
    /// Hash of the source file content.
    source_hash: blake3::Hash,
    /// Hashes of direct imports (for transitive invalidation).
    import_hashes: Vec<blake3::Hash>,
    /// Parsed AST (serialized).
    ast: Vec<u8>,
    /// Type-checked module info.
    types: Vec<u8>,
    /// Compiled bytecode.
    bytecode: Vec<u8>,
}
```

## CLI Commands

### `ambient init`

Provisions a new package with default structure:

```bash
$ ambient init my_project
Created my_project/ambient.toml
Created my_project/src/main.ab

$ cat my_project/ambient.toml
[package]
name = "my_project"
version = "0.1.0"

[build]
src = "src"

$ cat my_project/src/main.ab
pub fn run(): () with platform::Stdio {
    platform::Stdio::out!("Hello, world!");
}
```

For library packages, simply delete `main.ab` and create your own entry points.
There's no binary vs library distinction - a package is a library if other packages import from it.

### `ambient run`

```bash
$ ambient run .                    # Run package in current directory
$ ambient run ./path/to/package    # Run package at path
```

### `ambient build`

```bash
$ ambient build .                  # Compile package, output to build/
$ ambient build . --release        # Compile with optimizations
```

### `ambient check`

```bash
$ ambient check .                  # Type-check only, no codegen
```

## Implementation Phases

### Phase 1: Package Discovery & CLI

Add the ability to locate and parse `ambient.toml`, and the `init` command.

**Files affected:**
- New: `crates/ambient-engine/src/manifest.rs` - Parse `ambient.toml`
- Modify: `crates/ambient-cli/src/cli.rs` - Add `init` subcommand
- New: `crates/ambient-cli/src/commands/init.rs` - Implement `init`
- Modify: `crates/ambient-cli/src/commands/run.rs` - Locate package root

**Deliverables:**
- `Manifest` struct with package name, version, source dir
- `find_package_root()` function that walks up directories looking for `ambient.toml`
- `ambient init` command creates package scaffolding
- CLI commands work with directory argument: `ambient run .`
- Entry point is always `{src}/main.ab`
- Remove single-file execution mode (packages only)

### Phase 2: Lazy Module Loading

Parse modules on-demand based on imports, with boundary guards.

**Files affected:**
- New: `crates/ambient-engine/src/package.rs` - Package and lazy loader
- New: `crates/ambient-engine/src/module_path.rs` - Module path types
- Modify: `crates/ambient-parser/src/lib.rs` - Add module path to `Module`
- Modify: `crates/ambient-cli/src/commands/mod.rs` - Use lazy loading

**Deliverables:**
- `ModulePath` type with resolution logic
- `PackageLoader` that lazily parses modules as they're imported
- Boundary guard: error if import escapes above `build.src`
- Only files reachable from `main.ab` are parsed

### Phase 3: Import Syntax

Extend the parser to handle the full import syntax.

**Files affected:**
- Modify: `crates/ambient-parser/src/lexer.rs` - Add `self`, `super`, `pkg`, `core` keywords
- Modify: `crates/ambient-parser/src/parser/mod.rs` - Enhanced `use` parsing
- Modify: `crates/ambient-parser/src/cst.rs` - Update `CstUseDef`
- Modify: `crates/ambient-engine/src/ast.rs` - Update `UseDef`

**Syntax to support:**
```ambient
use pkg::module;
use pkg::module::item;
use pkg::module::{item1, item2};
use self::sibling;
use super::parent;
use super::super::grandparent;
use core::collections::List;
pub use pkg::other::Thing;
```

**Deliverables:**
- Extended `CstUseDef` with path prefix enum and re-export flag
- `ImportPrefix` enum with `Super(usize)` for chained super
- Parser handles all import variants
- Lowering to AST

### Phase 4: Cross-module Name Resolution

Resolve names across module boundaries with cycle support.

**Files affected:**
- Major rewrite: `crates/ambient-parser/src/resolve.rs` → move to `ambient-engine`
- New: `crates/ambient-engine/src/resolve.rs` - Package-level resolver
- Modify: `crates/ambient-engine/src/ast.rs` - `QualifiedName` includes module path

**Deliverables:**
- `SymbolTable` with all symbols across loaded modules (including private items for error messages)
- `PackageResolver` with two-phase resolution (signatures then bodies)
- Resolution of `pkg::module::item` to concrete definitions
- `super` chain resolution with boundary guard
- Re-export chain resolution with cycle detection
- Visibility tracking (private items in table but not accessible)
- Error messages: "X is private" vs "X not found"
- `QualifiedName` extended with full module path

### Phase 5: Cross-module Type Checking

Type check a package with cross-module dependencies.

**Files affected:**
- Modify: `crates/ambient-engine/src/infer/mod.rs` - Package-level type context
- Modify: `crates/ambient-engine/src/infer/env.rs` - Import types from other modules
- Modify: `crates/ambient-engine/src/infer/check.rs` - Check visibility, detect type cycles

**Deliverables:**
- `check_package()` function that type-checks all loaded modules
- Two-phase type checking: signatures first, bodies second
- Type environment populated with imported types
- Visibility enforcement (private items not accessible outside module)
- Cycle detection for type aliases and constants
- Correct handling of generic imports

### Phase 6: Cross-module Compilation

Compile a package to a unified bytecode bundle.

**Files affected:**
- Modify: `crates/ambient-engine/src/compiler/mod.rs` - Package compilation
- Modify: `crates/ambient-cli/src/commands/run.rs` - Run packages
- Modify: `crates/ambient-cli/src/commands/compile.rs` - Compile packages

**Deliverables:**
- `compile_package()` produces unified `CompiledModule`
- All functions get unique hashes (include module path in hash)
- Entry point is always `main` function in `main.ab`
- CLI commands work with packages

### Phase 7: Incremental Compilation

Add build caching to `build/` directory.

**Files affected:**
- New: `crates/ambient-engine/src/cache.rs` - Cache management
- Modify: `crates/ambient-engine/src/package.rs` - Integrate caching
- Modify: `crates/ambient-cli/src/commands/mod.rs` - Use cache

**Deliverables:**
- `build/cache/` directory structure
- `CacheManifest` tracking file hashes and dependencies
- Cache invalidation on file change
- Transitive invalidation when imports change
- Cache hit skips parsing/type-checking/compilation
- `ambient build --clean` to clear cache

### Phase 8: Core Library

Implement the `core` standard library.

**Files affected:**
- New: `crates/ambient-core/src/prelude.ab` - Core types and functions
- Modify: `crates/ambient-engine/src/resolve.rs` - Built-in `core` module
- Modify: `crates/ambient-engine/src/infer/mod.rs` - Preload core types

**Deliverables:**
- Core modules: `core::collections::List`, `core::Option`, `core::Result`, `core::primitives::String`
- Implicit prelude (Option, Result always available)
- Core abilities: `core::console`, `core::time`, `core::random`

### Phase 9: LSP Integration

Update the language server for multi-file support.

**Files affected:**
- Modify: `crates/ambient-lsp/src/workspace.rs` - Package-aware indexing
- Modify: `crates/ambient-lsp/src/analysis.rs` - Cross-file analysis
- Modify: `crates/ambient-lsp/src/completions.rs` - Import completions

**Eager vs Lazy Indexing:**

The LSP uses a different strategy than the compiler:
- **Compiler**: Lazy - only parses files reachable from `main.ab`
- **LSP**: Eager filesystem scan - indexes all `.ab` files in `src/` to enable completions for unimported modules

The LSP maintains two levels of information:
1. **Filesystem index**: All `.ab` files and their module paths (cheap, always up-to-date via file watcher)
2. **Parsed modules**: Full AST/types for open files and their imports (parsed on demand)

This allows "add import" completions to suggest `pkg::utils::format` even if nothing imports it yet.

**Deliverables:**
- Eager filesystem scanning of `src/` directory
- File watcher for new/deleted/renamed `.ab` files
- Go-to-definition works across files (including through re-exports)
- Go-to-definition on `use` statements jumps to the module/item
- Import autocompletion suggests all available modules (not just imported ones)
- Unused import warnings
- Private item access warnings (not shown in autocomplete)
- Find all references across package

## Data Structures

### ModulePath

```rust
/// A path to a module within a package.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModulePath {
    /// Path segments (e.g., ["utils", "format"]).
    pub segments: Vec<Arc<str>>,
}

impl ModulePath {
    /// Create the root module path (main.ab).
    pub fn root() -> Self {
        Self { segments: vec![Arc::from("main")] }
    }

    /// Get the parent module path.
    pub fn parent(&self) -> Option<Self> {
        if self.segments.len() <= 1 {
            None
        } else {
            Some(Self {
                segments: self.segments[..self.segments.len() - 1].to_vec(),
            })
        }
    }

    /// Resolve a relative path from this module.
    /// Returns None if the resolution would escape above the package root.
    pub fn resolve_relative(
        &self,
        prefix: ImportPrefix,
        path: &[Arc<str>],
    ) -> Result<ModulePath, ResolutionError> {
        match prefix {
            ImportPrefix::Pkg => Ok(ModulePath { segments: path.to_vec() }),
            ImportPrefix::Core => Err(ResolutionError::CoreHandledSeparately),
            ImportPrefix::Self_ => {
                // Same directory as current module
                let parent = self.parent().ok_or(ResolutionError::EscapedPackageRoot)?;
                let mut segments = parent.segments;
                segments.extend(path.iter().cloned());
                Ok(ModulePath { segments })
            }
            ImportPrefix::Super(count) => {
                // Go up `count` directories
                let mut segments = self.segments.clone();
                for _ in 0..=count {
                    if segments.is_empty() {
                        return Err(ResolutionError::EscapedPackageRoot);
                    }
                    segments.pop();
                }
                if segments.is_empty() {
                    return Err(ResolutionError::EscapedPackageRoot);
                }
                segments.extend(path.iter().cloned());
                Ok(ModulePath { segments })
            }
        }
    }

    /// Convert to filesystem path relative to src directory.
    pub fn to_file_path(&self) -> PathBuf {
        let mut path = PathBuf::new();
        for segment in &self.segments {
            path.push(segment.as_ref());
        }
        path.set_extension("ab");
        path
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolutionError {
    /// Import would escape above the package source root.
    EscapedPackageRoot,
    /// Core imports are handled separately.
    CoreHandledSeparately,
    /// Module not found.
    ModuleNotFound(ModulePath),
}
```

### Manifest

```rust
/// Package manifest from ambient.toml.
#[derive(Debug, Clone)]
pub struct Manifest {
    /// Package name.
    pub name: String,
    /// Package version.
    pub version: String,
    /// Source directory relative to manifest (default: "src").
    pub src_dir: PathBuf,
}
// Entry point is always {src_dir}/main.ab - not stored in manifest
```

### Package & Lazy Loader

```rust
/// A package with lazily-loaded modules.
pub struct Package {
    /// Package manifest.
    pub manifest: Manifest,
    /// Root directory of the package (where ambient.toml is).
    pub root: PathBuf,
    /// Loaded modules, keyed by module path.
    modules: HashMap<ModulePath, Module>,
    /// Modules currently being loaded (for cycle detection during parsing).
    loading: HashSet<ModulePath>,
}

impl Package {
    /// Load a module by path, parsing it if not already loaded.
    /// Returns a reference to the parsed module.
    pub fn load_module(&mut self, path: &ModulePath) -> Result<&Module, LoadError> {
        if self.modules.contains_key(path) {
            return Ok(&self.modules[path]);
        }

        // Check for parse-time cycles (shouldn't happen with lazy loading)
        if self.loading.contains(path) {
            return Err(LoadError::CycleDuringParsing(path.clone()));
        }

        self.loading.insert(path.clone());

        // Resolve to filesystem path
        let file_path = self.root.join(&self.manifest.src_dir).join(path.to_file_path());

        // Parse the file
        let source = std::fs::read_to_string(&file_path)
            .map_err(|_| LoadError::FileNotFound(path.clone()))?;

        let module = ambient_parser::parse(&source)
            .map_err(|e| LoadError::ParseError(path.clone(), e))?;

        self.loading.remove(path);
        self.modules.insert(path.clone(), module);

        Ok(&self.modules[path])
    }

    /// Load all modules transitively imported from main.ab.
    pub fn load_all_imports(&mut self) -> Result<(), LoadError> {
        let mut queue = vec![ModulePath::root()];
        let mut visited = HashSet::new();

        while let Some(path) = queue.pop() {
            if visited.contains(&path) {
                continue;
            }
            visited.insert(path.clone());

            let module = self.load_module(&path)?;

            // Extract imports and add to queue
            for item in &module.items {
                if let ItemKind::Use(use_def) = &item.kind {
                    let imported_path = path.resolve_relative(
                        use_def.prefix.clone(),
                        &use_def.path,
                    )?;
                    queue.push(imported_path);
                }
            }
        }

        Ok(())
    }
}
```

### Import Path Prefix

```rust
/// The prefix of an import path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportPrefix {
    /// Local package: `pkg::module`
    Pkg,
    /// Standard library: `core::module`
    Core,
    /// Same directory: `self::sibling`
    Self_,
    /// Parent directory: `super::parent`, `super::super::grandparent`
    /// The usize is how many levels up (1 for `super`, 2 for `super::super`, etc.)
    Super(usize),
}
```

### UseDef (Updated)

```rust
/// A use/import statement.
#[derive(Debug, Clone)]
pub struct UseDef {
    /// Whether this is a re-export (`pub use`).
    pub is_public: bool,
    /// The path prefix (pkg, core, self, super).
    pub prefix: ImportPrefix,
    /// The module path after the prefix.
    pub path: Vec<Arc<str>>,
    /// What to import from the path.
    pub imports: UseImports,
    /// Source span.
    pub span: Span,
}
```

### Symbol Table

```rust
/// A symbol in the package-wide symbol table.
#[derive(Debug, Clone)]
pub struct Symbol {
    /// The module where this symbol is defined.
    pub module: ModulePath,
    /// The symbol name.
    pub name: Arc<str>,
    /// The kind of symbol.
    pub kind: SymbolKind,
    /// Whether the symbol is public.
    pub is_public: bool,
    /// Source span of the definition.
    pub span: Span,
}

/// Package-wide symbol table for cross-module resolution.
#[derive(Debug)]
pub struct SymbolTable {
    /// All symbols, keyed by (module_path, name).
    /// Includes private items (for better error messages).
    symbols: HashMap<(ModulePath, Arc<str>), Symbol>,
    /// Re-exports: maps (exporting_module, name) -> (source_module, name).
    reexports: HashMap<(ModulePath, Arc<str>), (ModulePath, Arc<str>)>,
}
```

### Build Cache

```rust
/// Manifest tracking cached build artifacts.
#[derive(Debug, Serialize, Deserialize)]
pub struct CacheManifest {
    /// Compiler version (invalidate all on version change).
    pub compiler_version: String,
    /// Cached modules.
    pub entries: HashMap<ModulePath, CacheEntry>,
}

/// A cached module entry.
#[derive(Debug, Serialize, Deserialize)]
pub struct CacheEntry {
    /// Hash of the source file content.
    pub source_hash: String,  // blake3 hex
    /// Hashes of direct imports (for transitive invalidation).
    pub import_hashes: Vec<String>,
    /// Path to cached AST file.
    pub ast_path: PathBuf,
    /// Path to cached type info file.
    pub types_path: PathBuf,
    /// Path to cached bytecode file.
    pub bytecode_path: PathBuf,
}

impl CacheManifest {
    /// Check if a module's cache is valid.
    pub fn is_valid(&self, path: &ModulePath, current_source_hash: &str) -> bool {
        if let Some(entry) = self.entries.get(path) {
            if entry.source_hash != current_source_hash {
                return false;
            }
            // Also check that import hashes haven't changed
            // (handled by transitive invalidation)
            true
        } else {
            false
        }
    }

    /// Invalidate a module and all modules that import it.
    pub fn invalidate(&mut self, path: &ModulePath) {
        self.entries.remove(path);

        // Find all modules that import this one and invalidate them
        let dependents: Vec<_> = self.entries
            .iter()
            .filter(|(_, entry)| {
                // Check if any import hash matches this module
                // (simplified - real impl would track import paths)
                false  // TODO: implement dependency tracking
            })
            .map(|(p, _)| p.clone())
            .collect();

        for dep in dependents {
            self.invalidate(&dep);
        }
    }
}
```

## Future: Workspace Support

The design explicitly supports workspaces for future implementation:

```toml
# Root ambient.toml
[workspace]
members = [
    "packages/*",
    "tools/cli",
]

# No [package] section in workspace root
```

### Design Considerations for Workspaces

1. **Package resolution**: `use other_pkg::module` would resolve to sibling packages
2. **Shared build directory**: `build/` at workspace root, per-package subdirectories
3. **Parallel compilation**: Independent packages can compile in parallel
4. **Cross-package dependencies**: Declared in package manifest

The current design does not block any of these features:
- `ImportPrefix` can be extended with `Dep(String)` for dependencies
- `Package` can be extended to reference other packages
- Cache can be shared across workspace

## Testing Strategy

Each phase should include:
1. Unit tests for new data structures
2. Integration tests with multi-file packages
3. Error case tests (missing imports, visibility violations, invalid cycles)
4. CLI tests with actual filesystem
5. Cycle-specific tests (valid cycles, invalid type cycles, const cycles)
6. Performance tests (large package with selective imports)
7. Incremental build tests (modify file, verify minimal recompilation)

## Migration

Existing single-file programs need to be migrated to packages:

```bash
# Old: ambient run hello.ab
# New:
mkdir hello && cd hello
ambient init .
mv ../hello.ab src/main.ab
ambient run .

# Or for minimal structure:
# ambient.toml with build.src = "." and a main.ab in the same directory
```

The migration is intentionally breaking to enforce the package model from the start.
