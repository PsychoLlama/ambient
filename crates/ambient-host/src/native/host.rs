//! The host's side of a native program: making and reading values, and
//! running native code for its externs.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

use ambient_runtime::{Context, Fault, Word};
use serde::{Deserialize, Serialize};

use super::load::Program;
use super::values::{Host, Inside};
use super::waits::{Answers, asked};
use crate::parts::{Native, Parts};
use crate::value::{self, Handed, Live, Value};

/// The program that called native code, as far as native code reaches
/// it. See "Native side" in `docs/spec/externs.md`.
pub trait Running {
    /// Loads a function into the program and gives it as a value: one
    /// `Engine::link_to_load` made for the program's
    /// backend, with the native code for its externs. Its code stays
    /// while a value refers to it.
    ///
    /// # Errors
    ///
    /// Fails if the function cannot be loaded, or no program is
    /// running.
    fn load(&mut self, parts: &Parts, natives: &Natives) -> Result<Value, String>;

    /// The name of the backend the program was compiled by
    /// (`Backend::name`): what code is to be compiled by, so that it
    /// can be loaded, and what compiled parts are checked against.
    ///
    /// # Errors
    ///
    /// Fails if no program is running.
    fn backend(&self) -> Result<&str, String>;

    /// Says this call will answer later, and gives the ticket the
    /// answer is to come by: see [`super::Waits`]. The task that made
    /// the call waits, and what the call returns is not looked at.
    ///
    /// # Errors
    ///
    /// Fails if no program is running, or the extern was not declared
    /// `suspending`.
    fn later(&mut self) -> Result<u64, String>;

    /// Calls a function value of the program with `args` and nothing
    /// bound, and gives what it gives. The call is the task's that
    /// called this native code: see "Calling back" in
    /// `docs/spec/externs.md`.
    ///
    /// # Errors
    ///
    /// Fails if `function` is not a function among the arguments of
    /// this call that takes as many arguments: a copy of one holds
    /// nothing the function captured, and is not called. Fails if no
    /// program is running. Fails with
    /// [`UNWOUND`] if the call unwound: the native code is then over,
    /// and whatever it returns is not looked at.
    fn call(&mut self, function: &Value, args: &[Value]) -> Result<Value, String>;

    /// Performs an ability of the row the extern declared, which is
    /// named as an extern is: by its package and its module path and
    /// name. The handler bound where the extern was called answers,
    /// as it would a perform written there.
    ///
    /// # Errors
    ///
    /// Fails with [`UNDECLARED_ABILITY`] if the extern's row has no
    /// such ability, and otherwise as [`Self::call`] does.
    fn perform(&mut self, package: &str, path: &str, args: &[Value]) -> Result<Value, String>;

    /// Asks whatever is running the program a question of its own,
    /// by name: what native code that only passes a question on does.
    /// See [`Questions`].
    ///
    /// # Errors
    ///
    /// Fails if nothing that answers is running the program, or with
    /// what the answer failed with.
    fn ask(&mut self, _question: &str, _args: &[Value]) -> Result<Value, String> {
        Err(UNASKED.to_owned())
    }
}

/// What asking says where nothing answers: only a build does.
pub const UNASKED: &str = "nothing that is running the program answers questions";

/// What answers the questions a program asks of what runs it, each by
/// name. A build is one: a program it runs asks it what a definition
/// reaches or compiles to, which only the build knows. See "Build
/// abilities" in `docs/spec/tools.md`.
pub trait Questions {
    /// The answer to `question`, asked with `args`.
    ///
    /// # Errors
    ///
    /// Fails if there is no such question, or it has no answer.
    fn answer(&self, question: &str, args: &[Value]) -> Result<Value, String>;
}

/// What a perform says of an ability the extern's row does not hold.
pub const UNDECLARED_ABILITY: &str = "the extern does not declare the ability in its row";

/// Where an extern that declared `row` finds the handler of an ability
/// among those it was handed.
#[must_use]
pub fn bound(row: &[(String, String)], package: &str, path: &str) -> Option<u64> {
    let named = |(of, at): &(String, String)| of == package && at == path;
    Some(row.iter().position(named)? as u64)
}

/// What a call back that unwound says: a fault, an abort, or a
/// cancellation is on its way through the native code that made it.
pub const UNWOUND: &str = "the call back unwound";

/// What a call back says once native code has said it will answer
/// later: its task is waiting, and calls nothing.
pub const ANSWERING: &str = "native code that answers later has nothing more to call";

/// What a call back says of a value that cannot be called so.
pub const UNCALLABLE: &str = "not a function this call was handed that takes as many arguments";

/// What asking to answer later says of an extern that may not.
pub const UNDECLARED: &str = "the extern is not declared `suspending`, or has asked already";

/// No program: what native code is handed when it is called again only
/// to see whether what it returned still holds.
pub struct Nowhere;

impl Running for Nowhere {
    fn load(&mut self, _: &Parts, _: &Natives) -> Result<Value, String> {
        Err("no program is running".to_owned())
    }

    fn backend(&self) -> Result<&str, String> {
        Err("no program is running".to_owned())
    }

    fn later(&mut self) -> Result<u64, String> {
        Err("no program is running".to_owned())
    }

    fn call(&mut self, _: &Value, _: &[Value]) -> Result<Value, String> {
        Err("no program is running".to_owned())
    }

    fn perform(&mut self, _: &str, _: &str, _: &[Value]) -> Result<Value, String> {
        Err("no program is running".to_owned())
    }
}

/// One call of native code: the program it was made from, and the
/// functions it loaded, each held until what it returns is made.
struct Call<'a> {
    program: &'a Program,
    context: *mut Context,
    held: Vec<Word>,
    /// The ticket it took, if it is to answer later.
    ticket: Option<u64>,
    /// Whether a call it made back unwound: the unwinding goes on
    /// through it.
    unwound: bool,
    /// Its number, which the functions read for it hold: see
    /// [`Live`].
    number: u64,
    /// The abilities its extern declared, and the handlers bound for
    /// them where it was called: see [`bound`].
    row: Vec<(String, String)>,
    handlers: Word,
}

impl Call<'_> {
    /// Calls `callee`, a function of the program that something else
    /// holds, with nothing bound and `args`.
    fn made(&mut self, callee: Word, args: &[Value]) -> Result<Value, String> {
        if self.unwound {
            return Err(UNWOUND.to_owned());
        }
        if self.ticket.is_some() {
            return Err(ANSWERING.to_owned());
        }
        let mut make = Inside(self.program);
        // SAFETY: the word is a function, which has an arity; each
        // index is within the length asked for, and a tuple keeps a
        // reference of its own to what it is set. The context is the
        // one this call was handed, and each word made here is owned
        // here and not used after it is released.
        unsafe {
            if ambient_runtime::amb_closure_arity(callee) != args.len() as u64 + 1 {
                return Err(UNCALLABLE.to_owned());
            }
            let handed = ambient_runtime::amb_sequence_new(0, args.len() as u64 + 1);
            for (index, arg) in (1..).zip(args) {
                let word = value::encode(arg, &mut make);
                ambient_runtime::amb_sequence_set(handed, index, word);
                ambient_runtime::amb_release(word);
            }
            let result = ambient_runtime::amb_call_among(self.context, callee, handed);
            ambient_runtime::amb_release(handed);
            if result == ambient_runtime::UNWIND {
                self.unwound = true;
                return Err(UNWOUND.to_owned());
            }
            let value = value::decode(result, &mut Host);
            ambient_runtime::amb_release(result);
            Ok(value)
        }
    }
}

impl Running for Call<'_> {
    fn load(&mut self, parts: &Parts, natives: &Natives) -> Result<Value, String> {
        self.held.push(self.program.add(parts, natives)?);
        parts.function()
    }

    fn backend(&self) -> Result<&str, String> {
        Ok(super::NAME)
    }

    fn later(&mut self) -> Result<u64, String> {
        // SAFETY: the context is the one this call was handed.
        let ticket = unsafe { ambient_runtime::amb_await(self.context) };
        if ticket == 0 {
            return Err(UNDECLARED.to_owned());
        }
        self.ticket = Some(ticket);
        Ok(ticket)
    }

    fn call(&mut self, function: &Value, args: &[Value]) -> Result<Value, String> {
        let callee = Live::of(function, self.number).ok_or_else(|| UNCALLABLE.to_owned())?;
        self.made(callee, args)
    }

    fn perform(&mut self, package: &str, path: &str, args: &[Value]) -> Result<Value, String> {
        let handler =
            bound(&self.row, package, path).ok_or_else(|| UNDECLARED_ABILITY.to_owned())?;
        // SAFETY: the place is within the tuple of handlers, which the
        // extern's caller holds for the call.
        let handler = unsafe { ambient_runtime::amb_word_at(self.handlers, handler) };
        self.made(handler, args)
    }

    fn ask(&mut self, question: &str, args: &[Value]) -> Result<Value, String> {
        let questions = self.program.questions.get();
        questions
            .ok_or_else(|| UNASKED.to_owned())?
            .answer(question, args)
    }
}

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
        self.offered.insert(key, Offered { run, verifiable });
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

/// What the runtime calls to run an extern: see
/// [`ambient_runtime::Native`].
pub(super) unsafe extern "C" fn dispatch(
    context: *mut Context,
    index: u64,
    bindings: Word,
    args: Word,
) -> Word {
    // SAFETY: `Program::call` put itself in the context for the length of
    // the call this is part of.
    let program = unsafe {
        let address = usize::try_from((*context).host).unwrap_or(0);
        &*(address as *const Program)
    };
    program.heard();
    // A task left a wait: the host may stop working on it.
    if index == ambient_runtime::FORSAKEN {
        program.forsake(bindings);
        return ambient_runtime::NULL;
    }
    let number = Live::call();
    let mut handed = Handed {
        inspect: &mut Host,
        call: number,
    };
    let args = match &mut value::decode(args, &mut handed) {
        Value::Tuple(args) => std::mem::take(args),
        _ => Vec::new(),
    };
    let index = usize::try_from(index).unwrap_or(usize::MAX);
    // Native code may load more into the program, so nothing of the
    // program is borrowed while it runs.
    let (native, offered) = program.natives.borrow().get(index).cloned().unzip();
    // Native code is not to unwind through compiled code.
    let mut call = Call {
        program,
        context,
        held: Vec::new(),
        ticket: None,
        unwound: false,
        number,
        row: native
            .as_ref()
            .map(|native| native.row.clone())
            .unwrap_or_default(),
        handlers: bindings,
    };
    let outcome = match &offered {
        Some(offered) => catch_unwind(AssertUnwindSafe(|| (offered.run)(&mut call, &args)))
            .unwrap_or_else(|_| Err("native code panicked".to_owned())),
        None => Err("no such extern".to_owned()),
    };
    let observation = offered
        .as_ref()
        .zip(native)
        .map(|(offered, native)| Observation {
            native,
            args,
            result: outcome.clone(),
            verifiable: offered.verifiable,
        });
    let result = match (call.ticket, observation) {
        // What it called is unwinding, and goes on through it.
        _ if call.unwound => ambient_runtime::UNWIND,
        // It answers later: the task waits, and what it returned is
        // not looked at.
        (Some(ticket), Some(observation)) => {
            // SAFETY: the context is the one this call was handed.
            unsafe { asked(program, context, ticket, observation) };
            ambient_runtime::NULL
        }
        (_, observation) => {
            if let (Some(trace), Some(observation)) =
                (program.trace.borrow_mut().as_mut(), observation)
            {
                trace.push(observation);
            }
            answered(program, context, outcome)
        }
    };
    // What was loaded is held from here on by whatever of it the result
    // refers to.
    for function in call.held {
        // SAFETY: each is a word `Program::add` gave this call.
        unsafe { ambient_runtime::amb_release(function) };
    }
    result
}

/// What native code gave, as the word its caller takes.
fn answered(program: &Program, context: *mut Context, outcome: Result<Value, String>) -> Word {
    match outcome {
        Ok(value) => value::encode(&value, &mut Inside(program)),
        Err(message) => {
            *program.failure.borrow_mut() = Some(message);
            // SAFETY: the context is the one this call was handed.
            unsafe { ambient_runtime::amb_fault(context, Fault::Native as u64) }
        }
    }
}
