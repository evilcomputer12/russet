// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `lease.*` (ARCHITECTURE §6.3, §8.1 [D 68]): the owner releases its own lease (`read`), others need `system`
//! (never an agent), a connection presenting `sharedLease` is listed as a sharer but cannot release it, and a
//! disconnected owner's leases end after the grace.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "daemon_support.rs"]
mod support;

use std::time::Duration;

use russet_core::error::codes;
use russet_core::rpc::ClientKind;
use russet_protocol::lease::{LeaseKind, LeaseList, LeaseListParams, LeaseRelease, LeaseReleaseParams};
use support::{Home, client, start};

#[tokio::test]
async fn owners_release_sharers_watch_and_owners_gone_end_their_leases() {
    let home = Home::new("lease-rpc");
    let (daemon, cli) = start(home.options()).await;
    let (ide, ide_init) = daemon.connect(&client(ClientKind::Ide, "russet-ide", false)).await.expect("ide");
    // The IDE reserves the device (W4's device.reserve does this through the lease table).
    let ide_conn = russet_core::rpc::ConnInfo {
        id: ide_init.connection_id.clone(),
        kind: ClientKind::Ide,
        name: "russet-ide".into(),
        interactive: false,
        shared_lease: None,
    };
    let reservation = daemon.ctx().leases.reserve_device("UDID-1", Some("run".into()), &ide_conn).expect("reserved");

    // An agent the IDE started shares it (RUSSET_LEASE).
    let mut agent_info = client(ClientKind::Mcp, "claude-code", false);
    agent_info.shared_lease = Some(reservation.id.clone());
    let (agent, _) = daemon.connect(&agent_info).await.expect("agent");
    let listed = cli.call::<LeaseList>(LeaseListParams {}).await.expect("list").leases;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].kind, LeaseKind::DeviceReservation);
    assert_eq!(listed[0].shared_with.len(), 1, "{listed:?}");
    // Sharers are not owners: the agent's release is a system call, which agents never get.
    let e =
        agent.call::<LeaseRelease>(LeaseReleaseParams { lease_id: reservation.id.clone() }).await.expect_err("sharer");
    assert_eq!(e.code(), codes::perm::DENIED);

    // The owner releases its own lease (class read).
    let forward =
        daemon.ctx().leases.acquire(LeaseKind::Forward, Some(&ide_conn), Some("debugserver".into()), None, false);
    ide.call::<LeaseRelease>(LeaseReleaseParams { lease_id: forward.id.clone() }).await.expect("owner");
    assert!(forward.token.is_cancelled());
    // A CLI (system class) may force someone else's.
    let other = daemon.ctx().leases.acquire(LeaseKind::Forward, Some(&ide_conn), None, None, false);
    cli.call::<LeaseRelease>(LeaseReleaseParams { lease_id: other.id.clone() }).await.expect("forced");
    let e = cli.call::<LeaseRelease>(LeaseReleaseParams { lease_id: other.id }).await.expect_err("gone");
    assert_eq!(e.code(), codes::core::NOT_FOUND);

    // The IDE goes away: its reservation ends after the 2 s grace.
    ide.close();
    tokio::time::timeout(Duration::from_secs(4), reservation.token.cancelled())
        .await
        .expect("released after the grace");
    assert!(cli.call::<LeaseList>(LeaseListParams {}).await.expect("list").leases.is_empty());
    daemon.stop().await.expect("stop");
}
