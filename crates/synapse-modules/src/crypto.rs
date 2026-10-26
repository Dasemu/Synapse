//! Host-side cryptography for modules (`crypto` permission): Argon2id key derivation and
//! XChaCha20-Poly1305 authenticated encryption. Keys never enter the module: it gets opaque
//! handles into a per-instance [`Keyring`], whose keys are zeroed when dropped.

use crate::{Error, Result};
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::XChaCha20Poly1305;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use std::collections::HashMap;
use zeroize::Zeroizing;

pub const KEY_LEN: usize = 32;
pub const NONCE_LEN: usize = 24;
/// Upper bounds for Argon2id parameters a module may ask for (memory in KiB).
pub const MAX_KDF_MEMORY_KIB: u32 = 256 * 1024;
pub const MAX_KDF_ITERATIONS: u32 = 10;
pub const MAX_KDF_LANES: u32 = 4;
/// Largest plaintext or ciphertext handled in one call.
pub const MAX_CRYPTO_INPUT: usize = 1 << 20;
const MAX_KEYS: usize = 64;

type Key = Zeroizing<[u8; KEY_LEN]>;

pub fn random_bytes(n: usize) -> Result<Vec<u8>> {
    let mut v = vec![0u8; n];
    getrandom::fill(&mut v).map_err(|e| Error::Host(format!("no randomness: {e}")))?;
    Ok(v)
}

fn random_key() -> Result<Key> {
    let mut k = Zeroizing::new([0u8; KEY_LEN]);
    getrandom::fill(k.as_mut()).map_err(|e| Error::Host(format!("no randomness: {e}")))?;
    Ok(k)
}

/// Keys of one module instance, addressed by small integer handles.
#[derive(Default)]
pub struct Keyring {
    keys: HashMap<u32, Key>,
    next: u32,
}

impl Keyring {
    fn insert(&mut self, key: Key) -> Result<u32> {
        if self.keys.len() >= MAX_KEYS {
            return Err(Error::Invalid("too many keys; forget unused ones".into()));
        }
        self.next = self.next.wrapping_add(1).max(1);
        self.keys.insert(self.next, key);
        Ok(self.next)
    }

    fn get(&self, handle: u32) -> Result<&Key> {
        self.keys
            .get(&handle)
            .ok_or_else(|| Error::Invalid(format!("unknown key handle {handle}")))
    }

    /// Derives a key from a password with Argon2id.
    pub fn derive(
        &mut self,
        password: &str,
        salt: &[u8],
        memory_kib: u32,
        iterations: u32,
        lanes: u32,
    ) -> Result<u32> {
        if memory_kib > MAX_KDF_MEMORY_KIB
            || iterations > MAX_KDF_ITERATIONS
            || lanes > MAX_KDF_LANES
            || !(8..=64).contains(&salt.len())
        {
            return Err(Error::Invalid("Argon2 parameters out of range".into()));
        }
        let params = Params::new(memory_kib, iterations, lanes, Some(KEY_LEN))
            .map_err(|e| Error::Invalid(format!("Argon2 parameters: {e}")))?;
        let mut key = Zeroizing::new([0u8; KEY_LEN]);
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
            .hash_password_into(password.as_bytes(), salt, key.as_mut())
            .map_err(|e| Error::Host(format!("Argon2: {e}")))?;
        self.insert(key)
    }

    /// A new random key.
    pub fn generate(&mut self) -> Result<u32> {
        let k = random_key()?;
        self.insert(k)
    }

    /// Encrypts with a fresh random nonce; returns `nonce || ciphertext`.
    pub fn seal(&self, key: u32, plaintext: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
        if plaintext.len() > MAX_CRYPTO_INPUT {
            return Err(Error::Invalid("plaintext too large".into()));
        }
        let cipher = XChaCha20Poly1305::new_from_slice(self.get(key)?.as_ref())
            .map_err(|_| Error::Host("bad key length".into()))?;
        let nonce = random_bytes(NONCE_LEN)?;
        let ct = cipher
            .encrypt(
                nonce.as_slice().try_into().expect("nonce length"),
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .map_err(|_| Error::Host("encryption failed".into()))?;
        let mut out = nonce;
        out.extend(ct);
        Ok(out)
    }

    /// Decrypts `nonce || ciphertext`; fails when the key, data or associated data is wrong.
    pub fn open(&self, key: u32, data: &[u8], aad: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        if data.len() < NONCE_LEN || data.len() > MAX_CRYPTO_INPUT + NONCE_LEN + 16 {
            return Err(Error::Decrypt);
        }
        let cipher = XChaCha20Poly1305::new_from_slice(self.get(key)?.as_ref())
            .map_err(|_| Error::Host("bad key length".into()))?;
        let (nonce, ct) = data.split_at(NONCE_LEN);
        cipher
            .decrypt(
                nonce.try_into().expect("nonce length"),
                Payload { msg: ct, aad },
            )
            .map(Zeroizing::new)
            .map_err(|_| Error::Decrypt)
    }

    /// Encrypts key `target` with key `kek` (key wrapping).
    pub fn wrap(&self, kek: u32, target: u32, aad: &[u8]) -> Result<Vec<u8>> {
        let t = self.get(target)?.clone();
        self.seal(kek, t.as_ref(), aad)
    }

    /// Decrypts a wrapped key and returns a handle to it.
    pub fn unwrap_key(&mut self, kek: u32, data: &[u8], aad: &[u8]) -> Result<u32> {
        let plain = self.open(kek, data, aad)?;
        let bytes: [u8; KEY_LEN] = plain.as_slice().try_into().map_err(|_| Error::Decrypt)?;
        self.insert(Zeroizing::new(bytes))
    }

    pub fn forget(&mut self, handle: u32) {
        self.keys.remove(&handle);
    }

    pub fn clear(&mut self) {
        self.keys.clear();
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_open_and_wrap_round_trip() {
        let mut k = Keyring::default();
        let kek = k
            .derive("correct horse", b"saltsaltsalt", 1024, 1, 1)
            .unwrap();
        let same = k
            .derive("correct horse", b"saltsaltsalt", 1024, 1, 1)
            .unwrap();
        let other = k.derive("wrong", b"saltsaltsalt", 1024, 1, 1).unwrap();
        let dek = k.generate().unwrap();
        let wrapped = k.wrap(kek, dek, b"vault").unwrap();
        let dek2 = k.unwrap_key(same, &wrapped, b"vault").unwrap();
        assert!(k.unwrap_key(other, &wrapped, b"vault").is_err());
        assert!(k.unwrap_key(same, &wrapped, b"other aad").is_err());

        let ct = k.seal(dek, b"hunter2", b"entry/1").unwrap();
        assert_eq!(&**k.open(dek2, &ct, b"entry/1").unwrap(), b"hunter2");
        assert!(k.open(dek2, &ct, b"entry/2").is_err());
        let mut tampered = ct.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert!(k.open(dek2, &tampered, b"entry/1").is_err());
        // Fresh nonce each time.
        assert_ne!(ct, k.seal(dek, b"hunter2", b"entry/1").unwrap());

        k.forget(dek2);
        assert!(k.open(dek2, &ct, b"entry/1").is_err());
    }

    #[test]
    fn refuses_extreme_kdf_parameters() {
        let mut k = Keyring::default();
        assert!(
            k.derive("p", b"saltsaltsalt", MAX_KDF_MEMORY_KIB + 1, 1, 1)
                .is_err()
        );
        assert!(k.derive("p", b"short", 1024, 1, 1).is_err());
    }
}
