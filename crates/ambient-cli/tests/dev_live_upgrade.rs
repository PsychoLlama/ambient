//! End-to-end test of `ambient dev`'s live upgrade.
//!
//! Copies `examples/live_server` to a temp dir, runs it under the dev
//! loop, talks to it over TCP, edits the source mid-run, and verifies
//! the deploy hot-swapped the `stats` reducer while keeping its state
//! (the served-messages count continues instead of resetting).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Port distinct from the example's default so a developer's own
/// running instance never collides with the test.
const PORT: u16 = 7877;

struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Collect a child stream's lines into a shared buffer.
fn collect_lines(stream: impl Read + Send + 'static, into: Arc<Mutex<String>>) {
    std::thread::spawn(move || {
        let reader = BufReader::new(stream);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            let mut buf = into.lock().expect("log lock");
            buf.push_str(&line);
            buf.push('\n');
        }
    });
}

fn wait_for(log: &Arc<Mutex<String>>, needle: &str, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if log.lock().expect("log lock").contains(needle) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}; log so far:\n{}",
            log.lock().expect("log lock")
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn send_frame(stream: &mut TcpStream, payload: &[u8]) {
    #[allow(clippy::cast_possible_truncation)]
    let len = (payload.len() as u32).to_be_bytes();
    stream.write_all(&len).expect("write frame length");
    stream.write_all(payload).expect("write frame payload");
}

fn recv_frame(stream: &mut TcpStream) -> Vec<u8> {
    let mut len = [0u8; 4];
    stream.read_exact(&mut len).expect("read frame length");
    let mut buf = vec![0u8; u32::from_be_bytes(len) as usize];
    stream.read_exact(&mut buf).expect("read frame payload");
    buf
}

fn connect() -> TcpStream {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Ok(stream) = TcpStream::connect(("127.0.0.1", PORT)) {
            stream
                .set_read_timeout(Some(Duration::from_secs(30)))
                .expect("set timeout");
            return stream;
        }
        assert!(Instant::now() < deadline, "server never came up");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn dev_loop_hot_swaps_processes_keeping_state() {
    // Stage the example in a temp dir we can edit, on a test-only port.
    let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/live_server");
    let dir = tempfile::tempdir().expect("temp dir");
    let source = std::fs::read_to_string(example.join("main.ab")).expect("read example");
    let source = source.replace("127.0.0.1:7777", &format!("127.0.0.1:{PORT}"));
    std::fs::write(dir.path().join("main.ab"), &source).expect("stage main.ab");
    std::fs::copy(
        example.join("ambient.toml"),
        dir.path().join("ambient.toml"),
    )
    .expect("stage manifest");

    let mut child = Command::new(env!("CARGO_BIN_EXE_ambient"))
        .arg("dev")
        .arg(dir.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn ambient dev");
    let stdout_log = Arc::new(Mutex::new(String::new()));
    let stderr_log = Arc::new(Mutex::new(String::new()));
    collect_lines(
        child.stdout.take().expect("stdout"),
        Arc::clone(&stdout_log),
    );
    collect_lines(
        child.stderr.take().expect("stderr"),
        Arc::clone(&stderr_log),
    );
    let _child = KillOnDrop(child);

    // Round-trip two messages through the v1 tree.
    let mut conn = connect();
    send_frame(&mut conn, b"one");
    assert_eq!(recv_frame(&mut conn), b"one");
    send_frame(&mut conn, b"two");
    assert_eq!(recv_frame(&mut conn), b"two");
    wait_for(&stdout_log, "served 2 messages", "v1 stats output");

    // Edit the stats reducer mid-run and wait for the hot swap.
    let upgraded = source.replace("served ${", "SERVED-V2 ${");
    assert_ne!(upgraded, source, "edit must change the source");
    std::fs::write(dir.path().join("main.ab"), upgraded).expect("edit main.ab");
    wait_for(&stderr_log, "process `stats` upgraded", "the stats upgrade");

    // The same connection keeps working, and the swapped reducer
    // continues from the previous count — state survived the deploy.
    send_frame(&mut conn, b"three");
    assert_eq!(recv_frame(&mut conn), b"three");
    wait_for(&stdout_log, "SERVED-V2 3 messages", "v2 stats output");

    // The acceptor didn't change; content-hash diffing must leave it be.
    let stderr = stderr_log.lock().expect("log lock").clone();
    assert!(
        !stderr.contains("process `acceptor` upgraded"),
        "acceptor was upgraded despite identical code:\n{stderr}"
    );

    send_frame(&mut conn, b""); // polite close
}
