// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `toolchain.*` and `sdk.list` wire shapes (ARCHITECTURE §8.1, §16.2).

use std::collections::BTreeMap;

use russet_protocol::sdk::SdkListResult;
use russet_protocol::toolchain::{
    ManagedComponent, ToolComponent, ToolInfo, ToolProvenance, ToolchainInstallParams, ToolchainResolveParams,
};
use russet_protocol::{Permission, registry};
use serde_json::json;

#[test]
fn tool_info_carries_the_env_contract() {
    let t = ToolInfo {
        component: ToolComponent::LldbDap,
        version: "21.0.0".into(),
        path: "/usr/lib/swift/usr/bin/lldb-dap".into(),
        provenance: ToolProvenance::Uv,
        env: BTreeMap::from([("PYTHONHOME".to_owned(), "/pbs".to_owned())]),
        ok: true,
        problems: vec![],
    };
    assert_eq!(
        serde_json::to_value(&t).expect("ser"),
        json!({"component": "lldb-dap", "version": "21.0.0", "path": "/usr/lib/swift/usr/bin/lldb-dap",
               "provenance": {"kind": "uv"}, "env": {"PYTHONHOME": "/pbs"}, "ok": true, "problems": []})
    );
    let bundle = ToolProvenance::XtoolBundle { bundle: "/b".into() };
    assert_eq!(serde_json::to_value(&bundle).expect("ser"), json!({"kind": "xtool-bundle", "bundle": "/b"}));
    let managed: ToolProvenance =
        serde_json::from_value(json!({"kind": "managed", "pin": "python-shared@3.12.15"})).expect("de");
    assert_eq!(managed, ToolProvenance::Managed { pin: "python-shared@3.12.15".into() });
}

#[test]
fn components_and_classes() {
    let p: ToolchainInstallParams = serde_json::from_value(json!({"component": "python-shared"})).expect("install");
    assert_eq!((p.component, p.version), (ManagedComponent::PythonShared, None));
    let p: ToolchainInstallParams =
        serde_json::from_value(json!({"component": "darwin-tools", "version": "1.1.0"})).expect("install");
    assert_eq!(p.component, ManagedComponent::DarwinTools);
    let r: ToolchainResolveParams = serde_json::from_value(json!({"component": "darwin-sdk"})).expect("resolve");
    assert_eq!(r.component, ToolComponent::DarwinSdk);
    let class = |name: &str| registry().iter().find(|m| m.name == name).copied().expect(name);
    assert_eq!(class("toolchain.list").permission, Permission::Read);
    assert_eq!(class("toolchain.resolve").permission, Permission::Read);
    let install = class("toolchain.install");
    assert_eq!((install.permission, install.long_running), (Permission::System, true));
    assert_eq!(class("sdk.list").permission, Permission::Read);
    let sdks: SdkListResult = serde_json::from_value(json!({"sdks": [{"id": "xcode-27.0-27A266a",
        "xcodeVersion": "27.0", "xcodeBuild": "27A266a", "iosSdk": "27.0", "swiftSdkName": "darwin",
        "bundle": "/b", "darwinTools": "1.1.0", "compatibleSwift": "6.4"}]}))
    .expect("sdk.list");
    assert_eq!(sdks.sdks[0].darwin_tools.as_deref(), Some("1.1.0"));
}
