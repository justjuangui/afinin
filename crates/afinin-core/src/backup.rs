// SPDX-License-Identifier: MIT OR Apache-2.0

//! Backup and restore (FR-53): all local data (history, findings state,
//! workspaces, settings) in one password-encrypted file. Credentials are never
//! included: they live only in the OS keychain.
//!
//! File layout: `AFININ-BACKUP-1` · salt (16 bytes) · nonce (24 bytes) ·
//! XChaCha20-Poly1305 ciphertext of a tar archive of `EXPORT DATABASE`.
//! The key is derived from the password with Argon2id.

use std::path::{Path, PathBuf};

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};

use crate::error::{Error, Result};

const MAGIC: &[u8] = b"AFININ-BACKUP-1";
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;
pub const MIN_PASSWORD_LEN: usize = 10;

fn derive_key(password: &str, salt: &[u8]) -> Result<[u8; 32]> {
    let params = Params::new(64 * 1024, 3, 1, Some(32)).map_err(|e| Error::Other(e.to_string()))?;
    let mut key = [0u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|e| Error::Other(e.to_string()))?;
    Ok(key)
}

pub fn encrypt(plaintext: &[u8], password: &str) -> Result<Vec<u8>> {
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(Error::Other(format!(
            "use a password of at least {MIN_PASSWORD_LEN} characters"
        )));
    }
    let salt: [u8; SALT_LEN] = rand::random();
    let nonce: [u8; NONCE_LEN] = rand::random();
    let key = derive_key(password, &salt)?;
    let cipher = XChaCha20Poly1305::new(Key::from_slice(&key));
    let ciphertext = cipher
        .encrypt(XNonce::from_slice(&nonce), plaintext)
        .map_err(|_| Error::Other("encryption failed".into()))?;
    let mut out = Vec::with_capacity(MAGIC.len() + SALT_LEN + NONCE_LEN + ciphertext.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

pub fn decrypt(data: &[u8], password: &str) -> Result<Vec<u8>> {
    let header = MAGIC.len() + SALT_LEN + NONCE_LEN;
    if data.len() < header || &data[..MAGIC.len()] != MAGIC {
        return Err(Error::Other("not an Afinín backup file".into()));
    }
    let salt = &data[MAGIC.len()..MAGIC.len() + SALT_LEN];
    let nonce = &data[MAGIC.len() + SALT_LEN..header];
    let key = derive_key(password, salt)?;
    XChaCha20Poly1305::new(Key::from_slice(&key))
        .decrypt(XNonce::from_slice(nonce), &data[header..])
        .map_err(|_| Error::Other("wrong password, or the file is damaged".into()))
}

/// Deletes a temporary directory even when the operation fails.
pub(crate) struct TempDir(pub PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(crate) fn temp_dir(base: &Path) -> Result<TempDir> {
    let dir = base.join(format!("backup-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir)?;
    Ok(TempDir(dir))
}

pub(crate) fn tar_dir(dir: &Path) -> Result<Vec<u8>> {
    let mut builder = tar::Builder::new(Vec::new());
    builder.append_dir_all(".", dir)?;
    Ok(builder.into_inner()?)
}

pub(crate) fn untar(bytes: &[u8], dest: &Path) -> Result<()> {
    let mut archive = tar::Archive::new(bytes);
    for entry in archive.entries()? {
        let mut entry = entry?;
        // `unpack_in` refuses paths escaping `dest` (no `../`).
        if !entry.unpack_in(dest)? {
            return Err(Error::Other("backup contains an unsafe path".into()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_wrong_password() {
        let data = b"workspaces and costs";
        let enc = encrypt(data, "correct horse battery").unwrap();
        assert!(!enc.windows(data.len()).any(|w| w == data));
        assert_eq!(decrypt(&enc, "correct horse battery").unwrap(), data);
        assert!(decrypt(&enc, "wrong password!!").is_err());
        assert!(encrypt(data, "short").is_err());
        assert!(decrypt(b"garbage", "whatever12345").is_err());
    }
}
