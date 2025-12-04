#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Only process valid UTF-8 strings
    if let Ok(source) = std::str::from_utf8(data) {
        // Try to tokenize the source - we don't care about the result,
        // just that it doesn't panic or cause undefined behavior
        let mut lexer = ambient_parser::Lexer::new(source);
        let _ = lexer.tokenize();
    }
});
