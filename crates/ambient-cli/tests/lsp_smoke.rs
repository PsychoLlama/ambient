//! Transport smoke test for `ambient lsp`.
//!
//! The in-process `TestClient` (in `ambient-lsp`) covers server logic. This
//! test pins only the one thing that harness can't: that the real `ambient`
//! binary wires stdin/stdout LSP framing at all. It spawns the built binary
//! (`CARGO_BIN_EXE_ambient` — always present, no path guessing), sends one
//! `initialize` request, and asserts a `Content-Length`-framed response comes
//! back.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// The read timeout is deliberately generous: it exists only to fail a
/// genuine hang rather than to police latency, so parallel workspace load
/// can't flake it.
const READ_TIMEOUT: Duration = Duration::from_secs(30);

#[test]
fn ambient_lsp_frames_an_initialize_response() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ambient"))
        .arg("lsp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn `ambient lsp`");

    let init_msg = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"processId":null,"capabilities":{},"rootUri":"file:///tmp"}}"#;
    {
        let stdin = child.stdin.as_mut().expect("child stdin");
        write!(
            stdin,
            "Content-Length: {}\r\n\r\n{init_msg}",
            init_msg.len()
        )
        .and_then(|()| stdin.flush())
        .expect("write initialize request");
    }

    // Read the first response header line on a helper thread so a hung server
    // can't block the test forever.
    let stdout = child.stdout.take().expect("child stdout");
    let (tx, rx) = mpsc::channel();
    let reader_thread = thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut header_line = String::new();
        let _ = reader.read_line(&mut header_line);
        let _ = tx.send(header_line);
    });

    let result = rx.recv_timeout(READ_TIMEOUT);

    // Kill and reap on every path so no stray `ambient lsp` process leaks.
    let _ = child.kill();
    let _ = child.wait();
    let _ = reader_thread.join();

    match result {
        Ok(header) => assert!(
            header.starts_with("Content-Length:"),
            "expected a Content-Length-framed response, got: {header:?}"
        ),
        Err(_) => panic!("timed out waiting for the LSP initialize response"),
    }
}
