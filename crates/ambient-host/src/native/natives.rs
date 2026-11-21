//! The native code a host offers a program, and what a run observes
//! of each call of it. See "Linking" in `docs/spec/externs.md`.

use std::collections::HashMap;
use std::rc::Rc;

use serde::{Deserialize, Serialize};

use super::bundle::{Bundled, PLACED};
use super::host::Running;
use super::waits::Answers;
use crate::parts::Native;
use crate::value::Value;

/// Native code for one extern: the program that called it and its
/// arguments in, a result or what went wrong out.
pub type NativeFn = Rc<dyn Fn(&mut dyn Running, &[Value]) -> Result<Value, String>>;

/// Native code as a host offers it.
#[derive(Clone)]
pub struct Offered {
    pub run: NativeFn,
    /// Whether calling it again is cheap, has no side effects, and shows
    /// whether what it returned before still holds. See
    /// `docs/spec/memoization.md`.
    pub verifiable: bool,
    /// The function of a native bundle this is, which is called in
    /// place of `run` wherever values can be read where they lie.
    pub bundled: Option<Bundled>,
}

/// The native code a host offers, by the package that declares each
/// extern and the extern's tag.
#[derive(Clone, Default)]
pub struct Natives {
    offered: HashMap<(String, String), Offered>,
    /// What brings the answers of native code that answers later.
    waits: Option<Answers>,
}

impl Natives {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn offer(
        mut self,
        package: &str,
        path: &str,
        verifiable: bool,
        native: impl Fn(&mut dyn Running, &[Value]) -> Result<Value, String> + 'static,
    ) -> Self {
        let run: NativeFn = Rc::new(native);
        let key = (package.to_owned(), path.to_owned());
        let offered = Offered {
            run,
            verifiable,
            bundled: None,
        };
        self.offered.insert(key, offered);
        self
    }

    /// Adds native code for the extern of `package` whose module path
    /// and name are `path`, as `util.math.double`. It is volatile: a run that called it is never reused.
    #[must_use]
    pub fn with(
        self,
        package: &str,
        path: &str,
        native: impl Fn(&[Value]) -> Result<Value, String> + 'static,
    ) -> Self {
        self.offer(package, path, false, move |_, args| native(args))
    }

    /// Adds native code that is handed the program that called it, as
    /// well as its arguments. It is volatile, as [`Natives::with`] is.
    #[must_use]
    pub fn reaching(
        self,
        package: &str,
        path: &str,
        native: impl Fn(&mut dyn Running, &[Value]) -> Result<Value, String> + 'static,
    ) -> Self {
        self.offer(package, path, false, native)
    }

    /// Adds native code that can be called again, cheaply and without
    /// side effects, to see whether what it returned still holds.
    #[must_use]
    pub fn verifiable(
        self,
        package: &str,
        path: &str,
        native: impl Fn(&[Value]) -> Result<Value, String> + 'static,
    ) -> Self {
        self.offer(package, path, true, move |_, args| native(args))
    }

    /// Adds native code that passes `question` on to whatever is
    /// running the program: see [`Running::ask`]. It is verifiable,
    /// since what answers can be asked again.
    #[must_use]
    pub fn asking(self, package: &str, path: &str, question: &'static str) -> Self {
        self.offer(package, path, true, move |running, args| {
            running.ask(question, args)
        })
    }

    /// Adds a function of a native bundle for the extern of `package`
    /// whose module path and name are `path`. It is volatile, as
    /// [`Natives::with`] is.
    ///
    /// # Safety
    ///
    /// `function` keeps to what [`Bundled`] says of one: it is called
    /// with nothing checked.
    #[must_use]
    pub unsafe fn bundled(mut self, package: &str, path: &str, function: Bundled) -> Self {
        let offered = Offered {
            run: Rc::new(|_, _| Err(PLACED.to_owned())),
            verifiable: false,
            bundled: Some(function),
        };
        let key = (package.to_owned(), path.to_owned());
        self.offered.insert(key, offered);
        self
    }

    /// Everything offered here and in `other`, whose native code is
    /// taken where both offer some for one extern.
    #[must_use]
    pub fn and(mut self, other: Self) -> Self {
        self.offered.extend(other.offered);
        self.waits = other.waits.or(self.waits);
        self
    }

    /// Says what brings the answers of native code that answers later.
    #[must_use]
    pub fn answered_by(mut self, waits: Answers) -> Self {
        self.waits = Some(waits);
        self
    }

    /// What brings the answers, if anything does.
    #[must_use]
    pub fn waits(&self) -> Option<Answers> {
        self.waits.clone()
    }

    /// Each extern native code is offered for: its package, and its
    /// module path and name.
    pub fn named(&self) -> impl Iterator<Item = (&str, &str)> {
        let offered = self.offered.keys();
        offered.map(|(package, path)| (package.as_str(), path.as_str()))
    }

    /// The native code offered for one extern.
    #[must_use]
    pub fn get(&self, native: &Native) -> Option<&Offered> {
        self.offered
            .get(&(native.package.clone(), native.path.clone()))
    }

    /// The native code for each of `externs`, in their order.
    ///
    /// # Errors
    ///
    /// Fails if one is missing: the program cannot be linked.
    pub fn resolve(&self, externs: &[Native]) -> Result<Vec<Offered>, String> {
        externs
            .iter()
            .map(|native| {
                self.get(native).cloned().ok_or_else(|| {
                    format!(
                        "no native code for `{}` of package `{}`",
                        native.path, native.package
                    )
                })
            })
            .collect()
    }
}

/// One call of native code: what a run observed of the world.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    pub native: Native,
    pub args: Vec<Value>,
    pub result: Result<Value, String>,
    pub verifiable: bool,
}
