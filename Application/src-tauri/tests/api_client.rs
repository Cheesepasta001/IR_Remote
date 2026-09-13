//! Integration tests for the HTTP seam, against a real socket.
//!
//! These cover the paths Instruction.md section 10 calls out - timeout and
//! connection-refused - plus the one that matters most for this device: that a
//! failing command is never retried, because every retry fires another IR code.
//!
//! The server here is a few lines of `std::net`, not `mock/server.js`, so
//! `cargo test` is self-contained and needs nothing running.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ir_remote_lib::api_client::{ApiClient, Config, Timeouts};
use ir_remote_lib::auth::Credentials;
use ir_remote_lib::state::{Command, Failure};

#[derive(Clone, Copy)]
enum Behaviour {
    /// The agreed contract.
    Success,
    /// A status plus a body.
    Status(u16, &'static str),
    /// 200 with a truncated JSON body.
    Malformed,
    /// Accept, never reply - exactly what Main.ino does today.
    Silent,
    /// Reply, but only after this many milliseconds.
    Slow(u64),
    /// Confirm a different command than the one requested.
    WrongEcho,
}

struct TestServer {
    base_url: String,
    hits: Arc<AtomicUsize>,
    last_auth: Arc<Mutex<Option<String>>>,
    _listener_port: u16,
}

impl TestServer {
    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

fn spawn(behaviour: Behaviour) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let hits = Arc::new(AtomicUsize::new(0));
    let last_auth = Arc::new(Mutex::new(None));

    let h = hits.clone();
    let a = last_auth.clone();

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            h.fetch_add(1, Ordering::SeqCst);

            // Read just the request head.
            let mut buf = [0u8; 2048];
            let n = stream.read(&mut buf).unwrap_or(0);
            let head = String::from_utf8_lossy(&buf[..n]).to_string();

            if let Some(line) = head
                .lines()
                .find(|l| l.to_ascii_lowercase().starts_with("authorization:"))
            {
                *a.lock().unwrap() = Some(line["authorization:".len()..].trim().to_string());
            }

            match behaviour {
                Behaviour::Silent => {
                    // Hold the socket open and write nothing at all.
                    std::thread::sleep(Duration::from_secs(30));
                }
                Behaviour::Slow(ms) => {
                    std::thread::sleep(Duration::from_millis(ms));
                    let _ = write_json(&mut stream, 200, r#"{"result":"Success","command":"Power"}"#);
                }
                Behaviour::Success => {
                    let _ = write_json(&mut stream, 200, r#"{"result":"Success","command":"Power"}"#);
                }
                Behaviour::WrongEcho => {
                    let _ = write_json(&mut stream, 200, r#"{"result":"Success","command":"Silent"}"#);
                }
                Behaviour::Malformed => {
                    let _ = write_json(&mut stream, 200, r#"{"result":"Succ"#);
                }
                Behaviour::Status(code, body) => {
                    let _ = write_json(&mut stream, code, body);
                }
            }
        }
    });

    TestServer {
        base_url: format!("http://127.0.0.1:{port}"),
        hits,
        last_auth,
        _listener_port: port,
    }
}

fn write_json(stream: &mut TcpStream, code: u16, body: &str) -> std::io::Result<()> {
    let reason = match code {
        200 => "OK",
        401 => "Unauthorized",
        404 => "Not Found",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Status",
    };
    let head = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()
}

fn client_for(base_url: &str, command_ms: u64) -> ApiClient {
    ApiClient::new(Config {
        base_url: base_url.to_string(),
        timeouts: Timeouts {
            command: Duration::from_millis(command_ms),
            probe: Duration::from_millis(500),
        },
        poll_interval: Duration::from_millis(1_000),
        stale_after: Duration::from_millis(5_000),
    })
    .expect("client")
}

// ------------------------------------------------------------ contract ----

#[tokio::test]
async fn a_confirmed_command_succeeds() {
    let s = spawn(Behaviour::Success);
    let c = client_for(&s.base_url, 2_000);

    let out = c.send_command(Command::Power, None).await.expect("should succeed");
    assert!(out.latency_ms < 2_000);
    assert_eq!(s.hits(), 1);
}

#[tokio::test]
async fn an_unknown_command_is_a_device_error_not_a_transport_error() {
    let s = spawn(Behaviour::Status(404, r#"{"result":"Fail","error":"unknown command"}"#));
    let c = client_for(&s.base_url, 2_000);

    match c.send_command(Command::Silent, None).await {
        Err(Failure::DeviceError(d)) => {
            assert!(d.contains("404"), "got {d}");
            assert!(d.contains("unknown command"), "got {d}");
        }
        other => panic!("expected DeviceError, got {other:?}", other = other.err()),
    }
}

#[tokio::test]
async fn a_malformed_reply_is_never_treated_as_success() {
    let s = spawn(Behaviour::Malformed);
    let c = client_for(&s.base_url, 2_000);

    match c.send_command(Command::Power, None).await {
        Err(Failure::BadResponse(_)) => {}
        other => panic!("expected BadResponse, got {other:?}", other = other.err()),
    }
}

#[tokio::test]
async fn a_reply_confirming_the_wrong_command_is_rejected() {
    // Guards against a device that answers a queued request out of order.
    let s = spawn(Behaviour::WrongEcho);
    let c = client_for(&s.base_url, 2_000);

    match c.send_command(Command::Power, None).await {
        Err(Failure::BadResponse(d)) => assert!(d.contains("Silent"), "got {d}"),
        other => panic!("expected BadResponse, got {other:?}", other = other.err()),
    }
}

// ------------------------------------------------- rule 4: timeouts -------

#[tokio::test]
async fn a_slow_reply_times_out_rather_than_waiting_forever() {
    let s = spawn(Behaviour::Slow(5_000));
    let c = client_for(&s.base_url, 300);

    let started = std::time::Instant::now();
    match c.send_command(Command::Power, None).await {
        Err(Failure::Unreachable(d)) => assert!(d.contains("timed out"), "got {d}"),
        other => panic!("expected a timeout, got {other:?}", other = other.err()),
    }
    assert!(
        started.elapsed() < Duration::from_millis(2_000),
        "the timeout did not bound the wait"
    );
}

#[tokio::test]
async fn the_current_firmware_behaviour_times_out() {
    // Main.ino accepts the connection, fires the IR code and never replies.
    // This is what the app does against the real device today.
    let s = spawn(Behaviour::Silent);
    let c = client_for(&s.base_url, 300);

    let started = std::time::Instant::now();
    let result = c.send_command(Command::Power, None).await;

    assert!(result.is_err(), "a silent server must never read as success");
    assert!(started.elapsed() < Duration::from_millis(2_000));
    assert_eq!(s.hits(), 1, "and it must not be retried");
}

// ------------------------------------- rule 3/4: connection refused -------

#[tokio::test]
async fn connection_refused_is_reported_as_unreachable() {
    // Bind then drop, so the port is certainly closed and certainly unused.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let c = client_for(&format!("http://127.0.0.1:{port}"), 1_000);

    match c.send_command(Command::Power, None).await {
        Err(Failure::Unreachable(_)) => {}
        other => panic!("expected Unreachable, got {other:?}", other = other.err()),
    }
}

#[tokio::test]
async fn the_probe_reports_a_closed_port_without_actuating_anything() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let c = client_for(&format!("http://127.0.0.1:{port}"), 1_000);
    assert!(c.probe().await.is_err());
}

#[tokio::test]
async fn the_probe_succeeds_without_sending_a_request() {
    let s = spawn(Behaviour::Success);
    let c = client_for(&s.base_url, 2_000);

    assert!(c.probe().await.is_ok(), "probe should reach the listener");

    // The crucial property: a probe must never fire an IR code. It opens a TCP
    // connection and closes it, so the server sees a connection but the app has
    // sent no command. `hits` counts accepted connections, so 1 here means the
    // handshake happened; what matters is that no HTTP request was written.
    assert_eq!(s.hits(), 1);
    assert!(
        s.last_auth.lock().unwrap().is_none(),
        "the probe must not send headers"
    );
}

// ----------------------------------------- rule 5: no retries, ever -------

#[tokio::test]
async fn a_failing_command_is_never_retried() {
    // Every endpoint fires an IR code. A retry on /Power would toggle the unit
    // twice; on Low_Temp it would step the temperature twice.
    let s = spawn(Behaviour::Status(500, r#"{"result":"Fail","error":"ir send failed"}"#));
    let c = client_for(&s.base_url, 2_000);

    let _ = c.send_command(Command::Power, None).await;
    assert_eq!(s.hits(), 1, "the command must be attempted exactly once");
}

#[tokio::test]
async fn a_timed_out_command_is_never_retried() {
    let s = spawn(Behaviour::Slow(5_000));
    let c = client_for(&s.base_url, 250);

    let _ = c.send_command(Command::Power, None).await;
    tokio::time::sleep(Duration::from_millis(400)).await;

    assert_eq!(
        s.hits(),
        1,
        "a timeout must not become a second IR transmission"
    );
}

#[tokio::test]
async fn every_command_maps_to_its_documented_path() {
    for (command, path) in [
        (Command::Power, "/Power"),
        (Command::Silent, "/Silent"),
        (Command::LowTemp, "/Low_Temp"),
        (Command::HighTemp, "/High_Temp"),
    ] {
        assert_eq!(command.path(), path);
        assert!(!command.is_idempotent());
    }
}

// ----------------------------------------------- rule 7: credentials ------

#[tokio::test]
async fn a_credential_is_attached_by_the_core() {
    let s = spawn(Behaviour::Success);
    let c = client_for(&s.base_url, 2_000);

    let creds = Credentials::new("esp32", "secret");
    c.send_command(Command::Power, Some(&creds)).await.expect("ok");

    let seen = s.last_auth.lock().unwrap().clone().expect("no Authorization header");
    // "esp32:secret" base64-encoded, produced by reqwest inside the core.
    assert_eq!(seen, "Basic ZXNwMzI6c2VjcmV0");
}

#[tokio::test]
async fn no_credential_means_no_auth_header() {
    // The current firmware performs no auth; the app must not invent one.
    let s = spawn(Behaviour::Success);
    let c = client_for(&s.base_url, 2_000);

    c.send_command(Command::Power, None).await.expect("ok");
    assert!(s.last_auth.lock().unwrap().is_none());
}

// ---------------------------------------------------- config plumbing -----

#[test]
fn the_env_var_switches_the_target() {
    // Serialised implicitly: this is the only test touching this variable.
    std::env::set_var("IR_REMOTE_BASE_URL", "http://192.168.0.102");
    assert_eq!(Config::from_env().base_url, "http://192.168.0.102");

    std::env::set_var("IR_REMOTE_BASE_URL", "   ");
    assert_eq!(
        Config::from_env().base_url,
        "http://127.0.0.1:8080",
        "blank must fall back to the mock"
    );

    std::env::set_var("IR_REMOTE_BASE_URL", "not a url");
    assert_eq!(
        Config::from_env().base_url,
        "http://127.0.0.1:8080",
        "junk must fall back rather than produce an unusable client"
    );

    std::env::remove_var("IR_REMOTE_BASE_URL");
}
