//! The signed-in account: credentials, where they are stored, and how the app
//! turns them into a backend.
//!
//! Credentials are opaque strings here. Validating them and building the X
//! backend happen in `main.rs` (the only place that knows `twitty-x`), behind
//! [`Connector`].

use std::{
    fmt,
    sync::{Arc, Mutex, PoisonError},
};

use serde::{Deserialize, Serialize};
use twitty_model::Backend;

/// Version of the keyring payload.
pub const CREDENTIALS_SCHEMA_VERSION: u32 = 1;

/// The two cookies of an x.com session. `Debug` never shows the values.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    pub auth_token: String,
    pub ct0: String,
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Credentials { auth_token: ***, ct0: *** }")
    }
}

/// On-disk (keyring) form of [`Credentials`].
#[derive(Serialize, Deserialize)]
struct Payload {
    schema_version: u32,
    auth_token: String,
    ct0: String,
}

/// Why stored credentials couldn't be read or written. Carries no secret data.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[error("the credential store is unavailable: {0}")]
    Unavailable(String),
    #[error("the stored credentials are unreadable")]
    Corrupt,
    #[error("the stored credentials have unknown schema version {0}")]
    UnknownVersion(u32),
}

/// Encodes credentials for storage.
pub fn encode(credentials: &Credentials) -> String {
    let payload = Payload {
        schema_version: CREDENTIALS_SCHEMA_VERSION,
        auth_token: credentials.auth_token.clone(),
        ct0: credentials.ct0.clone(),
    };
    // Serialising two strings and a number can't fail.
    serde_json::to_string(&payload).unwrap_or_default()
}

/// Decodes stored credentials, rejecting versions this build doesn't know.
pub fn decode(raw: &str) -> Result<Credentials, StoreError> {
    #[derive(Deserialize)]
    struct Version {
        schema_version: u32,
    }
    let version: Version = serde_json::from_str(raw).map_err(|_| StoreError::Corrupt)?;
    if version.schema_version != CREDENTIALS_SCHEMA_VERSION {
        return Err(StoreError::UnknownVersion(version.schema_version));
    }
    let p: Payload = serde_json::from_str(raw).map_err(|_| StoreError::Corrupt)?;
    Ok(Credentials { auth_token: p.auth_token, ct0: p.ct0 })
}

/// Where the session is kept between launches.
pub trait CredentialStore: Send + Sync {
    fn load(&self) -> Result<Option<Credentials>, StoreError>;
    fn save(&self, credentials: &Credentials) -> Result<(), StoreError>;
    /// Removes the stored session; succeeds when there was none.
    fn delete(&self) -> Result<(), StoreError>;
}

/// Windows Credential Manager (or the platform keyring elsewhere).
#[derive(Debug, Clone)]
pub struct KeyringStore {
    service: String,
    user: String,
}

impl KeyringStore {
    /// The entry Twitty uses for its X session.
    pub fn x_session() -> Self {
        Self { service: "Twitty".into(), user: "x-session".into() }
    }

    fn entry(&self) -> Result<keyring::Entry, StoreError> {
        keyring::Entry::new(&self.service, &self.user).map_err(store_error)
    }
}

/// Maps keyring errors without their payloads (`BadEncoding` holds the raw secret).
fn store_error(e: keyring::Error) -> StoreError {
    match e {
        keyring::Error::BadEncoding(_) => StoreError::Corrupt,
        keyring::Error::PlatformFailure(e) | keyring::Error::NoStorageAccess(e) => {
            StoreError::Unavailable(e.to_string())
        }
        keyring::Error::NoEntry => StoreError::Unavailable("no entry".into()),
        keyring::Error::TooLong(attr, _) | keyring::Error::Invalid(attr, _) => {
            StoreError::Unavailable(format!("invalid attribute {attr}"))
        }
        keyring::Error::Ambiguous(_) => StoreError::Unavailable("ambiguous entry".into()),
        _ => StoreError::Unavailable("unknown keyring error".into()),
    }
}

impl CredentialStore for KeyringStore {
    fn load(&self) -> Result<Option<Credentials>, StoreError> {
        match self.entry()?.get_password() {
            Ok(raw) => decode(&raw).map(Some),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(store_error(e)),
        }
    }

    fn save(&self, credentials: &Credentials) -> Result<(), StoreError> {
        self.entry()?.set_password(&encode(credentials)).map_err(store_error)
    }

    fn delete(&self) -> Result<(), StoreError> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(store_error(e)),
        }
    }
}

/// An in-memory store for tests and for sessions that must not persist.
#[derive(Debug, Default, Clone)]
pub struct MemoryStore {
    raw: Arc<Mutex<Option<String>>>,
}

impl MemoryStore {
    /// The stored payload, as the keyring would hold it.
    pub fn raw(&self) -> Option<String> {
        self.raw.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

impl CredentialStore for MemoryStore {
    fn load(&self) -> Result<Option<Credentials>, StoreError> {
        self.raw().map(|raw| decode(&raw)).transpose()
    }

    fn save(&self, credentials: &Credentials) -> Result<(), StoreError> {
        *self.raw.lock().unwrap_or_else(PoisonError::into_inner) = Some(encode(credentials));
        Ok(())
    }

    fn delete(&self) -> Result<(), StoreError> {
        *self.raw.lock().unwrap_or_else(PoisonError::into_inner) = None;
        Ok(())
    }
}

/// Why credentials couldn't be turned into a backend.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConnectError {
    /// The values can't be cookies (empty, spaces, `;`…).
    #[error("these don't look like cookie values: {0}")]
    Malformed(String),
    /// The backend couldn't be created.
    #[error("could not start the connection: {0}")]
    Setup(String),
}

/// Builds a backend for credentials. `main.rs` provides the X implementation.
pub type Connector =
    Arc<dyn Fn(&Credentials) -> Result<Arc<dyn Backend>, ConnectError> + Send + Sync>;

/// Where the current session came from, which decides what "sign out" can do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionSource {
    /// Signed in through the dialog; stored in the keyring.
    Keyring,
    /// Environment variables or a session file the app doesn't own.
    External(String),
}

/// What the app needs to sign in and out.
#[derive(Clone)]
pub struct AccountServices {
    pub connect: Connector,
    pub store: Arc<dyn CredentialStore>,
}

impl fmt::Debug for AccountServices {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AccountServices")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn creds() -> Credentials {
        Credentials { auth_token: "tok-123".into(), ct0: "csrf-456".into() }
    }

    #[test]
    fn debug_hides_values() {
        let shown = format!("{:?}", creds());
        assert!(!shown.contains("tok-123") && !shown.contains("csrf-456"));
    }

    #[test]
    fn payload_round_trips() {
        assert_eq!(decode(&encode(&creds())).unwrap(), creds());
    }

    #[test]
    fn golden_v1_payload_still_decodes() {
        let raw = include_str!("../tests/golden/credentials_v1.json");
        let c = decode(raw).unwrap();
        assert_eq!((c.auth_token.as_str(), c.ct0.as_str()), ("fake-auth-token", "fake-ct0"));
    }

    #[test]
    fn unknown_or_broken_payloads_are_rejected() {
        assert_eq!(decode("{\"schema_version\": 9}"), Err(StoreError::UnknownVersion(9)));
        assert_eq!(decode("not json"), Err(StoreError::Corrupt));
        assert_eq!(decode("{\"schema_version\": 1}"), Err(StoreError::Corrupt));
    }

    #[test]
    fn bad_encoding_error_drops_the_bytes() {
        let e = store_error(keyring::Error::BadEncoding(b"tok-123".to_vec()));
        assert_eq!(e, StoreError::Corrupt);
        assert!(!format!("{e} {e:?}").contains("tok-123"));
    }

    #[test]
    fn memory_store_saves_loads_and_deletes() {
        let store = MemoryStore::default();
        assert_eq!(store.load().unwrap(), None);
        store.save(&creds()).unwrap();
        assert_eq!(store.load().unwrap(), Some(creds()));
        store.delete().unwrap();
        assert_eq!(store.load().unwrap(), None);
        store.delete().unwrap();
    }
}
