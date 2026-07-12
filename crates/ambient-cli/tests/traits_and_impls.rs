//! Trait system and inherent impl tests.

mod common;
use common::*;

// ─────────────────────────────────────────────────────────────────────────────
// Trait System Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_trait_definition_and_impl() {
    // Test trait definition and implementation for a nominal type
    CliTest::new(
        r#"
        unique(AAAAAAAA-BBBB-4CCC-8DDD-000000000001) trait Show {
            fn show(self): Number;
        }

        unique(A1B2C3D4-0000-0000-0000-000000000001) struct Counter { value: Number }

        impl Show for Counter {
            fn show(self): Number {
                self.value
            }
        }

        fn run(): Number {
            let c = Counter { value: 42 };
            c.show()
        }
    "#,
    )
    .expect_output("42");
}

#[test]
fn test_trait_method_with_args() {
    // Test trait method that takes additional arguments
    CliTest::new(
        r#"
        unique(AAAAAAAA-BBBB-4CCC-8DDD-000000000002) trait Scalable {
            fn scale(self, factor: Number): Number;
        }

        unique(A1B2C3D4-0000-0000-0000-000000000002) struct Size { width: Number }

        impl Scalable for Size {
            fn scale(self, factor: Number): Number {
                self.width * factor
            }
        }

        fn run(): Number {
            let s = Size { width: 10 };
            s.scale(5)
        }
    "#,
    )
    .expect_output("50");
}

#[test]
fn test_operator_overloading_add() {
    // Test Add trait for operator overloading. `Add` is the prelude trait:
    // operators anchor on the reserved core trait identities, so
    // implementing the prelude `Add` — not a same-named local trait — is
    // what enables `+`.
    CliTest::new(
        r#"
        unique(A1B2C3D4-0000-0000-0000-000000000003) struct Money { cents: Number }

        impl Add for Money {
            fn add(self, other: Money): Money {
                Money { cents: self.cents + other.cents }
            }
        }

        fn run(): Number {
            let a = Money { cents: 100 };
            let b = Money { cents: 50 };
            let total = a + b;
            total.cents
        }
    "#,
    )
    .expect_output("150");
}

#[test]
fn test_operator_overloading_eq() {
    // Test Eq trait for equality comparison (the prelude `Eq`, which is
    // what `==` anchors on).
    CliTest::new(
        r#"
        unique(A1B2C3D4-0000-0000-0000-000000000004) struct Id { value: Number }

        impl Eq for Id {
            fn eq(self, other: Id): Bool {
                self.value == other.value
            }
        }

        fn run(): Bool {
            let a = Id { value: 42 };
            let b = Id { value: 42 };
            a == b
        }
    "#,
    )
    .expect_output("true");
}

#[test]
fn test_default_trait_associated_call() {
    // `core::traits::Default` provides an associated (no-`self`)
    // `default(): Self`, invoked as `Type::default()`. It is not in the
    // prelude, so it must be imported explicitly.
    CliTest::new(
        r#"
        use core::traits::Default;

        unique(A1B2C3D4-0000-0000-0000-000000000010) struct Config { level: Number }

        impl Default for Config {
            fn default(): Config {
                Config { level: 7 }
            }
        }

        fn run(): Number {
            let c = Config::default();
            c.level
        }
    "#,
    )
    .expect_output("7");
}

#[test]
fn test_default_trait_requires_import() {
    // `Default` is not in the prelude (only the operator traits are), so
    // implementing it without `use core::traits::Default;` is an unknown
    // trait — the visible proof that trait defs are import-scoped.
    CliTest::new(
        r#"
        unique(A1B2C3D4-0000-0000-0000-000000000013) struct Config { level: Number }

        impl Default for Config {
            fn default(): Config {
                Config { level: 7 }
            }
        }

        fn run(): Number { 0 }
    "#,
    )
    .expect_error("unknown trait: `Default`");
}

#[test]
fn test_default_trait_composes_with_operator() {
    // An associated call is an ordinary expression: it nests and composes
    // with operators like any other value.
    CliTest::new(
        r#"
        use core::traits::Default;

        unique(A1B2C3D4-0000-0000-0000-000000000011) struct Vec2 { x: Number, y: Number }

        impl Default for Vec2 {
            fn default(): Vec2 {
                Vec2 { x: 0, y: 0 }
            }
        }

        impl Add for Vec2 {
            fn add(self, other: Vec2): Vec2 {
                Vec2 { x: self.x + other.x, y: self.y + other.y }
            }
        }

        fn run(): Number {
            let v = Vec2::default() + Vec2 { x: 3, y: 4 };
            v.x + v.y
        }
    "#,
    )
    .expect_output("7");
}

#[test]
fn test_associated_trait_method_with_argument() {
    // The associated-call mechanism is not special to `Default`: any
    // user-declared trait method without `self` is callable as
    // `Type::method(args)`.
    CliTest::new(
        r#"
        unique(AAAAAAAA-BBBB-4CCC-8DDD-000000000005) trait FromNumber {
            fn from_number(n: Number): Self;
        }

        unique(A1B2C3D4-0000-0000-0000-000000000012) struct Wrapped { value: Number }

        impl FromNumber for Wrapped {
            fn from_number(n: Number): Wrapped {
                Wrapped { value: n * 2 }
            }
        }

        fn run(): Number {
            let w = Wrapped::from_number(21);
            w.value
        }
    "#,
    )
    .expect_output("42");
}

// ─────────────────────────────────────────────────────────────────────────────
// Inherent Impl Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_inherent_impl_method_call() {
    // `impl Type { ... }` attaches methods directly to a nominal type;
    // dot dispatch resolves them without any trait.
    CliTest::new(
        r#"
        unique(B1B2C3D4-0000-0000-0000-000000000001) struct Money { cents: Number }

        impl Money {
            fn double(self): Money {
                Money { cents: self.cents * 2 }
            }
        }

        fn run(): Number {
            let m = Money { cents: 21 };
            m.double().cents
        }
    "#,
    )
    .expect_output("42");
}

#[test]
fn test_inherent_impl_associated_call() {
    // A no-`self` inherent method is an associated function, called as
    // `Type::method(args)` — no trait declaration needed.
    CliTest::new(
        r#"
        unique(B1B2C3D4-0000-0000-0000-000000000002) struct Money { cents: Number }

        impl Money {
            fn from_dollars(d: Number): Money {
                Money { cents: d * 100 }
            }
            fn cents(self): Number {
                self.cents
            }
        }

        fn run(): Number {
            Money::from_dollars(3).cents()
        }
    "#,
    )
    .expect_output("300");
}

#[test]
fn test_inherent_impl_methods_call_each_other() {
    // Inherent method signatures register before bodies are checked, so
    // methods can call each other regardless of declaration order.
    CliTest::new(
        r#"
        unique(B1B2C3D4-0000-0000-0000-000000000003) struct Counter { n: Number }

        impl Counter {
            fn bump_twice(self): Counter {
                self.bump().bump()
            }
            fn bump(self): Counter {
                Counter { n: self.n + 1 }
            }
        }

        fn run(): Number {
            let c = Counter { n: 0 };
            c.bump_twice().n
        }
    "#,
    )
    .expect_output("2");
}

#[test]
fn test_inherent_impl_generic_option() {
    // Generic inherent impls attach methods to built-in type constructors.
    // The receiver's type arguments instantiate the impl's parameters.
    CliTest::new(
        r#"
        impl<T> Option<T> {
            fn get_or(self, fallback: T): T {
                match self {
                    Some(v) => v,
                    None => fallback,
                }
            }
        }

        fn run(): Number {
            let a: Option<Number> = Some(40);
            let b: Option<Number> = None;
            a.get_or(0) + b.get_or(2)
        }
    "#,
    )
    .expect_output("42");
}

#[test]
fn test_inherent_impl_generic_method_on_user_enum() {
    // A generic method (its own type parameter, beyond the impl's) on a
    // user-declared enum.
    CliTest::new(
        r#"
        unique(C1B2C3D4-0000-0000-0000-000000000010) enum Box2 { Full(Number), Empty }

        impl Box2 {
            fn map_or<U>(self, fallback: U, f: (Number) -> U): U {
                match self {
                    Full(v) => f(v),
                    Empty => fallback,
                }
            }
        }

        fn run(): Number {
            let b = Full(20);
            b.map_or(0, (v) => v * 2) + Empty.map_or(2, (v) => v)
        }
    "#,
    )
    .expect_output("42");
}

#[test]
fn test_unit_struct_constructed_by_bare_name() {
    // A unit struct (`unique(...) struct Origin;`) is constructed by its bare
    // name — `Origin` *is* the sole value of its type, like a nullary enum
    // variant. It flows through a parameter of its own nominal type, proving
    // parse → resolve → check → compile → run.
    CliTest::new(
        r#"
        unique(C1B2C3D4-0000-0000-0000-000000000030) struct Origin;

        fn accept(_o: Origin): Number { 7 }

        fn run(): Number {
            let o = Origin;
            accept(o)
        }
    "#,
    )
    .expect_output("7");
}

#[test]
fn test_field_bearing_struct_used_bare_is_undefined() {
    // Only *unit* structs gain a value binding. A field-bearing struct named
    // in value position is not a value — it still fails as an undefined
    // variable, guarding against over-eager binding.
    CliTest::new(
        r#"
        unique(C1B2C3D4-0000-0000-0000-000000000031) struct Point { x: Number }

        fn run(): Number {
            let p = Point;
            0
        }
    "#,
    )
    .check()
    .expect_error("undefined variable: `Point`");
}

#[test]
fn test_inherent_method_with_ability() {
    // Inherent methods declare effects like public functions: a `with`
    // clause on the method, enforced on the body and required at call
    // sites.
    CliTest::new(
        r#"
        unique(B1B2C3D4-0000-0000-0000-000000000004) struct Greeter { name: String }

        impl Greeter {
            fn greet(self): () with core::system::Stdio {
                core::system::Stdio::out!("hello ${self.name}");
            }
        }

        pub fn run(): () with core::system::Stdio {
            let g = Greeter { name: "world" };
            g.greet();
        }
    "#,
    )
    .expect_output("hello world");
}

#[test]
fn test_inherent_method_undeclared_ability_error() {
    // A pure-signature inherent method whose body performs an ability is
    // rejected, exactly like a public function.
    CliTest::new(
        r#"
        unique(B1B2C3D4-0000-0000-0000-000000000005) struct Greeter { name: String }

        impl Greeter {
            fn greet(self): () {
                core::system::Stdio::out!("hello");
            }
        }

        fn run(): () {
            let g = Greeter { name: "x" };
            g.greet();
        }
    "#,
    )
    .expect_error("uses ability");
}

#[test]
fn test_inherent_method_ability_required_at_call_site() {
    // The method's declared abilities propagate to callers: a pure public
    // function cannot call an effectful method.
    CliTest::new(
        r#"
        unique(B1B2C3D4-0000-0000-0000-000000000006) struct Greeter { name: String }

        impl Greeter {
            fn greet(self): () with core::system::Stdio {
                core::system::Stdio::out!("hello");
            }
        }

        pub fn run(): () {
            let g = Greeter { name: "x" };
            g.greet();
        }
    "#,
    )
    .expect_error("uses ability");
}

#[test]
fn test_duplicate_inherent_method_error() {
    // Two definitions of the same method for the same type would compete
    // for one dispatch symbol; coherence rejects the second.
    CliTest::new(
        r#"
        unique(B1B2C3D4-0000-0000-0000-000000000007) struct Money { cents: Number }

        impl Money {
            fn double(self): Money {
                Money { cents: self.cents * 2 }
            }
        }

        impl Money {
            fn double(self): Money {
                Money { cents: self.cents * 4 }
            }
        }

        fn run(): Number {
            Money { cents: 1 }.double().cents
        }
    "#,
    )
    .expect_error("duplicate inherent method");
}

#[test]
fn test_inherent_method_shadows_trait_method() {
    // Dispatch precedence: inherent methods win over same-named trait
    // methods (like Rust), so adding an inherent method is a deliberate
    // local override rather than an ambiguity error.
    CliTest::new(
        r#"
        unique(AAAAAAAA-BBBB-4CCC-8DDD-000000000006) trait Doubler {
            fn double(self): Self;
        }

        unique(B1B2C3D4-0000-0000-0000-000000000008) struct Num { val: Number }

        impl Doubler for Num {
            fn double(self): Num {
                Num { val: self.val * 2 }
            }
        }

        impl Num {
            fn double(self): Num {
                Num { val: self.val * 10 }
            }
        }

        fn run(): Number {
            let n = Num { val: 4 };
            n.double().val
        }
    "#,
    )
    .expect_output("40");
}

#[test]
fn test_inherent_impl_multiple_blocks_merge() {
    // Several impl blocks for one type merge; only duplicate method
    // names collide.
    CliTest::new(
        r#"
        unique(B1B2C3D4-0000-0000-0000-000000000009) struct Point { x: Number, y: Number }

        impl Point {
            fn sum(self): Number {
                self.x + self.y
            }
        }

        impl Point {
            fn swap(self): Point {
                Point { x: self.y, y: self.x }
            }
        }

        fn run(): Number {
            let p = Point { x: 1, y: 41 };
            p.swap().sum()
        }
    "#,
    )
    .expect_output("42");
}

#[test]
fn test_inherent_impl_on_structural_type_error() {
    // Structural types have no identity to attach methods to.
    CliTest::new(
        r#"
        impl { x: Number } {
            fn get_x(self): Number {
                self.x
            }
        }

        fn run(): Number { 0 }
    "#,
    )
    .expect_failure();
}

#[test]
fn test_inherent_impl_missing_return_type_error() {
    // Inherent method signatures are the dispatch contract; the return
    // type must be declared.
    CliTest::new(
        r#"
        unique(B1B2C3D4-0000-0000-0000-00000000000A) struct Money { cents: Number }

        impl Money {
            fn double(self) {
                Money { cents: self.cents * 2 }
            }
        }

        fn run(): Number { 0 }
    "#,
    )
    .expect_error("must declare a return type");
}

#[test]
fn test_multiple_traits_same_type() {
    // Test implementing multiple traits for the same type
    CliTest::new(
        r#"
        unique(AAAAAAAA-BBBB-4CCC-8DDD-000000000007) trait Doubler {
            fn double(self): Self;
        }

        unique(AAAAAAAA-BBBB-4CCC-8DDD-000000000008) trait Tripler {
            fn triple(self): Self;
        }

        unique(A1B2C3D4-0000-0000-0000-000000000005) struct Num { val: Number }

        impl Doubler for Num {
            fn double(self): Num {
                Num { val: self.val * 2 }
            }
        }

        impl Tripler for Num {
            fn triple(self): Num {
                Num { val: self.val * 3 }
            }
        }

        fn run(): Number {
            let n = Num { val: 5 };
            let doubled = n.double();
            let tripled = n.triple();
            doubled.val + tripled.val
        }
    "#,
    )
    .expect_output("25");
}

#[test]
fn test_impl_method_calls_top_level_function() {
    // Regression: impl methods are compiled through the same hash
    // finalization as ordinary functions, so calls from an impl method to a
    // top-level function must resolve at runtime. (Previously the call was
    // left as an unresolved temporary hash: UnknownFunction at runtime.)
    CliTest::new(
        r#"
        unique(AAAAAAAA-BBBB-4CCC-8DDD-000000000009) trait Show {
            fn show(self): Number;
        }

        unique(A1B2C3D4-0000-0000-0000-000000000006) struct Wrapper { value: Number }

        fn double(n: Number): Number { n * 2 }

        impl Show for Wrapper {
            fn show(self): Number {
                double(self.value)
            }
        }

        fn run(): Number {
            let w = Wrapper { value: 21 };
            w.show()
        }
    "#,
    )
    .expect_output("42");
}

#[test]
fn test_impl_method_with_lambda() {
    // Regression: lambdas inside impl methods must be compiled and linked.
    // (Previously impl methods used a throwaway module context, so their
    // lambdas were silently dropped.)
    CliTest::new(
        r#"
        unique(AAAAAAAA-BBBB-4CCC-8DDD-00000000000A) trait Transform {
            fn apply(self): Number;
        }

        unique(A1B2C3D4-0000-0000-0000-000000000007) struct Box { value: Number }

        impl Transform for Box {
            fn apply(self): Number {
                let f = (x) => x + 1;
                f(self.value)
            }
        }

        fn run(): Number {
            let b = Box { value: 41 };
            b.apply()
        }
    "#,
    )
    .expect_output("42");
}

#[test]
fn test_operator_overloading_ne() {
    // `!=` must negate the prelude Eq trait's `eq` result.
    CliTest::new(
        r#"
        unique(A1B2C3D4-0000-0000-0000-000000000008) struct Id { value: Number }

        impl Eq for Id {
            fn eq(self, other: Id): Bool {
                self.value == other.value
            }
        }

        fn run(): Bool {
            let a = Id { value: 1 };
            let b = Id { value: 2 };
            a != b
        }
    "#,
    )
    .expect_output("true");
}

#[test]
fn test_operator_overloading_ordering() {
    // `<`, `<=`, `>`, `>=` must compare the prelude Ord trait's `cmp`
    // result (-1/0/1) against zero rather than returning it directly.
    CliTest::new(
        r#"
        unique(A1B2C3D4-0000-0000-0000-000000000009) struct Money { cents: Number }

        impl Ord for Money {
            fn cmp(self, other: Money): Number {
                if self.cents < other.cents { 0 - 1 } else {
                    if self.cents > other.cents { 1 } else { 0 }
                }
            }
        }

        fn run(): Number {
            let small = Money { cents: 50 };
            let big = Money { cents: 100 };

            let c1 = if small < big { 1 } else { 0 };
            let c2 = if big > small { 1 } else { 0 };
            let c3 = if small <= small { 1 } else { 0 };
            let c4 = if big >= small { 1 } else { 0 };
            let c5 = if big < small { 0 } else { 1 };
            c1 + c2 + c3 + c4 + c5
        }
    "#,
    )
    .expect_output("5");
}

// ─────────────────────────────────────────────────────────────────────────────
// Trait impls for enums
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_trait_impl_for_enum() {
    // A declared enum is nominal, so it can implement a trait: the impl
    // registers under the enum's uuid, direct dot dispatch (`.eq(...)`) and
    // operator sugar (`==`) both resolve, and the enum type satisfies a
    // `T: Eq` bound (its Eq dictionary flows to the bounded call).
    CliTest::new(
        r#"
        unique(C1B2C3D4-0000-0000-0000-0000000000E1) enum Shape {
            Circle(Number),
            Square(Number),
            Dot,
        }

        impl Eq for Shape {
            fn eq(self, other: Shape): Bool {
                match self {
                    Circle(r) => match other {
                        Circle(r2) => r == r2,
                        Square(_) => false,
                        Dot => false,
                    },
                    Square(s) => match other {
                        Square(s2) => s == s2,
                        Circle(_) => false,
                        Dot => false,
                    },
                    Dot => match other {
                        Dot => true,
                        Circle(_) => false,
                        Square(_) => false,
                    },
                }
            }
        }

        fn either_equal<T: Eq>(target: T, a: T, b: T): Bool {
            if target.eq(a) { true } else { target.eq(b) }
        }

        pub fn run(): Number {
            let direct = Circle(5).eq(Circle(5));
            let op = Dot == Dot;
            let bound = either_equal(Square(3), Circle(1), Square(3));
            let n1 = if direct { 1 } else { 0 };
            let n2 = if op { 10 } else { 0 };
            let n3 = if bound { 100 } else { 0 };
            n1 + n2 + n3
        }
    "#,
    )
    .expect_output("111");
}

#[test]
fn test_generic_trait_impl_on_container_accepted() {
    // A conditional trait impl on a builtin container (`impl<T> Eq for
    // List<T>`) is now a valid dictionary source: a bounded generic can
    // satisfy `List<Number>: Eq` through it.
    CliTest::new(
        r#"
        impl<T> Eq for List<T> {
            fn eq(self, other: List<T>): Bool { true }
        }

        fn check_eq<T: Eq>(a: T, b: T): Bool { a.eq(b) }

        fn run(): Bool { check_eq([1, 2, 3], [1, 2, 3]) }
    "#,
    )
    .expect_output("true");
}

#[test]
fn test_trait_impl_on_applied_container_accepted() {
    // A trait impl whose target carries concrete type arguments
    // (`impl Eq for Option<Number>`) is a conditional impl with no bounds;
    // the solver matches the exact instantiation, so `Option<Number>: Eq`
    // is satisfied while `Option<String>: Eq` is not.
    CliTest::new(
        r#"
        impl Eq for Option<Number> {
            fn eq(self, other: Option<Number>): Bool { true }
        }

        fn check_eq<T: Eq>(a: T, b: T): Bool { a.eq(b) }

        fn run(): Bool { check_eq(Some(1), Some(2)) }
    "#,
    )
    .expect_output("true");
}

#[test]
fn test_trait_impl_on_applied_container_wrong_instantiation_rejected() {
    // `impl Eq for Option<Number>` must not satisfy `Option<String>: Eq`:
    // the solver matches the concrete instantiation, not just the head uuid.
    CliTest::new(
        r#"
        impl Eq for Option<Number> {
            fn eq(self, other: Option<Number>): Bool { true }
        }

        fn check_eq<T: Eq>(a: T, b: T): Bool { a.eq(b) }

        fn run(): Bool { check_eq(Some("a"), Some("b")) }
    "#,
    )
    .check()
    .expect_failure();
}
