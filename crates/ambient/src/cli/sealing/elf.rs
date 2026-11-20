//! Making the names an ELF object defines its own, in place.
//!
//! A symbol table lists the symbols of its object alone before those
//! other objects see, and says how many they are. So a symbol made
//! the object's own moves, and whatever names a symbol by its place
//! is told the new one: each relocation, and each group of sections.
//! Nothing else of the object is read, so it may hold sections of
//! kinds this has never heard of.

use object::elf::{self, FileHeader64, SectionHeader64, Sym64};
use object::read::elf::{FileHeader, SectionHeader, Sym};
use object::{Endian, Endianness, pod};

type Read<T> = Result<T, String>;

/// A section, as much of it as is read here.
struct Section {
    kind: elf::SectionType,
    /// Where its content is in the file.
    content: std::ops::Range<usize>,
    /// The section it says it is about: for one that names symbols,
    /// their table.
    link: usize,
    info: u32,
}

fn malformed<E>(_: E) -> String {
    "it is no object file of this machine".to_owned()
}

/// The content of `section` in `object`.
fn content<'a>(object: &'a mut [u8], section: &Section) -> Read<&'a mut [u8]> {
    object
        .get_mut(section.content.clone())
        .ok_or_else(|| malformed(()))
}

/// Makes each symbol of `symbols` that is defined, and whose name
/// `shared` does not hold, the object's own, and puts those first.
/// Gives where each symbol now is, by where it was, and how many are
/// the object's own.
fn reorder(
    symbols: &mut [Sym64<Endianness>],
    names: &[u8],
    endian: Endianness,
    shared: fn(&[u8]) -> bool,
) -> (Vec<u32>, u32) {
    let name = |symbol: &Sym64<Endianness>| {
        let from = names
            .get(symbol.st_name(endian) as usize..)
            .unwrap_or_default();
        from.split(|byte| *byte == 0).next().unwrap_or_default()
    };
    // The first symbol is the one that stands for none.
    for symbol in symbols.iter_mut().skip(1) {
        let defined = !symbol.is_undefined(endian) && !symbol.is_common(endian);
        if defined && !shared(name(symbol)) {
            symbol.st_info = elf::STB_LOCAL | symbol.st_type();
        }
    }
    let was = symbols.to_vec();
    let own = |place: &usize| *place == 0 || was[*place].st_bind() == elf::STB_LOCAL;
    let (first, rest): (Vec<usize>, Vec<usize>) = (0..was.len()).partition(own);
    let mut now = vec![0; was.len()];
    for (place, from) in first.iter().chain(&rest).enumerate() {
        symbols[place] = was[*from];
        now[*from] = u32::try_from(place).unwrap_or_default();
    }
    (now, u32::try_from(first.len()).unwrap_or_default())
}

/// Tells each relocation in `relocations`, which are `size` bytes
/// each, where its symbol now is. Both kinds say which symbol in the
/// upper half of their second word.
fn relocated(relocations: &mut [u8], size: usize, endian: Endianness, now: &[u32]) {
    for relocation in relocations.chunks_exact_mut(size) {
        let Some(word) = relocation.get_mut(8..16) else {
            continue;
        };
        let mut bytes = [0; 8];
        bytes.copy_from_slice(word);
        let big = endian.is_big_endian();
        let info = if big {
            u64::from_be_bytes(bytes)
        } else {
            u64::from_le_bytes(bytes)
        };
        let symbol = now.get((info >> 32) as usize).copied().unwrap_or_default();
        let info = (u64::from(symbol) << 32) | (info & 0xffff_ffff);
        word.copy_from_slice(&if big {
            info.to_be_bytes()
        } else {
            info.to_le_bytes()
        });
    }
}

/// The table of the section each symbol is in, for an object with
/// more sections than a symbol can say: reordered as the symbols were.
fn indexed(table: &mut [u8], now: &[u32]) {
    let was = table.to_vec();
    for (from, index) in was.chunks_exact(4).enumerate() {
        let place = now.get(from).copied().unwrap_or_default() as usize * 4;
        if let Some(to) = table.get_mut(place..place + 4) {
            to.copy_from_slice(index);
        }
    }
}

/// The sections of `object`, and where their headers are in it.
fn sections(object: &[u8]) -> Read<(Endianness, usize, Vec<Section>)> {
    let header = FileHeader64::<Endianness>::parse(object).map_err(malformed)?;
    let endian = header.endian().map_err(malformed)?;
    let headers = header.section_headers(endian, object).map_err(malformed)?;
    let within = |header: &SectionHeader64<Endianness>| {
        let from = usize::try_from(header.sh_offset(endian)).ok()?;
        let size = usize::try_from(header.sh_size(endian)).ok()?;
        let empty = header.sh_type(endian) == elf::SHT_NOBITS;
        Some(from..from.checked_add(if empty { 0 } else { size })?)
    };
    let section = |header: &SectionHeader64<Endianness>| {
        Some(Section {
            kind: header.sh_type(endian),
            content: within(header)?,
            link: header.sh_link(endian) as usize,
            info: header.sh_info(endian),
        })
    };
    let sections: Option<Vec<Section>> = headers.iter().map(section).collect();
    let table = usize::try_from(header.e_shoff(endian)).map_err(malformed)?;
    Ok((endian, table, sections.ok_or_else(|| malformed(()))?))
}

/// Makes every name `object` defines its own, but those `shared`
/// holds.
///
/// # Errors
///
/// Fails if it is not an object file of 64 bits.
pub(super) fn own(object: &mut [u8], shared: fn(&[u8]) -> bool) -> Read<()> {
    let (endian, table, sections) = sections(object)?;
    let Some(symbols) = sections.iter().position(|s| s.kind == elf::SHT_SYMTAB) else {
        return Ok(());
    };
    let names = sections.get(sections[symbols].link);
    let names = content(object, names.ok_or_else(|| malformed(()))?)?.to_vec();
    let listed = content(object, &sections[symbols])?;
    let listed = pod::slice_from_all_bytes_mut(listed).map_err(malformed)?;
    let (now, own) = reorder(listed, &names, endian, shared);

    let count = sections.len();
    let about = sections
        .iter()
        .enumerate()
        .filter(|(_, s)| s.link == symbols);
    let mut said: Vec<(usize, Option<u32>, Option<u32>)> = vec![(symbols, None, Some(own))];
    for (index, section) in about.filter(|(index, _)| *index != symbols) {
        match section.kind {
            elf::SHT_REL => relocated(content(object, section)?, 16, endian, &now),
            elf::SHT_RELA => relocated(content(object, section)?, 24, endian, &now),
            elf::SHT_SYMTAB_SHNDX => indexed(content(object, section)?, &now),
            // A group is named by a symbol.
            elf::SHT_GROUP => said.push((index, None, now.get(section.info as usize).copied())),
            // Whatever else names symbols by place is told it no
            // longer does, as a linker that joins objects tells it.
            _ => said.push((index, Some(0), None)),
        }
    }
    let headers = object.get_mut(table..).ok_or_else(|| malformed(()))?;
    let (headers, _) = pod::slice_from_bytes_mut::<SectionHeader64<Endianness>>(headers, count)
        .map_err(malformed)?;
    for (index, link, info) in said {
        if let Some(link) = link {
            headers[index].sh_link.set(endian, link);
        }
        if let Some(info) = info {
            headers[index].sh_info.set(endian, info);
        }
    }
    Ok(())
}
