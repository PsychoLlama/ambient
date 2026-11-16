//! `ambient test`: the tests of a package, each run as a program of its
//! own. See "Testing" in `spec/packages.md`.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use ambient_runtime::FAULT_PACKAGE;

use super::{Launch, Session};
use crate::backend::native::Natives;
use crate::engine::Engine;
use crate::error::Error;
use crate::project::Project;
use crate::surface::DefinitionKind;
use crate::symbol::{ModuleId, Symbol};
use crate::value::Value;
use crate::workspace::Sources;

/// What begins the name of a test.
const PREFIX: &str = "test_";

/// What a test that passed is known by: see [`Engine::known`].
const PASSED: &str = "test passed";

impl Session<'_> {
    /// Runs the tests of the package at `path`, says how each went, and
    /// gives the code to exit with: zero if none failed. With `only`,
    /// the tests are those whose module path and name hold it.
    pub(super) fn test(&self, path: &Path, only: Option<&str>) -> Result<i32, Vec<String>> {
        let project = self.open(path)?;
        let engine = self.engine(&project);
        let launch = Launch {
            project: &project,
            engine: &engine,
        };
        let target = ModuleId::new(FAULT_PACKAGE, "test").value("tests");
        let (mut passed, mut failed, mut kept) = (0, 0, 0);
        let chosen = |test: &Symbol| only.is_none_or(|only| test.identity().contains(only));
        let tests = tests(&project, &engine)
            .map_err(|error| vec![project.report(&error, engine.blame(&error))])?;
        for test in tests.into_iter().filter(chosen) {
            // A test reaches no world, so one that passed passes for as
            // long as its program is the same, and is not run again. One
            // that failed is: what it says names places, which no hash
            // covers.
            let program = engine.program(&test, &target);
            let key = program.and_then(|root| engine.known(PASSED, &root)).ok();
            if key.is_some_and(|key| self.store.get(&key).is_some()) {
                (passed, kept) = (passed + 1, kept + 1);
                self.world.borrow_mut().write(&format!("ok    {test}\n"));
                continue;
            }
            let unmet = Rc::new(RefCell::new(Vec::new()));
            let ended = launch.run(&test, &target, &checks(&unmet));
            let mut problems = unmet.take();
            problems.extend(ended.err().into_iter().flatten());
            let mut world = self.world.borrow_mut();
            if problems.is_empty() {
                passed += 1;
                world.write(&format!("ok    {test}\n"));
                if let Some(key) = key {
                    self.store.put(key, Vec::new());
                }
            } else {
                failed += 1;
                world.write(&format!("FAIL  {test}\n"));
                for line in problems.iter().flat_map(|problem| problem.lines()) {
                    world.write(&format!("      {line}\n"));
                }
            }
        }
        let again = match kept {
            0 => String::new(),
            kept => format!(" ({kept} not run again)"),
        };
        let summary = format!("{passed} passed{again}, {failed} failed\n");
        self.world.borrow_mut().write(&summary);
        Ok(i32::from(failed > 0))
    }
}

/// Whether a definition of this name and kind is a test.
fn test<N>(name: &str, kind: &DefinitionKind<N>) -> bool {
    let takes_nothing =
        matches!(kind, DefinitionKind::Function(function) if function.params.is_empty());
    takes_nothing && name.starts_with(PREFIX)
}

/// The tests of the package a project was opened at, in the order of
/// their modules and of their definitions there: each function that
/// takes nothing and is named as a test. Those a generated module was
/// given follow what the module was written with.
fn tests(project: &Project, engine: &Engine) -> Result<Vec<Symbol>, Error> {
    let root = ModuleId::new(&project.root, "").package;
    let mut tests = Vec::new();
    for read in project
        .modules
        .iter()
        .filter(|read| read.module.package == root)
    {
        let Some(module) = project.workspace.module(&read.module) else {
            continue;
        };
        let written = module.definitions().iter();
        let written = written.filter(|definition| test(&definition.name, &definition.kind));
        tests.extend(written.map(|definition| read.module.value(&definition.name)));
        for made in engine.generated_in(&read.module)? {
            let resolved = engine.resolve(&made)?;
            if test(&made.name, &resolved.definition.kind) {
                tests.push(made);
            }
        }
    }
    Ok(tests)
}

/// The native code a test's target binds: a check that does not hold
/// adds what it was for to `unmet`, and the test goes on.
fn checks(unmet: &Rc<RefCell<Vec<String>>>) -> Natives {
    let unmet = Rc::clone(unmet);
    Natives::new().with(FAULT_PACKAGE, "test.checked", move |args| {
        match args {
            [Value::Bool(true), _] => {}
            [Value::Bool(false), Value::String(what)] => {
                unmet.borrow_mut().push(format!("does not hold: {what}"));
            }
            _ => return Err("a check takes whether it holds and what it is".to_owned()),
        }
        Ok(Value::Null)
    })
}
