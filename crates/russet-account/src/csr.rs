// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The key and certificate signing request of a new signing certificate (SIGN §6.3): RSA-2048 from the operating
//! system's CSPRNG, and a PKCS#10 request with subject `CN=Russet`, signed SHA-256 with RSA by `ring`. Apple
//! ignores the CN of API-created certificates [V4].

use std::str::FromStr;

use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey};
use russet_core::Result;
use russet_core::error::{Error, codes};
use x509_cert::der::asn1::BitString;
use x509_cert::der::oid::ObjectIdentifier;
use x509_cert::der::pem::LineEnding;
use x509_cert::der::{Any, Decode, Encode, EncodePem};
use x509_cert::name::Name;
use x509_cert::request::{CertReq, CertReqInfo, Version};
use x509_cert::spki::{AlgorithmIdentifierOwned, SubjectPublicKeyInfoOwned};
use zeroize::Zeroizing;

/// `sha256WithRSAEncryption` (RFC 4055).
const SHA256_WITH_RSA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.11");

/// Apple issues signing certificates for RSA-2048 keys.
const KEY_BITS: usize = 2048;

/// A new private key (PKCS#8 DER) and its CSR (PEM). CPU-bound: run it on a blocking thread.
pub(crate) fn generate() -> Result<(Zeroizing<Vec<u8>>, String)> {
    let key = rsa::RsaPrivateKey::new(&mut OsRng, KEY_BITS).map_err(|e| failed("generate the signing key", e))?;
    let pkcs8 =
        Zeroizing::new(key.to_pkcs8_der().map_err(|e| failed("encode the signing key", e))?.as_bytes().to_vec());
    let spki = key.to_public_key().to_public_key_der().map_err(|e| failed("encode the public key", e))?;
    let info = CertReqInfo {
        version: Version::V1,
        subject: Name::from_str("CN=Russet").map_err(|e| failed("build the request subject", e))?,
        public_key: SubjectPublicKeyInfoOwned::from_der(spki.as_bytes())
            .map_err(|e| failed("read the public key", e))?,
        attributes: Default::default(),
    };
    let tbs = info.to_der().map_err(|e| failed("encode the request", e))?;
    let signer = ring::signature::RsaKeyPair::from_pkcs8(&pkcs8).map_err(|e| failed("load the signing key", e))?;
    let mut signature = vec![0; signer.public().modulus_len()];
    signer
        .sign(&ring::signature::RSA_PKCS1_SHA256, &ring::rand::SystemRandom::new(), &tbs, &mut signature)
        .map_err(|e| failed("sign the request", e))?;
    let request = CertReq {
        info,
        algorithm: AlgorithmIdentifierOwned { oid: SHA256_WITH_RSA, parameters: Some(Any::null()) },
        signature: BitString::from_bytes(&signature).map_err(|e| failed("encode the signature", e))?,
    };
    let pem = request.to_pem(LineEnding::LF).map_err(|e| failed("encode the request", e))?;
    Ok((pkcs8, pem))
}

/// Whether `cert_der` certifies the key `key_pkcs8` (its subject public key is that key's).
pub(crate) fn certifies(cert_der: &[u8], key_pkcs8: &[u8]) -> Result<bool> {
    use rsa::pkcs8::DecodePrivateKey;
    let cert = x509_cert::Certificate::from_der(cert_der).map_err(|e| failed("read the certificate", e))?;
    let key = rsa::RsaPrivateKey::from_pkcs8_der(key_pkcs8).map_err(|e| failed("read the signing key", e))?;
    let ours = key.to_public_key().to_public_key_der().map_err(|e| failed("encode the public key", e))?;
    let theirs =
        cert.tbs_certificate.subject_public_key_info.to_der().map_err(|e| failed("read the certificate key", e))?;
    Ok(ours.as_bytes() == theirs.as_slice())
}

fn failed(what: &str, e: impl std::fmt::Display) -> Error {
    Error::new(codes::core::INTERNAL, format!("cannot {what}: {e}"))
        .with_hint("Report it with `russet bugreport`; no certificate was requested.")
}

/// `rsa`'s key generation draws from the operating system's CSPRNG, as every other Russet secret does.
struct OsRng;

impl rsa::rand_core::RngCore for OsRng {
    fn next_u32(&mut self) -> u32 {
        let mut b = [0; 4];
        russet_core::time::os_fill(&mut b);
        u32::from_le_bytes(b)
    }
    fn next_u64(&mut self) -> u64 {
        let mut b = [0; 8];
        russet_core::time::os_fill(&mut b);
        u64::from_le_bytes(b)
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        russet_core::time::os_fill(dest);
    }
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> std::result::Result<(), rsa::rand_core::Error> {
        russet_core::time::os_fill(dest);
        Ok(())
    }
}

impl rsa::rand_core::CryptoRng for OsRng {}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use x509_cert::der::DecodePem;

    #[test]
    fn the_request_is_a_signed_rsa_2048_pkcs10_for_its_key() {
        let (key, pem) = generate().unwrap();
        assert!(pem.starts_with("-----BEGIN CERTIFICATE REQUEST-----\n"), "{pem}");
        let req = CertReq::from_pem(pem.as_bytes()).unwrap();
        assert_eq!(req.info.subject.to_string(), "CN=Russet");
        assert_eq!(req.algorithm.oid, SHA256_WITH_RSA);

        // The request carries the key's public half and verifies under it.
        let signer = ring::signature::RsaKeyPair::from_pkcs8(&key).unwrap();
        assert_eq!(signer.public().modulus_len() * 8, KEY_BITS);
        let spki = req.info.public_key.subject_public_key.raw_bytes();
        let public = ring::signature::UnparsedPublicKey::new(&ring::signature::RSA_PKCS1_2048_8192_SHA256, spki);
        public.verify(&req.info.to_der().unwrap(), req.signature.raw_bytes()).unwrap();
        assert!(public.verify(b"another message", req.signature.raw_bytes()).is_err());
    }
}
