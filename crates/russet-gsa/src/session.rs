// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! GrandSlam session material (SIGN §7.3) and its sealed form (`secret:apple-id-session/<DSID>`). Every type here
//! keeps tokens in zeroized buffers and has a `Debug` that prints no token, key or Apple ID.

use base64::Engine as _;
use russet_core::Result;
use russet_core::error::{Error, codes};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use zeroize::Zeroizing;

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn unb64(s: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD.decode(s).ok()
}

/// `c` as GrandSlam sent it (data in `spd`; echoed back unchanged).
#[derive(Clone, PartialEq, Eq)]
pub enum Cookie {
    Data(Vec<u8>),
    Text(String),
}

impl Cookie {
    pub(crate) fn to_plist(&self) -> plist::Value {
        match self {
            Self::Data(d) => plist::Value::Data(d.clone()),
            Self::Text(s) => plist::Value::String(s.clone()),
        }
    }

    pub(crate) fn from_plist(v: &plist::Value) -> Option<Self> {
        match v {
            plist::Value::Data(d) => Some(Self::Data(d.clone())),
            plist::Value::String(s) => Some(Self::Text(s.clone())),
            _ => None,
        }
    }
}

/// The decrypted `spd` of a successful (or two-factor-pending) SRP login.
#[derive(Clone)]
pub struct GsSession {
    /// The alternate DSID (`adsid`): what tokens and DS2 requests are issued for.
    pub adsid: String,
    /// `DsPrsId`: the account's DSID (the id of `appleid:<DSID>`); `adsid` when Apple omits it.
    pub dsid: String,
    pub(crate) idms_token: Zeroizing<String>,
    pub(crate) sk: Zeroizing<Vec<u8>>,
    pub(crate) c: Cookie,
}

impl std::fmt::Debug for GsSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GsSession").finish_non_exhaustive()
    }
}

impl GsSession {
    /// `X-Apple-Identity-Token`: `base64(adsid ":" GsIdmsToken)`.
    pub(crate) fn identity_token(&self) -> Zeroizing<String> {
        Zeroizing::new(b64(format!("{}:{}", self.adsid, self.idms_token.as_str()).as_bytes()))
    }
}

/// How Apple asks for the second factor (`Status.au`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecondFactor {
    /// `trustedDeviceSecondaryAuth`: a push to trusted devices (SMS also possible).
    TrustedDevice,
    /// `secondaryAuth`: SMS to a trusted phone number.
    Sms,
}

/// A login waiting for its two-factor code.
#[derive(Debug, Clone)]
pub struct PendingTwoFactor {
    pub(crate) session: GsSession,
    pub kind: SecondFactor,
}

/// Where a code is requested or checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TwoFactorVia {
    TrustedDevice,
    Sms { phone_id: u32 },
}

/// A trusted phone number (Apple's obfuscated form only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedNumber {
    pub id: u32,
    pub obfuscated: String,
}

/// The outcome of [`crate::GsaClient::login`].
#[derive(Debug)]
pub enum LoginState {
    Authenticated(GsSession),
    NeedsTwoFactor(PendingTwoFactor),
}

/// The Xcode app token (`com.apple.gs.xcode.auth`) DS2 requests carry.
#[derive(Clone)]
pub struct XcodeToken {
    pub adsid: String,
    pub(crate) token: Zeroizing<String>,
    pub expires_at: OffsetDateTime,
}

impl std::fmt::Debug for XcodeToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("XcodeToken").field("expires_at", &self.expires_at).finish_non_exhaustive()
    }
}

impl XcodeToken {
    /// Whether the token is valid for at least `margin` more.
    pub fn fresh(&self, now: OffsetDateTime, margin: time::Duration) -> bool {
        self.expires_at - margin > now
    }
}

/// The sealed form of a signed-in Apple ID (`AppleIdSession`, JSON). It holds no password.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredSession {
    pub v: u32,
    pub adsid: String,
    pub dsid: String,
    idms_token: String,
    sk: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    c_data: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    c_text: Option<String>,
    xcode_token: String,
    /// RFC 3339.
    pub xcode_token_expires: String,
}

impl std::fmt::Debug for StoredSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredSession").field("xcode_token_expires", &self.xcode_token_expires).finish_non_exhaustive()
    }
}

fn corrupt() -> Error {
    Error::new(codes::gsa::SESSION_EXPIRED, "the stored Apple ID session is unreadable")
        .with_hint("Sign in again (the Apple ID page of the IDE, or `russet account apple-id login`).")
}

impl StoredSession {
    pub fn new(s: &GsSession, t: &XcodeToken) -> Self {
        let (c_data, c_text) = match &s.c {
            Cookie::Data(d) => (Some(b64(d)), None),
            Cookie::Text(t) => (None, Some(t.clone())),
        };
        Self {
            v: 1,
            adsid: s.adsid.clone(),
            dsid: s.dsid.clone(),
            idms_token: s.idms_token.to_string(),
            sk: b64(&s.sk),
            c_data,
            c_text,
            xcode_token: t.token.to_string(),
            xcode_token_expires: t
                .expires_at
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default(),
        }
    }

    pub fn to_bytes(&self) -> Zeroizing<Vec<u8>> {
        Zeroizing::new(serde_json::to_vec(self).unwrap_or_default())
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        serde_json::from_slice(bytes).map_err(|_| corrupt())
    }

    pub fn session(&self) -> Result<GsSession> {
        let c = match (&self.c_data, &self.c_text) {
            (Some(d), _) => Cookie::Data(unb64(d).ok_or_else(corrupt)?),
            (None, Some(t)) => Cookie::Text(t.clone()),
            (None, None) => return Err(corrupt()),
        };
        Ok(GsSession {
            adsid: self.adsid.clone(),
            dsid: self.dsid.clone(),
            idms_token: Zeroizing::new(self.idms_token.clone()),
            sk: Zeroizing::new(unb64(&self.sk).ok_or_else(corrupt)?),
            c,
        })
    }

    pub fn token(&self) -> Result<XcodeToken> {
        let expires_at =
            OffsetDateTime::parse(&self.xcode_token_expires, &time::format_description::well_known::Rfc3339)
                .map_err(|_| corrupt())?;
        Ok(XcodeToken { adsid: self.adsid.clone(), token: Zeroizing::new(self.xcode_token.clone()), expires_at })
    }

    /// Replaces the Xcode token (after [`crate::GsaClient::xcode_token`] refreshed it).
    pub fn set_token(&mut self, t: &XcodeToken) {
        self.xcode_token = t.token.to_string();
        self.xcode_token_expires =
            t.expires_at.format(&time::format_description::well_known::Rfc3339).unwrap_or_default();
    }
}

impl Drop for StoredSession {
    fn drop(&mut self) {
        use zeroize::Zeroize as _;
        self.idms_token.zeroize();
        self.sk.zeroize();
        self.xcode_token.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_sessions_round_trip_and_debug_hides_them() {
        let s = GsSession {
            adsid: "000123-00-canary".into(),
            dsid: "4242".into(),
            idms_token: Zeroizing::new("canary-idms".into()),
            sk: Zeroizing::new(vec![1, 2, 3]),
            c: Cookie::Data(vec![4, 5]),
        };
        let t = XcodeToken {
            adsid: s.adsid.clone(),
            token: Zeroizing::new("canary-xcode-token".into()),
            expires_at: OffsetDateTime::UNIX_EPOCH + time::Duration::days(20000),
        };
        let stored = StoredSession::new(&s, &t);
        let back = StoredSession::from_bytes(&stored.to_bytes()).expect("parses");
        let s2 = back.session().expect("session");
        assert_eq!(s2.idms_token.as_str(), "canary-idms");
        assert_eq!(&s2.sk[..], &[1, 2, 3]);
        assert!(s2.c == Cookie::Data(vec![4, 5]));
        assert_eq!(back.token().expect("token").token.as_str(), "canary-xcode-token");
        let text = format!("{s:?} {t:?} {back:?} {:?}", PendingTwoFactor { session: s2, kind: SecondFactor::Sms });
        assert!(!text.contains("canary"), "{text}");
    }
}
