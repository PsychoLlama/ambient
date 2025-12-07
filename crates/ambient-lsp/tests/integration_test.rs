use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

#[test]
fn test_lsp_initialize() {
    // Find the workspace root and build the ambient binary path
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent().unwrap().parent().unwrap();
    let ambient_path = workspace_root.join("target/debug/ambient");

    // Ensure the binary exists (it should have been built by cargo test)
    assert!(
        ambient_path.exists(),
        "ambient binary not found at {}, run `cargo build` first",
        ambient_path.display()
    );

    let mut child = Command::new(&ambient_path)
        .arg("lsp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn ambient lsp");

    let stdin = child.stdin.as_mut().expect("Failed to get stdin");
    let stdout = child.stdout.take().expect("Failed to get stdout");

    // Send initialize request
    let init_msg = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"processId":null,"capabilities":{},"rootUri":"file:///tmp"}}"#;
    let header = format!("Content-Length: {}\r\n\r\n", init_msg.len());

    stdin
        .write_all(header.as_bytes())
        .expect("Failed to write header");
    stdin
        .write_all(init_msg.as_bytes())
        .expect("Failed to write message");
    stdin.flush().expect("Failed to flush");

    eprintln!("Sent initialize request, waiting for response...");

    // Read response
    let mut reader = BufReader::new(stdout);
    let mut header_line = String::new();

    // Set a timeout by using a thread
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
        reader
            .read_line(&mut header_line)
            .expect("Failed to read header");
        tx.send(header_line).ok();
    });

    match rx.recv_timeout(std::time::Duration::from_secs(5)) {
        Ok(header) => {
            eprintln!("Got header: {}", header);
            assert!(
                header.starts_with("Content-Length:"),
                "Expected Content-Length header"
            );
        }
        Err(_) => {
            // Check stderr for errors
            let stderr = child.stderr.take().expect("Failed to get stderr");
            let mut stderr_output = String::new();
            BufReader::new(stderr).read_line(&mut stderr_output).ok();
            eprintln!("Stderr: {}", stderr_output);

            child.kill().ok();
            panic!("Timeout waiting for LSP response");
        }
    }

    child.kill().ok();
    handle.join().ok();
}
