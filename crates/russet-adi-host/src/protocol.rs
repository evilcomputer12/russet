// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The JSON Lines protocol of SIGN §7.5 (the daemon's side is `russet_anisette::local`). One request per line on
//! stdin, one answer per line on stdout; errors are `{"ok":false,"adiCode":…}` or `{"ok":false,"error":…}`.
//! Answers carry no secret material beyond the base64 fields the protocol defines, and nothing is logged.

use serde::{Deserialize, Serialize};

/// One request line.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum Request {
    #[serde(rename_all = "camelCase")]
    Load {
        lib_dir: String,
        android_id: String,
        provisioning_path: String,
    },
    IsProvisioned {
        dsid: i64,
    },
    StartProvisioning {
        dsid: i64,
        spim: String,
    },
    EndProvisioning {
        session: u32,
        ptm: String,
        tk: String,
    },
    #[serde(rename = "requestOtp")]
    Otp {
        dsid: i64,
    },
    Erase {
        dsid: i64,
    },
}

/// One answer line.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provisioned: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpim: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub otp: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adi_code: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Response {
    pub fn ok() -> Self {
        Self { ok: true, ..Self::default() }
    }

    pub fn adi(code: i64) -> Self {
        Self { ok: false, adi_code: Some(code), ..Self::default() }
    }

    pub fn error(msg: impl Into<String>) -> Self {
        Self { ok: false, error: Some(msg.into()), ..Self::default() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_parse_and_errors_serialize() {
        let r: Request = serde_json::from_str(
            r#"{"op":"load","libDir":"/l","androidId":"0123456789abcdef","provisioningPath":"/p"}"#,
        )
        .expect("parses");
        assert!(matches!(r, Request::Load { .. }));
        let r: Request =
            serde_json::from_str(r#"{"op":"endProvisioning","session":1,"ptm":"AA==","tk":"AA=="}"#).expect("parses");
        assert_eq!(r, Request::EndProvisioning { session: 1, ptm: "AA==".into(), tk: "AA==".into() });
        let e = serde_json::to_string(&Response::adi(-45061)).expect("json");
        assert_eq!(e, r#"{"ok":false,"adiCode":-45061}"#);
    }
}
