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
/// plus long-lived services (`live_server`, `live_site`) that never
/// exit and so can't be run as a one-shot smoke test. `live_site` is
/// exercised under the dev loop in [`live_site_upgrades_under_the_dev_loop`].
const PAIRED_EXAMPLES: &[&str] = &[
    "network_client",
    "network_server",
    "remote_client",
    "remote_server",
    "deploy_client",
    "deploy_server",
    "live_server",
    "live_site",
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

/// One HTTP request to the live site; `None` until the server answers.
fn http_get(port: u16, path: &str) -> Option<String> {
    use std::io::Write;
    let addr = format!("127.0.0.1:{port}");
    let mut stream = std::net::TcpStream::connect(&addr).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    stream
        .write_all(format!("GET {path} HTTP/1.0\r\n\r\n").as_bytes())
        .ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    (!response.is_empty()).then_some(response)
}

/// Poll `http_get` until the response satisfies `accept` (the dev loop
/// compiles and deploys asynchronously) or the deadline trips; the
/// timeout panic quotes the dev loop's own narration from `log`.
fn await_response(
    port: u16,
    path: &str,
    accept: impl Fn(&str) -> bool,
    what: &str,
    log: &Path,
) -> String {
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut last = None;
    loop {
        if let Some(response) = http_get(port, path) {
            if accept(&response) {
                return response;
            }
            last = Some(response);
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what} from the live site\nlast response: {last:?}\ndev loop said:\n{}",
            fs::read_to_string(log).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// The hit counter in a `live_site` response (`... hit #7</p> ...`).
fn hit_number(response: &str) -> Option<u32> {
    let (_, rest) = response.split_once("hit #")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Phase 6's exit criteria, end to end: `examples/live_site` runs under
/// `ambient dev`, an edited handler lands on the next request, cell
/// state survives the deploy, and a task the entry stops declaring is
/// drained through its cleanup arm.
#[test]
fn live_site_upgrades_under_the_dev_loop() {
    // Work on a copy (the test edits source), on a port of its own so
    // a developer's `ambient dev examples/live_site` can coexist.
    let tmp = tempfile::tempdir().expect("temp dir");
    let site = tmp.path().join("live_site");
    copy_dir(&examples_dir().join("live_site"), &site);
    let port: u16 = 7899;
    let main_ab = site.join("main.ab");
    let rewritten = fs::read_to_string(&main_ab)
        .expect("read main.ab")
        .replace("7878", &port.to_string());
    fs::write(&main_ab, rewritten).expect("rewrite port");

    // Capture both streams to files: a piped-but-unread stderr would
    // eventually block the dev loop, and files let assertions quote the
    // loop's narration on failure.
    let out_path = tmp.path().join("dev.out");
    let err_path = tmp.path().join("dev.err");
    let out_file = fs::File::create(&out_path).expect("create stdout capture");
    let err_file = fs::File::create(&err_path).expect("create stderr capture");
    let dev = Command::new(ambient_bin())
        .arg("dev")
        .arg(&site)
        .stdout(Stdio::from(out_file))
        .stderr(Stdio::from(err_file))
        .spawn()
        .expect("spawn ambient dev");
    let _dev = KillOnDrop(dev);

    // Generation one serves, and the stats cell counts.
    let first = await_response(
        port,
        "/",
        |r| r.contains("hello from generation one"),
        "gen 1",
        &err_path,
    );
    let hits_before = hit_number(&first).expect("hit counter in the page");

    // Edit the handler while it serves: the next request must pick up
    // the rebinding, and the hit count must survive the deploy.
    let handlers = site.join("handlers.ab");
    let edited = fs::read_to_string(&handlers)
        .expect("read handlers.ab")
        .replace("hello from generation one", "hello from generation two");
    fs::write(&handlers, edited).expect("edit handlers.ab");
    let second = await_response(
        port,
        "/",
        |r| r.contains("hello from generation two"),
        "gen 2",
        &err_path,
    );
    let hits_after = hit_number(&second).expect("hit counter in the page");
    assert!(
        hits_after > hits_before,
        "the stats cell must survive the deploy (was {hits_before}, now {hits_after})"
    );

    // Stop declaring the ticker: the reconciler drains it, its
    // Drain::requested arm runs, and the goodbye lands on stdout.
    let undeclared = fs::read_to_string(&main_ab)
        .expect("read main.ab")
        .replace("Task::ensure!(\"ticker\", ticker::ticker)", "()");
    fs::write(&main_ab, undeclared).expect("edit main.ab");
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let stdout = fs::read_to_string(&out_path).unwrap_or_default();
        if stdout.contains("ticker: drained, goodbye") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the undeclared ticker never drained; stdout so far:\n{stdout}\ndev loop said:\n{}",
            fs::read_to_string(&err_path).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(200));
    }

    // The site is still serving after the drain.
    await_response(
        port,
        "/",
        |r| r.contains("hit #"),
        "post-drain traffic",
        &err_path,
    );
}

/// Recursively copy an example directory (skipping any local store).
fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("create copy target");
    for entry in fs::read_dir(from).expect("read source dir") {
        let entry = entry.expect("dir entry");
        let name = entry.file_name();
        if name == ".ambient" {
            continue;
        }
        let target = to.join(&name);
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).expect("copy file");
        }
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

/// One length-prefixed message out (the platform's framed `send`).
fn send_frame(conn: &mut std::net::TcpStream, data: &[u8]) {
    use std::io::Write;
    let len = u32::try_from(data.len()).expect("frame fits");
    conn.write_all(&len.to_be_bytes())
        .expect("write frame length");
    conn.write_all(data).expect("write frame");
}

/// One length-prefixed message in.
fn recv_frame(conn: &mut std::net::TcpStream) -> Vec<u8> {
    let mut len_buf = [0u8; 4];
    conn.read_exact(&mut len_buf).expect("read frame length");
    let mut buf = vec![0u8; u32::from_be_bytes(len_buf) as usize];
    conn.read_exact(&mut buf).expect("read frame");
    buf
}

/// Phase 9's exit criteria, end to end: a running remote service is
/// upgraded mid-conversation — over TCP, through `Deploy::apply!`, no
/// dev loop anywhere — and its cell state survives the deploy.
#[test]
fn deploy_pair_upgrades_a_remote_service_mid_conversation() {
    // Private ports so a developer's own `ambient run examples/deploy_server`
    // (on the 7910/7911 defaults) can coexist with the test.
    let service_port = "7913";
    let deploy_port = "7914";

    // Build generation two: the same package with a new greeting,
    // compiled to a shippable artifact pack.
    let tmp = tempfile::tempdir().expect("tempdir");
    let v2_src = tmp.path().join("v2");
    copy_dir(&examples_dir().join("deploy_server"), &v2_src);
    let handlers = v2_src.join("handlers.ab");
    let source = fs::read_to_string(&handlers).expect("read handlers.ab");
    fs::write(
        &handlers,
        source.replace("generation one", "generation two"),
    )
    .expect("write handlers.ab");
    let pack = tmp.path().join("v2.ambient");
    let compile = Command::new(ambient_bin())
        .arg("compile")
        .arg(&v2_src)
        .arg("-o")
        .arg(&pack)
        .output()
        .expect("compile v2 pack");
    assert!(
        compile.status.success(),
        "compiling the v2 pack failed:\n{}",
        String::from_utf8_lossy(&compile.stderr)
    );

    // Start generation one, plainly (`ambient run`, not `dev`).
    let server = Command::new(ambient_bin())
        .arg("run")
        .arg(examples_dir().join("deploy_server"))
        .env("SERVICE_PORT", service_port)
        .env("DEPLOY_PORT", deploy_port)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn server");
    let mut server = KillOnDrop(server);

    // The client holds one conversation and upgrades the server in the
    // middle of it. Retry until the server is up.
    let deadline = Instant::now() + Duration::from_secs(30);
    let stdout = loop {
        let output = Command::new(ambient_bin())
            .arg("run")
            .arg(examples_dir().join("deploy_client"))
            .env("SERVICE_PORT", service_port)
            .env("DEPLOY_PORT", deploy_port)
            .env("DEPLOY_PACK", &pack)
            .output()
            .expect("run client");
        if output.status.success() {
            break strip_ansi(&String::from_utf8_lossy(&output.stdout));
        }
        assert!(
            Instant::now() < deadline,
            "deploy client never connected:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        std::thread::sleep(Duration::from_millis(200));
    };

    // Request one answered by generation one; request two — on the same
    // connection — by generation two, with the hit counter carried
    // across the deploy (the cell never left the runtime).
    assert!(
        stdout.contains("reply: generation one: hello #1"),
        "first reply missing:\n{stdout}"
    );
    assert!(
        stdout.contains("reply: generation two: hello again #2"),
        "post-deploy reply missing:\n{stdout}"
    );
    // The server's report names the rebound handler.
    assert!(
        stdout.contains("rebound: workspace::deploy_server::handlers::respond"),
        "deploy report missing the rebinding:\n{stdout}"
    );

    // A malformed pack is rejected as the perform's Err value — the
    // server answers and keeps running, nothing faults.
    let mut conn = std::net::TcpStream::connect(format!("127.0.0.1:{deploy_port}"))
        .expect("connect to deploy port");
    send_frame(&mut conn, b"not a pack");
    let rejection = String::from_utf8_lossy(&recv_frame(&mut conn)).into_owned();
    assert!(
        rejection.starts_with("rejected: invalid generation pack"),
        "malformed pack should be rejected: {rejection}"
    );
    drop(conn);

    // And the service still serves, from generation two, still counting.
    let mut conn = std::net::TcpStream::connect(format!("127.0.0.1:{service_port}"))
        .expect("connect to service port");
    send_frame(&mut conn, b"still there?");
    let reply = String::from_utf8_lossy(&recv_frame(&mut conn)).into_owned();
    assert_eq!(
        reply, "generation two: still there? #3",
        "service should keep serving after a rejected deploy"
    );
    drop(conn);

    let _ = server.0.kill();
    let mut server_err = String::new();
    if let Some(stderr) = server.0.stderr.as_mut() {
        let _ = stderr.read_to_string(&mut server_err);
    }
    assert!(
        !server_err.contains("Runtime error"),
        "server reported a runtime error:\n{server_err}"
    );
    let mut server_out = String::new();
    if let Some(out) = server.0.stdout.as_mut() {
        let _ = out.read_to_string(&mut server_out);
    }
    assert!(
        server_out.contains("deploy applied") && server_out.contains("deploy rejected"),
        "server should narrate both deploy outcomes:\n{server_out}"
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
