//! Units: code loaded into a program that is running, and who holds it.
//! See "Receiving" in `spec/exchange.md`.
//!
//! A unit is a block of words a host laid out: definitions, closures,
//! type arguments, and descriptors, none of them reclaimed one by one.
//! The block begins with a record of how many references reach into it.
//! Whatever can be referred to from outside the block is preceded by one
//! word, the address of that record, or zero in a block that is never
//! reclaimed. When the count reaches zero the unit joins its host's list
//! of units to reclaim; only the host knows how.

/// The words of the record a unit begins with: its count, where its
/// host's list of units to reclaim begins, the unit after it in that
/// list, and whether it is in the list.
pub const UNIT_WORDS: usize = 4;
/// Where in the record the address of the host's list is, in words.
pub const UNIT_LIST: usize = 1;
/// Where in the record the next unit of the list is, in words.
pub const UNIT_NEXT: usize = 2;
const LISTED: usize = 3;

/// The unit that holds whatever is at `address`: a definition, an
/// object of a block, or a descriptor a constant names. Zero if none
/// does.
pub(crate) unsafe fn of(address: u64) -> u64 {
    unsafe { (address as usize as *const u64).sub(1).read() }
}

/// Takes one more reference into a unit, or does nothing for zero.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_unit_retain(unit: u64) {
    if unit != 0 {
        unsafe { *(unit as usize as *mut u64) += 1 }
    }
}

/// Gives up one reference into a unit, or does nothing for zero. A unit
/// nothing refers to joins its host's list; a reference taken before
/// the host looks keeps it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_unit_release(unit: u64) {
    if unit == 0 {
        return;
    }
    unsafe {
        let record = unit as usize as *mut u64;
        *record -= 1;
        if *record == 0 && *record.add(LISTED) == 0 {
            let list = *record.add(UNIT_LIST) as usize as *mut u64;
            *record.add(UNIT_NEXT) = *list;
            *record.add(LISTED) = 1;
            *list = unit;
        }
    }
}

/// Takes the first unit from a host's list, and says whether nothing
/// refers to it still. Zero if the list is empty.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn amb_unit_next(list: *mut u64, unused: *mut bool) -> u64 {
    unsafe {
        let unit = *list;
        if unit != 0 {
            let record = unit as usize as *mut u64;
            *list = *record.add(UNIT_NEXT);
            *record.add(LISTED) = 0;
            *unused = *record == 0;
        }
        unit
    }
}
