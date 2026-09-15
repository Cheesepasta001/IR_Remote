//! All HTTP lives here. The frontend never constructs a URL and never sees a
//! credential (rule 7).
//!
//! Retry policy (rule 5): commands are NEVER retried. Every endpoint on this
//! device fires an IR code that mutates physical state - `/Power` is a toggle -
//! and the API documents no idempotency. Only the reachability probe, which
//! actuates nothing, is allowed to retry.

use std::time::{Duration, Instant};

use serde::Deserialize;
use tokio::net::TcpStream;

use crate::auth::Credentials;
use crate::state::{Command, Failure};

/// Rule 4: every request has an explicit timeout. No unbounded waits anywhere.
#[derive(Debug, Clone)]
pub struct Timeouts {
    pub command: Duration,
    pub probe: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Timeouts {
            // Instruction.md rule 4: 2-3s for a command, shorter for a poll.
            command: Duration::from_millis(3_000),
            probe: Duration::from_millis(1_500),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    /// e.g. "http://192.168.0.102" or "http://127.0.0.1:8080" for the mock.
    pub base_url: String,
    pub timeouts: Timeouts,
    pub poll_interval: Duration,
    pub stale_after: Duration,
}

/// The mock, unless told otherwise. See `Config::from_env`.
pub const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8080";

/// One-step switch between the mock and the real device (see README).
pub const BASE_URL_ENV: &str = "IR_REMOTE_BASE_URL";

/// Where the base URL came from, so the UI can say so instead of implying the
/// value is editable when it is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BaseUrlSource {
    /// Saved in the app's own storage by the user. Android only - an APK has no
    /// `.env` beside it, so the address has to be settable in the app.
    Stored,
    /// A process environment variable.
    Environment,
    /// A `.env` file next to the app. Desktop only in practice.
    DotEnv,
    /// Compiled in from `IR_REMOTE_BASE_URL` at build time.
    Compiled,
    /// Nothing configured; the built-in mock address.
    Default,
}

/// The value of `IR_REMOTE_BASE_URL` when this binary was COMPILED.
///
/// This is how an APK gets a sensible default: there is no `.env` inside an
/// installed app, so the build bakes one in. `build.rs` declares a
/// `rerun-if-env-changed` so changing it actually triggers a rebuild.
const COMPILED_BASE_URL: Option<&str> = option_env!("IR_REMOTE_BASE_URL");

impl Config {
    /// Read the target, in priority order:
    ///
    /// 1. `stored` - what the user saved in the app (Android)
    /// 2. the process environment
    /// 3. a `.env` file beside the app (desktop)
    /// 4. the value compiled in at build time
    /// 5. the built-in default, the mock
    ///
    /// An unparseable value at any level is reported and skipped rather than
    /// silently accepted: pointing the app at an address it cannot resolve
    /// should be loud, not a mystery timeout later.
    pub fn resolve(stored: Option<String>) -> (Config, BaseUrlSource) {
        let mut config = Config::default();

        for (candidate, origin, source) in [
            (stored, "saved setting", BaseUrlSource::Stored),
            (
                std::env::var(BASE_URL_ENV).ok(),
                "environment",
                BaseUrlSource::Environment,
            ),
            (dot_env_value(BASE_URL_ENV), ".env", BaseUrlSource::DotEnv),
            (
                COMPILED_BASE_URL.map(str::to_string),
                "build",
                BaseUrlSource::Compiled,
            ),
        ] {
            if let Some(v) = usable(candidate, origin) {
                config.base_url = v;
                return (config, source);
            }
        }

        (config, BaseUrlSource::Default)
    }

    /// No saved override - the desktop path, where `.env` is the source of truth.
    pub fn from_env() -> (Config, BaseUrlSource) {
        Config::resolve(None)
    }
}

/// Whether the address is settable from inside the app.
///
/// True on Android, where there is no `.env` to edit and no shell to set an
/// environment variable in. False on desktop, where `.env` deliberately remains
/// the single source of truth.
pub const fn base_url_editable() -> bool {
    cfg!(target_os = "android")
}

fn usable(raw: Option<String>, origin: &str) -> Option<String> {
    let value = raw?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    match host_port(trimmed) {
        Ok(_) => Some(trimmed.to_string()),
        Err(e) => {
            eprintln!("{BASE_URL_ENV} from {origin} ignored: {e}");
            None
        }
    }
}

/// Look for `.env` beside the working directory, its parent, and the
/// executable. `tauri dev` runs from `src-tauri/`, while a release binary runs
/// from wherever it was installed, so all three are worth checking.
fn dot_env_candidates() -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        paths.push(cwd.join(".env"));
        if let Some(parent) = cwd.parent() {
            paths.push(parent.join(".env"));
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            paths.push(dir.join(".env"));
        }
    }
    paths
}

fn dot_env_value(key: &str) -> Option<String> {
    for path in dot_env_candidates() {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Some(v) = parse_dot_env(&text, key) {
            return Some(v);
        }
    }
    None
}

/// Minimal `.env` parsing: `KEY=value`, `#` comments, optional surrounding
/// quotes, `export ` prefix tolerated. Deliberately not a full dotenv
/// implementation - one key is read and nothing is exported into the process.
fn parse_dot_env(text: &str, key: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        if k.trim() != key {
            continue;
        }
        let v = v.trim();
        // Strip one layer of matching quotes, then any trailing comment.
        let v = if (v.starts_with('"') && v.ends_with('"') && v.len() >= 2)
            || (v.starts_with('\'') && v.ends_with('\'') && v.len() >= 2)
        {
            &v[1..v.len() - 1]
        } else {
            v.split('#').next().unwrap_or(v).trim()
        };
        return Some(v.to_string());
    }
    None
}

impl Default for Config {
    fn default() -> Self {
        Config {
            base_url: DEFAULT_BASE_URL.to_string(),
            timeouts: Timeouts::default(),
            poll_interval: Duration::from_millis(2_000),
            stale_after: Duration::from_millis(5_000),
        }
    }
}

/// The agreed success body: {"result":"Success","command":"Power"}
#[derive(Debug, Deserialize)]
struct DeviceReply {
    result: String,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

pub struct CommandOutcome {
    pub latency_ms: u64,
}

/// Cheap to clone - `reqwest::Client` is an Arc internally. Callers clone it out
/// of the shared lock so the lock is never held across an await.
#[derive(Clone)]
pub struct ApiClient {
    http: reqwest::Client,
    config: Config,
}

impl ApiClient {
    pub fn new(config: Config) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            // A belt-and-braces ceiling; each request also sets its own.
            .timeout(config.timeouts.command)
            .connect_timeout(config.timeouts.probe)
            // The device speaks HTTP/1.1 and closes the socket; pooling a
            // connection to an ESP32 that serves one client at a time causes
            // more trouble than it saves.
            .pool_max_idle_per_host(0)
            .build()
            .map_err(|e| format!("could not build http client: {e}"))?;
        Ok(ApiClient { http, config })
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn set_config(&mut self, config: Config) -> Result<(), String> {
        *self = ApiClient::new(config)?;
        Ok(())
    }

    /// Fire one command. Exactly once - no retry, ever. See the module note.
    pub async fn send_command(
        &self,
        command: Command,
        creds: Option<&Credentials>,
    ) -> Result<CommandOutcome, Failure> {
        debug_assert!(
            !command.is_idempotent(),
            "if a command ever becomes idempotent, revisit the retry policy"
        );

        let url = format!("{}{}", self.config.base_url.trim_end_matches('/'), command.path());
        let started = Instant::now();

        let mut req = self.http.get(&url).timeout(self.config.timeouts.command);
        if let Some(c) = creds {
            // The credential is attached here, in the core, and never crosses IPC.
            req = req.basic_auth(c.username(), Some(c.password()));
        }

        let response = req.send().await.map_err(classify_transport)?;
        let status = response.status();

        let body = response
            .text()
            .await
            .map_err(|e| Failure::BadResponse(format!("could not read body: {}", redact(&e.to_string()))))?;

        let latency_ms = started.elapsed().as_millis() as u64;

        if !status.is_success() {
            let detail = match serde_json::from_str::<DeviceReply>(&body) {
                Ok(r) => r.error.unwrap_or(r.result),
                Err(_) => truncate(&body),
            };
            return Err(Failure::DeviceError(format!("HTTP {} - {}", status.as_u16(), detail)));
        }

        match parse_body(&body, command)? {
            Verdict::Success => Ok(CommandOutcome { latency_ms }),
            Verdict::Failed(detail) => Err(Failure::DeviceError(detail)),
        }
    }

    /// Reachability probe.
    ///
    /// This is a bare TCP connect, NOT an HTTP request, and that is deliberate:
    /// the device exposes no readable state, and every HTTP path it does expose
    /// fires an IR code. Polling `/Power` to see if the device is alive would
    /// toggle the air conditioner every poll interval. A TCP handshake proves
    /// the device is up and actuates nothing.
    pub async fn probe(&self) -> Result<u64, String> {
        let addr = host_port(&self.config.base_url)?;
        let started = Instant::now();

        let attempt = tokio::time::timeout(self.config.timeouts.probe, TcpStream::connect(&addr)).await;

        match attempt {
            Ok(Ok(stream)) => {
                drop(stream);
                Ok(started.elapsed().as_millis() as u64)
            }
            Ok(Err(e)) => Err(e.to_string()),
            Err(_) => Err(format!(
                "no answer within {}ms",
                self.config.timeouts.probe.as_millis()
            )),
        }
    }
}

enum Verdict {
    Success,
    Failed(String),
}

/// Read a 2xx body from the device.
///
/// Two shapes are accepted, because the firmware and Instruction.md section 1
/// do not agree on one:
///
///  - the JSON contract, `{"result":"Success","command":"Power"}`
///  - a bare word, `success` or `fail`, which is what `Main.ino` writes today
///    via `client.println("success")`
///
/// Anything else is a `BadResponse` - it is never optimistically read as
/// success (rule 1). Matching is case-insensitive and ignores trailing CRLF,
/// since `println` appends one.
fn parse_body(body: &str, command: Command) -> Result<Verdict, Failure> {
    let trimmed = body.trim();

    // Shape 1: the JSON contract.
    if let Ok(reply) = serde_json::from_str::<DeviceReply>(trimmed) {
        if !reply.result.eq_ignore_ascii_case("success") {
            let detail = reply.error.unwrap_or(reply.result);
            return Ok(Verdict::Failed(detail));
        }
        // Guard against a reply that confirms a different command than we sent.
        if let Some(echo) = reply.command.as_deref() {
            let expected = command.path().trim_start_matches('/');
            if !echo.eq_ignore_ascii_case(expected) {
                return Err(Failure::BadResponse(format!(
                    "asked for {expected}, device confirmed {echo}"
                )));
            }
        }
        return Ok(Verdict::Success);
    }

    // Shape 2: a bare word. The firmware sends no command name with it, so
    // there is nothing to cross-check - see the note in README about what that
    // costs when more than one request is in flight.
    match trimmed.to_ascii_lowercase().as_str() {
        "success" => Ok(Verdict::Success),
        "fail" => Ok(Verdict::Failed("device reported 'fail'".to_string())),
        _ => Err(Failure::BadResponse(format!(
            "unrecognised reply: {}",
            truncate(trimmed)
        ))),
    }
}

fn classify_transport(e: reqwest::Error) -> Failure {
    if e.is_timeout() {
        Failure::Unreachable("timed out".to_string())
    } else if e.is_connect() {
        Failure::Unreachable(format!("connect failed: {}", redact(&e.to_string())))
    } else if e.is_decode() {
        Failure::BadResponse(redact(&e.to_string()))
    } else {
        Failure::Unreachable(redact(&e.to_string()))
    }
}

/// reqwest errors can carry the request URL. If a credential ever ends up in a
/// userinfo-style URL, this keeps it out of the log pane.
fn redact(msg: &str) -> String {
    let mut out = String::with_capacity(msg.len());
    for part in msg.split(' ') {
        if part.contains("://") && part.contains('@') {
            if let Some(scheme_end) = part.find("://") {
                if let Some(at) = part.rfind('@') {
                    out.push_str(&part[..scheme_end + 3]);
                    out.push_str("***@");
                    out.push_str(&part[at + 1..]);
                    out.push(' ');
                    continue;
                }
            }
        }
        out.push_str(part);
        out.push(' ');
    }
    out.trim_end().to_string()
}

fn truncate(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.len() <= 120 {
        trimmed.to_string()
    } else {
        format!("{}...", &trimmed[..120])
    }
}

/// Pull "host:port" out of a base URL, defaulting to port 80 - the port the
/// firmware binds with `NetworkServer server(80)`.
pub fn host_port(base_url: &str) -> Result<String, String> {
    let url = reqwest::Url::parse(base_url).map_err(|e| format!("bad base URL {base_url:?}: {e}"))?;
    let host = url.host_str().ok_or_else(|| format!("no host in {base_url:?}"))?;
    let port = url.port_or_known_default().unwrap_or(80);
    Ok(format!("{host}:{port}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_port_defaults_to_80() {
        assert_eq!(host_port("http://192.168.0.102").unwrap(), "192.168.0.102:80");
    }

    #[test]
    fn host_port_keeps_explicit_port() {
        assert_eq!(host_port("http://127.0.0.1:8080").unwrap(), "127.0.0.1:8080");
    }

    #[test]
    fn host_port_rejects_junk() {
        assert!(host_port("not a url").is_err());
    }

    #[test]
    fn timeouts_are_bounded_and_sane() {
        let t = Timeouts::default();
        // Rule 4: 2-3s for a command, shorter for a poll.
        assert!(t.command <= Duration::from_millis(3_000));
        assert!(t.probe < t.command);
    }

    #[test]
    fn redact_strips_userinfo() {
        let msg = "error sending request for url http://bob:hunter2@192.168.0.102/Power failed";
        let out = redact(msg);
        assert!(!out.contains("hunter2"), "password leaked into log: {out}");
        assert!(out.contains("192.168.0.102"));
    }

    #[test]
    fn redact_leaves_ordinary_messages_alone() {
        assert_eq!(redact("connect failed: refused"), "connect failed: refused");
    }

    fn verdict(body: &str) -> Result<bool, Failure> {
        parse_body(body, Command::Power).map(|v| matches!(v, Verdict::Success))
    }

    #[test]
    fn accepts_the_json_contract() {
        assert_eq!(verdict(r#"{"result":"Success","command":"Power"}"#).unwrap(), true);
    }

    #[test]
    fn accepts_the_firmwares_bare_word() {
        // Main.ino writes client.println("success") -> "success\r\n".
        assert_eq!(verdict("success\r\n").unwrap(), true);
        assert_eq!(verdict("SUCCESS").unwrap(), true);
    }

    #[test]
    fn a_bare_fail_is_a_device_error_not_a_success() {
        match parse_body("fail\r\n", Command::Power).unwrap() {
            Verdict::Failed(d) => assert!(d.contains("fail")),
            Verdict::Success => panic!("'fail' must never read as success"),
        }
    }

    #[test]
    fn an_unrecognised_body_is_never_success() {
        // Rule 1: anything we cannot positively read as confirmation is a
        // failure, not an optimistic pass.
        assert!(matches!(
            verdict("Sending Power Signal..."),
            Err(Failure::BadResponse(_))
        ));
        assert!(matches!(verdict(""), Err(Failure::BadResponse(_))));
        assert!(matches!(verdict("<html>404</html>"), Err(Failure::BadResponse(_))));
    }

    #[test]
    fn a_switch_fallthrough_body_is_rejected() {
        // A C++ `switch` with no `break` writes every case in turn. If
        // socketHandler() ships like that, one command produces five JSON
        // objects concatenated - which is not valid JSON and must not be read
        // as a success just because the first line happens to say "Success".
        let fallthrough = concat!(
            "{\"result\":\"Success\",\"command\":\"Power\"}\r\n",
            "{\"result\":\"Success\",\"command\":\"Silent\"}\r\n",
            "{\"result\":\"Success\",\"command\":\"High_Temp\"}\r\n",
            "{\"result\":\"Success\",\"command\":\"Low_Temp\"}\r\n",
            "{\"result\":\"Fail\",\"command\":\"Fail\"}\r\n",
        );
        assert!(
            matches!(verdict(fallthrough), Err(Failure::BadResponse(_))),
            "a fall-through reply must not confirm anything"
        );
    }

    #[test]
    fn json_echoing_the_wrong_command_is_rejected() {
        assert!(matches!(
            parse_body(r#"{"result":"Success","command":"Silent"}"#, Command::Power),
            Err(Failure::BadResponse(_))
        ));
    }

    #[test]
    fn dot_env_reads_a_plain_assignment() {
        let text = "IR_REMOTE_BASE_URL=http://192.168.0.102\n";
        assert_eq!(
            parse_dot_env(text, "IR_REMOTE_BASE_URL").as_deref(),
            Some("http://192.168.0.102")
        );
    }

    #[test]
    fn dot_env_ignores_comments_blank_lines_and_other_keys() {
        let text = "# target\n\nOTHER=nope\nIR_REMOTE_BASE_URL = http://10.0.0.5:80 \n";
        assert_eq!(
            parse_dot_env(text, "IR_REMOTE_BASE_URL").as_deref(),
            Some("http://10.0.0.5:80")
        );
        assert_eq!(parse_dot_env(text, "MISSING"), None);
    }

    #[test]
    fn dot_env_strips_quotes_and_export() {
        assert_eq!(
            parse_dot_env("export IR_REMOTE_BASE_URL=\"http://a.b\"\n", "IR_REMOTE_BASE_URL")
                .as_deref(),
            Some("http://a.b")
        );
        assert_eq!(
            parse_dot_env("IR_REMOTE_BASE_URL='http://c.d'\n", "IR_REMOTE_BASE_URL").as_deref(),
            Some("http://c.d")
        );
    }

    #[test]
    fn dot_env_drops_a_trailing_comment() {
        assert_eq!(
            parse_dot_env("IR_REMOTE_BASE_URL=http://a.b # the device\n", "IR_REMOTE_BASE_URL")
                .as_deref(),
            Some("http://a.b")
        );
    }

    #[test]
    fn an_unusable_value_is_skipped_not_accepted() {
        // A junk base URL must not become the target - it would fail every
        // request later with no explanation.
        assert_eq!(usable(Some("not a url".into()), "test"), None);
        assert_eq!(usable(Some("   ".into()), "test"), None);
        assert_eq!(
            usable(Some(" http://192.168.0.102 ".into()), "test").as_deref(),
            Some("http://192.168.0.102")
        );
    }

    #[test]
    fn truncate_caps_long_bodies() {
        let long = "x".repeat(500);
        assert!(truncate(&long).len() < 130);
    }
}
