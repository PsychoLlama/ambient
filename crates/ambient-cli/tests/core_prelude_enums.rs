//! The canonical `Option`/`Result` declarations live in Ambient source
//! (`core_lib/option.ab`, `core_lib/result.ab`), while the type checker's
//! prelude is built from the engine-side spec (`PRELUDE_ENUMS`) because
//! the engine cannot parse. `validate_reserved_declaration` pins the two
//! together at build time; this test pins them at test time — parse the
//! embedded sources and check the declarations are present, reserved,
//! and canonical.

use std::sync::Arc;

use ambient_engine::ast::ItemKind;
use ambient_engine::core_library::CoreLibrary;
use ambient_engine::infer::enums::validate_reserved_declaration;
use ambient_engine::types::{OPTION_UUID, RESULT_UUID};

#[test]
fn core_sources_declare_the_canonical_prelude_enums() {
    let cases = [("Option", OPTION_UUID), ("Result", RESULT_UUID)];

    for (name, uuid) in cases {
        let source = CoreLibrary::get_source(&[Arc::from(name)])
            .unwrap_or_else(|e| panic!("core module `{name}` has embedded source: {e}"));
        let module = ambient_parser::parse(source)
            .unwrap_or_else(|e| panic!("core module `{name}` parses: {e}"));

        let def = module
            .items
            .iter()
            .find_map(|item| match &item.kind {
                ItemKind::Enum(def) if def.name.as_ref() == name => Some(def),
                _ => None,
            })
            .unwrap_or_else(|| panic!("core module `{name}` declares enum `{name}`"));

        assert!(def.is_public, "`{name}` must be `pub`");
        assert_eq!(def.uuid, uuid, "`{name}` must carry its reserved uuid");
        validate_reserved_declaration(def)
            .unwrap_or_else(|e| panic!("`{name}` declaration drifted from the prelude spec: {e}"));
    }
}
