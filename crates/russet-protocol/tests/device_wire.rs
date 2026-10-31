// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The wire shapes of W4's namespaces against ARCHITECTURE §8.2 and §9.1: field names, enum strings, the
//! `kind`-tagged DDI source, the `{service}|{tunnelPort}|{usbmuxPort}` target object, omitted optionals and
//! tolerated unknown fields.

use russet_protocol::{app, device, forward, logs, process};
use serde_json::json;

#[test]
fn device_params_and_results() {
    let p: device::DeviceDdiMountParams =
        serde_json::from_value(json!({"source": {"kind": "dir", "path": "/srv/ddi/iOS"}, "force": true, "future": 1}))
            .expect("ddi.mount params");
    assert_eq!(p.source, Some(device::DdiSourceParam::Dir { path: "/srv/ddi/iOS".into() }));
    assert_eq!(p.force, Some(true));
    let imported: device::DdiSourceParam =
        serde_json::from_value(json!({"kind": "imported", "ddiBuild": "27A266a"})).expect("imported");
    assert_eq!(imported, device::DdiSourceParam::Imported { ddi_build: Some("27A266a".into()) });
    assert_eq!(
        serde_json::to_value(device::DdiSourceParam::Pmd3AutoMount { allow_mirror: false }).expect("json"),
        json!({"kind": "pmd3-auto-mount", "allowMirror": false})
    );
    assert_eq!(
        serde_json::to_value(device::DdiSourceParam::Pmd3Cache {}).expect("json"),
        json!({"kind": "pmd3-cache"})
    );
    let prepare: device::DevicePrepareParams =
        serde_json::from_value(json!({"udid": "iphone12", "revealDeveloperMode": true})).expect("prepare");
    assert_eq!(prepare.reveal_developer_mode, Some(true));
    assert_eq!(
        serde_json::to_value(device::DevicePairResult { status: device::PairStatus::Pending }).expect("json"),
        json!({"status": "pending"})
    );
    assert_eq!(serde_json::to_value(device::DeviceChange::Updated).expect("json"), json!("updated"));
    // Empty params serialize as `{}` (ARCHITECTURE §7.3: params are always objects).
    assert_eq!(serde_json::to_value(device::DeviceListParams {}).expect("json"), json!({}));
}

#[test]
fn app_params_and_results() {
    let launch: app::AppLaunchParams =
        serde_json::from_value(json!({"bundleId": "dev.russet.test.probe", "suspended": true})).expect("launch");
    assert!(launch.args.is_empty() && launch.env.is_empty());
    assert_eq!((launch.kill_existing, launch.console), (None, None));
    let list: app::AppListParams =
        serde_json::from_value(json!({"kind": "all", "bundleIdPrefix": "dev."})).expect("list");
    assert_eq!(list.kind, Some(app::AppKind::All));
    let out = app::AppOutputParams {
        lease_id: serde_json::from_value(json!("lse_01J00000000000000000000000")).expect("lease"),
        stream: app::AppOutputStream::Stdout,
        data: "hello".into(),
    };
    assert_eq!(
        serde_json::to_value(out).expect("json"),
        json!({"leaseId": "lse_01J00000000000000000000000", "stream": "stdout", "data": "hello"})
    );
    let exited: app::AppExitedParams =
        serde_json::from_value(json!({"leaseId": "lse_01J00000000000000000000000", "pid": 932, "signal": 9}))
            .expect("exited");
    assert_eq!((exited.code, exited.signal), (None, Some(9)));
}

#[test]
fn forward_process_and_logs_shapes() {
    let open: forward::ForwardOpenParams = serde_json::from_value(json!({
        "target": {"service": "com.apple.internal.dt.remote.debugproxy"},
        "bind": {"unix": "/run/user/1000/russet/x.sock"}
    }))
    .expect("forward.open");
    assert_eq!(open.target.service.as_deref(), Some("com.apple.internal.dt.remote.debugproxy"));
    assert_eq!(
        serde_json::to_value(&open.target).expect("json"),
        json!({"service": "com.apple.internal.dt.remote.debugproxy"})
    );
    let port: forward::ForwardTargetParam = serde_json::from_value(json!({"tunnelPort": 1234})).expect("port");
    assert_eq!(port.tunnel_port, Some(1234));

    let signal: process::ProcessSignalParams =
        serde_json::from_value(json!({"pid": 932, "signal": 19})).expect("signal");
    assert_eq!((signal.pid, signal.signal), (932, 19));

    let sub: logs::LogsSubscribeParams =
        serde_json::from_value(json!({"sources": ["oslog", "stdout"], "filter": {"pid": 932, "minLevel": "error"}}))
            .expect("subscribe");
    assert_eq!(sub.sources, vec![logs::LogsSource::Oslog, logs::LogsSource::Stdout]);
    assert_eq!(sub.filter.pid, Some(932));
    let tail = logs::LogsTailResult { entries: vec![], truncated: false, cursor: None };
    assert_eq!(serde_json::to_value(tail).expect("json"), json!({"entries": [], "truncated": false}));
}
