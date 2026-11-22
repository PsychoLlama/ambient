//! `ambient test`: the tests of a package, as `Engine::found` finds
//! them, compiled into one program the engine writes
//! (`Engine::testing`) and run as `ambient run` runs a program: in this
//! process, or linked with the native bundles it reaches. What runs,
//! keeps, and reports them is `system.test`. See `docs/spec/testing.md`.

use std::collections::BTreeMap;
use std::path::Path;

use ambient_host::launch::{code, started};
use ambient_runtime::FAULT_PACKAGE;

use super::{Chosen, Launch, Session, trace};
use crate::engine::{Case, Tested, Testing};
use crate::error::Error;
use crate::host;
use crate::symbol::ModuleId;
use crate::system;
use crate::value::Value;
use crate::workspace::Sources;

/// Where under a build directory what testing keeps is kept.
const KEPT: &str = "tests";

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
        let reported = |error: Error| vec![super::report(&project, &engine, &error)];
        let package = ModuleId::new(&project.root, "").package;
        let target = system::target();
        // A test that cannot be run is said to fail, and runs no other
        // the worse for it.
        let cases = engine
            .found(&package)
            .map_err(reported)?
            .into_iter()
            .map(|found| {
                let refused = engine.testable(&found.function, &target).err();
                Case {
                    test: found.test,
                    function: found.function,
                    refused: refused.map(|error| super::report(&project, &engine, &error)),
                }
            });
        let built = project
            .above
            .as_deref()
            .unwrap_or(path)
            .join(super::picked::BUILT);
        let outcome = ModuleId::new(FAULT_PACKAGE, "test").value("outcome");
        let testing = Testing {
            directory: built.join(KEPT).to_string_lossy().into_owned(),
            harness: trace::reach(&engine, &outcome).into_iter().collect(),
            packages: vec![Tested {
                id: project.workspace.identity(&package),
                runner: engine.runner(&package),
                cases: cases.collect(),
            }],
            bundles: self.bundles(&project).map_err(|problem| vec![problem])?,
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

    /// The digest of each native bundle of `project` for this machine,
    /// by the ID of its package: what stands for the native code a test
    /// reaches in the key its outcome is kept under.
    fn bundles(
        &self,
        project: &crate::project::Project,
    ) -> Result<BTreeMap<String, String>, String> {
        let mut digests = BTreeMap::new();
        for (package, bundles) in &project.natives {
            let Some(bundle) = bundles.get(ambient_host::native::MACHINE) else {
                continue;
            };
            let identity = project
                .workspace
                .identity(&crate::symbol::PackageId(package.clone()));
            let digest = match (&bundle.digest, &bundle.path) {
                (Some(digest), _) => digest.clone(),
                // What a member keeps in its directory is what it is.
                (None, Some(place)) => {
                    let place = project.packages[package].directory.join(place);
                    let bytes = self.files.bytes(&place).map_err(|error| {
                        format!("the native bundle of package `{package}`: {error}")
                    })?;
                    crate::hash::Hasher::new("bundle")
                        .bytes(&bytes)
                        .finish()
                        .to_string()
                }
                (None, None) => continue,
            };
            digests.insert(identity, digest);
        }
        Ok(digests)
    }
}
