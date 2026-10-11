// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `initialize` (ARCHITECTURE §7.4): the result describes the daemon (build id, executable identity, device
//! backend), the connection id, the grant of the client's kind, and every registered method and notification.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "daemon_support.rs"]
mod support;

use russet_core::rpc::{ClientKind, Permission};
use russetd::InProcess;
use support::{Home, client};

#[tokio::test]
async fn the_result_follows_section_7_4() {
    let home = Home::new("initialize");
    let (daemon, _peer, result) =
        InProcess::start_with(home.options(), &client(ClientKind::Ide, "russet-vscode", false)).await.expect("start");
    assert_eq!(result.protocol, russet_protocol::PROTOCOL);
    assert_eq!(result.server.name, "russetd");
    assert_eq!(result.server.build_id, russet_core::build_info::BUILD_ID);
    assert_eq!(result.server.device_backend, "none");
    let (_, id) = russet_core::platform::current_executable().expect("exe");
    assert_eq!(result.server.executable_id, id.into(), "the executable identity of ARCHITECTURE §3.3");
    assert!(result.connection_id.starts_with("c-"));
    assert_eq!(result.grant.profile, "ide");
    assert!(result.grant.allow.contains(&Permission::Secrets));
    assert_eq!(result.grant.confirm, vec![Permission::AccountDestructive, Permission::Publish]);
    assert!(result.capabilities.methods.iter().any(|m| m == "daemon.status"));
    assert!(result.capabilities.notifications.iter().any(|n| n == "job.updated"));
    assert_eq!(result.capabilities.methods.len(), russet_protocol::registry().len());

    // Each kind gets its profile.
    let (_, mcp) = daemon.connect(&client(ClientKind::Mcp, "claude-code", false)).await.expect("mcp");
    assert_eq!((mcp.grant.profile.as_str(), mcp.grant.tier_max), ("mcp", Some(1)));
    assert!(mcp.grant.deny.contains(&Permission::System));
    let (_, flutter) = daemon.connect(&client(ClientKind::FlutterDevice, "flutter", false)).await.expect("flutter");
    assert_eq!(flutter.grant.allow, vec![Permission::Read, Permission::Device, Permission::Build]);
    let (_, headless) = daemon.connect(&client(ClientKind::Cli, "script", false)).await.expect("cli");
    assert!(!headless.grant.allow.contains(&Permission::Secrets), "secrets need a TTY");
    assert!(headless.grant.confirm.contains(&Permission::Secrets), "the local-file imports confirm");
    assert_ne!(mcp.connection_id, flutter.connection_id);
    daemon.stop().await.expect("stop");
}
