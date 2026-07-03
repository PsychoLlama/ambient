//! Golden tests for every program in `examples/`.
//!
//! Each example directory contains an `expected_output.txt` capturing the
//! program's exact output (ANSI escapes stripped). Any change to language
//! semantics that alters an example's behavior fails here.
//!
//! To regenerate goldens after an intentional change:
//!
//! ```bash
//! BLESS=1 cargo test -p ambient-cli --test examples
//! ```
//!
//! Client/server examples can't run standalone; they're exercised as live
//! pairs in [`network_pair_echoes`] and [`remote_pair_executes`].

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Examples that require a live peer and are tested as pairs instead,
/// plus long-lived services with their own dedicated tests
/// (`live_server` is exercised by the `dev_live_upgrade` test).
const PAIRED_EXAMPLES: &[&str] = &[
    "network_client",
    "network_server",
    "remote_client",
    "remote_server",
    "live_server",
];

fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .canonicalize()
        .expect("examples directory exists")
}

fn ambient_bin() -> &'static str {
    env!("CARGO_BIN_EXE_ambient")
}

/// Strip ANSI escape sequences (colors) from output.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Skip until the terminating letter of the CSI sequence.
            if chars.peek() == Some(&'[') {
                chars.next();
                for t in chars.by_ref() {
                    if t.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn run_example(dir: &Path) -> String {
    let output = Command::new(ambient_bin())
        .arg("run")
        .arg(dir)
        .output()
        .expect("failed to spawn ambient");
    let stdout = strip_ansi(&String::from_utf8_lossy(&output.stdout));
    let stderr = strip_ansi(&String::from_utf8_lossy(&output.stderr));
    assert!(
        output.status.success(),
        "example {} failed:\nstdout:\n{stdout}\nstderr:\n{stderr}",
        dir.display()
    );
    stdout
}

#[test]
fn examples_match_expected_output() {
    let bless = std::env::var("BLESS").is_ok();
    let mut checked = 0;
    let mut failures = Vec::new();

    let mut entries: Vec<_> = fs::read_dir(examples_dir())
        .expect("read examples dir")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.join("ambient.toml").exists())
        .collect();
    entries.sort();

    for dir in entries {
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        if PAIRED_EXAMPLES.contains(&name.as_str()) {
            continue;
        }

        let actual = run_example(&dir);
        let golden_path = dir.join("expected_output.txt");

        if bless {
            fs::write(&golden_path, &actual).expect("write golden");
            checked += 1;
            continue;
        }

        let Ok(expected) = fs::read_to_string(&golden_path) else {
            failures.push(format!(
                "{name}: missing expected_output.txt (run with BLESS=1 to create)"
            ));
            continue;
        };

        if actual != expected {
            failures.push(format!(
                "{name}: output mismatch\n--- expected ---\n{expected}\n--- actual ---\n{actual}"
            ));
        }
        checked += 1;
    }

    assert!(
        failures.is_empty(),
        "{} example(s) failed:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
    assert!(
        checked > 10,
        "expected to check many examples, got {checked}"
    );
}

/// A child process that is killed when dropped, even if the test panics.
struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn network_pair_echoes() {
    // No TCP probe here: the echo server serves exactly one connection, and
    // a probe would be accepted (and its disconnect read as the quit
    // signal). The client retries until the server is up instead.
    let server = Command::new(ambient_bin())
        .arg("run")
        .arg(examples_dir().join("network_server"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn server");
    let _server = KillOnDrop(server);

    // Retry the client until the server is up (compile + bind takes time).
    let deadline = Instant::now() + Duration::from_secs(30);
    let stdout = loop {
        let output = Command::new(ambient_bin())
            .arg("run")
            .arg(examples_dir().join("network_client"))
            .output()
            .expect("run client");
        if output.status.success() {
            break strip_ansi(&String::from_utf8_lossy(&output.stdout));
        }
        assert!(
            Instant::now() < deadline,
            "client never connected:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        std::thread::sleep(Duration::from_millis(200));
    };

    assert!(
        stdout.contains("Client: received 19 bytes back"),
        "echo round-trip missing from client output:\n{stdout}"
    );
    assert!(
        stdout.contains("Client: done"),
        "client did not finish:\n{stdout}"
    );
}

#[test]
fn remote_pair_executes() {
    let server = Command::new(ambient_bin())
        .arg("run")
        .arg(examples_dir().join("remote_server"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn server");
    let mut server = KillOnDrop(server);

    // Retry the client until the server is up.
    let deadline = Instant::now() + Duration::from_secs(30);
    let stdout = loop {
        let output = Command::new(ambient_bin())
            .arg("run")
            .arg(examples_dir().join("remote_client"))
            .output()
            .expect("run client");
        if output.status.success() {
            break strip_ansi(&String::from_utf8_lossy(&output.stdout));
        }
        assert!(
            Instant::now() < deadline,
            "remote client never connected:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        std::thread::sleep(Duration::from_millis(200));
    };

    // The client sends `double` (x * 2, logging via the Log ability) with
    // arg 21 for remote execution.
    assert!(
        stdout.contains("42"),
        "remote execution result missing from client output:\n{stdout}"
    );

    // Surface server-side errors if the assertion above ever fails silently.
    let _ = server.0.kill();
    let mut server_err = String::new();
    if let Some(stderr) = server.0.stderr.as_mut() {
        let _ = stderr.read_to_string(&mut server_err);
    }
    assert!(
        !server_err.contains("Runtime error"),
        "server reported a runtime error:\n{server_err}"
    );

    // The shipped function performs Log; the server's host granted Log to
    // executed code, so the log line must appear on the SERVER's output.
    let mut server_out = String::new();
    if let Some(out) = server.0.stdout.as_mut() {
        let _ = out.read_to_string(&mut server_out);
    }
    assert!(
        server_out.contains("doubling remotely"),
        "remote Log perform missing from server output:\n{server_out}"
    );
}
