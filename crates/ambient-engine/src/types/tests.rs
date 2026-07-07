use std::collections::HashMap;

use uuid::Uuid;

use super::*;

/// A distinct, recognizable `AbilityId` for tests.
fn aid(n: u8) -> AbilityId {
    AbilityId::from_bytes([n; 32])
}

#[test]
fn test_primitive_types_display() {
    assert_eq!(Type::Unit.to_string(), "()");
    assert_eq!(Type::bool().to_string(), "Bool");
    assert_eq!(Type::number().to_string(), "Number");
    assert_eq!(Type::string().to_string(), "String");
    assert_eq!(Type::Never.to_string(), "!");
}

#[test]
fn test_tuple_type_display() {
    let tuple = Type::tuple(vec![Type::number(), Type::string()]);
    assert_eq!(tuple.to_string(), "(Number, String)");
}

#[test]
fn test_record_type_display() {
    let record = Type::record([("x", Type::number()), ("y", Type::number())]);
    assert_eq!(record.to_string(), "{ x: Number, y: Number }");
}

#[test]
fn test_function_type_display() {
    let func = Type::function(vec![Type::number(), Type::number()], Type::number());
    assert_eq!(func.to_string(), "(Number, Number) -> Number");
}

#[test]
fn test_named_type_display() {
    let list = Type::named("List", vec![Type::number()]);
    assert_eq!(list.to_string(), "List<Number>");

    let map = Type::named("Map", vec![Type::string(), Type::number()]);
    assert_eq!(map.to_string(), "Map<String, Number>");
}

#[test]
fn test_type_var_display() {
    let var = Type::var(0);
    assert_eq!(var.to_string(), "'0");
}

#[test]
fn test_forall_type_display() {
    let forall = Type::forall(vec![0, 1], Type::function(vec![Type::var(0)], Type::var(1)));
    assert_eq!(forall.to_string(), "forall '0 '1. ('0) -> '1");
}

#[test]
fn test_type_var_generator() {
    let mut r#gen = TypeVarGen::new();
    let v1 = r#gen.fresh();
    let v2 = r#gen.fresh();
    let v3 = r#gen.fresh();

    assert_eq!(v1, Type::var(0));
    assert_eq!(v2, Type::var(1));
    assert_eq!(v3, Type::var(2));
}

#[test]
fn test_record_field_access() {
    let record =
        if let Type::Record(rec) = Type::record([("x", Type::number()), ("y", Type::string())]) {
            rec
        } else {
            panic!("Expected record type");
        };

    assert_eq!(record.get_field("x"), Some(&Type::number()));
    assert_eq!(record.get_field("y"), Some(&Type::string()));
    assert_eq!(record.get_field("z"), None);
}

#[test]
fn test_free_vars() {
    let t = Type::function(vec![Type::var(0)], Type::var(1));
    let vars = t.free_vars();
    assert_eq!(vars, vec![0, 1]);
}

#[test]
fn test_free_vars_in_forall() {
    // forall '0. ('0 -> '1) should have '1 free, '0 bound
    let t = Type::forall(vec![0], Type::function(vec![Type::var(0)], Type::var(1)));
    let vars = t.free_vars();
    assert_eq!(vars, vec![1]);
}

#[test]
fn test_substitute() {
    let t = Type::function(vec![Type::var(0)], Type::var(1));
    let mut subst = HashMap::new();
    subst.insert(0, Type::number());
    subst.insert(1, Type::string());

    let result = t.substitute(&subst);
    assert_eq!(result, Type::function(vec![Type::number()], Type::string()));
}

#[test]
fn test_is_concrete() {
    assert!(Type::number().is_concrete());
    assert!(Type::function(vec![Type::number()], Type::string()).is_concrete());
    assert!(!Type::var(0).is_concrete());
    assert!(!Type::function(vec![Type::var(0)], Type::number()).is_concrete());
}

#[test]
fn test_nominal_type_inequality() {
    let uuid1 = Uuid::new_v4();
    let uuid2 = Uuid::new_v4();

    let nominal1 = Type::nominal(uuid1, Type::string(), Some("UserId"));
    let nominal2 = Type::nominal(uuid2, Type::string(), Some("OrderId"));

    // Same structure, different UUIDs -> different types
    assert_ne!(nominal1, nominal2);
}

#[test]
fn test_nominal_type_equality() {
    let uuid = Uuid::new_v4();

    let nominal1 = Type::nominal(uuid, Type::string(), Some("UserId"));
    let nominal2 = Type::nominal(uuid, Type::string(), Some("UserId"));

    // Same UUID -> same type
    assert_eq!(nominal1, nominal2);
}

// ─────────────────────────────────────────────────────────────────────────
// Ability type tests (Milestone 8)
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn test_ability_set_empty() {
    let empty = AbilitySet::empty();
    assert!(empty.is_empty());
    assert!(empty.is_pure());
    assert!(!empty.contains(aid(1)));
    assert_eq!(empty.to_string(), "{}");
}

#[test]
fn test_ability_set_single() {
    let single = AbilitySet::single(aid(1));
    assert!(!single.is_empty());
    assert!(!single.is_pure());
    assert!(single.contains(aid(1)));
    assert!(!single.contains(aid(2)));
    assert_eq!(single.to_string(), format!("{{#{}}}", aid(1)));
}

#[test]
fn test_ability_set_from_abilities() {
    let abilities = AbilitySet::from_abilities([aid(3), aid(1), aid(2), aid(1)]); // duplicates should be removed
    assert!(abilities.contains(aid(1)));
    assert!(abilities.contains(aid(2)));
    assert!(abilities.contains(aid(3)));
    assert!(!abilities.contains(aid(4)));
    // Should be sorted
    assert_eq!(abilities.concrete_abilities(), &[aid(1), aid(2), aid(3)]);
}

#[test]
fn test_ability_set_var() {
    let var = AbilitySet::var(42);
    assert!(!var.is_empty());
    assert!(!var.is_pure());
    assert_eq!(var.ability_var(), Some(42));
    assert_eq!(var.to_string(), "E42!");
}

#[test]
fn test_ability_set_row() {
    let row = AbilitySet::row([aid(1), aid(2)], 99);
    assert!(!row.is_empty());
    assert!(row.contains(aid(1)));
    assert!(row.contains(aid(2)));
    assert_eq!(row.ability_var(), Some(99));
    assert_eq!(
        row.to_string(),
        format!("{{#{}, #{}, E99!}}", aid(1), aid(2))
    );
}

#[test]
fn test_ability_set_union() {
    let a = AbilitySet::from_abilities([aid(1), aid(2)]);
    let b = AbilitySet::from_abilities([aid(2), aid(3)]);
    let union = a.union(&b);

    if let AbilitySet::Concrete(abilities) = union {
        assert_eq!(abilities, vec![aid(1), aid(2), aid(3)]);
    } else {
        panic!("Expected concrete ability set");
    }
}

#[test]
fn test_ability_set_union_with_var() {
    let concrete = AbilitySet::from_abilities([aid(1), aid(2)]);
    let var = AbilitySet::var(0);
    let union = concrete.union(&var);

    if let AbilitySet::Row { concrete, tail } = union {
        assert_eq!(concrete, vec![aid(1), aid(2)]);
        assert_eq!(tail, 0);
    } else {
        panic!("Expected row ability set");
    }
}

#[test]
fn test_ability_set_free_vars() {
    let empty = AbilitySet::empty();
    assert!(empty.free_ability_vars().is_empty());

    let concrete = AbilitySet::from_abilities([aid(1), aid(2)]);
    assert!(concrete.free_ability_vars().is_empty());

    let var = AbilitySet::var(5);
    assert_eq!(var.free_ability_vars(), vec![5]);

    let row = AbilitySet::row([aid(1), aid(2)], 10);
    assert_eq!(row.free_ability_vars(), vec![10]);
}

#[test]
fn test_ability_value_type() {
    let av = Type::ability_value(Type::string(), AbilitySet::single(aid(1)));
    assert_eq!(av.to_string(), format!("Ability<String, {{#{}}}>", aid(1)));

    if let Type::AbilityValue(avt) = av {
        assert_eq!(*avt.result, Type::string());
        assert!(avt.ability.contains(aid(1)));
    } else {
        panic!("Expected AbilityValue type");
    }
}

#[test]
fn test_function_with_abilities() {
    let func = Type::function_with_abilities(
        vec![Type::string()],
        Type::Unit,
        AbilitySet::from_abilities([aid(1), aid(2)]),
    );

    assert_eq!(
        func.to_string(),
        format!("(String) -> () with {{#{}, #{}}}", aid(1), aid(2))
    );

    if let Type::Function(ft) = func {
        assert!(!ft.is_pure());
        assert!(ft.abilities.contains(aid(1)));
        assert!(ft.abilities.contains(aid(2)));
    } else {
        panic!("Expected function type");
    }
}

#[test]
fn test_pure_function() {
    let func = Type::function(vec![Type::number()], Type::number());

    if let Type::Function(ft) = func {
        assert!(ft.is_pure());
        assert!(ft.abilities.is_empty());
    } else {
        panic!("Expected function type");
    }
}

#[test]
fn test_ability_var_generator() {
    let mut r#gen = TypeVarGen::new();
    let v1 = r#gen.fresh_ability_var();
    let v2 = r#gen.fresh_ability_var();

    assert_eq!(v1, AbilitySet::Var(0));
    assert_eq!(v2, AbilitySet::Var(1));
}

#[test]
fn test_forall_with_ability_vars() {
    let forall = Type::Forall(ForallType::with_abilities(
        vec![0],
        vec![1],
        Type::function_with_abilities(vec![Type::var(0)], Type::var(0), AbilitySet::var(1)),
    ));

    assert_eq!(forall.to_string(), "forall '0 E1!. ('0) -> '0 with E1!");
}

#[test]
fn test_ability_value_is_not_concrete() {
    let av = Type::ability_value(Type::string(), AbilitySet::var(0));
    assert!(!av.is_concrete());

    let av_concrete = Type::ability_value(Type::string(), AbilitySet::single(aid(1)));
    assert!(av_concrete.is_concrete());
}

#[test]
fn test_function_with_ability_var_is_not_concrete() {
    let func =
        Type::function_with_abilities(vec![Type::number()], Type::number(), AbilitySet::var(0));
    assert!(!func.is_concrete());
}

#[test]
fn test_free_ability_vars_in_function() {
    let func =
        Type::function_with_abilities(vec![Type::number()], Type::number(), AbilitySet::var(42));
    assert_eq!(func.free_ability_vars(), vec![42]);
}

#[test]
fn test_free_ability_vars_in_ability_value() {
    let av = Type::ability_value(Type::string(), AbilitySet::var(10));
    assert_eq!(av.free_ability_vars(), vec![10]);
}

#[test]
fn test_substitute_ability_vars() {
    let func = Type::function_with_abilities(vec![Type::var(0)], Type::var(0), AbilitySet::var(1));

    let type_subst: HashMap<TypeVarId, Type> = [(0, Type::number())].into_iter().collect();
    let ability_subst: HashMap<AbilityVarId, AbilitySet> =
        [(1, AbilitySet::single(aid(99)))].into_iter().collect();

    let result = func.substitute_all(&type_subst, &ability_subst);

    if let Type::Function(ft) = result {
        assert_eq!(ft.params, vec![Type::number()]);
        assert_eq!(*ft.ret, Type::number());
        assert_eq!(ft.abilities, AbilitySet::single(aid(99)));
    } else {
        panic!("Expected function type");
    }
}

#[test]
fn test_type_hole_display() {
    assert_eq!(Type::Hole.to_string(), "_");
}

#[test]
fn test_type_hole_is_not_concrete() {
    assert!(!Type::Hole.is_concrete());
    // Hole in nested type
    assert!(!Type::function(vec![Type::Hole], Type::number()).is_concrete());
    assert!(!Type::Tuple(vec![Type::number(), Type::Hole]).is_concrete());
}

#[test]
fn test_ability_registry_basic() {
    let mut registry = AbilityRegistry::new();

    let info = AbilityInfo::new("Console").with_method("print", vec![Type::string()], Type::Unit);

    registry.register(aid(1), info);

    assert!(registry.get(aid(1)).is_some());
    assert_eq!(registry.lookup("Console"), Some(aid(1)));
    assert_eq!(registry.lookup("Unknown"), None);
}

#[test]
fn test_ability_registry_dependencies() {
    let mut registry = AbilityRegistry::new();

    // IO is a base ability
    registry.register(aid(1), AbilityInfo::new("IO"));

    // FileSystem depends on IO
    registry.register(
        aid(2),
        AbilityInfo::new("FileSystem").with_dependency(aid(1)),
    );

    // Database depends on IO
    registry.register(aid(3), AbilityInfo::new("Database").with_dependency(aid(1)));

    // App depends on FileSystem and Database
    registry.register(
        aid(4),
        AbilityInfo::new("App")
            .with_dependency(aid(2))
            .with_dependency(aid(3)),
    );

    // Check transitive dependencies
    assert!(registry.transitive_dependencies(aid(1)).is_empty());
    assert_eq!(registry.transitive_dependencies(aid(2)), vec![aid(1)]);
    assert_eq!(registry.transitive_dependencies(aid(3)), vec![aid(1)]);

    // App should transitively depend on IO via both FileSystem and Database
    let app_deps = registry.transitive_dependencies(aid(4));
    assert!(app_deps.contains(&aid(1))); // IO
    assert!(app_deps.contains(&aid(2))); // FileSystem
    assert!(app_deps.contains(&aid(3))); // Database
}

#[test]
fn test_ability_with_dependencies() {
    let mut registry = AbilityRegistry::new();

    registry.register(aid(1), AbilityInfo::new("IO"));
    registry.register(
        aid(2),
        AbilityInfo::new("FileSystem").with_dependency(aid(1)),
    );

    let set = registry.ability_with_dependencies(aid(2));

    // Should include both FileSystem (2) and IO (1)
    if let AbilitySet::Concrete(abilities) = set {
        assert!(abilities.contains(&aid(1)));
        assert!(abilities.contains(&aid(2)));
    } else {
        panic!("Expected concrete ability set");
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Option and Result type tests (Milestone 15)
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn test_option_type() {
    let opt_num = Type::option(Type::number());
    assert_eq!(opt_num.to_string(), "Option<Number>");

    // Check as_option works
    assert_eq!(opt_num.as_option(), Some(&Type::number()));

    // Non-option types return None
    assert_eq!(Type::number().as_option(), None);
    assert_eq!(Type::named("List", vec![Type::number()]).as_option(), None);
}

#[test]
fn test_result_type() {
    let res = Type::result(Type::string(), Type::number());
    assert_eq!(res.to_string(), "Result<String, Number>");

    // Check as_result works
    assert_eq!(res.as_result(), Some((&Type::string(), &Type::number())));

    // Non-result types return None
    assert_eq!(Type::number().as_result(), None);
    assert_eq!(Type::option(Type::number()).as_result(), None);
}

#[test]
fn test_as_list() {
    let list = Type::named("List", vec![Type::number()]);
    assert_eq!(list.as_list(), Some(&Type::number()));

    // Non-list types return None
    assert_eq!(Type::number().as_list(), None);
    assert_eq!(Type::option(Type::number()).as_list(), None);
}

#[test]
fn test_nested_option_result() {
    // Option<Result<number, string>>
    let nested = Type::option(Type::result(Type::number(), Type::string()));
    assert_eq!(nested.to_string(), "Option<Result<Number, String>>");

    // Check we can extract inner types
    if let Some(inner) = nested.as_option() {
        if let Some((ok, err)) = inner.as_result() {
            assert_eq!(ok, &Type::number());
            assert_eq!(err, &Type::string());
        } else {
            panic!("Expected Result inside Option");
        }
    } else {
        panic!("Expected Option type");
    }
}
