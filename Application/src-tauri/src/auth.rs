//! Credential storage (rule 7).
//!
//! The password is read from the OS keychain, attached to outgoing requests
//! inside `api_client`, and never returned over IPC, never logged, never written
//! to a config file in plaintext.
//!
//! The only thing the frontend may ever learn is whether a credential EXISTS
//! and what the username is - never the secret itself. See `Credentials`,
//! whose Debug impl deliberately refuses to print the password.
//!
//! ## Platform support
//!
//! Windows uses the Credential Manager via the `keyring` crate. **Android has
//! no backend** - keyring 3.6 implements only linux, macos, ios and windows -
//! so this module compiles to a no-op store there and says so. That costs
//! nothing today: the firmware performs no authentication, so nothing is ever
//! stored or sent. If the firmware gains Basic auth, Android needs a real
//! implementation (the Android Keystore, through a Tauri plugin) before the
//! credential UI means anything on the phone.

use std::fmt;

/// A username/password pair for HTTP Basic. Deliberately not serializable:
/// there is no `Serialize` impl, so it cannot cross the Tauri IPC boundary even
/// by accident.
pub struct Credentials {
    username: String,
    password: String,
}

impl Credentials {
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        Credentials {
            username: username.into(),
            password: password.into(),
        }
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    /// Only `api_client` calls this, to put the value in an Authorization header.
    pub fn password(&self) -> &str {
        &self.password
    }
}

/// Keeps the secret out of any `{:?}` in a log line or panic message.
impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

/// True where a real OS-backed store exists. The UI uses this to explain itself
/// rather than offering a credential field that would silently do nothing.
pub const fn is_supported() -> bool {
    cfg!(windows)
}

// ------------------------------------------------------- windows backend ----

#[cfg(windows)]
mod backend {
    use super::Credentials;
    use keyring::Entry;

    const SERVICE: &str = "ir-remote-esp32";

    fn entry(username: &str) -> Result<Entry, String> {
        Entry::new(SERVICE, username).map_err(|e| format!("keychain unavailable: {e}"))
    }

    pub fn store(username: &str, password: &str) -> Result<(), String> {
        entry(username)?
            .set_password(password)
            .map_err(|e| format!("could not save credential: {e}"))
    }

    pub fn load(username: &str) -> Result<Option<Credentials>, String> {
        match entry(username)?.get_password() {
            Ok(password) => Ok(Some(Credentials::new(username, password))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(format!("could not read credential: {e}")),
        }
    }

    pub fn clear(username: &str) -> Result<(), String> {
        match entry(username)?.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(format!("could not delete credential: {e}")),
        }
    }
}

// ------------------------------------------------- no-op (Android, other) ----

#[cfg(not(windows))]
mod backend {
    use super::Credentials;

    const UNSUPPORTED: &str =
        "no OS keychain on this platform - a credential cannot be stored securely here";

    pub fn store(_username: &str, _password: &str) -> Result<(), String> {
        // Refuse loudly. Silently accepting and discarding a password would be
        // worse than not offering the feature at all.
        Err(UNSUPPORTED.to_string())
    }

    pub fn load(_username: &str) -> Result<Option<Credentials>, String> {
        // Nothing stored means nothing sent, which matches a firmware that
        // performs no authentication.
        Ok(None)
    }

    pub fn clear(_username: &str) -> Result<(), String> {
        Ok(())
    }
}

/// Store a credential in the OS keychain. The plaintext never touches disk.
pub fn store(username: &str, password: &str) -> Result<(), String> {
    if username.is_empty() {
        return Err("username must not be empty".to_string());
    }
    backend::store(username, password)
}

/// Load a credential. Returns `Ok(None)` when nothing is stored, which is the
/// normal case for this device - the current firmware performs no auth at all.
pub fn load(username: &str) -> Result<Option<Credentials>, String> {
    if username.is_empty() {
        return Ok(None);
    }
    backend::load(username)
}

pub fn clear(username: &str) -> Result<(), String> {
    backend::clear(username)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_prints_the_password() {
        let c = Credentials::new("bob", "hunter2");
        let rendered = format!("{c:?}");
        assert!(
            !rendered.contains("hunter2"),
            "password leaked through Debug: {rendered}"
        );
        assert!(rendered.contains("bob"));
        assert!(rendered.contains("redacted"));
    }

    #[test]
    fn empty_username_loads_nothing() {
        assert!(load("").unwrap().is_none());
    }

    #[test]
    fn empty_username_cannot_be_stored() {
        assert!(store("", "x").is_err());
    }

    #[test]
    fn accessors_round_trip() {
        let c = Credentials::new("bob", "hunter2");
        assert_eq!(c.username(), "bob");
        assert_eq!(c.password(), "hunter2");
    }

    #[test]
    #[cfg(not(windows))]
    fn an_unsupported_platform_refuses_rather_than_pretending() {
        // Accepting a password and dropping it would look like it worked.
        let err = store("bob", "hunter2").unwrap_err();
        assert!(err.contains("keychain"), "got {err}");
        assert!(load("bob").unwrap().is_none());
    }
}
