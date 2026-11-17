//! Loading compiled parts into this process, calling them, and reading
//! what they return.

use std::cell::{Cell, RefCell, UnsafeCell};
use std::collections::{HashMap, HashSet};

use ambient_runtime::{Entry, Fault, NULL, Word};

use super::externs::Externs;
use super::host::{Natives, Observation, dispatch};
use super::unit::{Unit, map};
use super::values::Host;
use super::waits::{Answers, Asked};
use crate::hash::Hash;
use crate::parts::{Body, CLOSED, Part, Parts};
use crate::stack;
use crate::statics::{self, Base, Field, Known, Place};
use crate::value::{self, Value};

/// A program loaded into this process, ready to call. More can be
/// loaded into it while it lives: see [`Program::add`].
pub struct Program {
    /// What each load brought, by the address of its block.
    units: RefCell<HashMap<u64, Unit>>,
    /// The first of the units nothing refers to, which the runtime
    /// keeps up to date: see [`Self::reclaim`].
    unused: Box<UnsafeCell<u64>>,
    /// Where each definition it has is, by hash.
    known: RefCell<HashMap<Hash, Known>>,
    /// The functions among them that have a metered form.
    metered: RefCell<HashSet<Hash>>,
    entries: Box<[usize]>,
    /// The closure of the function it was first loaded for.
    entry: Cell<Word>,
    /// Each extern and the native code for it, by the index compiled
    /// code names it by.
    pub(super) natives: RefCell<Externs>,
    /// Every call of native code so far, if they are being kept.
    pub(super) trace: RefCell<Option<Vec<Observation>>>,
    /// What native code last said went wrong.
    pub(super) failure: RefCell<Option<String>>,
    /// The fault that ended the last call, as an error.
    error: RefCell<Option<Value>>,
    /// The first of the cells calls have made that are still live, which
    /// the runtime keeps up to date: a cell outlives the call that made
    /// it, and none outlives the program.
    cells: Box<UnsafeCell<u64>>,
    /// Whether the program was linked for code to be loaded into it.
    open: bool,
    /// What brings the answers of native code that answers later.
    pub(super) waits: Option<Answers>,
    /// The calls that are to be answered, by ticket.
    pub(super) asked: RefCell<HashMap<u64, Asked>>,
}

impl Drop for Program {
    fn drop(&mut self) {
        // SAFETY: the word is the one every call's context was given.
        // No call is running, so nothing reads a cell after this.
        unsafe { ambient_runtime::amb_cells_drop(self.cells.get()) }
        // What its cells held goes with the program, and no host is
        // told: the next program on this thread must not hear of it.
        while ambient_runtime::amb_handle_next() != 0 {}
        // A unit that goes gives up the units it refers into, which may
        // be gone already when every unit goes.
        let mut units = self.units.borrow_mut();
        for unit in units.values_mut() {
            unit.holds.clear();
        }
        units.clear();
    }
}

impl Program {
    /// Loads the parts of a program compiled by the native backend, with
    /// the native code for its externs.
    ///
    /// # Errors
    ///
    /// Fails if `natives` lacks an extern the program reaches, or
    /// executable memory cannot be mapped.
    pub fn load(parts: &Parts, natives: &Natives) -> Result<Self, String> {
        let program = Self {
            units: RefCell::default(),
            unused: Box::default(),
            known: RefCell::default(),
            metered: RefCell::default(),
            entries: Entry::ALL.iter().map(|entry| entry.address()).collect(),
            entry: Cell::new(NULL),
            natives: RefCell::default(),
            trace: RefCell::default(),
            failure: RefCell::default(),
            error: RefCell::default(),
            cells: Box::default(),
            open: parts.open,
            waits: natives.waits(),
            asked: RefCell::default(),
        };
        program.entry.set(program.place(parts, natives, false)?);
        Ok(program)
    }

    /// Loads more into the program: the parts of a function and what it
    /// reaches, as `Engine::link_to_load` makes them.
    /// Returns the function, a value the caller owns.
    ///
    /// A part the program has already, by hash, is not loaded again;
    /// what is loaded uses the one that is there. What is loaded is held
    /// by the values that refer to it: the function returned, closures
    /// over its definitions, cells and type arguments made at its
    /// types. Once there are none it is dropped, no sooner than the
    /// next load or the end of a call.
    ///
    /// # Errors
    ///
    /// Fails if `natives` lacks an extern the parts reach, or executable
    /// memory cannot be mapped. Nothing is loaded then.
    pub fn add(&self, parts: &Parts, natives: &Natives) -> Result<Word, String> {
        // What is loaded may suspend in the middle of a call the
        // program made, and only an open program is compiled for that.
        if !self.open {
            return Err(CLOSED.to_owned());
        }
        self.reclaim();
        self.place(parts, natives, true)
    }

    /// Drops what was loaded and nothing refers to any more, and
    /// whatever only that referred to. Every load begins with this and
    /// every call ends with it.
    pub fn reclaim(&self) {
        self.heard();
        loop {
            // SAFETY: the word is the list every unit was given.
            let unit = unsafe { ambient_runtime::amb_unit_take(self.unused.get()) };
            if unit == 0 {
                break;
            }
            let Some(gone) = self.units.borrow_mut().remove(&unit) else {
                continue;
            };
            self.natives.borrow_mut().remove(gone.natives.clone());
            let mut known = self.known.borrow_mut();
            for hash in &gone.hashes {
                // A later unit may have brought the definition again.
                if known.get(hash).is_some_and(|known| known.unit == unit) {
                    known.remove(hash);
                    self.metered.borrow_mut().remove(hash);
                }
            }
        }
    }

    /// Lays out a block for `parts` and maps their code: as a unit if
    /// `loaded`, and for good if not. Returns the entry, owned.
    fn place(&self, parts: &Parts, natives: &Natives, loaded: bool) -> Result<Word, String> {
        let offered = natives.resolve(&parts.natives)?;
        let wants_metered =
            |part: &Part| matches!(&part.body, Body::Function { metered, .. } if metered.is_some());
        // A function that is here without the metered form these parts
        // call it by is loaded again, with one.
        let lacking: HashSet<Hash> = parts
            .parts
            .iter()
            .filter(|part| wants_metered(part) && !self.metered.borrow().contains(&part.hash))
            .map(|part| part.hash)
            .collect();
        let known = |hash: &Hash| {
            let known = self.known.borrow().get(hash).copied();
            known.filter(|_| !lacking.contains(hash))
        };
        let plan = statics::plan(
            parts,
            &Base {
                known: &known,
                loaded,
            },
        );
        let (code, offsets) = map(parts, &plan)?;
        let externs = self.natives.borrow().next();
        let code_base = code.as_ptr() as usize;

        let block = Box::into_raw(vec![0u64; plan.words.max(1)].into_boxed_slice());
        let base = block.cast::<u64>() as usize;
        let address = |word: usize| (base + word * 8) as u64;
        let mut unit = Unit {
            _code: code,
            statics: block,
            strings: Vec::new(),
            holds: Vec::new(),
            hashes: Vec::new(),
            natives: self.natives.borrow_mut().add(&parts.natives, offered),
        };
        // SAFETY: each word written is one of the block's.
        let write = |word: usize, value: u64| unsafe { block.cast::<u64>().add(word).write(value) };
        for (word, field) in &plan.fields {
            let value = match *field {
                Field::Word(value) => value,
                Field::Address(target) => address(target),
                Field::Code(code) => (code_base + offsets[code]) as u64,
                Field::Root(part) => address(plan.placed[part].map_or(0, |(root, _)| root)),
                Field::Definition(part) => address(plan.placed[part].map_or(0, |(_, at)| at)),
                Field::Native(native) => (externs + native) as u64,
                Field::Text {
                    make,
                    offset,
                    length,
                } => unit.text(make, &plan.text[offset..offset + length], loaded),
            };
            write(*word, value);
        }
        let record = if loaded { address(0) } else { 0 };
        if loaded {
            write(ambient_runtime::UNIT_LIST, self.unused.get() as u64);
        }

        for (part, placed) in parts.parts.iter().zip(&plan.placed) {
            let Some((root, definition)) = *placed else {
                // What the block refers to elsewhere, it holds.
                let held = known(&part.hash).map_or(0, |known| known.unit);
                if held != 0 && !unit.holds.contains(&held) {
                    // SAFETY: the unit is one of the program's.
                    unsafe { ambient_runtime::amb_unit_retain(held) };
                    unit.holds.push(held);
                }
                continue;
            };
            let known = Known {
                root: address(root),
                definition: address(definition),
                unit: record,
            };
            self.known.borrow_mut().insert(part.hash, known);
            unit.hashes.push(part.hash);
            if wants_metered(part) {
                self.metered.borrow_mut().insert(part.hash);
            }
        }
        self.units.borrow_mut().insert(address(0), unit);
        let entry = match plan.entry {
            Some(Place::Root(part)) => address(plan.placed[part].map_or(0, |(root, _)| root)),
            Some(Place::There(closure)) => closure,
            _ => return Err("the parts have no entry".to_owned()),
        };
        // SAFETY: the entry is a closure and the record a unit's. The
        // unit is held while the entry is taken, so that one the entry
        // is not in joins the units to drop.
        unsafe {
            ambient_runtime::amb_unit_retain(record);
            ambient_runtime::amb_retain(entry);
            ambient_runtime::amb_unit_release(record);
        }
        Ok(entry)
    }

    /// A function the program has, by the hash of its definition: a
    /// value the caller owns.
    #[must_use]
    pub fn function(&self, hash: &Hash) -> Option<Word> {
        let root = self.known.borrow().get(hash).map(|known| known.root)?;
        // SAFETY: the root is a closure of a unit not yet dropped.
        unsafe { ambient_runtime::amb_retain(root) };
        Some(root)
    }

    /// Whether the program has a definition, by its hash. One that was
    /// loaded is there while a value refers to it.
    #[must_use]
    pub fn has(&self, hash: &Hash) -> bool {
        self.known.borrow().contains_key(hash)
    }

    /// Where the definition of a function the program has is.
    pub(super) fn definition(&self, hash: &Hash) -> Option<u64> {
        let known = self.known.borrow().get(hash).copied();
        known.map(|known| known.definition).filter(|at| *at != 0)
    }

    /// What native code said went wrong, if the last call ended with
    /// [`Fault::Native`].
    #[must_use]
    pub fn failure(&self) -> Option<String> {
        self.failure.borrow().clone()
    }

    /// The fault that ended the last call, as the error a boundary
    /// would have made of it: its kind, and the trace of where it was.
    #[must_use]
    pub fn error(&self) -> Option<Value> {
        self.error.borrow().clone()
    }

    /// Keeps every call of native code from now on, for
    /// [`Self::observations`].
    pub fn observe(&self) {
        *self.trace.borrow_mut() = Some(Vec::new());
    }

    /// Every call of native code since [`Self::observe`], in order.
    #[must_use]
    pub fn observations(&self) -> Vec<Observation> {
        self.trace.borrow().clone().unwrap_or_default()
    }

    /// Calls the entry with `args`, which it borrows. The result is a word
    /// the caller owns, or the fault that ended the call.
    ///
    /// # Errors
    ///
    /// Returns the fault if the program faulted.
    ///
    /// # Panics
    ///
    /// Panics if `args` is not as many words as the entry takes.
    pub fn call(&self, args: &[Word]) -> Result<Word, Fault> {
        self.call_value(self.entry.get(), args)
    }

    /// Calls the entry with `args` as values, and gives what it gave as
    /// one.
    ///
    /// # Errors
    ///
    /// Returns the fault if the program faulted.
    ///
    /// # Panics
    ///
    /// Panics if `args` is not as many values as the entry takes.
    pub fn run(&self, args: &[Value]) -> Result<Value, Fault> {
        let words: Vec<Word> = args.iter().map(super::values::encode).collect();
        let result = self.call(&words);
        for word in words {
            // SAFETY: each word was made above and is not used again.
            unsafe { ambient_runtime::amb_release(word) };
        }
        result.map(|word| {
            let value = crate::value::decode(word, &mut Host);
            // SAFETY: the word is owned here and not used again.
            unsafe { ambient_runtime::amb_release(word) };
            // What it gave may have held the last of a handle.
            self.heard();
            value
        })
    }

    /// Calls a function value of this program with `args`, which it
    /// borrows, and with nothing bound: what [`Self::call`] does with
    /// the entry.
    ///
    /// # Errors
    ///
    /// Returns the fault if the function faulted.
    ///
    /// # Panics
    ///
    /// Panics if `function` is not a function, or `args` is not as many
    /// words as it takes.
    pub fn call_value(&self, function: Word, args: &[Word]) -> Result<Word, Fault> {
        stack::roomy(|| self.started(function, args))
    }

    /// Calls a function value on the stack it is running on.
    fn started(&self, function: Word, args: &[Word]) -> Result<Word, Fault> {
        // SAFETY: a value's kind can always be asked.
        let kind = unsafe { ambient_runtime::amb_kind(function) };
        assert_eq!(
            kind,
            ambient_runtime::Kind::Function as u64,
            "not a function"
        );
        // SAFETY: it is a function.
        let arity = unsafe { ambient_runtime::amb_closure_arity(function) };
        // The function is handed no bindings: nothing is provided to it.
        let mut words = vec![NULL];
        words.extend_from_slice(args);
        assert_eq!(words.len() as u64, arity, "wrong number of arguments");
        let closure = function;
        // SAFETY: the function takes as many words as it is handed, and
        // the context is laid out as the runtime defines.
        unsafe {
            let context = ambient_runtime::amb_context_new();
            (*context).entries = self.entries.as_ptr() as u64;
            (*context).native = dispatch as *const () as u64;
            (*context).host = std::ptr::from_ref(self) as u64;
            (*context).cells = self.cells.get() as u64;
            (*context).stacks = std::ptr::from_ref(&super::stacks::STACKS) as u64;
            let handed = ambient_runtime::amb_sequence_new(0, words.len() as u64);
            for (index, word) in (0..).zip(&words) {
                ambient_runtime::amb_sequence_set(handed, index, *word);
            }
            let invoke = move || ambient_runtime::amb_start(context, closure, handed, 0);
            // Unwinding in this mode is the root task suspending: the
            // others take their turns, and the entry runs again to
            // resume it where it was.
            let mut result = invoke();
            while result == ambient_runtime::UNWIND
                && (*context).mode == ambient_runtime::SUSPENDING
            {
                (*context).mode = ambient_runtime::RUNNING;
                self.schedule(context);
                result = invoke();
            }
            ambient_runtime::amb_release(handed);
            let fault = Fault::from_code((*context).fault);
            // The root is a boundary: a fault that reached it is an
            // error, for whoever called.
            let error = (result == ambient_runtime::UNWIND)
                .then(|| ambient_runtime::amb_context_error(context))
                .filter(|error| *error != NULL)
                .map(|word| {
                    let error = value::decode(word, &mut Host);
                    ambient_runtime::amb_release(word);
                    error
                });
            *self.error.borrow_mut() = error;
            self.asked.borrow_mut().clear();
            ambient_runtime::amb_context_free(context);
            self.reclaim();
            if result == ambient_runtime::UNWIND {
                Err(fault)
            } else {
                Ok(result)
            }
        }
    }
}
