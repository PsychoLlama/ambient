//! The program that runs tests: an entry the engine writes, which hands
//! `system.test.main` each package's runner and its tests. See "Running"
//! and "Keeping" in `docs/spec/testing.md`.
//!
//! What a test is handed as is plain data, but for the function that is
//! the test and the runner, which are named where the entry is written.
//! A test's key is worked out here, where its hash is known.

use std::collections::BTreeSet;

use ambient_runtime::FAULT_PACKAGE;

use super::Engine;
use crate::error::Error;
use crate::exchange;
use crate::hash::{Hasher, digest};
use crate::literal::Literal;
use crate::surface::{Expr, Key, Reference};
use crate::symbol::{ModuleId, Symbol};
use crate::types::abilities;
use crate::value::Value;

/// The tests of a run, as the command chose them.
pub struct Testing {
    /// The directory each package's cache is in, by its ID.
    pub directory: String,
    /// The hashes of what runs a test, which a trace leaves out.
    pub harness: Vec<String>,
    pub packages: Vec<Tested>,
}

/// A package's tests, and what runs them.
pub struct Tested {
    pub id: String,
    pub runner: Symbol,
    pub cases: Vec<Case>,
}

/// A test that was found: a `test.Test`, the function that is the test,
/// and why it cannot be run, if it cannot.
pub struct Case {
    pub test: Value,
    pub function: Symbol,
    pub refused: Option<String>,
}

/// The module of `system` that runs tests.
fn running() -> ModuleId {
    ModuleId::new(crate::system::PACKAGE, "test")
}

/// What `test.outcome` provides a test: what of its row is no world.
fn provided() -> [Symbol; 2] {
    [
        ModuleId::new(FAULT_PACKAGE, "test").ability("unmet"),
        ModuleId::new(FAULT_PACKAGE, "errors").ability("throw"),
    ]
}

/// Plain data as an expression that makes it.
fn written(value: &Value) -> Option<Expr<Reference>> {
    let all = |values: &[Value]| values.iter().map(written).collect::<Option<Vec<_>>>();
    Some(match value {
        Value::Null => Expr::Null,
        Value::Bool(value) => Expr::Literal(Literal::Bool(*value)),
        Value::Int(value) => Expr::Literal(Literal::Int(*value)),
        Value::Float(value) => Expr::Literal(Literal::Float(value.to_bits())),
        Value::String(text) => Expr::Literal(Literal::String(text.clone())),
        Value::Bytes(bytes) => Expr::Literal(Literal::Bytes(bytes.clone())),
        Value::Tag { package, tag } => Expr::Literal(Literal::Tag {
            package: package.clone(),
            tag: tag.clone(),
        }),
        Value::List(members) => Expr::List(all(members)?),
        Value::Tuple(members) => Expr::Tuple(all(members)?),
        Value::Record(fields) => record(
            fields
                .iter()
                .map(|(name, value)| Some((name.as_str(), written(value)?)))
                .collect::<Option<Vec<_>>>()?,
        ),
        _ => return None,
    })
}

/// A record of `fields`.
fn record(fields: Vec<(&str, Expr<Reference>)>) -> Expr<Reference> {
    let field = |(name, value): (&str, _)| (Key::Field(name.to_owned()), value);
    Expr::Record(fields.into_iter().map(field).collect())
}

/// A field of a record, or null.
fn field(value: &Value, name: &str) -> Value {
    match value {
        Value::Record(fields) => fields.get(name).cloned().unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

fn named(symbol: &Symbol) -> Expr<Reference> {
    Expr::Name(Reference::Definition(symbol.clone()))
}

impl Engine<'_> {
    /// Whether `function` can be run as a test under `target`: a
    /// function that takes nothing, has no type parameters, and whose
    /// world `target` binds, and everything it reaches checks.
    ///
    /// # Errors
    ///
    /// Fails with what is wrong with it.
    pub fn testable(&self, function: &Symbol, target: &Symbol) -> Result<(), Error> {
        let resolved = self.resolve(function)?;
        let header = self.function_of(function, &resolved.definition)?;
        let header = header.ok_or_else(|| Error::NotCompilable(function.clone()))?;
        if !header.type_params.is_empty() {
            return Err(Error::GenericEntry(function.clone()));
        }
        if !header.params.is_empty() {
            return Err(Error::TestTakes(function.clone()));
        }
        let world = self.world_of(function)?;
        let bound = self.bound(target, world.clone())?;
        if let Some(ability) = world
            .into_iter()
            .find(|ability| !bound.contains_key(ability))
        {
            return Err(Error::Unoffered {
                symbol: function.clone(),
                target: target.clone(),
                ability,
            });
        }
        // What the test reaches is compiled with it, so a part that
        // does not check would keep every other test from running.
        let mut checked = Ok(());
        self.walk(function, &mut |symbol, said| {
            if said.is_none() && checked.is_ok() && self.root_of(symbol).is_none() {
                checked = self.check(symbol).map(|_| ());
            }
        })?;
        checked
    }

    /// What of the world `function` reaches: its row, less what
    /// `test.outcome` provides.
    fn world_of(&self, function: &Symbol) -> Result<Vec<Symbol>, Error> {
        let resolved = self.resolve(function)?;
        let header = self.function_of(function, &resolved.definition)?;
        let header = header.ok_or_else(|| Error::NotCompilable(function.clone()))?;
        let provided = provided();
        let row = abilities(&header.row).into_iter();
        Ok(row.filter(|ability| !provided.contains(ability)).collect())
    }

    /// What of the world `runner` reaches, if it runs a package's tests
    /// under `target`: it takes `system.test.Cases` and a `Cache` and
    /// gives a `Tally`, `target` binds what its row names besides
    /// `report`, and it checks. Asked before the root is written, so
    /// what is wrong is said of the runner and not of the root.
    fn runnable(&self, runner: &Symbol, target: &Symbol) -> Result<Vec<Symbol>, Error> {
        let named = |name: &str| {
            crate::types::Ty::Named(Reference::Definition(running().ty(name)), Vec::new())
        };
        let signature = self.signature(runner)?.map(|(signature, _)| signature);
        let fits = signature.filter(|signature| {
            signature.type_params.is_empty()
                && signature.params == [named("Cases"), named("Cache")]
                && signature.ret == named("Tally")
        });
        let signature = fits.ok_or_else(|| Error::NotRunner(runner.clone()))?;
        let report = running().ability("report");
        let world: Vec<Symbol> = signature
            .row
            .into_iter()
            .filter(|ability| *ability != report)
            .collect();
        let bound = self.bound(target, world.clone())?;
        if let Some(ability) = world.iter().find(|ability| !bound.contains_key(*ability)) {
            return Err(Error::Unoffered {
                symbol: runner.clone(),
                target: target.clone(),
                ability: ability.clone(),
            });
        }
        self.check(runner)?;
        Ok(world)
    }

    /// The root of the program that runs `testing` under `target`:
    /// `system.test.main`, handed what the tests are as an expression
    /// the engine writes. What a test or a runner reaches of the world
    /// it captures where the root names it, so the target binds that
    /// too.
    ///
    /// # Errors
    ///
    /// Fails if a test is not a function, or as [`Self::runnable`] does
    /// of a runner.
    pub fn testing(&self, testing: &Testing, target: &Symbol) -> Result<Symbol, Error> {
        let mut world = BTreeSet::new();
        let mut packages = Vec::new();
        for tested in &testing.packages {
            world.extend(self.runnable(&tested.runner, target)?);
            let mut cases = Vec::new();
            for case in &tested.cases {
                cases.push(self.case(case, &tested.runner, target, &mut world)?);
            }
            packages.push(record(vec![
                ("id", Expr::Literal(Literal::String(tested.id.clone()))),
                ("runner", named(&tested.runner)),
                ("cases", Expr::List(cases)),
            ]));
        }
        let hashes = testing.harness.iter().cloned().map(Value::String).collect();
        let handed = record(vec![
            (
                "directory",
                Expr::Literal(Literal::String(testing.directory.clone())),
            ),
            (
                "harness",
                written(&Value::List(hashes)).unwrap_or(Expr::Null),
            ),
            ("packages", Expr::List(packages)),
        ]);
        let main = running().value("main");
        Ok(self.handing(&main, target, handed, world.into_iter().collect()))
    }

    /// One test as `system.test` takes it: a `Case`, or a `Refused`.
    fn case(
        &self,
        case: &Case,
        runner: &Symbol,
        target: &Symbol,
        world: &mut BTreeSet<Symbol>,
    ) -> Result<Expr<Reference>, Error> {
        let name = written(&field(&case.test, "name")).unwrap_or(Expr::Null);
        let group = written(&field(&case.test, "group")).unwrap_or(Expr::Null);
        if let Some(refused) = &case.refused {
            let refused = Expr::Literal(Literal::String(refused.clone()));
            return Ok(record(vec![
                ("name", name),
                ("group", group),
                ("refused", refused),
            ]));
        }
        let reaches = self.world_of(&case.function)?;
        let key = if self.computes(&reaches, target)? {
            Expr::Literal(Literal::String(self.key(&case.function, runner)?))
        } else {
            Expr::Null
        };
        world.extend(reaches);
        // What is said of the test is plain data, which a runner reads.
        let said = self
            .annotations_of(&case.function)?
            .into_iter()
            .filter_map(|said| {
                let value = written(&exchange::decode(&said.value).ok()?)?;
                let key = Expr::Literal(Literal::Tag {
                    package: said.package,
                    tag: said.tag,
                });
                Some(record(vec![("key", key), ("value", value)]))
            });
        Ok(record(vec![
            ("name", name),
            ("group", group),
            ("said", Expr::List(said.collect())),
            ("key", key),
            ("function", named(&case.function)),
        ]))
    }

    /// Whether what a test `reaches` of the world is only computed: the
    /// pure target binds each, and `target` binds it as that does. The
    /// test is then pure, whatever its row says, since what it reaches
    /// of native code is the host's own.
    fn computes(&self, reaches: &[Symbol], target: &Symbol) -> Result<bool, Error> {
        let pure = self.bound(&crate::system::pure(), reaches.to_vec())?;
        if reaches.iter().any(|ability| !pure.contains_key(ability)) {
            return Ok(false);
        }
        let under = self.bound(target, reaches.to_vec())?;
        let same = |(ability, (handler, _)): (&Symbol, &(Symbol, Vec<Symbol>))| {
            under
                .get(ability)
                .is_some_and(|(bound, _)| bound == handler)
        };
        Ok(pure.iter().all(same))
    }

    /// The key a pure test's outcome is kept under: its hash, what runs
    /// it, and the build. Its name is no part of it, and no native code
    /// is: native code is reached only by performing an ability, and a
    /// pure test reaches only the host's, which the build names.
    fn key(&self, test: &Symbol, runner: &Symbol) -> Result<String, Error> {
        let mut key = Hasher::new("test");
        key.hash(&self.hash(test)?).hash(&self.hash(runner)?);
        key.bytes(super::BUILD.as_bytes());
        Ok(key.finish().to_string())
    }

    /// The root that hands `entry` what `handed` makes under `target`,
    /// where the target also binds `world`.
    fn handing(
        &self,
        entry: &Symbol,
        target: &Symbol,
        handed: Expr<Reference>,
        world: Vec<Symbol>,
    ) -> Symbol {
        let written = digest("handed", &handed).to_string();
        self.rooted_handing(entry, target, handed, world, &written)
    }
}
