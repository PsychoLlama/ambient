//! A function body whose calls into the runtime are not numbered yet.
//!
//! Which functions a module has before its own depends on what it
//! reaches of the runtime (`spec/linking.md`), so a body compiled on
//! its own cannot say the index of one. It leaves room, and says where.

use ambient_runtime::Entry;
use wasm_encoder::Function;

use super::link::leb128;

/// Where a body calls the runtime: the offset of each index it left
/// room for, and the name of the function to call.
pub(super) type Calls = Vec<(usize, &'static str)>;

/// The room left for an index: five bytes, as many as any index takes.
const ROOM: [u8; 5] = [0x80, 0x80, 0x80, 0x80, 0x00];

/// Emits a call of the runtime's function `name`.
pub(super) fn call(body: &mut Function, calls: &mut Calls, name: &'static str) {
    body.raw([0x10]);
    calls.push((body.byte_len(), name));
    body.raw(ROOM);
}

/// A body and its calls as an artifact: the body, each call as four
/// bytes of offset and two of the entry point's number, and how many
/// calls there are.
pub(super) fn artifact(mut body: Vec<u8>, calls: &[(usize, Entry)]) -> Result<Vec<u8>, String> {
    let small = |value: usize| u32::try_from(value).map_err(|error| error.to_string());
    for (offset, entry) in calls {
        body.extend(small(*offset)?.to_le_bytes());
        body.extend((*entry as u16).to_le_bytes());
    }
    body.extend(small(calls.len())?.to_le_bytes());
    Ok(body)
}

/// Reads what [`artifact`] wrote.
pub(super) fn read(artifact: &[u8]) -> Result<(&[u8], Calls), String> {
    let broken = || "an artifact is not a function body".to_owned();
    let (rest, count) = artifact.split_last_chunk::<4>().ok_or_else(broken)?;
    let count = u32::from_le_bytes(*count) as usize;
    let body = rest.len().checked_sub(count * 6).ok_or_else(broken)?;
    let (body, table) = rest.split_at(body);
    let mut calls = Calls::new();
    for call in table.chunks_exact(6) {
        let offset = u32::from_le_bytes([call[0], call[1], call[2], call[3]]) as usize;
        let entry = usize::from(u16::from_le_bytes([call[4], call[5]]));
        let entry = Entry::ALL.get(entry).ok_or_else(broken)?;
        calls.push((offset, entry.name()));
    }
    Ok((body, calls))
}

/// A body with each call given the index `index` says its function
/// has, written as short as it goes.
pub(super) fn numbered(
    body: &[u8],
    calls: &Calls,
    index: &dyn Fn(&str) -> Result<u32, String>,
) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(body.len());
    let mut copied = 0;
    for (offset, name) in calls {
        out.extend(&body[copied..*offset]);
        leb128(&mut out, u64::from(index(name)?));
        copied = offset + ROOM.len();
    }
    out.extend(&body[copied..]);
    Ok(out)
}
