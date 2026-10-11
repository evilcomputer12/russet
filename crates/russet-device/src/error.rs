// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Device errors (device.md §9, ARCHITECTURE §13.3–§13.4): `russet_core::Error` values with `device`
//! (3000–3099, 3200–3299) and `app` (3100–3199) codes, each with its standard hint, plus the CoreDevice
//! NSError chain decoder and the mapping of idevice errors.
//!
//! Logs never print raw `userInfoWithNSSecureCoding` blobs: [`NsErrorChain`] keeps only domain, code,
//! description and failure reason of each link.

use std::fmt;

use idevice::IdeviceError;
use russet_core::error::{Error, ErrorCode, codes};

/// The standard next step for a device or app error code (ARCHITECTURE §13.1: every user-visible error has a
/// hint when a next step exists).
pub fn hint_for(code: ErrorCode) -> Option<&'static str> {
    use codes::{app, device};
    let hint = match code {
        c if c == device::NOT_FOUND => {
            "Check the cable and run `russet device list`; pass `--device <udid|alias>` when several are attached."
        }
        c if c == device::NOT_PAIRED => "Run `russet device pair`, then unlock the iPhone and tap Trust.",
        c if c == device::LOCKED => "Unlock the iPhone and keep it unlocked during the operation.",
        c if c == device::UNTRUSTED => {
            "The iPhone refused to trust this computer: reset Location & Privacy on the iPhone, then run `russet device pair` again."
        }
        c if c == device::DEVELOPER_MODE_DISABLED => {
            "Turn on Settings > Privacy & Security > Developer Mode on the iPhone and restart it."
        }
        c if c == device::DDI_NOT_MOUNTED => {
            "Run `russet device prepare` to mount the Developer Disk Image from `[device.ddi]`."
        }
        c if c == device::TUNNEL_FAILED => "Retry; if it persists, replug the cable and run `russet doctor`.",
        c if c == device::DISCONNECTED => "Reconnect the iPhone and retry.",
        c if c == device::USBMUX_UNAVAILABLE => {
            "Start usbmuxd (Linux: `systemctl start usbmuxd`, or replug the iPhone) or the Apple Mobile Device Service (Windows); run `russet doctor`."
        }
        c if c == device::TRUST_PENDING => "Unlock the iPhone and tap Trust in the dialog.",
        c if c == device::DDI_PERSONALIZATION_FAILED => {
            "Check the network connection to Apple's signing server (gs.apple.com) and that the DDI matches this iOS version; retry."
        }
        c if c == device::TUNNEL_STALLED => {
            "Retry; replug the cable if it happens again, and report it with `russet bugreport`."
        }
        c if c == device::SERVICE_UNAVAILABLE => {
            "Mount the Developer Disk Image with `russet device prepare`; if it is mounted, the service is missing on this iOS version."
        }
        c if c == device::SERVICE_NOT_IMPLEMENTED => "This iOS version does not implement the feature.",
        c if c == device::TIMEOUT => "Retry; keep the iPhone unlocked and connected.",
        c if c == device::UNSUPPORTED => "This operation is not supported for this device, iOS version or transport.",
        c if c == device::IO => "Retry; replug the iPhone if it happens again.",
        c if c == device::PROTOCOL => "Retry; if it persists, report it with `russet bugreport`.",
        c if c == device::RESERVED => {
            "Another client reserved this device; wait for it to finish, or ask it to release the lease (`russet daemon status` lists leases)."
        }
        c if c == device::LIVE_BACKEND_REFUSED => {
            "Development daemons need RUSSET_LANE=A for the live device; otherwise use RUSSET_DEVICE_BACKEND=scripted:<scenario.toml>."
        }
        c if c == device::DDI_SOURCE_MISSING => {
            "russet sdk import --ddi-only <Xcode_27.xip | XcodeSystemResources.pkg | a copy of a Mac's /Library/Developer/DeveloperDiskImages/iOS_DDI>, or set `[device.ddi] dir`."
        }
        c if c == device::DDI_INCOMPATIBLE => "Import the DDI of an Xcode that supports this iOS version.",
        c if c == device::DDI_MIRROR_CONSENT_REQUIRED => {
            "Mirror DDIs are Apple files from an unlicensed third-party mirror; set `[device.ddi] allow-mirror = true` only if you accept that."
        }
        c if c == device::FALLBACK_UNAVAILABLE => {
            "Install pymobiledevice3 >= 11.26 (`uv tool install pymobiledevice3`)."
        }
        c if c == app::NOT_INSTALLED => "Install the app first: `russet install <app|ipa>` or `russet run`.",
        c if c == app::INSTALL_FAILED => {
            "Check the signature, the provisioning profile (does it list this device?) and the minimum iOS version; `russet sign verify <app>` helps."
        }
        c if c == app::LAUNCH_FAILED => {
            "Unlock the iPhone, check that the app is installed and signed for this device, and retry."
        }
        _ => return None,
    };
    Some(hint)
}

/// A device- or app-family error with its standard hint.
pub fn device_error(code: ErrorCode, msg: impl Into<String>) -> Error {
    let e = Error::new(code, msg);
    let retryable = [
        codes::device::LOCKED,
        codes::device::TIMEOUT,
        codes::device::TUNNEL_FAILED,
        codes::device::TUNNEL_STALLED,
        codes::device::DISCONNECTED,
        codes::device::TRUST_PENDING,
        codes::device::DDI_PERSONALIZATION_FAILED,
        codes::device::IO,
    ]
    .contains(&code);
    let e = e.retryable(retryable);
    match hint_for(code) {
        Some(h) => e.with_hint(h),
        None => e,
    }
}

/// A plain-text cause for [`Error::caused_by`] (the decoded NSError chain or an idevice error rendering).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cause(pub String);

impl fmt::Display for Cause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Cause {}

/// Maps an idevice error to a device-family error. `what` names the operation for the message.
pub fn from_idevice(e: IdeviceError, what: &str) -> Error {
    let code = classify_idevice(&e);
    let detail = render_idevice(&e);
    let msg = format!("{what}: {detail}");
    let err = device_error(code, msg);
    if code == codes::app::INSTALL_FAILED {
        let subkind = install_subkind(&detail);
        return err.with_details(serde_json::json!({ "subkind": subkind }));
    }
    err.caused_by(Cause(detail))
}

/// The code an idevice error maps to (ARCHITECTURE §13.3 remap of device.md §9).
pub fn classify_idevice(e: &IdeviceError) -> ErrorCode {
    use codes::{app, device};
    match e {
        IdeviceError::Socket(io) => classify_io(io),
        IdeviceError::Timeout => device::TIMEOUT,
        IdeviceError::DeviceNotFound => device::NOT_FOUND,
        IdeviceError::InvalidHostID => device::NOT_PAIRED,
        IdeviceError::PairingDialogResponsePending => device::TRUST_PENDING,
        IdeviceError::UserDeniedPairing => device::UNTRUSTED,
        IdeviceError::PasswordProtected | IdeviceError::DeviceLocked => device::LOCKED,
        IdeviceError::DeveloperModeNotEnabled => device::DEVELOPER_MODE_DISABLED,
        IdeviceError::ImageNotMounted => device::DDI_NOT_MOUNTED,
        IdeviceError::ServiceNotFound => device::SERVICE_UNAVAILABLE,
        IdeviceError::BadBuildManifest => device::DDI_INCOMPATIBLE,
        IdeviceError::ApplicationVerificationFailed(_) => app::INSTALL_FAILED,
        IdeviceError::Usbmuxd(u) => {
            use idevice::usbmuxd::errors::UsbmuxdError;
            match u {
                UsbmuxdError::BadDevice => device::DISCONNECTED,
                UsbmuxdError::ConnectionRefused => device::SERVICE_UNAVAILABLE,
                _ => device::PROTOCOL,
            }
        }
        IdeviceError::UnknownErrorType(s) | IdeviceError::InternalError(s) => classify_text(s),
        IdeviceError::InstallationProxy(_) => app::INSTALL_FAILED,
        IdeviceError::Cryptexd(c) => {
            use idevice::cryptexd::CryptexdError;
            match c {
                CryptexdError::BadDdiBundle(_) => device::DDI_SOURCE_MISSING,
                _ => device::DDI_PERSONALIZATION_FAILED,
            }
        }
        IdeviceError::Http(_) => device::DDI_PERSONALIZATION_FAILED,
        IdeviceError::Rustls(_) | IdeviceError::PemParseFailed(_) | IdeviceError::TlsBuilderFailed(_) => {
            device::NOT_PAIRED
        }
        IdeviceError::CdTunnel(_) => device::TUNNEL_FAILED,
        _ => device::PROTOCOL,
    }
}

/// The code of an I/O error on a device stream.
pub fn classify_io(io: &std::io::Error) -> ErrorCode {
    use std::io::ErrorKind as K;
    match io.kind() {
        K::TimedOut => codes::device::TIMEOUT,
        K::BrokenPipe | K::UnexpectedEof | K::ConnectionReset | K::ConnectionAborted | K::NotConnected => {
            codes::device::DISCONNECTED
        }
        K::NetworkUnreachable => codes::device::TUNNEL_FAILED,
        _ => codes::device::IO,
    }
}

/// Classifies a lockdown or installation_proxy error string (`Error` plus `ErrorDescription`).
fn classify_text(s: &str) -> ErrorCode {
    use codes::{app, device};
    let lower = s.to_ascii_lowercase();
    if lower.contains("devicelocked") || lower.contains("passwordprotected") {
        device::LOCKED
    } else if lower.contains("developer mode") {
        device::DEVELOPER_MODE_DISABLED
    } else if install_subkind_opt(s).is_some() || lower.contains("installation") || lower.contains("verification") {
        app::INSTALL_FAILED
    } else if lower.contains("applicationnotinstalled") || lower.contains("not installed") {
        app::NOT_INSTALLED
    } else {
        device::PROTOCOL
    }
}

/// `details.subkind` of `app.installFailed` (ARCHITECTURE §13.3): `signature`, `no-matching-profile`,
/// `minimum-os` or `app-id-mismatch`; `other` when none matches.
pub fn install_subkind(detail: &str) -> &'static str {
    install_subkind_opt(detail).unwrap_or("other")
}

fn install_subkind_opt(detail: &str) -> Option<&'static str> {
    let lower = detail.to_ascii_lowercase();
    if lower.contains("0xe8008015") || lower.contains("provisioning profile") {
        Some("no-matching-profile")
    } else if lower.contains("deviceosversiontoolow") || lower.contains("minimumosversion") {
        Some("minimum-os")
    } else if lower.contains("mismatchedapplicationidentifierentitlement") {
        Some("app-id-mismatch")
    } else if lower.contains("applicationverificationfailed")
        || lower.contains("signature")
        || lower.contains("0xe8008017")
        || lower.contains("0xe800801c")
    {
        Some("signature")
    } else {
        None
    }
}

/// Renders an idevice error without debug blobs: the variant's message plus its source chain.
pub fn render_idevice(e: &IdeviceError) -> String {
    let mut out = e.to_string();
    let mut next = std::error::Error::source(e);
    while let Some(s) = next {
        let text = s.to_string();
        if !out.contains(&text) {
            out.push_str(": ");
            out.push_str(&text);
        }
        next = s.source();
    }
    match e {
        IdeviceError::UnknownErrorType(s) | IdeviceError::InternalError(s) | IdeviceError::UnexpectedResponse(s)
            if !out.contains(s.as_str()) =>
        {
            out.push_str(": ");
            out.push_str(s);
        }
        IdeviceError::ApplicationVerificationFailed(s) if !out.contains(s.as_str()) => {
            out.push_str(": ");
            out.push_str(s);
        }
        _ => {}
    }
    out
}

/// One link of a decoded NSError chain (outermost first).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NsErrorLink {
    pub domain: String,
    pub code: i64,
    pub description: Option<String>,
    pub failure_reason: Option<String>,
    /// `BSErrorCodeDescription` (FrontBoard), for example `Locked` or `RequestDenied`.
    pub code_description: Option<String>,
}

impl fmt::Display for NsErrorLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.domain, self.code)?;
        if let Some(c) = &self.code_description {
            write!(f, " ({c})")?;
        }
        if let Some(d) = &self.description {
            write!(f, " \"{d}\"")?;
        }
        Ok(())
    }
}

/// A CoreDevice `CoreDevice.error` dictionary decoded into its NSError links (device.md §9).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NsErrorChain {
    pub links: Vec<NsErrorLink>,
}

/// Nesting is two or three levels on the device; anything deeper is cut off.
const MAX_CHAIN_DEPTH: usize = 8;

impl NsErrorChain {
    /// Walks `domain`/`code`/`userInfo` and `NSUnderlyingError` (also inside `userInfo`).
    pub fn from_plist(v: &plist::Value) -> Self {
        let mut links = Vec::new();
        let mut cur = Some(v);
        while let Some(value) = cur.take() {
            if links.len() >= MAX_CHAIN_DEPTH {
                break;
            }
            let Some(d) = value.as_dictionary() else { break };
            let domain = d.get("domain").and_then(|x| x.as_string());
            let code = d.get("code").and_then(int_of);
            let user_info = d.get("userInfo").and_then(|x| x.as_dictionary());
            if let (Some(domain), Some(code)) = (domain, code) {
                let s = |k: &str| user_info.and_then(|u| u.get(k)).and_then(string_of);
                links.push(NsErrorLink {
                    domain: domain.to_owned(),
                    code,
                    description: s("NSLocalizedDescription"),
                    failure_reason: s("NSLocalizedFailureReason"),
                    code_description: s("BSErrorCodeDescription"),
                });
            }
            cur = user_info.and_then(|u| u.get("NSUnderlyingError")).or_else(|| d.get("NSUnderlyingError"));
        }
        Self { links }
    }

    /// True when some link has this domain and code.
    pub fn contains(&self, domain: &str, code: i64) -> bool {
        self.links.iter().any(|l| l.domain == domain && l.code == code)
    }

    /// The Russet code for this chain (device.md §9 mapping rules on `(domain, code)`).
    pub fn classify(&self) -> ErrorCode {
        use codes::{app, device};
        if self.contains("FBSOpenApplicationErrorDomain", 7)
            || self.links.iter().any(|l| l.code_description.as_deref() == Some("Locked"))
        {
            return device::LOCKED;
        }
        if self.contains("CoreDevice.ActionError", 2) {
            return device::SERVICE_NOT_IMPLEMENTED;
        }
        let text = self.to_string().to_ascii_lowercase();
        if text.contains("developer mode") {
            return device::DEVELOPER_MODE_DISABLED;
        }
        if self.contains("FBSOpenApplicationErrorDomain", 4)
            || text.contains("not installed")
            || text.contains("could not be found")
            || text.contains("no application")
        {
            return app::NOT_INSTALLED;
        }
        app::LAUNCH_FAILED
    }

    /// The chain as `error.data.chain` strings, outermost first.
    pub fn lines(&self) -> Vec<String> {
        self.links.iter().map(ToString::to_string).collect()
    }

    /// The most specific human text: the innermost description, else the outermost.
    pub fn summary(&self) -> Option<String> {
        self.links.iter().rev().find_map(|l| l.description.clone())
    }

    /// A Russet error for a failed CoreDevice action.
    pub fn to_error(&self, what: &str) -> Error {
        let code = self.classify();
        let msg = match self.summary() {
            Some(s) => format!("{what}: {s}"),
            None => format!("{what} failed"),
        };
        device_error(code, msg)
            .with_details(serde_json::json!({ "nserror": self.lines() }))
            .caused_by(Cause(self.to_string()))
    }
}

impl fmt::Display for NsErrorChain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, l) in self.links.iter().enumerate() {
            if i > 0 {
                f.write_str(" <- ")?;
            }
            write!(f, "{l}")?;
        }
        Ok(())
    }
}

fn int_of(v: &plist::Value) -> Option<i64> {
    match v {
        plist::Value::Integer(i) => i.as_signed().or_else(|| i.as_unsigned().and_then(|u| i64::try_from(u).ok())),
        plist::Value::Real(r) => Some(*r as i64),
        plist::Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn string_of(v: &plist::Value) -> Option<String> {
    match v {
        plist::Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

/// `core.invalidArgument` with a message (validation inside device handlers).
pub fn invalid_argument(msg: impl Into<String>) -> Error {
    Error::new(codes::core::INVALID_ARGUMENT, msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict(pairs: Vec<(&str, plist::Value)>) -> plist::Value {
        let mut d = plist::Dictionary::new();
        for (k, v) in pairs {
            d.insert(k.to_owned(), v);
        }
        plist::Value::Dictionary(d)
    }

    /// The "Locked" error of device.md Appendix A.7, as CoreDevice sends it (`userInfo` trimmed).
    fn locked_fixture() -> plist::Value {
        let inner = dict(vec![
            ("domain", "FBSOpenApplicationErrorDomain".into()),
            ("code", 7.into()),
            ("userInfo", dict(vec![("BSErrorCodeDescription", "Locked".into())])),
        ]);
        let middle = dict(vec![
            ("domain", "FBSOpenApplicationServiceErrorDomain".into()),
            ("code", 1.into()),
            (
                "userInfo",
                dict(vec![
                    ("BSErrorCodeDescription", "RequestDenied".into()),
                    (
                        "NSLocalizedFailureReason",
                        "The request was denied by service delegate (SBMainWorkspace) for reason: Locked".into(),
                    ),
                    ("NSUnderlyingError", inner),
                ]),
            ),
        ]);
        dict(vec![
            ("domain", "com.apple.dt.CoreDeviceError".into()),
            ("code", 10002.into()),
            (
                "userInfo",
                dict(vec![
                    ("NSLocalizedDescription", "The application failed to launch.".into()),
                    ("NSUnderlyingError", middle),
                    ("userInfoWithNSSecureCoding", plist::Value::Data(vec![0; 64])),
                ]),
            ),
        ])
    }

    #[test]
    fn the_locked_fixture_maps_to_device_locked_with_its_remedy() {
        let chain = NsErrorChain::from_plist(&locked_fixture());
        assert_eq!(chain.links.len(), 3);
        assert_eq!(chain.links[0].domain, "com.apple.dt.CoreDeviceError");
        assert_eq!(chain.links[2].code_description.as_deref(), Some("Locked"));
        assert_eq!(chain.classify(), codes::device::LOCKED);
        let e = chain.to_error("launch");
        assert_eq!(e.code(), codes::device::LOCKED);
        assert!(e.hint().is_some_and(|h| h.contains("Unlock")));
        // The decoded chain, never the raw secure-coding blob.
        let data = e.to_data();
        assert!(data.chain[0].contains("FBSOpenApplicationErrorDomain 7 (Locked)"), "{:?}", data.chain);
        assert!(!data.chain[0].contains("SecureCoding"));
        assert!(data.retryable);
    }

    #[test]
    fn action_error_2_is_service_not_implemented() {
        let v = dict(vec![
            ("domain", "CoreDevice.ActionError".into()),
            ("code", 2.into()),
            (
                "userInfo",
                dict(vec![(
                    "NSLocalizedDescription",
                    "Action 'com.apple.coredevice.feature.getlockstate' is not implemented".into(),
                )]),
            ),
        ]);
        let chain = NsErrorChain::from_plist(&v);
        assert_eq!(chain.classify(), codes::device::SERVICE_NOT_IMPLEMENTED);
        assert_eq!(
            chain.summary().as_deref(),
            Some("Action 'com.apple.coredevice.feature.getlockstate' is not implemented")
        );
    }

    #[test]
    fn unknown_chains_are_launch_failures_and_depth_is_bounded() {
        let mut v = dict(vec![("domain", "X".into()), ("code", 1.into())]);
        for i in 0..20 {
            v = dict(vec![
                ("domain", format!("D{i}").into()),
                ("code", i.into()),
                ("userInfo", dict(vec![("NSUnderlyingError", v)])),
            ]);
        }
        let chain = NsErrorChain::from_plist(&v);
        assert_eq!(chain.links.len(), MAX_CHAIN_DEPTH);
        assert_eq!(chain.classify(), codes::app::LAUNCH_FAILED);
    }

    #[test]
    fn idevice_errors_map_to_device_codes() {
        assert_eq!(classify_idevice(&IdeviceError::InvalidHostID), codes::device::NOT_PAIRED);
        assert_eq!(classify_idevice(&IdeviceError::PairingDialogResponsePending), codes::device::TRUST_PENDING);
        assert_eq!(classify_idevice(&IdeviceError::UserDeniedPairing), codes::device::UNTRUSTED);
        assert_eq!(classify_idevice(&IdeviceError::PasswordProtected), codes::device::LOCKED);
        assert_eq!(classify_idevice(&IdeviceError::DeveloperModeNotEnabled), codes::device::DEVELOPER_MODE_DISABLED);
        assert_eq!(classify_idevice(&IdeviceError::ImageNotMounted), codes::device::DDI_NOT_MOUNTED);
        assert_eq!(classify_idevice(&IdeviceError::ServiceNotFound), codes::device::SERVICE_UNAVAILABLE);
        let eof = IdeviceError::Socket(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
        assert_eq!(classify_idevice(&eof), codes::device::DISCONNECTED);
        let e = from_idevice(
            IdeviceError::ApplicationVerificationFailed(
                "Failed to verify code signature of x : 0xe8008015 (A valid provisioning profile for this executable was not found.)"
                    .into(),
            ),
            "install",
        );
        assert_eq!(e.code(), codes::app::INSTALL_FAILED);
        assert_eq!(e.to_data().details, Some(serde_json::json!({"subkind": "no-matching-profile"})));
    }

    #[test]
    fn install_subkinds_follow_the_contract() {
        assert_eq!(install_subkind("DeviceOSVersionTooLow (needs 28.0)"), "minimum-os");
        assert_eq!(install_subkind("MismatchedApplicationIdentifierEntitlement"), "app-id-mismatch");
        assert_eq!(install_subkind("ApplicationVerificationFailed: bad signature"), "signature");
        assert_eq!(install_subkind("something else"), "other");
    }

    #[test]
    fn every_device_and_app_code_has_a_hint() {
        for c in codes::device::ALL.iter().chain(codes::app::ALL) {
            assert!(hint_for(*c).is_some(), "{} has no hint", c.as_str());
        }
    }
}
