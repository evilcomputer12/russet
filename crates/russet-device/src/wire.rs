// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Domain types → the wire shapes of ARCHITECTURE §9.5 (`russet_protocol::common`) and the device namespaces.
//! russet-protocol (layer L1) cannot name russet-device's types, so the conversions live here (ARCHITECTURE §5.2:
//! "the protocol module converts"; the richer domain enums stay in this crate).

use russet_protocol::common as w;
use russet_protocol::{app as wa, device as wd, forward as wf, process as wp};

use crate::apps::AppInfo;
use crate::ddi::{DdiKind, DdiOrigin, DdiSource, DdiStatus};
use crate::debug::{ForwardInfo, ForwardTarget};
use crate::logs::rfc3339_millis;
use crate::model::{
    CodeSigningMonitor, DeviceInfo, DeviceState, DeviceSummary, Readiness, Transport, Tri, TunnelState,
};
use crate::process::ProcessInfo;

/// `dir`, `imported`, … as on the wire.
pub fn origin_name(o: DdiOrigin) -> &'static str {
    match o {
        DdiOrigin::Imported => "imported",
        DdiOrigin::Dir => "dir",
        DdiOrigin::Pmd3Cache => "pmd3-cache",
        DdiOrigin::Pmd3AutoMount => "pmd3-auto-mount",
        DdiOrigin::Unknown => "unknown",
    }
}

pub fn transport(t: Transport) -> w::DeviceTransport {
    match t {
        Transport::Usb => w::DeviceTransport::Usb,
        Transport::Wifi => w::DeviceTransport::Wifi,
    }
}

pub fn tri(t: Tri) -> w::Tristate {
    match t {
        Tri::Yes => w::Tristate::Yes,
        Tri::No => w::Tristate::No,
        Tri::Unknown => w::Tristate::Unknown,
    }
}

pub fn csm(c: CodeSigningMonitor) -> w::CodeSigningMonitor {
    match c {
        CodeSigningMonitor::Txm => w::CodeSigningMonitor::Txm,
        CodeSigningMonitor::Ppl => w::CodeSigningMonitor::Ppl,
        CodeSigningMonitor::None => w::CodeSigningMonitor::None,
        CodeSigningMonitor::Unknown => w::CodeSigningMonitor::Unknown,
    }
}

pub fn ddi_kind(k: DdiKind) -> w::DdiKind {
    match k {
        DdiKind::Legacy => w::DdiKind::Legacy,
        DdiKind::PersonalizedDmg => w::DdiKind::PersonalizedDmg,
        DdiKind::Cryptex1 => w::DdiKind::Cryptex1,
    }
}

pub fn ddi_origin(o: DdiOrigin) -> w::DdiOrigin {
    match o {
        DdiOrigin::Imported => w::DdiOrigin::Imported,
        DdiOrigin::Dir => w::DdiOrigin::Dir,
        DdiOrigin::Pmd3Cache => w::DdiOrigin::Pmd3Cache,
        DdiOrigin::Pmd3AutoMount => w::DdiOrigin::Pmd3AutoMount,
        DdiOrigin::Unknown => w::DdiOrigin::Unknown,
    }
}

pub fn ddi_status(s: &DdiStatus) -> w::DdiStatus {
    w::DdiStatus {
        mounted: s.mounted,
        kind: ddi_kind(s.kind),
        version: s.version.clone(),
        product_build: s.product_build.clone(),
        source: ddi_origin(s.source),
    }
}

pub fn tunnel(t: &TunnelState) -> w::TunnelStatus {
    match t {
        TunnelState::Down => {
            w::TunnelStatus { state: w::TunnelState::Down, generation: None, since: None, error: None }
        }
        TunnelState::Up { generation, since } => w::TunnelStatus {
            state: w::TunnelState::Up,
            generation: Some(*generation),
            since: Some(rfc3339_millis(*since)),
            error: None,
        },
        TunnelState::Failed { error } => {
            w::TunnelStatus { state: w::TunnelState::Failed, generation: None, since: None, error: Some(error.clone()) }
        }
    }
}

pub fn readiness(r: &Readiness) -> w::Readiness {
    w::Readiness {
        paired: r.paired,
        developer_mode: tri(r.developer_mode),
        locked: tri(r.locked),
        ddi: r.ddi.as_ref().map(ddi_status),
        tunnel: Some(tunnel(&r.tunnel)),
    }
}

pub fn state(s: &DeviceState) -> (w::DeviceState, Option<russet_core::error::ErrorData>) {
    match s {
        DeviceState::Attached => (w::DeviceState::Attached, None),
        DeviceState::NeedsTrust => (w::DeviceState::NeedsTrust, None),
        DeviceState::TrustPending => (w::DeviceState::TrustPending, None),
        DeviceState::Ready => (w::DeviceState::Ready, None),
        DeviceState::Unavailable { error } => (w::DeviceState::Unavailable, Some(error.clone())),
    }
}

/// `DeviceSummary` of ARCHITECTURE §9.5. Identity fields that were not read yet are empty strings.
pub fn summary(s: &DeviceSummary) -> w::DeviceSummary {
    let id = s.identity.clone().unwrap_or_default();
    let (st, error) = state(&s.state);
    w::DeviceSummary {
        udid: s.udid.0.clone(),
        name: id.name.clone().unwrap_or_default(),
        product_type: id.product_type.clone().unwrap_or_default(),
        marketing_name: id.marketing_name().map(str::to_owned),
        os_version: id.os_version.clone().unwrap_or_default(),
        build: id.build.clone(),
        transport: transport(s.transport),
        code_signing_monitor: s.identity.as_ref().map(|i| csm(i.code_signing_monitor())),
        state: st,
        readiness: s.readiness.as_ref().map(readiness),
        error,
    }
}

pub fn info(i: &DeviceInfo) -> wd::DeviceInfoResult {
    let (st, _) = state(&i.state);
    wd::DeviceInfoResult {
        udid: i.udid.0.clone(),
        name: i.identity.name.clone(),
        product_type: i.identity.product_type.clone(),
        marketing_name: i.identity.marketing_name().map(str::to_owned),
        os_version: i.identity.os_version.clone(),
        build: i.identity.build.clone(),
        device_class: i.identity.device_class.clone(),
        hardware_model: i.identity.hardware_model.clone(),
        cpu_architecture: i.identity.cpu_architecture.clone(),
        chip_id: i.identity.chip_id,
        transport: transport(i.transport),
        code_signing_monitor: Some(csm(i.identity.code_signing_monitor())),
        state: st,
        properties: i.properties.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
    }
}

pub fn ddi_source(p: &wd::DdiSourceParam) -> DdiSource {
    match p {
        wd::DdiSourceParam::Imported { ddi_build } => DdiSource::Imported { ddi_build: ddi_build.clone() },
        wd::DdiSourceParam::Dir { path } => DdiSource::Dir(path.into()),
        wd::DdiSourceParam::Pmd3Cache {} => DdiSource::Pmd3Cache,
        wd::DdiSourceParam::Pmd3AutoMount { allow_mirror } => DdiSource::Pmd3AutoMount { allow_mirror: *allow_mirror },
    }
}

pub fn app(a: &AppInfo) -> wa::AppInfo {
    wa::AppInfo {
        bundle_id: a.bundle_id.clone(),
        name: a.name.clone(),
        version: a.version.clone(),
        build: a.build.clone(),
        path: a.path.clone(),
        executable: a.executable.clone(),
        developer_app: a.developer_app,
        removable: a.removable,
        hidden: a.hidden,
    }
}

pub fn process(p: &ProcessInfo) -> wp::ProcessInfo {
    wp::ProcessInfo {
        pid: p.pid,
        name: p.name.clone(),
        executable: p.executable.clone(),
        bundle_id: p.bundle_id.clone(),
        is_application: p.is_application,
    }
}

pub fn forward_target(t: &ForwardTarget) -> wf::ForwardTargetParam {
    match t {
        ForwardTarget::RsdService(s) => wf::ForwardTargetParam { service: Some(s.clone()), ..Default::default() },
        ForwardTarget::TunnelPort(p) => wf::ForwardTargetParam { tunnel_port: Some(*p), ..Default::default() },
        ForwardTarget::UsbmuxPort(p) => wf::ForwardTargetParam { usbmux_port: Some(*p), ..Default::default() },
    }
}

/// `{service}|{tunnelPort}|{usbmuxPort}` with exactly one field set.
pub fn parse_forward_target(t: &wf::ForwardTargetParam) -> russet_core::Result<ForwardTarget> {
    match (&t.service, t.tunnel_port, t.usbmux_port) {
        (Some(s), None, None) => Ok(ForwardTarget::RsdService(s.clone())),
        (None, Some(p), None) => Ok(ForwardTarget::TunnelPort(p)),
        (None, None, Some(p)) => Ok(ForwardTarget::UsbmuxPort(p)),
        _ => Err(crate::error::invalid_argument("target needs exactly one of service, tunnelPort or usbmuxPort")),
    }
}

pub fn forward(f: &ForwardInfo) -> wf::ForwardInfo {
    wf::ForwardInfo {
        lease_id: f.lease.clone(),
        udid: f.udid.0.clone(),
        target: forward_target(&f.target),
        local: f.local.display(),
        connections: f.connections,
        opened_at: rfc3339_millis(f.opened_at),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Identity, Udid};

    #[test]
    fn summaries_follow_architecture_9_5() {
        let s = DeviceSummary {
            udid: Udid("00008101-000A1B2C3D4E5F60".into()),
            transport: Transport::Usb,
            identity: Some(Identity {
                name: Some("iPhone".into()),
                product_type: Some("iPhone13,2".into()),
                os_version: Some("27.0".into()),
                build: Some("24A437".into()),
                chip_id: Some(0x8101),
                ..Default::default()
            }),
            state: DeviceState::Ready,
            readiness: Some(Readiness {
                paired: true,
                developer_mode: Tri::Yes,
                ddi: Some(DdiStatus {
                    mounted: true,
                    kind: DdiKind::Cryptex1,
                    version: Some("27.1.5228.8".into()),
                    product_build: Some("27A5228h".into()),
                    source: DdiOrigin::Unknown,
                }),
                locked: Tri::Unknown,
                tunnel: TunnelState::Down,
            }),
        };
        let json = serde_json::to_value(summary(&s)).expect("json");
        assert_eq!(json["productType"], "iPhone13,2");
        assert_eq!(json["marketingName"], "iPhone 12");
        assert_eq!(json["osVersion"], "27.0");
        assert_eq!(json["codeSigningMonitor"], "txm");
        assert_eq!(json["state"], "ready");
        assert_eq!(json["readiness"]["ddi"]["kind"], "cryptex1");
        assert_eq!(json["readiness"]["ddi"]["source"], "unknown");
        assert_eq!(json["readiness"]["tunnel"]["state"], "down");
        assert!(json.get("error").is_none());
    }

    #[test]
    fn forward_targets_need_exactly_one_field() {
        let t = wf::ForwardTargetParam {
            service: Some("com.apple.internal.dt.remote.debugproxy".into()),
            ..Default::default()
        };
        assert_eq!(
            parse_forward_target(&t).expect("one"),
            ForwardTarget::RsdService("com.apple.internal.dt.remote.debugproxy".into())
        );
        assert!(parse_forward_target(&wf::ForwardTargetParam::default()).is_err());
        let both = wf::ForwardTargetParam { tunnel_port: Some(1), usbmux_port: Some(2), ..Default::default() };
        assert!(parse_forward_target(&both).is_err());
        assert_eq!(forward_target(&ForwardTarget::UsbmuxPort(5)).usbmux_port, Some(5));
    }
}
