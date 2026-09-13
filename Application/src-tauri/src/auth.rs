//! Credential storage (rule 7).
//!
//! The password is read from the OS keychain (Windows Credential Manager),
//! attached to outgoing requests inside `api_client`, and never returned over
//! IPC, never logged, never written to a config file in plaintext.
//!
//! The only thing the frontend may ever learn is whether a credential EXISTS
//! and what the username is - never the secret itself. See `Credentials`,
//! whose Debug impl deliberately refuses to print the password.

use std::fmt;

use keyring::Entry;

const SERVICE: &str = "ir-remote-esp32";

/// A username/password pair for HTTP Basic. Deliberately not `Clone`-into-JS:
/// there is no Serialize impl, so it cannot cross the Tauri IPC boundary even
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

fn entry(username: &str) -> Result<Entry, String> {
    Entry::new(SERVICE, username).map_err(|e| format!("keychain unavailable: {e}"))
}

/// Store a credential in the OS keychain. The plaintext never touches disk.
pub fn store(username: &str, password: &str) -> Result<(), String> {
    if username.is_empty() {
        return Err("username must not be empty".to_string());
    }
    entry(username)?
        .set_password(password)
        .map_err(|e| format!("could not save credential: {e}"))
}

/// Load a credential. Returns `Ok(None)` when nothing is stored, which is the
/// normal case for this device - the current firmware performs no auth at all.
pub fn load(username: &str) -> Result<Option<Credentials>, String> {
    if username.is_empty() {
        return Ok(None);
    }
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
}
