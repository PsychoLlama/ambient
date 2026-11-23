//! `ambient test`: the tests of a package, as `Engine::found` finds
//! them, compiled into one program the engine writes
//! (`Engine::testing`) and run as `ambient run` runs a program: in this
//! process, or linked with the native bundles it reaches. What runs,
//! keeps, and reports them is `system.test`. See `docs/spec/testing.md`.

use std::path::{Path, PathBuf};

use ambient_host::launch::{code, started};
use ambient_runtime::FAULT_PACKAGE;

use super::picked::BUILT;
use super::{Chosen, Launch, Session, trace};
use crate::engine::{Case, Tested, Testing};
use crate::error::Error;
use crate::host;
use crate::project::Update;
use crate::symbol::{ModuleId, PackageId};
use crate::system;
use crate::value::Value;
use crate::workspace::Sources;

/// Where under a build directory what testing keeps is kept.
const KEPT: &str = "tests";

impl Session<'_> {
    /// Runs the tests of the packages `chosen` is, as one program, says
    /// how each went, and gives the code to exit with: zero if none
    /// failed. With `only`, the tests are those whose name, after those
    /// of their groups, holds it.
    pub(super) fn test(&self, chosen: &Chosen, only: Option<&str>) -> Result<i32, Vec<String>> {
        let paths = self.each(chosen)?;
        let (project, names) = self.open_members(&paths, &Update::Nothing)?;
        let engine = self.engine(&project);
        let launch = Launch {
            project: &project,
            engine: &engine,
        };
        let reported = |error: Error| vec![super::report(&project, &engine, &error)];
        let target = system::target();
        let mut packages = Vec::new();
        for name in names {
            let package = PackageId(name);
            // A test that cannot be run is said to fail, and runs no
            // other the worse for it.
            let found = engine.found(&package).map_err(reported)?;
            let cases = found.into_iter().map(|found| {
                let refused = engine.testable(&found.function, &target).err();
                Case {
                    test: found.test,
                    function: found.function,
                    refused: refused.map(|error| super::report(&project, &engine, &error)),
                }
            });
            packages.push(Tested {
                id: project.workspace.identity(&package),
                runner: engine.runner(&package),
                cases: cases.collect(),
            });
        }
        let above = project.above.as_deref();
        let built = above
            .or(paths.first().map(PathBuf::as_path))
            .unwrap_or(Path::new(""));
        let outcome = ModuleId::new(FAULT_PACKAGE, "test").value("outcome");
        let testing = Testing {
            directory: built.join(BUILT).join(KEPT).to_string_lossy().into_owned(),
            harness: trace::reach(&engine, &outcome).into_iter().collect(),
            packages,
        };
        let root = engine.testing(&testing, &target).map_err(reported)?;
        let main = ModuleId::new(system::PACKAGE, "test").value("main");
        let linked = launch.link_root(&main, root)?;
        // Which tests run is the program's argument, so choosing
        // compiles nothing.
        let args: Vec<String> = only.map(str::to_owned).into_iter().collect();
        let link = self
            .linking(&project, &linked.parts)
            .map_err(|problem| vec![problem])?;
        if !link.bundles.is_empty() {
            let executable = self.executable(&launch, &linked, &link)?;
            return self
                .linker
                .run(&executable, &args)
                .map_err(|problem| vec![problem]);
        }
        let told = Value::List(args.into_iter().map(Value::String).collect());
        let natives = host::natives(&self.world, linked.trees.clone()).with(
            system::PACKAGE,
            "env.args",
            move |_| Ok(told.clone()),
        );
        let gave = started(&linked.parts, &natives, || launch.frames(&linked.root))?;
        Ok(code(&gave))
    }
}
