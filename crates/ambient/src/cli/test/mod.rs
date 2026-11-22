//! `ambient test`: the tests of a package, as `Engine::found` finds
//! them, handed to the program that runs them, `ambient.test.tests`
//! under `ambient.test.running`. What that program asks, the command
//! answers (`answers`). See `docs/spec/testing.md`.

mod answers;
mod said;

use std::path::Path;

use ambient_runtime::FAULT_PACKAGE;

use super::{Chosen, Launch, Session};
use crate::backend::native::Natives;
use crate::engine::{Found, asking};
use crate::error::Error;
use crate::symbol::ModuleId;
use crate::value::Value;
use answers::Answers;

impl Session<'_> {
    /// Runs the tests of the packages `chosen` is, says how each
    /// went, and gives the code to exit with: zero if none failed.
    /// With `only`, the tests are those whose name, after those of
    /// their groups, holds it.
    pub(super) fn test(&self, chosen: &Chosen, only: Option<&str>) -> Result<i32, Vec<String>> {
        let packages = self.each(chosen)?;
        let mut code = 0;
        for package in &packages {
            code = code.max(self.tested(package, only)?);
        }
        Ok(code)
    }

    /// Runs the tests of the one package at `path`.
    fn tested(&self, path: &Path, only: Option<&str>) -> Result<i32, Vec<String>> {
        let project = self.open(path)?;
        let engine = self.engine(&project);
        let launch = Launch {
            project: &project,
            engine: &engine,
        };
        let module = ModuleId::new(FAULT_PACKAGE, "test");
        let reported = |error: Error| vec![super::report(&project, &engine, &error)];
        let package = ModuleId::new(&project.root, "").package;
        let found = engine.found(&package).map_err(reported)?;
        let chosen = |test: &&Found| only.is_none_or(|only| test.name.join(".").contains(only));
        let tests = found.iter().filter(chosen).map(|test| test.test.clone());
        let answers = Answers::new(
            &launch,
            &self.world,
            module.value("outcome"),
            module.value("runner"),
        );
        // What the runner asks of the build, as what a test is
        // annotated with, the engine answers; the rest, the command.
        let natives = crate::host::computing(Natives::new())
            .and(asking())
            .asking(FAULT_PACKAGE, "test.run", "test.run")
            .asking(FAULT_PACKAGE, "test.kept", "test.kept")
            .asking(FAULT_PACKAGE, "test.report", "test.report");
        let (runner, running) = (engine.runner(&package), module.value("running"));
        let tests = [Value::List(tests.collect())];
        engine
            .run_once(&runner, &tests, &running, &natives, &answers)
            .map_err(reported)?;
        Ok(i32::from(answers.failed()))
    }
}
