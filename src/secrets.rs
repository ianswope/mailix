//! Cross-platform secret storage, shared by every provider.
//!
//! All secrets live under one service name in the OS credential store, keyed
//! per-account. The `keyring` crate abstracts the backend: Secret Service
//! (GNOME Keyring / KWallet) on Linux/BSD, the Keychain on macOS. Only the key
//! derivation differs per provider — Google keeps a refresh token, IMAP/iCloud
//! keep a password.

use keyring::Entry;

const SERVICE: &str = "com.ianswope.Mailix";

#[derive(Debug)]
pub enum SecretError {
    Keyring(keyring::Error),
}

impl std::fmt::Display for SecretError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SecretError::Keyring(e) => write!(f, "couldn't access the system keyring: {e}"),
        }
    }
}

impl std::error::Error for SecretError {}

fn entry(key: &str) -> Result<Entry, SecretError> {
    Entry::new(SERVICE, key).map_err(SecretError::Keyring)
}

/// Reads a secret, treating "no such entry" as `None` rather than an error.
pub fn get(key: &str) -> Result<Option<String>, SecretError> {
    match entry(key)?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(SecretError::Keyring(e)),
    }
}

pub fn set(key: &str, secret: &str) -> Result<(), SecretError> {
    entry(key)?.set_password(secret).map_err(SecretError::Keyring)
}

/// Removes a secret. A missing entry is treated as success — deleting an
/// account that never stored one shouldn't error.
pub fn delete(key: &str) -> Result<(), SecretError> {
    match entry(key)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(SecretError::Keyring(e)),
    }
}

/// Keyring key for a Google account's OAuth refresh token, namespaced by the
/// account's stable identity (its primary email).
pub fn google_refresh_key(account: &str) -> String {
    format!("google-refresh-token:{}", account.trim().to_lowercase())
}

/// Keyring key for an IMAP/iCloud account password. Includes the host so the
/// same username on two servers gets distinct secrets.
pub fn imap_password_key(host: &str, username: &str) -> String {
    format!(
        "imap-password:{}|{}",
        host.trim().to_lowercase(),
        username.trim().to_lowercase()
    )
}
