//! The flat resolved-import view: [`ResolvedImports`] and the
//! [`ModuleRegistry::resolve_imports`] adapter over scope building.

use std::collections::HashMap;
use std::sync::Arc;

use crate::module_path::ModulePath;

use super::{ExportKind, ModuleRegistry, RegistryError};

/// A resolved import - what a name refers to after processing `use` statements.
#[derive(Debug, Clone)]
pub enum ResolvedImport {
    /// The import refers to a module itself (e.g., `use pkg::utils;`)
    Module(ModulePath),
    /// The import refers to a specific symbol from a module.
    Symbol {
        /// The module that defines the symbol, with `pub use` re-export
        /// chains resolved to their origin — which is where the compiled
        /// function hashes live.
        from_module: ModulePath,
        /// The kind of symbol.
        export_kind: ExportKind,
        /// The span of the `use` item that created this import.
        span: crate::ast::Span,
    },
}

/// An import that failed to resolve, with the span of the `use`
/// declaration that caused it.
#[derive(Debug, Clone)]
pub struct ImportError {
    /// Why the import failed.
    pub error: RegistryError,
    /// The span of the `use` item in the importing module.
    pub span: crate::ast::Span,
}

/// The outcome of resolving a module's imports: the bindings that
/// resolved, plus a diagnostic for each import that did not.
#[derive(Debug, Default)]
pub struct ResolvedImports {
    /// Successfully resolved imports, keyed by local name.
    ///
    /// A single name can carry up to two bindings — a module and a symbol
    /// — because modules, values, and types occupy separate namespaces
    /// resolved by syntactic position (`c(...)` is the symbol, `c::foo` the
    /// module). `use a::b::c` binds whichever of those `c` names actually
    /// exist under `a::b`; when both do, both land here and the use site
    /// disambiguates. Within one namespace the last `use` wins.
    pub imports: HashMap<Arc<str>, Vec<ResolvedImport>>,
    /// Imports that failed to resolve.
    pub errors: Vec<ImportError>,
}

impl ResolvedImports {
    /// Bind `import` to `name`, keeping at most one module binding and one
    /// symbol binding per name. A later import of the same namespace shadows
    /// the earlier one; a module and a symbol coexist.
    fn bind(&mut self, name: Arc<str>, import: ResolvedImport) {
        let is_module = matches!(import, ResolvedImport::Module(_));
        let entry = self.imports.entry(name).or_default();
        entry.retain(|existing| matches!(existing, ResolvedImport::Module(_)) != is_module);
        entry.push(import);
    }
}

impl ModuleRegistry {
    /// Get all imported symbols for a module.
    ///
    /// An adapter over [`Self::build_module_scope`] for consumers that
    /// want the flat binding view.
    ///
    /// # Errors
    ///
    /// Returns an error if the importing module itself is not in the
    /// registry.
    pub fn resolve_imports(
        &self,
        module_path: &ModulePath,
    ) -> Result<ResolvedImports, RegistryError> {
        if !self.modules.contains_key(&module_path.to_string()) {
            return Err(RegistryError::ModuleNotFound(module_path.to_string()));
        }
        let scope = self.build_module_scope(module_path);

        let mut resolved = ResolvedImports {
            imports: HashMap::new(),
            errors: scope.errors,
        };
        for (local, module) in scope.modules {
            resolved.bind(local, ResolvedImport::Module(module));
        }
        for (local, imports) in scope.items {
            let entry = resolved.imports.entry(local).or_default();
            for import in imports {
                entry.push(ResolvedImport::Symbol {
                    from_module: import.module,
                    export_kind: import.kind,
                    span: import.span,
                });
            }
        }
        // Prelude re-exports are ordinary imports to the checker's
        // consumers (`register_imported_enums`, `retain_imported_type_aliases`),
        // at lowest precedence. Enum *variants* are excluded: the resolve
        // pass reads them straight off `ModuleScope::prelude_items` (so bare
        // `Some`/`None` still resolve), and surfacing them here would trip
        // `build_import_env`'s "import its enum instead" guard for a name
        // the user never wrote a `use` for.
        for (local, imports) in scope.prelude_items {
            let entry = resolved.imports.entry(local).or_default();
            for import in imports {
                if import.kind == ExportKind::EnumVariant {
                    continue;
                }
                entry.push(ResolvedImport::Symbol {
                    from_module: import.module,
                    export_kind: import.kind,
                    span: import.span,
                });
            }
        }
        Ok(resolved)
    }
}
