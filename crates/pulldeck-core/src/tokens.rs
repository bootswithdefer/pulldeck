//! Where the session token is kept between runs.
//!
//! The token is a credential, so it must never go in the plain-text settings file. The real
//! implementation belongs in the OS keyring; this module defines the interface the rest of the
//! app depends on and an in-memory store for tests.

use crate::session::SessionToken;
use std::sync::Mutex;

/// Why the token store failed.
#[derive(Debug, thiserror::Error)]
#[error("token store: {0}")]
pub struct TokenStoreError(pub String);

/// Persistent storage for the session token.
pub trait TokenStore: Send + Sync {
    /// The saved token, if any.
    fn load(&self) -> Result<Option<SessionToken>, TokenStoreError>;
    /// Saves the token, replacing any previous one.
    fn save(&self, token: &SessionToken) -> Result<(), TokenStoreError>;
    /// Forgets the token (sign out).
    fn clear(&self) -> Result<(), TokenStoreError>;
}

/// Keeps the token in the OS keyring (Secret Service on Linux, Keychain on macOS, Credential
/// Manager on Windows), under one entry per server so signing in elsewhere never clobbers it.
#[derive(Debug)]
pub struct KeyringTokenStore {
    service: String,
    account: String,
}

impl KeyringTokenStore {
    /// A store for the session token issued by `server`.
    pub fn new(server: &crate::settings::ServerUrl) -> Self {
        Self {
            service: "pulldeck".into(),
            account: server.url(""),
        }
    }

    fn entry(&self) -> Result<keyring::Entry, TokenStoreError> {
        keyring::Entry::new(&self.service, &self.account).map_err(store_error)
    }
}

fn store_error(error: keyring::Error) -> TokenStoreError {
    // keyring errors describe the backend, never the secret.
    TokenStoreError(error.to_string())
}

impl TokenStore for KeyringTokenStore {
    fn load(&self) -> Result<Option<SessionToken>, TokenStoreError> {
        match self.entry()?.get_password() {
            Ok(token) => Ok(Some(SessionToken::new(token))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(store_error(error)),
        }
    }

    fn save(&self, token: &SessionToken) -> Result<(), TokenStoreError> {
        self.entry()?
            .set_password(token.expose())
            .map_err(store_error)
    }

    fn clear(&self) -> Result<(), TokenStoreError> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(store_error(error)),
        }
    }
}

/// Keeps the token in memory only: lost on exit. For tests and for running without a keyring.
#[derive(Debug, Default)]
pub struct MemoryTokenStore {
    token: Mutex<Option<SessionToken>>,
}

impl TokenStore for MemoryTokenStore {
    fn load(&self) -> Result<Option<SessionToken>, TokenStoreError> {
        Ok(self.lock().clone())
    }

    fn save(&self, token: &SessionToken) -> Result<(), TokenStoreError> {
        *self.lock() = Some(token.clone());
        Ok(())
    }

    fn clear(&self) -> Result<(), TokenStoreError> {
        *self.lock() = None;
        Ok(())
    }
}

impl MemoryTokenStore {
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<SessionToken>> {
        // A poisoned lock only means another thread panicked mid-write of a plain Option; the
        // value is still usable.
        self.token
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_load_clear() {
        let store = MemoryTokenStore::default();
        assert_eq!(store.load().unwrap(), None);
        store.save(&SessionToken::new("a")).unwrap();
        assert_eq!(store.load().unwrap().unwrap().expose(), "a");
        store.save(&SessionToken::new("b")).unwrap();
        assert_eq!(store.load().unwrap().unwrap().expose(), "b");
        store.clear().unwrap();
        assert_eq!(store.load().unwrap(), None);
    }

    /// Needs a running Secret Service (a desktop session): `cargo test -- --ignored`.
    #[test]
    #[ignore = "needs an OS keyring"]
    fn keyring_round_trip() {
        let server = crate::settings::ServerUrl::parse("http://127.0.0.1:9").unwrap();
        let store = KeyringTokenStore::new(&server);
        store.clear().unwrap();
        assert_eq!(store.load().unwrap(), None);
        store.save(&SessionToken::new("secret")).unwrap();
        assert_eq!(store.load().unwrap().unwrap().expose(), "secret");
        store.clear().unwrap();
        assert_eq!(store.load().unwrap(), None);
    }

    #[test]
    fn errors_do_not_carry_the_token() {
        let err = TokenStoreError("keyring locked".into());
        assert_eq!(err.to_string(), "token store: keyring locked");
    }
}
