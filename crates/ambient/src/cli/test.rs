//! `ambient test`: the tests of a package, each run as a program of its
//! own. See "Testing" in `spec/packages.md`.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use ambient_runtime::FAULT_PACKAGE;

use super::{Launch, Session};
use crate::backend::native::Natives;
use crate::engine::Engine;
use crate::project::Project;
use crate::surface::DefinitionKind;
use crate::symbol::{ModuleId, Symbol};
use crate::value::Value;
use crate::workspace::Sources;

/// What begins the name of a test.
const PREFIX: &str = "test_";

impl Session<'_> {
    /// Runs the tests of the package at `path`, says how each went, and
    /// gives the code to exit with: zero if none failed. With `only`,
    /// the tests are those whose module path and name hold it.
    pub(super) fn test(&self, path: &Path, only: Option<&str>) -> Result<i32, Vec<String>> {
        let project = self.open(path)?;
        let engine = Engine::new(&project.workspace, self.store);
        let launch = Launch {
            project: &project,
            engine: &engine,
        };
        let target = ModuleId::new(FAULT_PACKAGE, "system").value("tests");
        let (mut passed, mut failed) = (0, 0);
        let chosen = |test: &Symbol| only.is_none_or(|only| test.identity().contains(only));
        for test in tests(&project).into_iter().filter(chosen) {
            let unmet = Rc::new(RefCell::new(Vec::new()));
            let ended = launch.run(&test, &target, &checks(&unmet));
            let mut problems = unmet.take();
            problems.extend(ended.err().into_iter().flatten());
            let mut world = self.world.borrow_mut();
            if problems.is_empty() {
                passed += 1;
                world.write(&format!("ok    {test}\n"));
            } else {
                failed += 1;
                world.write(&format!("FAIL  {test}\n"));
                for line in problems.iter().flat_map(|problem| problem.lines()) {
                    world.write(&format!("      {line}\n"));
                }
            }
        }
        let summary = format!("{passed} passed, {failed} failed\n");
        self.world.borrow_mut().write(&summary);
        Ok(i32::from(failed > 0))
    }
}

/// The tests of the package a project was opened at, in the order of
/// their modules and of their definitions there: each function that
/// takes nothing and is named as a test.
fn tests(project: &Project) -> Vec<Symbol> {
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
        for definition in module.definitions() {
            if let DefinitionKind::Function(function) = &definition.kind
                && function.params.is_empty()
                && definition.name.starts_with(PREFIX)
            {
                tests.push(read.module.value(&definition.name));
            }
        }
    }
    tests
}

/// The native code a test's target binds: a check that does not hold
/// adds what it was for to `unmet`, and the test goes on.
fn checks(unmet: &Rc<RefCell<Vec<String>>>) -> Natives {
    let unmet = Rc::clone(unmet);
    Natives::new().with(FAULT_PACKAGE, "system.test_check", move |args| {
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
