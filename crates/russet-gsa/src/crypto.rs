// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The symmetric pieces of GrandSlam (SIGN §7.2): `spd` is AES-256-CBC (PKCS#7) under keys derived from the SRP
//! session key, the app-token checksum is HMAC-SHA256 under `sk`, and the app tokens (`et`) are AES-256-GCM with a
//! 16-byte nonce: `version (3) ‖ nonce (16) ‖ ciphertext ‖ tag (16)`, the version bytes being the associated data.

use aes_gcm::aead::{Aead as _, KeyInit as _, Payload};
use aes_gcm::aes::Aes256;
use cbc::cipher::block_padding::Pkcs7;
use cbc::cipher::{BlockModeDecrypt as _, KeyIvInit as _};
use zeroize::Zeroizing;

type Gcm16 = aes_gcm::AesGcm<Aes256, aes_gcm::aead::consts::U16>;

pub(crate) fn hmac_sha256(key: &[u8], parts: &[&[u8]]) -> Vec<u8> {
    let k = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key);
    let mut c = ring::hmac::Context::with_key(&k);
    for p in parts {
        c.update(p);
    }
    c.sign().as_ref().to_vec()
}

/// The `spd` key and IV for session key `k`.
pub(crate) fn spd_key_iv(k: &[u8]) -> (Zeroizing<Vec<u8>>, Vec<u8>) {
    let key = Zeroizing::new(hmac_sha256(k, &[b"extra data key:"]));
    let iv = hmac_sha256(k, &[b"extra data iv:"])[..16].to_vec();
    (key, iv)
}

/// Decrypts `spd`.
pub(crate) fn decrypt_spd(k: &[u8], spd: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    let (key, iv) = spd_key_iv(k);
    let dec = cbc::Decryptor::<Aes256>::new_from_slices(&key, &iv).ok()?;
    dec.decrypt_padded_vec::<Pkcs7>(spd).ok().map(Zeroizing::new)
}

/// `checksum = HMAC-SHA256(sk, "apptokens" ‖ adsid ‖ app…)`.
pub(crate) fn app_token_checksum(sk: &[u8], adsid: &str, apps: &[&str]) -> Vec<u8> {
    let mut parts: Vec<&[u8]> = vec![b"apptokens", adsid.as_bytes()];
    parts.extend(apps.iter().map(|a| a.as_bytes()));
    hmac_sha256(sk, &parts)
}

/// Decrypts an `et` blob under `sk`.
pub(crate) fn decrypt_gcm(sk: &[u8], blob: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    if blob.len() < 3 + 16 + 16 {
        return None;
    }
    let (version, rest) = blob.split_at(3);
    let (nonce, ct) = rest.split_at(16);
    let cipher = Gcm16::new_from_slice(sk).ok()?;
    let nonce = aes_gcm::Nonce::<aes_gcm::aead::consts::U16>::try_from(nonce).ok()?;
    cipher.decrypt(&nonce, Payload { msg: ct, aad: version }).ok().map(Zeroizing::new)
}

/// The fake's side: encryptions.
#[cfg(any(test, feature = "fake"))]
pub(crate) mod seal {
    use super::*;
    use cbc::cipher::BlockModeEncrypt as _;

    pub fn encrypt_spd(k: &[u8], plain: &[u8]) -> Vec<u8> {
        let (key, iv) = spd_key_iv(k);
        match cbc::Encryptor::<Aes256>::new_from_slices(&key, &iv) {
            Ok(enc) => enc.encrypt_padded_vec::<Pkcs7>(plain),
            Err(_) => Vec::new(),
        }
    }

    pub fn encrypt_gcm(sk: &[u8], nonce: &[u8; 16], plain: &[u8]) -> Vec<u8> {
        let version = b"XYZ";
        let Ok(cipher) = Gcm16::new_from_slice(sk) else { return Vec::new() };
        let Ok(n) = aes_gcm::Nonce::<aes_gcm::aead::consts::U16>::try_from(&nonce[..]) else { return Vec::new() };
        let ct = cipher.encrypt(&n, Payload { msg: plain, aad: version }).unwrap_or_default();
        [version.as_slice(), nonce, &ct].concat()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spd_and_et_round_trip() {
        let k = [9u8; 32];
        let spd = seal::encrypt_spd(&k, b"<plist>spd</plist>");
        assert_eq!(decrypt_spd(&k, &spd).as_deref().map(Vec::as_slice), Some(&b"<plist>spd</plist>"[..]));
        assert!(decrypt_spd(&[8u8; 32], &spd).is_none_or(|p| p.as_slice() != b"<plist>spd</plist>"));
        let sk = [5u8; 32];
        let et = seal::encrypt_gcm(&sk, &[1u8; 16], b"token plist");
        assert_eq!(decrypt_gcm(&sk, &et).as_deref().map(Vec::as_slice), Some(&b"token plist"[..]));
        let mut tampered = et.clone();
        tampered[0] ^= 1;
        assert!(decrypt_gcm(&sk, &tampered).is_none());
        assert_eq!(app_token_checksum(&sk, "adsid", &["a", "b"]), hmac_sha256(&sk, &[b"apptokensadsidab"]));
    }
}
