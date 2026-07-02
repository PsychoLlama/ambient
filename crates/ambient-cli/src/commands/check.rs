//! Check command implementation.

use std::path::Path;
use std::sync::Arc;

use anyhow::{bail, Result};

use ambient_engine::module_path::ModulePath;

use super::{core_context, platform_prelude, prelude_resolver, read_source};
use crate::diagnostic::print_diagnostic;

/// Check an Ambient source file for errors.
pub fn cmd_check(file: &Path) -> Result<()> {
    let source = read_source(file)?;

    // Parse.
    let module = match ambient_parser::parse(&source) {
        Ok(m) => m,
        Err(e) => {
            print_diagnostic(&source, file, &e);
            bail!("parse error in {}", file.display());
        }
    };

    // Type check with the core modules and platform prelude visible.
    let mut core = core_context()?;
    let main_path = ModulePath::root();
    core.registry.register(&main_path, Arc::new(module.clone()));
    let prelude = platform_prelude()?;
    let result = ambient_engine::infer::check_module_with_registry_and_resolver(
        module,
        &main_path,
        &core.registry,
        prelude_resolver(&prelude),
    );

    if result.is_ok() {
        eprintln!("No errors found in {}", file.display());
        Ok(())
    } else {
        // Format and print errors
        for error in &result.errors {
            print_diagnostic(&source, file, error);
        }
        bail!(
            "Found {} type error(s) in {}",
            result.errors.len(),
            file.display()
        );
    }
}
