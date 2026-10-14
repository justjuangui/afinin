// SPDX-License-Identifier: MIT OR Apache-2.0

//! Secret storage (FR-2). Secrets live only in the OS keychain; never in the
//! database, logs or config files.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::error::{Error, Result};

/// Keychain service name under which every Afinín secret is stored.
pub const SERVICE: &str = "afinin";

pub trait SecretStore: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<String>>;
    fn set(&self, key: &str, value: &str) -> Result<()>;
    fn delete(&self, key: &str) -> Result<()>;
}

/// OS keychain: macOS Keychain, Windows Credential Manager, Linux Secret Service.
#[derive(Debug, Default)]
pub struct KeychainStore;

impl KeychainStore {
    fn entry(key: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(SERVICE, key).map_err(|e| Error::Secrets(e.to_string()))
    }
}

impl SecretStore for KeychainStore {
    fn get(&self, key: &str) -> Result<Option<String>> {
        match Self::entry(key)?.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(Error::Secrets(e.to_string())),
        }
    }

    fn set(&self, key: &str, value: &str) -> Result<()> {
        Self::entry(key)?
            .set_password(value)
            .map_err(|e| Error::Secrets(e.to_string()))
    }

    fn delete(&self, key: &str) -> Result<()> {
        match Self::entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(Error::Secrets(e.to_string())),
        }
    }
}

/// In-memory store for tests and ephemeral sessions.
#[derive(Debug, Default)]
pub struct MemoryStore(Mutex<HashMap<String, String>>);

impl SecretStore for MemoryStore {
    fn get(&self, key: &str) -> Result<Option<String>> {
        Ok(self.0.lock().unwrap().get(key).cloned())
    }

    fn set(&self, key: &str, value: &str) -> Result<()> {
        self.0
            .lock()
            .unwrap()
            .insert(key.to_owned(), value.to_owned());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<()> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
}

/// Returns the secret under `key`, generating and storing a random 256-bit hex
/// value on first use.
pub fn get_or_create_key(store: &dyn SecretStore, key: &str) -> Result<String> {
    if let Some(existing) = store.get(key)? {
        return Ok(existing);
    }
    let bytes: [u8; 32] = rand::random();
    let value = hex::encode(bytes);
    store.set(key, &value)?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_key_once() {
        let store = MemoryStore::default();
        let a = get_or_create_key(&store, "db").unwrap();
        let b = get_or_create_key(&store, "db").unwrap();
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
    }
}
