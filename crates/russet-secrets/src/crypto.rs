// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Sealing (ARCHITECTURE §12.2) [D 13]: every secret is a file encrypted with XChaCha20-Poly1305 under the
//! 32-byte master key, with the additional authenticated data `id ‖ kind ‖ version`, so a sealed file cannot be
//! swapped for another id's.
//!
//! File layout: the magic `RSSEAL01` (8 bytes), the format version (1 byte), the random 24-byte nonce, then the
//! ciphertext with its 16-byte tag.

use std::fmt;

use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use russet_core::error::{Error, Result, codes};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::{SecretBytes, SecretId};

/// The sealed-file magic.
pub const MAGIC: &[u8; 8] = b"RSSEAL01";
/// The sealed-file format version (part of the AAD).
pub const VERSION: u8 = 1;
const NONCE_LEN: usize = 24;
const TAG_LEN: usize = 16;
const HEADER_LEN: usize = MAGIC.len() + 1 + NONCE_LEN;

/// The 32-byte master key: zeroized on drop, never printed.
#[derive(Clone)]
pub struct MasterKey(Zeroizing<[u8; 32]>);

impl MasterKey {
    /// A fresh random key.
    pub fn generate() -> Result<Self> {
        let mut k = Zeroizing::new([0u8; 32]);
        random_fill(k.as_mut())?;
        Ok(Self(k))
    }

    /// A key from exactly 32 bytes.
    pub fn from_bytes(b: &[u8]) -> Result<Self> {
        let arr: [u8; 32] = b.try_into().map_err(|_| {
            Error::new(codes::secrets::LOCKED, "the stored master key has the wrong length")
                .with_hint("The keyring entry was changed outside Russet; restore it, or reset the store.")
        })?;
        Ok(Self(Zeroizing::new(arr)))
    }

    /// The key bytes, in-process only.
    pub fn expose(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MasterKey(***)")
    }
}

/// Fills `buf` from the operating system's CSPRNG.
pub fn random_fill(buf: &mut [u8]) -> Result<()> {
    getrandom::fill(buf).map_err(|e| {
        Error::new(codes::core::INTERNAL, format!("the operating system's random source failed: {e}"))
            .with_hint("This should not happen; check the kernel's entropy source.")
    })
}

/// Lowercase hex of SHA-256(`data`).
pub fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    let mut s = String::with_capacity(64);
    for b in digest {
        s.push(char::from_digit(u32::from(b >> 4), 16).unwrap_or('0'));
        s.push(char::from_digit(u32::from(b & 0x0f), 16).unwrap_or('0'));
    }
    s
}

fn aad(id: &SecretId) -> Vec<u8> {
    let id_string = id.to_string();
    let mut a = Vec::with_capacity(id_string.len() + 32);
    a.extend_from_slice(b"russet-secret\0");
    a.extend_from_slice(id_string.as_bytes());
    a.push(0);
    a.extend_from_slice(id.kind.as_str().as_bytes());
    a.push(0);
    a.push(VERSION);
    a
}

fn cipher(key: &MasterKey) -> Result<XChaCha20Poly1305> {
    XChaCha20Poly1305::new_from_slice(key.expose())
        .map_err(|_| Error::new(codes::core::INTERNAL, "invalid master key length").with_hint("Report this bug."))
}

/// Seals `plaintext` for `id` under `key`.
pub fn seal(key: &MasterKey, id: &SecretId, plaintext: &[u8]) -> Result<Vec<u8>> {
    let mut nonce_bytes = [0u8; NONCE_LEN];
    random_fill(&mut nonce_bytes)?;
    let nonce = XNonce::from(nonce_bytes);
    let aad = aad(id);
    let ct = cipher(key)?
        .encrypt(&nonce, Payload { msg: plaintext, aad: &aad })
        .map_err(|_| Error::new(codes::core::INTERNAL, "sealing failed").with_hint("Report this bug."))?;
    let mut out = Vec::with_capacity(HEADER_LEN + ct.len());
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Opens a sealed file of `id`. Fails with `secrets.locked` when the file was sealed under another key, for
/// another id, or was altered.
pub fn open(key: &MasterKey, id: &SecretId, sealed: &[u8]) -> Result<SecretBytes> {
    let not_ours = || {
        Error::new(codes::secrets::LOCKED, format!("the sealed secret {id} does not open with this master key"))
            .with_hint(
                "The file was altered, or sealed under another master key (another RUSSET_HOME or backend). \
                 Store the secret again with `russet secrets set` or the command that created it.",
            )
    };
    if sealed.len() < HEADER_LEN + TAG_LEN || &sealed[..MAGIC.len()] != MAGIC || sealed[MAGIC.len()] != VERSION {
        return Err(not_ours());
    }
    let nonce_bytes: [u8; NONCE_LEN] = sealed[MAGIC.len() + 1..HEADER_LEN].try_into().map_err(|_| not_ours())?;
    let nonce = XNonce::from(nonce_bytes);
    let aad = aad(id);
    let pt = cipher(key)?.decrypt(&nonce, Payload { msg: &sealed[HEADER_LEN..], aad: &aad }).map_err(|_| not_ours())?;
    Ok(SecretBytes::new(pt))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SecretKind;

    fn id(name: &str) -> SecretId {
        SecretId { kind: SecretKind::AscKey, name: name.into() }
    }

    #[test]
    fn seal_and_open_round_trip_and_bind_the_id() {
        let key = MasterKey::generate().expect("key");
        let sealed = seal(&key, &id("A"), b"canary-plaintext").expect("seal");
        assert!(!sealed.windows(16).any(|w| w == b"canary-plaintext"), "no plaintext in the file");
        assert_eq!(open(&key, &id("A"), &sealed).expect("open").expose(), b"canary-plaintext");
        assert_eq!(open(&key, &id("B"), &sealed).map_err(|e| e.code()).err(), Some(codes::secrets::LOCKED));
        let other = MasterKey::generate().expect("key");
        assert!(open(&other, &id("A"), &sealed).is_err());
        let mut tampered = sealed.clone();
        if let Some(last) = tampered.last_mut() {
            *last ^= 1;
        }
        assert!(open(&key, &id("A"), &tampered).is_err());
        assert!(open(&key, &id("A"), b"short").is_err());
    }

    #[test]
    fn keys_and_hashes() {
        assert_eq!(format!("{:?}", MasterKey::generate().expect("key")), "MasterKey(***)");
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert!(MasterKey::from_bytes(&[0u8; 31]).is_err());
    }
}
