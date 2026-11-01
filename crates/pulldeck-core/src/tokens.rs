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

    #[test]
    fn errors_do_not_carry_the_token() {
        let err = TokenStoreError("keyring locked".into());
        assert_eq!(err.to_string(), "token store: keyring locked");
    }
}
