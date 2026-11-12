//! Tags: no two declarations of a package share one. See
//! `spec/definitions.md`.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::{Engine, Query, memo};
use crate::error::Error;
use crate::surface::DefinitionKind;
use crate::symbol::{Namespace, PackageId, Symbol};

impl Engine<'_> {
    /// Checks that every ability and extern of `package` declares a tag
    /// of its own.
    ///
    /// A tag is the identity of what declares it, so two declarations
    /// with one tag would be one thing to anything outside the package.
    ///
    /// # Errors
    ///
    /// Fails with the first two declarations found to share a tag.
    pub(super) fn tags(&self, package: &PackageId) -> Query<()> {
        memo(&self.tags, package, || {
            let mut declared: BTreeMap<String, Symbol> = BTreeMap::new();
            for id in self.sources.modules(package) {
                for definition in &self.read(&id)?.definitions {
                    let (namespace, tag) = match &definition.kind {
                        DefinitionKind::Ability(ability) => (Namespace::Ability, &ability.tag),
                        DefinitionKind::Extern(native) => (Namespace::Value, &native.tag),
                        _ => continue,
                    };
                    let symbol = id.symbol(namespace, &definition.name);
                    if let Some(first) = declared.insert(tag.clone(), symbol.clone()) {
                        return Err(Error::DuplicateTag {
                            first,
                            second: symbol,
                            tag: tag.clone(),
                        });
                    }
                }
            }
            Ok(Arc::new(()))
        })
    }
}
