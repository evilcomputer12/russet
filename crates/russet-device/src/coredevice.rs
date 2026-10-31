// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! A CoreDevice RemoteXPC client (`com.apple.coredevice.*` services; device.md §6.4–§6.5).
//!
//! idevice's `CoreDeviceServiceClient` turns a `CoreDevice.error` reply into a `Debug` string, which loses the
//! NSError chain the decoder of [`crate::error::NsErrorChain`] needs (Locked, ActionError 2, …). This client builds
//! the same request envelope on idevice's public `RemoteXpcClient` and keeps the error as a plist.

use idevice::xpc::{Dictionary, XPCObject};
use idevice::{ReadWrite, RemoteXpcClient};
use russet_core::Result;
use russet_core::error::codes;

use crate::error::{NsErrorChain, device_error, from_idevice};

/// The CoreDevice version Russet announces (idevice uses the same).
pub const CORE_DEVICE_VERSION: &str = "629.3";

/// The appservice features Russet uses.
pub mod feature {
    pub const LAUNCH_APPLICATION: &str = "com.apple.coredevice.feature.launchapplication";
    pub const LIST_PROCESSES: &str = "com.apple.coredevice.feature.listprocesses";
    pub const SEND_SIGNAL: &str = "com.apple.coredevice.feature.sendsignaltoprocess";
    pub const UNINSTALL_APP: &str = "com.apple.coredevice.feature.uninstallapp";
    pub const LIST_APPS: &str = "com.apple.coredevice.feature.listapps";
}

/// RSD service names.
pub mod service {
    pub const APP_SERVICE: &str = "com.apple.coredevice.appservice";
    pub const OPEN_STDIO_SOCKET: &str = "com.apple.coredevice.openstdiosocket";
    pub const DT_SERVICE_HUB: &str = "com.apple.instruments.dtservicehub";
    pub const SCREEN_CAPTURE: &str = "com.apple.coredevice.screencaptureservice";
    pub const CRYPTEXD: &str = "com.apple.security.cryptexd.remote";
    pub const DEBUG_PROXY: &str = "com.apple.internal.dt.remote.debugproxy";
}

/// A failed CoreDevice invocation.
#[derive(Debug)]
pub enum InvokeError {
    /// The device answered with `CoreDevice.error` (an NSError dictionary).
    Device(plist::Value),
    /// Transport or format problem.
    Transport(russet_core::Error),
}

impl InvokeError {
    /// As a Russet error; `what` names the action.
    pub fn into_error(self, what: &str) -> russet_core::Error {
        match self {
            Self::Device(v) => NsErrorChain::from_plist(&v).to_error(what),
            Self::Transport(e) => e,
        }
    }
}

/// One CoreDevice service connection.
#[derive(Debug)]
pub struct CoreDeviceClient {
    inner: RemoteXpcClient<Box<dyn ReadWrite>>,
}

/// The `{components, originalComponentsCount, stringValue}` version dictionary CoreDevice expects.
pub fn version_dictionary(version: &str) -> Dictionary {
    let components: Vec<XPCObject> = version.split('.').filter_map(|c| c.parse().ok()).map(XPCObject::UInt64).collect();
    let mut d = Dictionary::new();
    d.insert("originalComponentsCount".into(), XPCObject::Int64(components.len() as i64));
    d.insert("components".into(), XPCObject::Array(components));
    d.insert("stringValue".into(), XPCObject::String(version.to_owned()));
    d
}

/// The request envelope of one feature invocation.
pub fn invocation(feature: &str, input: Dictionary) -> Dictionary {
    let mut req = Dictionary::new();
    req.insert("CoreDevice.CoreDeviceDDIProtocolVersion".into(), XPCObject::Int64(0));
    req.insert("CoreDevice.action".into(), XPCObject::Dictionary(Dictionary::new()));
    req.insert("CoreDevice.coreDeviceVersion".into(), XPCObject::Dictionary(version_dictionary(CORE_DEVICE_VERSION)));
    req.insert("CoreDevice.deviceIdentifier".into(), XPCObject::String(uuid::Uuid::new_v4().to_string()));
    req.insert("CoreDevice.featureIdentifier".into(), XPCObject::String(feature.to_owned()));
    req.insert("CoreDevice.input".into(), XPCObject::Dictionary(input));
    req.insert("CoreDevice.invocationIdentifier".into(), XPCObject::String(uuid::Uuid::new_v4().to_string()));
    req
}

/// Splits a reply into the output or the device's error.
pub fn parse_reply(reply: plist::Value) -> std::result::Result<plist::Value, InvokeError> {
    let mut d = match reply {
        plist::Value::Dictionary(d) => d,
        _ => {
            return Err(InvokeError::Transport(device_error(
                codes::device::PROTOCOL,
                "CoreDevice reply is not a dictionary",
            )));
        }
    };
    if let Some(out) = d.remove("CoreDevice.output") {
        return Ok(out);
    }
    match d.remove("CoreDevice.error") {
        Some(e) => Err(InvokeError::Device(e)),
        None => Err(InvokeError::Transport(device_error(
            codes::device::PROTOCOL,
            "CoreDevice reply has neither output nor error",
        ))),
    }
}

impl CoreDeviceClient {
    /// Performs the RemoteXPC handshake on `stream`.
    pub async fn new(stream: Box<dyn ReadWrite>) -> Result<Self> {
        let mut inner = RemoteXpcClient::new(stream).await.map_err(|e| from_idevice(e, "CoreDevice connect"))?;
        inner.do_handshake().await.map_err(|e| from_idevice(e, "CoreDevice handshake"))?;
        Ok(Self { inner })
    }

    /// Invokes one feature.
    pub async fn invoke(&mut self, feature: &str, input: Dictionary) -> std::result::Result<plist::Value, InvokeError> {
        let req = invocation(feature, input);
        self.inner
            .send_object(XPCObject::Dictionary(req), true)
            .await
            .map_err(|e| InvokeError::Transport(from_idevice(e, feature)))?;
        let reply = self.inner.recv().await.map_err(|e| InvokeError::Transport(from_idevice(e, feature)))?;
        parse_reply(reply)
    }
}

/// A plist dictionary → XPC dictionary without idevice's panicking `From<plist::Value>` (dates, UIDs and
/// unsigned integers above `i64::MAX` are converted, never `todo!()`).
pub fn xpc_from_plist(v: &plist::Value) -> XPCObject {
    match v {
        plist::Value::Array(a) => XPCObject::Array(a.iter().map(xpc_from_plist).collect()),
        plist::Value::Dictionary(d) => {
            let mut out = Dictionary::new();
            for (k, v) in d {
                out.insert(k.clone(), xpc_from_plist(v));
            }
            XPCObject::Dictionary(out)
        }
        plist::Value::Boolean(b) => XPCObject::Bool(*b),
        plist::Value::Data(d) => XPCObject::Data(d.clone()),
        plist::Value::Date(d) => XPCObject::Date(std::time::SystemTime::from(*d)),
        plist::Value::Real(r) => XPCObject::Double(*r),
        plist::Value::Integer(i) => match i.as_signed() {
            Some(s) => XPCObject::Int64(s),
            None => XPCObject::UInt64(i.as_unsigned().unwrap_or_default()),
        },
        plist::Value::String(s) => XPCObject::String(s.clone()),
        plist::Value::Uid(u) => XPCObject::UInt64(u.get()),
        _ => XPCObject::Null,
    }
}

/// The `launchapplication` input (device.md §6.5; idevice `app_service.rs` for the field names).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaunchInput {
    pub bundle_id: String,
    pub args: Vec<String>,
    pub env: std::collections::BTreeMap<String, String>,
    pub start_suspended: bool,
    pub terminate_existing: bool,
    /// The UUID read from `openstdiosocket`; stdin, stdout and stderr all use it.
    pub stdio: Option<uuid::Uuid>,
}

impl LaunchInput {
    /// The XPC input dictionary.
    pub fn to_xpc(&self) -> Dictionary {
        let mut spec = Dictionary::new();
        let mut bid = Dictionary::new();
        bid.insert("_0".into(), XPCObject::String(self.bundle_id.clone()));
        spec.insert("bundleIdentifier".into(), XPCObject::Dictionary(bid));

        let mut env = Dictionary::new();
        for (k, v) in &self.env {
            env.insert(k.clone(), XPCObject::String(v.clone()));
        }
        let mut user = Dictionary::new();
        user.insert("active".into(), XPCObject::Bool(true));
        let mut platform = Vec::new();
        // An empty XML plist dictionary, as idevice sends it.
        let _ = plist::to_writer_xml(&mut platform, &plist::Dictionary::new());

        let mut options = Dictionary::new();
        options
            .insert("arguments".into(), XPCObject::Array(self.args.iter().cloned().map(XPCObject::String).collect()));
        options.insert("environmentVariables".into(), XPCObject::Dictionary(env));
        options.insert("standardIOUsesPseudoterminals".into(), XPCObject::Bool(true));
        options.insert("startStopped".into(), XPCObject::Bool(self.start_suspended));
        options.insert("terminateExisting".into(), XPCObject::Bool(self.terminate_existing));
        options.insert("user".into(), XPCObject::Dictionary(user));
        options.insert("platformSpecificOptions".into(), XPCObject::Data(platform));

        let mut input = Dictionary::new();
        input.insert("applicationSpecifier".into(), XPCObject::Dictionary(spec));
        input.insert("options".into(), XPCObject::Dictionary(options));
        let mut stdio = Dictionary::new();
        if let Some(u) = self.stdio {
            for k in ["standardInput", "standardOutput", "standardError"] {
                stdio.insert(k.into(), XPCObject::Uuid(u));
            }
        }
        input.insert("standardIOIdentifiers".into(), XPCObject::Dictionary(stdio));
        input
    }
}

/// What `launchapplication` returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launched {
    pub pid: u32,
    /// The device path of the executable (`executableURL.relative`).
    pub executable: String,
}

/// Parses the `launchapplication` output (`processToken`).
pub fn parse_launch(out: &plist::Value) -> Result<Launched> {
    let token = out
        .as_dictionary()
        .and_then(|d| d.get("processToken"))
        .and_then(|t| t.as_dictionary())
        .ok_or_else(|| device_error(codes::device::PROTOCOL, "launchapplication: no processToken"))?;
    let pid = token
        .get("processIdentifier")
        .and_then(|p| p.as_signed_integer().or_else(|| p.as_unsigned_integer().map(|u| u as i64)))
        .and_then(|p| u32::try_from(p).ok())
        .ok_or_else(|| device_error(codes::device::PROTOCOL, "launchapplication: no processIdentifier"))?;
    let executable = token
        .get("executableURL")
        .and_then(|u| u.as_dictionary())
        .and_then(|u| u.get("relative"))
        .and_then(|r| r.as_string())
        .map(strip_file_url)
        .unwrap_or_default();
    Ok(Launched { pid, executable })
}

/// `file:///private/var/…` → `/private/var/…`.
pub fn strip_file_url(s: &str) -> String {
    s.strip_prefix("file://").unwrap_or(s).to_owned()
}

/// One process of `listprocesses`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessToken {
    pub pid: u32,
    pub executable: Option<String>,
}

/// Parses the `listprocesses` output.
pub fn parse_processes(out: &plist::Value) -> Result<Vec<ProcessToken>> {
    let list = out
        .as_dictionary()
        .and_then(|d| d.get("processTokens"))
        .and_then(|t| t.as_array())
        .ok_or_else(|| device_error(codes::device::PROTOCOL, "listprocesses: no processTokens"))?;
    Ok(list
        .iter()
        .filter_map(|t| t.as_dictionary())
        .filter_map(|t| {
            let pid =
                t.get("processIdentifier").and_then(|p| p.as_signed_integer()).and_then(|p| u32::try_from(p).ok())?;
            let executable = t
                .get("executableURL")
                .and_then(|u| u.as_dictionary())
                .and_then(|u| u.get("relative"))
                .and_then(|r| r.as_string())
                .map(strip_file_url);
            Some(ProcessToken { pid, executable })
        })
        .collect())
}

/// The `sendsignaltoprocess` input.
pub fn signal_input(pid: u32, signal: i32) -> Dictionary {
    let mut process = Dictionary::new();
    process.insert("processIdentifier".into(), XPCObject::Int64(i64::from(pid)));
    let mut d = Dictionary::new();
    d.insert("process".into(), XPCObject::Dictionary(process));
    d.insert("signal".into(), XPCObject::Int64(i64::from(signal)));
    d
}

/// The `uninstallapp` input.
pub fn uninstall_input(bundle_id: &str) -> Dictionary {
    let mut d = Dictionary::new();
    d.insert("bundleIdentifier".into(), XPCObject::String(bundle_id.to_owned()));
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_input_carries_start_stopped_and_one_stdio_uuid() {
        let u = uuid::Uuid::from_bytes([7; 16]);
        let input = LaunchInput {
            bundle_id: "dev.russet.test.probe".into(),
            args: vec!["-x".into()],
            env: [("NSUnbufferedIO".to_owned(), "YES".to_owned())].into(),
            start_suspended: true,
            terminate_existing: true,
            stdio: Some(u),
        }
        .to_xpc();
        let opts = input.get("options").and_then(|o| o.as_dictionary()).expect("options");
        assert_eq!(opts.get("startStopped"), Some(&XPCObject::Bool(true)));
        assert_eq!(opts.get("terminateExisting"), Some(&XPCObject::Bool(true)));
        let stdio = input.get("standardIOIdentifiers").and_then(|o| o.as_dictionary()).expect("stdio");
        assert_eq!(stdio.len(), 3);
        assert!(stdio.values().all(|v| *v == XPCObject::Uuid(u)));
        // The envelope wraps it with the feature id and a version dictionary.
        let req = invocation(feature::LAUNCH_APPLICATION, input);
        assert_eq!(
            req.get("CoreDevice.featureIdentifier"),
            Some(&XPCObject::String(feature::LAUNCH_APPLICATION.into()))
        );
        // Encodes with idevice's XPC codec.
        XPCObject::Dictionary(req).encode().expect("encodes");
    }

    #[test]
    fn replies_split_into_output_and_nserror() {
        let mut ok = plist::Dictionary::new();
        let mut token = plist::Dictionary::new();
        token.insert("processIdentifier".into(), 932.into());
        let mut url = plist::Dictionary::new();
        url.insert(
            "relative".into(),
            "file:///private/var/containers/Bundle/Application/X/RussetProbe.app/RussetProbe".into(),
        );
        token.insert("executableURL".into(), url.into());
        let mut out = plist::Dictionary::new();
        out.insert("processToken".into(), token.into());
        ok.insert("CoreDevice.output".into(), out.into());
        let launched = parse_launch(&parse_reply(ok.into()).expect("output")).expect("parsed");
        assert_eq!(launched.pid, 932);
        assert_eq!(launched.executable, "/private/var/containers/Bundle/Application/X/RussetProbe.app/RussetProbe");

        let mut err = plist::Dictionary::new();
        let mut e = plist::Dictionary::new();
        e.insert("domain".into(), "CoreDevice.ActionError".into());
        e.insert("code".into(), 2.into());
        err.insert("CoreDevice.error".into(), e.into());
        match parse_reply(err.into()) {
            Err(InvokeError::Device(v)) => {
                assert_eq!(NsErrorChain::from_plist(&v).classify(), codes::device::SERVICE_NOT_IMPLEMENTED);
            }
            other => panic!("expected a device error, got {other:?}"),
        }
    }

    #[test]
    fn process_lists_parse() {
        let mut t = plist::Dictionary::new();
        t.insert("processIdentifier".into(), 1.into());
        let mut out = plist::Dictionary::new();
        out.insert("processTokens".into(), plist::Value::Array(vec![t.into()]));
        let list = parse_processes(&out.into()).expect("parsed");
        assert_eq!(list, vec![ProcessToken { pid: 1, executable: None }]);
    }

    #[test]
    fn plist_conversion_never_panics() {
        let v = plist::Value::Date(plist::Date::from(std::time::SystemTime::UNIX_EPOCH));
        assert!(matches!(xpc_from_plist(&v), XPCObject::Date(_)));
        let big = plist::Value::Integer(u64::MAX.into());
        assert_eq!(xpc_from_plist(&big), XPCObject::UInt64(u64::MAX));
    }
}
