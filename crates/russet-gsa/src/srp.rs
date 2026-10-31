// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! SRP-6a as GrandSlam speaks it (SIGN §7.2): the RFC 5054 2048-bit group, `g = 2`, SHA-256, and
//!
//! - `k = H(N ‖ PAD(g))`, `u = H(PAD(A) ‖ PAD(B))` (RFC 5054);
//! - `x = H(s ‖ H(":" ‖ P))` — no user name in `x`, and `P` is the PBKDF2 output of [`password_key`];
//! - `K = H(S)`, `M1 = H(H(N) ⊕ H(PAD(g)) ‖ H(I) ‖ s ‖ A ‖ B ‖ K)`, `M2 = H(A ‖ M1 ‖ K)`.
//!
//! The arithmetic is checked against RFC 5054 Appendix B (SHA-1, 1024-bit group) with the hash and group as
//! parameters; the GrandSlam fake runs the server side of the same code.

use num_bigint::BigUint;
use zeroize::Zeroizing;

/// A hash over concatenated parts.
pub(crate) type HashFn = fn(&[&[u8]]) -> Vec<u8>;

pub(crate) fn sha256(parts: &[&[u8]]) -> Vec<u8> {
    let mut c = ring::digest::Context::new(&ring::digest::SHA256);
    for p in parts {
        c.update(p);
    }
    c.finish().as_ref().to_vec()
}

/// RFC 5054 Appendix A, 2048-bit group.
const N_2048: &str = concat!(
    "AC6BDB41324A9A9BF166DE5E1389582FAF72B6651987EE07FC3192943DB56050A37329CBB4A099ED8193E0757767A13DD52312AB4B03310D",
    "CD7F48A9DA04FD50E8083969EDB767B0CF6095179A163AB3661A05FBD5FAAAE82918A9962F0B93B855F97993EC975EEAA80D740ADBF4FF74",
    "7359D041D5C33EA71D281E446B14773BCA97B43A23FB801676BD207A436C6481F1D2B9078717461A5B9D32E688F87748544523B524B0D57D",
    "5EA77A2775D2ECFA032CFBDBF52FB3786160279004E57AE6AF874E7303CE53299CCC041C7BC308D82A5698F3A8D0C38271AE35F8E9DBFBB6",
    "94B5C803D89F7AE435DE236D525F54759B65E372FCD68EF20FA7111F9E4AFF73",
);

/// An SRP group.
#[derive(Debug, Clone)]
pub(crate) struct Group {
    pub n: BigUint,
    pub g: BigUint,
    pub hash: HashFn,
}

impl Group {
    pub fn gsa() -> Self {
        // A constant, valid hex string.
        let n = BigUint::parse_bytes(N_2048.as_bytes(), 16).unwrap_or_default();
        Self { n, g: BigUint::from(2u8), hash: sha256 }
    }

    /// The byte length of `N`.
    pub fn len(&self) -> usize {
        self.n.bits().div_ceil(8) as usize
    }

    /// `x` left-padded with zeros to the length of `N`.
    pub fn pad(&self, x: &BigUint) -> Vec<u8> {
        let b = x.to_bytes_be();
        let mut out = vec![0u8; self.len().saturating_sub(b.len())];
        out.extend_from_slice(&b);
        out
    }

    fn h(&self, parts: &[&[u8]]) -> Vec<u8> {
        (self.hash)(parts)
    }

    /// `k = H(N ‖ PAD(g))`.
    pub fn k(&self) -> BigUint {
        BigUint::from_bytes_be(&self.h(&[&self.n.to_bytes_be(), &self.pad(&self.g)]))
    }

    /// `u = H(PAD(A) ‖ PAD(B))`.
    pub fn u(&self, a_pub: &BigUint, b_pub: &BigUint) -> BigUint {
        BigUint::from_bytes_be(&self.h(&[&self.pad(a_pub), &self.pad(b_pub)]))
    }

    /// `x = H(s ‖ H(":" ‖ P))` (GrandSlam omits the user name).
    pub fn x(&self, salt: &[u8], password_key: &[u8]) -> BigUint {
        let inner = self.h(&[b":", password_key]);
        BigUint::from_bytes_be(&self.h(&[salt, &inner]))
    }

    /// `M1 = H(H(N) ⊕ H(PAD(g)) ‖ H(I) ‖ s ‖ PAD(A) ‖ PAD(B) ‖ K)`.
    pub fn m1(&self, user: &[u8], salt: &[u8], a_pub: &BigUint, b_pub: &BigUint, key: &[u8]) -> Vec<u8> {
        let hn = self.h(&[&self.n.to_bytes_be()]);
        let hg = self.h(&[&self.pad(&self.g)]);
        let xor: Vec<u8> = hn.iter().zip(hg.iter()).map(|(a, b)| a ^ b).collect();
        let hi = self.h(&[user]);
        self.h(&[&xor, &hi, salt, &self.pad(a_pub), &self.pad(b_pub), key])
    }

    /// `M2 = H(PAD(A) ‖ M1 ‖ K)`.
    pub fn m2(&self, a_pub: &BigUint, m1: &[u8], key: &[u8]) -> Vec<u8> {
        self.h(&[&self.pad(a_pub), m1, key])
    }

    /// The client's premaster secret `S = (B − k·g^x)^(a + u·x) mod N`, or `None` for an unsafe `B` or `u`.
    pub fn client_s(&self, a: &BigUint, a_pub: &BigUint, b_pub: &BigUint, x: &BigUint) -> Option<BigUint> {
        let zero = BigUint::from(0u8);
        if b_pub % &self.n == zero {
            return None;
        }
        let u = self.u(a_pub, b_pub);
        if u == zero {
            return None;
        }
        let kgx = (self.k() * self.g.modpow(x, &self.n)) % &self.n;
        let base = ((b_pub % &self.n) + &self.n - kgx) % &self.n;
        Some(base.modpow(&(a + u * x), &self.n))
    }
}

/// The GrandSlam password key: `PBKDF2-HMAC-SHA256(SHA256(password), salt, iterations, 32)`, where protocol
/// `s2k_fo` uses the lowercase hex string of `SHA256(password)` instead of its bytes.
pub(crate) fn password_key(password: &[u8], salt: &[u8], iterations: u32, protocol: &str) -> Zeroizing<Vec<u8>> {
    let digest = Zeroizing::new(sha256(&[password]));
    let input: Zeroizing<Vec<u8>> = if protocol == "s2k_fo" {
        Zeroizing::new(digest.iter().map(|b| format!("{b:02x}")).collect::<String>().into_bytes())
    } else {
        digest
    };
    let mut out = Zeroizing::new(vec![0u8; 32]);
    let iterations = std::num::NonZeroU32::new(iterations.max(1)).unwrap_or(std::num::NonZeroU32::MIN);
    ring::pbkdf2::derive(ring::pbkdf2::PBKDF2_HMAC_SHA256, iterations, salt, &input, &mut out);
    out
}

/// The client side of one handshake.
pub(crate) struct Client {
    group: Group,
    a: Zeroizing<Vec<u8>>,
    a_pub: BigUint,
}

/// What the client proves and expects.
pub(crate) struct Proof {
    pub m1: Vec<u8>,
    pub m2: Vec<u8>,
    /// `K`, the session key.
    pub key: Zeroizing<Vec<u8>>,
}

impl Client {
    /// A random 256-bit `a`, drawn again until `A` fills the group's length (so no padding question arises).
    pub fn new(group: Group) -> Self {
        let rng = ring::rand::SystemRandom::new();
        loop {
            let mut a = Zeroizing::new(vec![0u8; 32]);
            if ring::rand::SecureRandom::fill(&rng, &mut a).is_err() {
                continue;
            }
            let a_pub = group.g.modpow(&BigUint::from_bytes_be(&a), &group.n);
            if a_pub.to_bytes_be().len() == group.len() {
                return Self { group, a, a_pub };
            }
        }
    }

    #[cfg(test)]
    pub fn with_secret(group: Group, a: &[u8]) -> Self {
        let a_pub = group.g.modpow(&BigUint::from_bytes_be(a), &group.n);
        Self { group, a: Zeroizing::new(a.to_vec()), a_pub }
    }

    /// `A`, as sent (`A2k`).
    pub fn a_pub(&self) -> Vec<u8> {
        self.group.pad(&self.a_pub)
    }

    /// The proofs for the server's `B`, or `None` when `B` is unsafe.
    pub fn process(&self, user: &[u8], salt: &[u8], password_key: &[u8], b_pub: &[u8]) -> Option<Proof> {
        let b = BigUint::from_bytes_be(b_pub);
        let x = self.group.x(salt, password_key);
        let a = BigUint::from_bytes_be(&self.a);
        let s = self.group.client_s(&a, &self.a_pub, &b, &x)?;
        let key = Zeroizing::new((self.group.hash)(&[&s.to_bytes_be()]));
        let m1 = self.group.m1(user, salt, &self.a_pub, &b, &key);
        let m2 = self.group.m2(&self.a_pub, &m1, &key);
        Some(Proof { m1, m2, key })
    }
}

/// The server side, for the GrandSlam fake.
#[cfg(any(test, feature = "fake"))]
pub(crate) mod server {
    use super::*;

    /// A user record: salt, iterations and verifier `v = g^x`.
    #[derive(Debug, Clone)]
    pub struct Record {
        pub salt: Vec<u8>,
        pub iterations: u32,
        pub v: BigUint,
    }

    impl Record {
        pub fn new(group: &Group, password: &str, salt: &[u8], iterations: u32, protocol: &str) -> Self {
            let key = password_key(password.as_bytes(), salt, iterations, protocol);
            let x = group.x(salt, &key);
            Self { salt: salt.to_vec(), iterations, v: group.g.modpow(&x, &group.n) }
        }
    }

    /// One server handshake: `b`, `B`.
    #[derive(Debug, Clone)]
    pub struct Handshake {
        pub b: BigUint,
        pub b_pub: BigUint,
    }

    pub fn start(group: &Group, rec: &Record, b: &[u8]) -> Handshake {
        let b = BigUint::from_bytes_be(b);
        let b_pub = (group.k() * &rec.v + group.g.modpow(&b, &group.n)) % &group.n;
        Handshake { b, b_pub }
    }

    /// `(M1 expected, M2, K)` for the client's `A`.
    pub fn finish(
        group: &Group,
        rec: &Record,
        hs: &Handshake,
        user: &[u8],
        a_pub: &[u8],
    ) -> Option<(Vec<u8>, Vec<u8>, Vec<u8>)> {
        let a = BigUint::from_bytes_be(a_pub);
        if &a % &group.n == BigUint::from(0u8) {
            return None;
        }
        let u = group.u(&a, &hs.b_pub);
        let s = ((a * rec.v.modpow(&u, &group.n)) % &group.n).modpow(&hs.b, &group.n);
        let key = (group.hash)(&[&s.to_bytes_be()]);
        let m1 = group.m1(user, &rec.salt, &BigUint::from_bytes_be(a_pub), &hs.b_pub, &key);
        let m2 = group.m2(&BigUint::from_bytes_be(a_pub), &m1, &key);
        Some((m1, m2, key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sha1(parts: &[&[u8]]) -> Vec<u8> {
        let mut c = ring::digest::Context::new(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY);
        for p in parts {
            c.update(p);
        }
        c.finish().as_ref().to_vec()
    }

    fn hex(s: &str) -> BigUint {
        let clean: String = s.chars().filter(char::is_ascii_hexdigit).collect();
        BigUint::parse_bytes(clean.as_bytes(), 16).expect("hex")
    }

    /// RFC 5054 Appendix A (1024-bit) with SHA-1, the hash of Appendix B.
    fn rfc5054_1024() -> Group {
        Group {
            n: hex("EEAF0AB9 ADB38DD6 9C33F80A FA8FC5E8 60726187 75FF3C0B 9EA2314C 9C256576 D674DF74 96EA81D3 383B4813
                    D692C6E0 E0D5D8E2 50B98BE4 8E495C1D 6089DAD1 5DC7D7B4 6154D6B6 CE8EF4AD 69B15D49 82559B29 7BCF1885
                    C529F566 660E57EC 68EDBC3C 05726CC0 2FD4CBF4 976EAA9A FD5138FE 8376435B 9FC61D2F C0EB06E3"),
            g: BigUint::from(2u8),
            hash: sha1,
        }
    }

    #[test]
    fn rfc5054_appendix_b_vectors() {
        let g = rfc5054_1024();
        assert_eq!(g.k(), hex("7556AA04 5AEF2CDD 07ABAF0F 665C3E81 8913186F"));
        let salt = hex("BEB25379 D1A8581E B5A72767 3A2441EE").to_bytes_be();
        // RFC 5054's x includes the user name: H(s ‖ H("alice:password123")).
        let x = BigUint::from_bytes_be(&sha1(&[&salt, &sha1(&[b"alice:password123"])]));
        assert_eq!(x, hex("94B7555A ABE9127C C58CCF49 93DB6CF8 4D16C124"));
        let a = hex("60975527 035CF2AD 1989806F 0407210B C81EDC04 E2762A56 AFD529DD DA2D4393");
        let b = hex("E487CB59 D31AC550 471E81F0 0F6928E0 1DDA08E9 74A004F4 9E61F5D1 05284D20");
        let a_pub = g.g.modpow(&a, &g.n);
        let v = g.g.modpow(&x, &g.n);
        let b_pub = (g.k() * &v + g.g.modpow(&b, &g.n)) % &g.n;
        assert_eq!(
            b_pub,
            hex("BD0C6151 2C692C0C B6D041FA 01BB152D 4916A1E7 7AF46AE1 05393011 BAF38964 DC46A067 0DD125B9 5A981652
                 236F99D9 B681CBF8 7837EC99 6C6DA044 53728610 D0C6DDB5 8B318885 D7D82C7F 8DEB75CE 7BD4FBAA 37089E6F
                 9C6059F3 88838E7A 00030B33 1EB76840 910440B1 B27AAEAE EB4012B7 D7665238 A8E3FB00 4B117B58")
        );
        assert_eq!(g.u(&a_pub, &b_pub), hex("CE38B959 3487DA98 554ED47D 70A7AE5F 462EF019"));
        let s = g.client_s(&a, &a_pub, &b_pub, &x).expect("safe");
        assert_eq!(
            s,
            hex("B0DC82BA BCF30674 AE450C02 87745E79 90A3381F 63B387AA F271A10D 233861E3 59B48220 F7C4693C 9AE12B0A
                 6F67809F 0876E2D0 13800D6C 41BB59B6 D5979B5C 00A172B4 A2A5903A 0BDCAF8A 709585EB 2AFAFA8F 3499B200
                 210DCC1F 10EB3394 3CD67FC8 8A2F39A4 BE5BEC4E C0A3212D C346D7E4 74B29EDE 8A469FFE CA686E5A")
        );
    }

    #[test]
    fn the_gsa_group_is_2048_bits() {
        let g = Group::gsa();
        assert_eq!(g.len(), 256);
        assert_eq!(g.n.bits(), 2048);
    }

    #[test]
    fn client_and_server_agree_and_reject_a_wrong_password() {
        let g = Group::gsa();
        let salt = b"0123456789abcdef";
        for protocol in ["s2k", "s2k_fo"] {
            let rec = server::Record::new(&g, "correct horse", salt, 1000, protocol);
            let hs = server::start(&g, &rec, &[7u8; 32]);
            let client = Client::new(g.clone());
            let key = password_key(b"correct horse", salt, 1000, protocol);
            let proof = client.process(b"user@example.invalid", salt, &key, &g.pad(&hs.b_pub)).expect("safe B");
            let (m1, m2, k) = server::finish(&g, &rec, &hs, b"user@example.invalid", &client.a_pub()).expect("safe A");
            assert_eq!(proof.m1, m1, "{protocol}");
            assert_eq!(proof.m2, m2);
            assert_eq!(&proof.key[..], &k[..]);
            let wrong = password_key(b"wrong", salt, 1000, protocol);
            let bad = client.process(b"user@example.invalid", salt, &wrong, &g.pad(&hs.b_pub)).expect("safe B");
            assert_ne!(bad.m1, m1);
        }
    }

    #[test]
    fn s2k_and_s2k_fo_differ_and_an_unsafe_b_is_refused() {
        let salt = b"salt";
        assert_ne!(&password_key(b"pw", salt, 10, "s2k")[..], &password_key(b"pw", salt, 10, "s2k_fo")[..]);
        let g = Group::gsa();
        let c = Client::with_secret(g.clone(), &[3u8; 32]);
        assert!(c.process(b"u", salt, b"k", &g.n.to_bytes_be()).is_none());
        assert!(c.process(b"u", salt, b"k", &[0u8]).is_none());
    }
}
