//! Run: a function and its arguments yield a value, and a record of what
//! the run observed. Comptime calls and tests are both this. See
//! `docs/spec/comptime.md` and `docs/spec/memoization.md`.

use std::cell::OnceCell;

use serde::{Deserialize, Serialize};

use super::asked::{self, Again};
use super::{Engine, Stage, Taking};
use crate::backend::native::{Native, Natives, Observation, Program, Questions, Running};
use crate::checker::Evaluate;
use crate::error::Error;
use crate::hash::{Hash, Hasher, digest, encode as bytes};
use crate::symbol::{PackageId, Symbol};
use crate::value::Value;

/// The backend that runs code during a build, made on first use.
#[derive(Default)]
pub(super) struct Runner(OnceCell<Result<Native, String>>);

/// The world comptime calls run in: a target, and the native code of the
/// machine doing the build.
pub(super) struct Build {
    target: Symbol,
    /// The native code for the calls written in a package: what is
    /// read is read from where that package is.
    natives: Box<dyn Fn(&PackageId) -> Natives>,
}

/// A run, kept with everything it observed.
#[derive(Serialize, Deserialize)]
struct Run {
    outcome: Result<Value, Error>,
    /// Every call of native code the run made. Native code is the only
    /// thing a run reaches that its key does not cover.
    trace: Vec<Observation>,
}

/// A run that a checked definition's content came from.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Dependency {
    /// The package the call was written in.
    from: PackageId,
    function: Symbol,
    args: Vec<Value>,
    /// A digest of what the run gave.
    outcome: Hash,
}

/// Whether everything in a trace, repeated now, gives what it gave then.
/// A call that cannot be repeated never holds.
fn holds(trace: &[Observation], natives: &Natives, running: &mut dyn Running) -> bool {
    trace.iter().all(|seen| {
        seen.verifiable
            && natives.get(&seen.native).is_some_and(|offered| {
                offered.verifiable && (offered.run)(running, &seen.args) == seen.result
            })
    })
}

impl Engine<'_> {
    /// Runs comptime calls that take abilities under `target`, with the
    /// native code in `natives`.
    #[must_use]
    pub fn with_build(self, target: Symbol, natives: Natives) -> Self {
        self.with_build_for(target, move |_| natives.clone())
    }

    /// Runs comptime calls that take abilities under `target`, with the
    /// native code `natives` gives for the package a call is written
    /// in.
    #[must_use]
    pub fn with_build_for(
        mut self,
        target: Symbol,
        natives: impl Fn(&PackageId) -> Natives + 'static,
    ) -> Self {
        let natives = Box::new(natives);
        self.build = Some(Build { target, natives });
        self
    }

    /// Calls `function` with `args` during the build, for a call
    /// written in the package `from`. If it takes abilities, it runs
    /// under the build target.
    ///
    /// # Errors
    ///
    /// Fails if the function does not compile for this machine, faults, or
    /// takes abilities when the build has no target.
    pub fn run(&self, from: &PackageId, function: &Symbol, args: &[Value]) -> Result<Value, Error> {
        if !self.takes_abilities(function)? {
            let natives = crate::host::computing(Natives::new());
            return self.memoized(function, function, args, &natives, "");
        }
        let Some(build) = &self.build else {
            return Err(Error::ComptimeAbilities(function.clone()));
        };
        let root = self.program(function, &build.target)?;
        // What a run reads depends on where its package is, so the
        // runs of two packages are kept apart.
        // The engine answers what a run asks of the build itself.
        let natives = crate::host::computing((build.natives)(from)).and(asked::natives());
        self.memoized(function, &root, args, &natives, &from.0)
    }

    /// Calls `function` with `args` under `target`, with the native code
    /// in `natives`.
    ///
    /// The run is kept under the hash of the program and the arguments,
    /// with what it observed. It is made again only if something it
    /// observed, looked at again, has changed, or cannot be looked at
    /// again.
    ///
    /// # Errors
    ///
    /// Fails if the program does not compile for this machine, or faults.
    pub fn run_under(
        &self,
        function: &Symbol,
        args: &[Value],
        target: &Symbol,
        natives: &Natives,
    ) -> Result<Value, Error> {
        let root = self.program(function, target)?;
        self.memoized(function, &root, args, natives, "")
    }

    /// Hands `entry` to `within` under `target`, with the native code
    /// in `natives`, and gives what `within` gave: see
    /// [`Self::program_within`]. It always runs, and is kept as
    /// [`Self::run_under`] keeps a run, in place of what was; `entry`'s
    /// name is no part of its key.
    ///
    /// # Errors
    ///
    /// Fails if the program does not compile for this machine, or faults
    /// past what `within` contains.
    pub fn ran_within(
        &self,
        entry: &Symbol,
        within: &Symbol,
        target: &Symbol,
        natives: &Natives,
    ) -> Result<Value, Error> {
        let root = self.program_within(entry, within, target)?;
        let run = self.memoized_taking(entry, &root, &[], natives, "", Taking::Made)?;
        run.unwrap_or_else(|| unreachable!("a run that is made is made"))
    }

    /// Calls `function` with `args` under `target`, with the native code
    /// in `natives`, and gives what it gave. It is kept nowhere, and
    /// what it asks is answered by `questions`: it is what runs tests,
    /// which asks to run each.
    ///
    /// # Errors
    ///
    /// Fails if the program does not compile for this machine, or faults.
    pub fn run_once(
        &self,
        function: &Symbol,
        args: &[Value],
        target: &Symbol,
        natives: &Natives,
        questions: &dyn Questions,
    ) -> Result<Value, Error> {
        let root = self.program(function, target)?;
        let failed = |message: String| Error::Backend {
            symbol: function.clone(),
            message,
        };
        let native = match self.runner.0.get_or_init(Native::new) {
            Ok(native) => native,
            Err(message) => return Err(failed(message.clone())),
        };
        let parts = self.link(&root, native)?;
        let program = Program::load(&parts, natives).map_err(failed)?;
        program
            .run_asking(args, questions)
            .map_err(|fault| Error::ComptimeFault {
                function: function.clone(),
                fault: match program.failure() {
                    Some(message) => format!("{fault:?}: {message}"),
                    None => format!("{fault:?}"),
                },
            })
    }

    /// What [`Self::ran_within`] gave when it last ran, if it is kept
    /// and what it observed still holds: nothing runs. What it gave is
    /// given whether it passed or not.
    ///
    /// # Errors
    ///
    /// Fails if the program does not compile, or as what is kept does.
    pub fn kept_within(
        &self,
        entry: &Symbol,
        within: &Symbol,
        target: &Symbol,
        natives: &Natives,
    ) -> Result<Option<Value>, Error> {
        let root = self.program_within(entry, within, target)?;
        let run = self.memoized_taking(entry, &root, &[], natives, "", Taking::Stored)?;
        run.transpose()
    }

    fn takes_abilities(&self, function: &Symbol) -> Result<bool, Error> {
        let signature = self.signature(function)?;
        Ok(signature.is_some_and(|(signature, _)| !signature.row.is_empty()))
    }

    /// Runs `entry`, which stands for `function`, or takes the run from
    /// the store if it still holds. A run for the package `from` is
    /// kept apart from one for another.
    fn memoized(
        &self,
        function: &Symbol,
        entry: &Symbol,
        args: &[Value],
        natives: &Natives,
        from: &str,
    ) -> Result<Value, Error> {
        let run = self.memoized_taking(function, entry, args, natives, from, Taking::Either)?;
        run.unwrap_or_else(|| unreachable!("a run that may be made is made"))
    }

    /// Runs `entry` as [`Self::memoized`] does, taking what `taking`
    /// says: nothing is what [`Taking::Stored`] gives where no stored
    /// run holds.
    fn memoized_taking(
        &self,
        function: &Symbol,
        entry: &Symbol,
        args: &[Value],
        natives: &Natives,
        from: &str,
        taking: Taking,
    ) -> Result<Option<Result<Value, Error>>, Error> {
        let mut key = Hasher::new("run");
        key.hash(&self.hash(entry)?);
        key.bytes(&bytes(&args)).bytes(from.as_bytes());
        let failed = |message: String| Error::Backend {
            symbol: function.clone(),
            message,
        };
        let native = match self.runner.0.get_or_init(Native::new) {
            Ok(native) => native,
            Err(message) => return Err(failed(message.clone())),
        };
        let still = |stored: &Result<Run, Error>| {
            let held = |run: &Run| holds(&run.trace, natives, &mut Again(self));
            stored.as_ref().is_ok_and(held)
        };
        let run = self.staged(Stage::Run, function, key.finish(), taking, still, || {
            // Everything the program is made of is covered by the
            // entry's hash, so linking here reads nothing the key does
            // not cover. A run that is reused links nothing.
            let parts = self.link(entry, native)?;
            let program = Program::load(&parts, natives).map_err(failed)?;
            program.observe();
            let ran = program.run_asking(args, self);
            let outcome = ran.map_err(|fault| Error::ComptimeFault {
                function: function.clone(),
                fault: match program.failure() {
                    Some(message) => format!("{fault:?}: {message}"),
                    None => format!("{fault:?}"),
                },
            });
            Ok(Run {
                outcome,
                trace: program.observations(),
            })
        });
        match run {
            Some(run) => Ok(Some(run?.outcome.clone())),
            None => Ok(None),
        }
    }

    /// Whether each run a definition's content came from still gives what
    /// it gave.
    pub(super) fn still_gives(&self, runs: &[Dependency]) -> bool {
        runs.iter().all(|run| {
            let outcome = self.run(&run.from, &run.function, &run.args);
            digest("outcome", &outcome) == run.outcome
        })
    }

    /// Starts keeping the runs made on behalf of one definition.
    pub(super) fn begin_runs(&self) {
        self.observed.borrow_mut().push(Vec::new());
    }

    /// The runs made since the matching [`Self::begin_runs`].
    pub(super) fn end_runs(&self) -> Vec<Dependency> {
        self.observed.borrow_mut().pop().unwrap_or_default()
    }
}

impl Evaluate for Engine<'_> {
    fn evaluate(
        &self,
        from: &PackageId,
        function: &Symbol,
        args: &[Value],
    ) -> Result<Value, Error> {
        let outcome = self.run(from, function, args);
        // A pure function's result follows from its hash, which the key
        // of whatever is being checked already covers. Any other is kept,
        // to be looked at again.
        if self.takes_abilities(function).unwrap_or(false)
            && let Some(runs) = self.observed.borrow_mut().last_mut()
        {
            runs.push(Dependency {
                from: from.clone(),
                function: function.clone(),
                args: args.to_vec(),
                outcome: digest("outcome", &outcome),
            });
        }
        outcome
    }

    fn constant(&self, symbol: &Symbol) -> Result<Option<Value>, Error> {
        let open = self.check(symbol)?;
        let crate::core::Tree::Constant { value, .. } = &open.tree else {
            return Ok(None);
        };
        // A constant it names that does not check is no data here, and
        // is reported where it is checked for its own sake.
        let held = |slot: crate::core::Slot| {
            let named = open.slots.get(slot.0)?;
            self.constant(named).ok().flatten()
        };
        Ok(value.data(&held))
    }

    fn granted(&self, package: &PackageId, ability: &Symbol) -> Result<bool, Error> {
        self.holds(package, ability)
    }

    fn nameable(&self, package: &PackageId, symbol: &Symbol) -> bool {
        Engine::nameable(self, package, symbol)
    }
}
