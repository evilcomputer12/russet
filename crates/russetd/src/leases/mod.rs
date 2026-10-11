// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Leases and sessions with the 2 s grace (ARCHITECTURE §6.3 [D 65, D 68]).
//!
//! A lease pins a resource (a device reservation, a forward, a launch, a session, a mirror) to its owner
//! connection. When the owner disconnects, its leases end after the grace (`device.lease-grace`, 2 s), unless
//! they are `detached`. Ending a lease cancels its token, which the resource's task watches. A device
//! reservation belongs to its owner plus every connection that presented its id as `initialize.sharedLease`;
//! others get `device.reserved`. Only the owner releases a lease (`system`-class callers may force it); a
//! sharer is never an owner.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russet_core::error::{Error, Result, codes};
use russet_core::ids::LeaseId;
use russet_core::rpc::ConnInfo;
use russet_protocol::common::ConnectionRef;
use russet_protocol::lease::{LeaseInfo, LeaseKind};
use tokio_util::sync::CancellationToken;

/// The owner-gone grace (`device.lease-grace`, ARCHITECTURE §14.3).
pub const LEASE_GRACE: Duration = Duration::from_secs(2);

#[derive(Debug)]
struct Entry {
    info: LeaseInfo,
    owner_conn: Option<String>,
    token: CancellationToken,
}

#[derive(Debug)]
struct Inner {
    leases: Mutex<BTreeMap<LeaseId, Entry>>,
    grace: Duration,
}

/// A cheap-to-clone handle (`DaemonCtx.leases`).
#[derive(Debug, Clone)]
pub struct LeaseTable {
    inner: Arc<Inner>,
}

impl Default for LeaseTable {
    fn default() -> Self {
        Self::new(LEASE_GRACE)
    }
}

/// A held lease: its id and the token that fires when it ends.
#[derive(Debug, Clone)]
pub struct Lease {
    pub id: LeaseId,
    pub token: CancellationToken,
}

impl LeaseTable {
    /// A table whose owner-gone grace is `grace`.
    pub fn new(grace: Duration) -> Self {
        Self { inner: Arc::new(Inner { leases: Mutex::new(BTreeMap::new()), grace }) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<LeaseId, Entry>> {
        self.inner.leases.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Creates a lease of `kind` owned by `owner` (`None`: the daemon's own, never released by a disconnect).
    pub fn acquire(
        &self,
        kind: LeaseKind,
        owner: Option<&ConnInfo>,
        resource: Option<String>,
        purpose: Option<String>,
        detached: bool,
    ) -> Lease {
        let id = LeaseId::generate();
        let token = CancellationToken::new();
        let info = LeaseInfo {
            lease_id: id.clone(),
            kind,
            owner: owner.map(|c| ConnectionRef { kind: c.kind, name: c.name.clone(), connection_id: c.id.clone() }),
            detached,
            resource,
            purpose,
            created_at: russet_core::time::now_rfc3339(),
            shared_with: Vec::new(),
        };
        let entry = Entry { info, owner_conn: owner.map(|c| c.id.clone()), token: token.clone() };
        self.lock().insert(id.clone(), entry);
        Lease { id, token }
    }

    /// Reserves device `udid` for `owner` (`device.reserve`); `device.reserved` when another principal holds it.
    pub fn reserve_device(&self, udid: &str, purpose: Option<String>, owner: &ConnInfo) -> Result<Lease> {
        self.check_device(udid, owner)?;
        Ok(self.acquire(LeaseKind::DeviceReservation, Some(owner), Some(udid.to_owned()), purpose, false))
    }

    /// `device.reserved` (3018) unless `conn` is a principal of `udid`'s reservation, or there is none.
    pub fn check_device(&self, udid: &str, conn: &ConnInfo) -> Result<()> {
        let map = self.lock();
        let held = map.values().find(|e| {
            e.info.kind == LeaseKind::DeviceReservation
                && e.info.resource.as_deref() == Some(udid)
                && !e.token.is_cancelled()
        });
        match held {
            None => Ok(()),
            Some(e) if e.owner_conn.as_deref() == Some(conn.id.as_str()) => Ok(()),
            Some(e) if conn.shared_lease.as_ref() == Some(&e.info.lease_id) => Ok(()),
            Some(e) => {
                Err(Error::new(codes::device::RESERVED, format!("the device is reserved by {}", holder_name(&e.info)))
                    .with_hint("Wait until the reservation ends, or share it (RUSSET_LEASE, ARCHITECTURE §6.3).")
                    .with_details(serde_json::json!({"leaseId": e.info.lease_id, "owner": e.info.owner})))
            }
        }
    }

    /// Records that connection `conn_id` shares lease `id` (it presented it at `initialize`).
    pub fn add_sharer(&self, id: &LeaseId, conn_id: &str) {
        if let Some(e) = self.lock().get_mut(id)
            && !e.info.shared_with.iter().any(|c| c == conn_id)
        {
            e.info.shared_with.push(conn_id.to_owned());
        }
    }

    /// Ends lease `id`. Only its owner may, unless `force` (a `system`-class caller, ARCHITECTURE §8.1).
    pub fn release(&self, id: &LeaseId, conn: &ConnInfo, force: bool) -> Result<()> {
        let mut map = self.lock();
        let Some(e) = map.get(id) else {
            return Err(Error::new(codes::core::NOT_FOUND, format!("lease {id} does not exist"))
                .with_hint("List the leases with `russet daemon status`."));
        };
        if !force && e.owner_conn.as_deref() != Some(conn.id.as_str()) {
            return Err(Error::new(codes::perm::DENIED, format!("lease {id} belongs to another connection"))
                .with_hint("Only the owner releases a lease; a shared reservation stays with its owner."));
        }
        if let Some(e) = map.remove(id) {
            e.token.cancel();
        }
        Ok(())
    }

    /// Every live lease.
    pub fn list(&self) -> Vec<LeaseInfo> {
        self.lock().values().map(|e| e.info.clone()).collect()
    }

    /// Whether connection `conn_id` owns lease `id` (sharers do not).
    pub fn owns(&self, conn_id: &str, id: &LeaseId) -> bool {
        self.lock().get(id).is_some_and(|e| e.owner_conn.as_deref() == Some(conn_id))
    }

    /// The number of detached leases (they keep the daemon from idling out).
    pub fn detached_count(&self) -> usize {
        self.lock().values().filter(|e| e.info.detached).count()
    }

    /// How many leases are live.
    pub fn count(&self) -> usize {
        self.lock().len()
    }

    /// The owner of some leases disconnected: its non-detached leases end after the grace; detached ones lose
    /// their owner but stay. Connections that only shared a lease stop sharing it.
    pub fn owner_gone(&self, conn_id: &str) {
        let mut doomed = Vec::new();
        for (id, e) in self.lock().iter_mut() {
            e.info.shared_with.retain(|c| c != conn_id);
            if e.owner_conn.as_deref() == Some(conn_id) {
                if e.info.detached {
                    e.owner_conn = None;
                    e.info.owner = None;
                } else {
                    doomed.push(id.clone());
                }
            }
        }
        if doomed.is_empty() {
            return;
        }
        let table = self.clone();
        let grace = self.inner.grace;
        tokio::spawn(async move {
            tokio::time::sleep(grace).await;
            let mut map = table.lock();
            for id in doomed {
                if let Some(e) = map.remove(&id) {
                    e.token.cancel();
                }
            }
        });
    }

    /// Ends every lease (shutdown).
    pub fn release_all(&self) {
        for (_, e) in std::mem::take(&mut *self.lock()) {
            e.token.cancel();
        }
    }
}

fn holder_name(info: &LeaseInfo) -> String {
    match &info.owner {
        Some(o) => format!("{} ({})", o.name, o.connection_id),
        None => "a detached session".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use russet_core::rpc::ClientKind;

    use super::*;

    fn conn(id: &str, shared: Option<LeaseId>) -> ConnInfo {
        ConnInfo { id: id.into(), kind: ClientKind::Ide, name: "ide".into(), interactive: false, shared_lease: shared }
    }

    #[tokio::test]
    async fn reservations_have_principals_and_end_after_the_grace() {
        let table = LeaseTable::new(Duration::from_millis(50));
        let owner = conn("c-1", None);
        let lease = table.reserve_device("UDID", Some("run".into()), &owner).expect("free");
        assert!(table.owns("c-1", &lease.id));
        // A stranger is refused; a connection sharing the lease is a principal but not an owner.
        let stranger = conn("c-2", None);
        assert_eq!(table.check_device("UDID", &stranger).expect_err("reserved").code(), codes::device::RESERVED);
        let agent = conn("c-3", Some(lease.id.clone()));
        table.check_device("UDID", &agent).expect("a sharer is a principal");
        table.add_sharer(&lease.id, "c-3");
        assert!(!table.owns("c-3", &lease.id));
        assert_eq!(
            table.release(&lease.id, &agent, false).expect_err("sharers cannot release").code(),
            codes::perm::DENIED
        );
        // The owner disconnects: the reservation ends after the grace.
        table.owner_gone("c-1");
        assert_eq!(table.count(), 1, "still there during the grace");
        tokio::time::timeout(Duration::from_secs(2), lease.token.cancelled()).await.expect("released after the grace");
        assert_eq!(table.count(), 0);
        table.check_device("UDID", &stranger).expect("free again");
    }

    #[tokio::test]
    async fn detached_leases_survive_their_owner_and_owners_release() {
        let table = LeaseTable::new(Duration::from_millis(10));
        let owner = conn("c-1", None);
        let detached = table.acquire(LeaseKind::Session, Some(&owner), Some("ses_1".into()), None, true);
        let owned = table.acquire(LeaseKind::Forward, Some(&owner), None, None, false);
        assert_eq!(table.detached_count(), 1);
        table.release(&owned.id, &owner, false).expect("owner releases");
        assert!(owned.token.is_cancelled());
        table.owner_gone("c-1");
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!detached.token.is_cancelled(), "detached leases survive");
        assert!(table.list()[0].owner.is_none());
        table.release(&detached.id, &conn("c-9", None), true).expect("system may force");
        assert!(detached.token.is_cancelled());
    }
}
