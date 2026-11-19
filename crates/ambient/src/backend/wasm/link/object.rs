//! Reading the runtime as pieces: each function and each block of data
//! of a relocatable object, with what it refers to. See "Pieces" in
//! `docs/spec/linking.md`.
//!
//! The object is what a linker makes of the runtime and everything it
//! is built on: one symbol table, and beside the code and the data a
//! list of the places that hold an index or an address the final
//! module decides.

use std::collections::HashMap;
use std::ops::Range;

use wasm_encoder::ValType;

use crate::hash::Hash;
use wasmparser::{
    KnownCustom, Linking, Parser, Payload, RelocationEntry, RelocationType, SymbolFlags,
    SymbolInfo, TypeRef,
};

/// The runtime, as pieces.
pub struct Library {
    /// The function types its pieces name, by index.
    pub types: Vec<Signature>,
    pub pieces: Vec<Piece>,
    /// The functions other code can name, and the piece each is.
    pub named: HashMap<String, usize>,
    /// The name of each piece: see [`super::hash`].
    pub hashes: Vec<Hash>,
}

/// A function type: what it takes and what it gives.
pub type Signature = (Vec<ValType>, Vec<ValType>);

pub struct Piece {
    /// What the runtime's source calls it, for a message.
    pub name: String,
    pub body: Body,
    /// The places in the body that hold what the final module decides,
    /// in the order they are in.
    pub relocs: Vec<Reloc>,
}

pub enum Body {
    /// A function: its locals and its instructions, and its type.
    Function { ty: u32, code: Vec<u8> },
    /// A function the module's host supplies.
    Import {
        module: String,
        field: String,
        ty: u32,
    },
    Data {
        bytes: Vec<u8>,
        /// The power of two its address is a multiple of.
        align: u32,
        /// Whether the runtime writes to it: see "Pieces" in
        /// `docs/spec/linking.md`.
        state: bool,
    },
}

/// A place that holds what the final module decides.
pub struct Reloc {
    /// Where in the piece's body.
    pub offset: usize,
    pub form: Form,
    pub target: Target,
}

/// How a place holds its number.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Form {
    /// Five bytes of an unsigned number, seven bits to a byte.
    Unsigned,
    /// Five bytes of a signed one.
    Signed,
    /// Four bytes.
    Word,
}

impl Form {
    pub fn len(self) -> usize {
        match self {
            Self::Unsigned | Self::Signed => 5,
            Self::Word => 4,
        }
    }
}

/// What a place holds.
#[derive(Clone, Copy)]
pub enum Target {
    /// The index of a function piece.
    Function(usize),
    /// Where the table has a function piece.
    Slot(usize),
    /// An address this far into a data piece.
    Address(usize, i64),
    /// The index of one of the library's types.
    Type(u32),
    /// The index of the global that points at the stack.
    Stack,
    /// The index of the table.
    Table,
}

/// A symbol of the object: what a relocation names.
#[derive(Clone, Copy)]
enum Symbol {
    Function(usize),
    Data(usize, i64),
    Stack,
    Table,
    /// One nothing here can stand for.
    Other,
}

/// What reading keeps of a section that relocations point into: where
/// its content starts in the object, and where in the object each piece
/// of it is.
#[derive(Default)]
struct Section {
    start: usize,
    pieces: Vec<(usize, Range<usize>)>,
}

fn text(error: impl std::fmt::Display) -> String {
    format!("the runtime's object: {error}")
}

fn at(offset: u64) -> Result<usize, String> {
    usize::try_from(offset).map_err(text)
}

fn value(ty: wasmparser::ValType) -> Result<ValType, String> {
    match ty {
        wasmparser::ValType::I32 => Ok(ValType::I32),
        wasmparser::ValType::I64 => Ok(ValType::I64),
        wasmparser::ValType::F32 => Ok(ValType::F32),
        wasmparser::ValType::F64 => Ok(ValType::F64),
        other => Err(text(format!("a function takes or gives {other:?}"))),
    }
}

/// What the names of the functions the standard library panics through
/// hold: everything in `core::panicking`, and the functions that fail
/// an index, an `expect`, or an allocation. Each is a path as a mangled
/// name spells it. One that is missing here costs bytes and nothing
/// else; `tests/footprint.rs` would say.
const PANICS: &[&str] = &[
    "4core9panicking",
    "4core5slice5index16slice_index_fail",
    "4core6option13expect_failed",
    "4core6result13unwrap_failed",
    "4core3str16slice_error_fail",
    "4core4cell22panic_already_borrowed",
    "4core4cell30panic_already_mutably_borrowed",
    "5alloc7raw_vec12handle_error",
    "5alloc7raw_vec17capacity_overflow",
    "5alloc5alloc18handle_alloc_error",
    "alloc_error_handler",
];

/// A block of data as the object has it.
struct Segment<'a> {
    bytes: &'a [u8],
    align: u32,
    /// Whether it is in a section the runtime writes to.
    state: bool,
}

/// What reading an object gathers before it can say what refers to
/// what: the symbol table comes after everything it names.
#[derive(Default)]
struct Reading<'a> {
    code: Section,
    data: Section,
    /// The type of each function the object defines.
    defined: Vec<u32>,
    symbols: Vec<SymbolInfo<'a>>,
    /// Each relocation, and whether it is in the code.
    relocs: Vec<(bool, RelocationEntry)>,
    /// Each block of data, and where the object has it.
    segments: Vec<(Segment<'a>, Range<usize>)>,
}

/// Reads what a linker is told: the symbols, and the relocations.
fn custom<'a>(
    reader: &wasmparser::CustomSectionReader<'a>,
    reading: &mut Reading<'a>,
) -> Result<(), String> {
    match reader.as_known() {
        KnownCustom::Linking(linking) => {
            for part in linking.subsections() {
                match part.map_err(text)? {
                    Linking::SegmentInfo(infos) => {
                        for (info, (segment, _)) in infos.into_iter().zip(&mut reading.segments) {
                            let info = info.map_err(text)?;
                            segment.align = info.alignment;
                            // Named for the section its source put it
                            // in, and only this one is never written.
                            segment.state = !info.name.starts_with(".rodata");
                        }
                    }
                    Linking::SymbolTable(table) => {
                        for symbol in table {
                            reading.symbols.push(symbol.map_err(text)?);
                        }
                    }
                    Linking::InitFuncs(_) => {
                        return Err(text("it has code to run before any other"));
                    }
                    _ => {}
                }
            }
        }
        KnownCustom::Reloc(section) => {
            let of_code = reader.name() == "reloc.CODE";
            for entry in section.entries() {
                reading.relocs.push((of_code, entry.map_err(text)?));
            }
        }
        _ => {}
    }
    Ok(())
}

impl Library {
    /// Reads a relocatable object.
    ///
    /// # Errors
    ///
    /// Fails if the bytes are not one, or hold what a module made here
    /// has no place for.
    pub fn read(object: &[u8]) -> Result<Self, String> {
        let mut library = Self {
            types: Vec::new(),
            pieces: Vec::new(),
            named: HashMap::new(),
            hashes: Vec::new(),
        };
        let mut reading = Reading::default();
        for payload in Parser::new(0).parse_all(object) {
            match payload.map_err(text)? {
                Payload::CustomSection(reader) => custom(&reader, &mut reading)?,
                payload => library.section(payload, object, &mut reading)?,
            }
        }
        let functions = library.pieces.len();
        for (segment, range) in std::mem::take(&mut reading.segments) {
            reading.data.pieces.push((library.pieces.len(), range));
            library.pieces.push(Piece {
                name: String::new(),
                body: Body::Data {
                    bytes: segment.bytes.to_vec(),
                    align: segment.align,
                    state: segment.state,
                },
                relocs: Vec::new(),
            });
        }
        let symbols = library.symbols(&reading.symbols, functions);
        for (of_code, entry) in &reading.relocs {
            let section = if *of_code {
                &reading.code
            } else {
                &reading.data
            };
            library.place(section, entry, &symbols)?;
        }
        library.trap();
        library.hashes = super::hash::hashes(&library);
        Ok(library)
    }

    /// Reads one of the sections every module has.
    fn section<'a>(
        &mut self,
        payload: Payload<'a>,
        object: &'a [u8],
        reading: &mut Reading<'a>,
    ) -> Result<(), String> {
        match payload {
            Payload::TypeSection(reader) => {
                for ty in reader.into_iter_err_on_gc_types() {
                    let ty = ty.map_err(text)?;
                    let all = |types: &[wasmparser::ValType]| {
                        types.iter().copied().map(value).collect::<Result<_, _>>()
                    };
                    self.types.push((all(ty.params())?, all(ty.results())?));
                }
            }
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.map_err(text)?;
                    if let TypeRef::Func(ty) = import.ty {
                        self.pieces.push(Piece {
                            name: import.name.to_owned(),
                            body: Body::Import {
                                module: import.module.to_owned(),
                                field: import.name.to_owned(),
                                ty,
                            },
                            relocs: Vec::new(),
                        });
                    }
                }
            }
            Payload::FunctionSection(reader) => {
                for ty in reader {
                    reading.defined.push(ty.map_err(text)?);
                }
            }
            Payload::CodeSectionStart {
                unchecked_range, ..
            } => reading.code.start = at(unchecked_range.start)?,
            Payload::CodeSectionEntry(body) => {
                let ty = reading.defined.get(reading.code.pieces.len()).copied();
                let ty = ty.ok_or_else(|| text("a function has no type"))?;
                let range = at(body.range().start)?..at(body.range().end)?;
                reading.code.pieces.push((self.pieces.len(), range.clone()));
                let code = object[range].to_vec();
                self.pieces.push(Piece {
                    name: String::new(),
                    body: Body::Function { ty, code },
                    relocs: Vec::new(),
                });
            }
            Payload::DataSection(reader) => {
                reading.data.start = at(reader.range().start)?;
                for segment in reader {
                    let segment = segment.map_err(text)?;
                    let start = segment.data.as_ptr() as usize - object.as_ptr() as usize;
                    let range = start..start + segment.data.len();
                    let segment = Segment {
                        bytes: segment.data,
                        align: 0,
                        state: true,
                    };
                    reading.segments.push((segment, range));
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Cuts the functions the standard library panics through down to
    /// a trap.
    ///
    /// A panic in the runtime is a trap: its handler reads nothing. But
    /// these functions are compiled before the runtime is, to format a
    /// message first, and what formats would be reached by every
    /// program. None of them returns, so a trap is all they ever did.
    fn trap(&mut self) {
        for piece in &mut self.pieces {
            let panics = PANICS.iter().any(|name| piece.name.contains(name));
            if let (true, Body::Function { code, .. }) = (panics, &mut piece.body) {
                // No locals, and one instruction.
                *code = vec![0x00, 0x00, 0x0b];
                piece.relocs.clear();
            }
        }
    }

    /// What each symbol stands for, naming the pieces as it goes.
    fn symbols(&mut self, symbols: &[SymbolInfo], functions: usize) -> Vec<Symbol> {
        let each = symbols.iter().map(|symbol| match *symbol {
            SymbolInfo::Func { flags, index, name } => {
                let piece = index as usize;
                if let (Some(name), Some(found)) = (name, self.pieces.get_mut(piece)) {
                    name.clone_into(&mut found.name);
                    let hidden =
                        flags.intersects(SymbolFlags::BINDING_LOCAL | SymbolFlags::UNDEFINED);
                    if !hidden {
                        self.named.insert(name.to_owned(), piece);
                    }
                }
                Symbol::Function(piece)
            }
            SymbolInfo::Data {
                name,
                symbol: Some(at),
                ..
            } => {
                let piece = functions + at.index as usize;
                if let Some(found) = self
                    .pieces
                    .get_mut(piece)
                    .filter(|piece| piece.name.is_empty())
                {
                    name.clone_into(&mut found.name);
                }
                Symbol::Data(piece, i64::from(at.offset))
            }
            SymbolInfo::Global { name, .. }
                if name.is_none_or(|name| name == "__stack_pointer") =>
            {
                Symbol::Stack
            }
            SymbolInfo::Table { .. } => Symbol::Table,
            _ => Symbol::Other,
        });
        each.collect()
    }

    /// Gives a relocation to the piece it is in.
    fn place(
        &mut self,
        section: &Section,
        entry: &RelocationEntry,
        symbols: &[Symbol],
    ) -> Result<(), String> {
        let at = section.start + entry.offset as usize;
        let found = section.pieces.partition_point(|(_, range)| range.end <= at);
        let Some((piece, range)) = section
            .pieces
            .get(found)
            .filter(|(_, range)| range.contains(&at))
        else {
            return Err(text("a relocation is outside every piece"));
        };
        let symbol = symbols.get(entry.index as usize).copied();
        let (form, target) = match (entry.ty, symbol) {
            (RelocationType::TypeIndexLeb, _) => (Form::Unsigned, Target::Type(entry.index)),
            (RelocationType::FunctionIndexLeb, Some(Symbol::Function(piece))) => {
                (Form::Unsigned, Target::Function(piece))
            }
            (RelocationType::TableIndexSleb, Some(Symbol::Function(piece))) => {
                (Form::Signed, Target::Slot(piece))
            }
            (RelocationType::TableIndexI32, Some(Symbol::Function(piece))) => {
                (Form::Word, Target::Slot(piece))
            }
            (RelocationType::MemoryAddrLeb, Some(Symbol::Data(piece, offset))) => (
                Form::Unsigned,
                Target::Address(piece, offset + entry.addend),
            ),
            (RelocationType::MemoryAddrSleb, Some(Symbol::Data(piece, offset))) => {
                (Form::Signed, Target::Address(piece, offset + entry.addend))
            }
            (RelocationType::MemoryAddrI32, Some(Symbol::Data(piece, offset))) => {
                (Form::Word, Target::Address(piece, offset + entry.addend))
            }
            (RelocationType::GlobalIndexLeb, Some(Symbol::Stack)) => {
                (Form::Unsigned, Target::Stack)
            }
            (RelocationType::TableNumberLeb, Some(Symbol::Table)) => {
                (Form::Unsigned, Target::Table)
            }
            (ty, _) => {
                let name = &self.pieces[*piece].name;
                return Err(text(format!("{name} holds a {ty:?} nothing here places")));
            }
        };
        self.pieces[*piece].relocs.push(Reloc {
            offset: at - range.start,
            form,
            target,
        });
        Ok(())
    }
}
