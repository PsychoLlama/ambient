//! Numbers as WebAssembly writes them: seven bits to a byte.

/// Writes an unsigned number seven bits to a byte.
pub fn leb128(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = value.to_le_bytes()[0] & 0x7f;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// Writes a signed number seven bits to a byte.
pub fn sleb128(out: &mut Vec<u8>, mut value: i64) {
    loop {
        let byte = value.to_le_bytes()[0] & 0x7f;
        value >>= 7;
        let done = (value == 0 && byte & 0x40 == 0) || (value == -1 && byte & 0x40 != 0);
        out.push(if done { byte } else { byte | 0x80 });
        if done {
            return;
        }
    }
}
