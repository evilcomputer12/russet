// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Subscriptions with bounded queues and drop counters (ARCHITECTURE §6.4).
//!
//! - A subscription belongs to the connection that opened it; `subscription.cancel` from that connection, or
//!   the connection closing, ends it.
//! - Each has a bounded [`DropQueue`]: 10,000 entries for log streams, 256 messages otherwise. A slow consumer
//!   loses the **oldest** entries and the next notification carries `dropped: n`.
//! - Producers never block: they push into the queue; a pump sends batches while the connection's write
//!   backlog is low, so a client that stops reading cannot make the daemon buffer without bound.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russet_core::error::{Error, Result, codes};
use russet_core::ids::SubscriptionId;
use russet_ipc::Peer;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

/// Queue bound for log streams (`daemon.logBatch`, `logs.entries`, `run.output`).
pub const LOG_QUEUE: usize = 10_000;
/// Queue bound for every other subscription.
pub const MESSAGE_QUEUE: usize = 256;
/// A pump holds back while more than this many frames wait in the connection's writer.
pub const MAX_BACKLOG: usize = 64;
/// How long a pump collects items into one notification (`job.events` is batched for at most 50 ms).
pub const BATCH_WINDOW: Duration = Duration::from_millis(50);

#[derive(Debug)]
struct Entry {
    conn_id: String,
    kind: &'static str,
    cancel: CancellationToken,
}

/// A cheap-to-clone handle (`DaemonCtx.subs`).
#[derive(Debug, Clone, Default)]
pub struct SubscriptionHub {
    subs: Arc<Mutex<BTreeMap<SubscriptionId, Entry>>>,
}

/// An open subscription: its id and the token that ends it.
#[derive(Debug, Clone)]
pub struct Subscription {
    pub id: SubscriptionId,
    pub cancel: CancellationToken,
}

impl SubscriptionHub {
    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<SubscriptionId, Entry>> {
        self.subs.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Opens a subscription of `kind` for `peer`; it ends with the connection.
    pub fn open(&self, peer: &Peer, kind: &'static str) -> Subscription {
        let id = SubscriptionId::generate();
        let cancel = peer.closed_token().child_token();
        self.lock()
            .insert(id.clone(), Entry { conn_id: peer.connection_id().to_owned(), kind, cancel: cancel.clone() });
        let hub = self.clone();
        let (id2, done) = (id.clone(), cancel.clone());
        tokio::spawn(async move {
            done.cancelled().await;
            hub.lock().remove(&id2);
        });
        Subscription { id, cancel }
    }

    /// Ends subscription `id` of connection `conn_id` (`subscription.cancel`).
    pub fn cancel(&self, id: &SubscriptionId, conn_id: &str) -> Result<()> {
        let mut map = self.lock();
        match map.get(id) {
            Some(e) if e.conn_id == conn_id => {
                if let Some(e) = map.remove(id) {
                    e.cancel.cancel();
                }
                Ok(())
            }
            _ => {
                Err(Error::new(codes::core::NOT_FOUND, format!("subscription {id} does not exist on this connection"))
                    .with_hint("Subscriptions belong to the connection that opened them."))
            }
        }
    }

    /// Ends every subscription of a connection.
    pub fn close_connection(&self, conn_id: &str) {
        let mut map = self.lock();
        let ids: Vec<SubscriptionId> =
            map.iter().filter(|(_, e)| e.conn_id == conn_id).map(|(id, _)| id.clone()).collect();
        for id in ids {
            if let Some(e) = map.remove(&id) {
                e.cancel.cancel();
            }
        }
    }

    /// Ends every subscription (shutdown).
    pub fn close_all(&self) {
        for (_, e) in std::mem::take(&mut *self.lock()) {
            e.cancel.cancel();
        }
    }

    /// Open subscriptions as `(id, kind, connection)`.
    pub fn list(&self) -> Vec<(SubscriptionId, &'static str, String)> {
        self.lock().iter().map(|(id, e)| (id.clone(), e.kind, e.conn_id.clone())).collect()
    }
}

#[derive(Debug)]
struct QueueState<T> {
    items: VecDeque<T>,
    dropped: u64,
}

/// A bounded queue that drops its **oldest** items when full and counts them (ARCHITECTURE §6.4).
#[derive(Debug)]
pub struct DropQueue<T> {
    state: Mutex<QueueState<T>>,
    cap: usize,
    ready: Notify,
}

impl<T> DropQueue<T> {
    /// A queue of at most `cap` items.
    pub fn new(cap: usize) -> Self {
        Self {
            state: Mutex::new(QueueState { items: VecDeque::new(), dropped: 0 }),
            cap: cap.max(1),
            ready: Notify::new(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, QueueState<T>> {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Adds an item, dropping the oldest when full. Never blocks on the consumer.
    pub fn push(&self, item: T) {
        let mut s = self.lock();
        if s.items.len() == self.cap {
            s.items.pop_front();
            s.dropped += 1;
        }
        s.items.push_back(item);
        drop(s);
        self.ready.notify_one();
    }

    /// Counts items lost before they reached the queue (for example a lagging broadcast receiver).
    pub fn note_dropped(&self, n: u64) {
        self.lock().dropped += n;
        self.ready.notify_one();
    }

    /// Takes up to `max` items and the number dropped since the last take.
    pub fn take(&self, max: usize) -> (Vec<T>, u64) {
        let mut s = self.lock();
        let n = s.items.len().min(max);
        let items: Vec<T> = s.items.drain(..n).collect();
        let dropped = std::mem::take(&mut s.dropped);
        (items, dropped)
    }

    /// Items waiting.
    pub fn len(&self) -> usize {
        self.lock().items.len()
    }

    /// Whether nothing waits (dropped counts aside).
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Waits until something is pushed (or dropped).
    pub async fn ready(&self) {
        self.ready.notified().await;
    }
}

/// Waits until `peer`'s writer has room (its backlog is at most [`MAX_BACKLOG`]) or `cancel` fires; `false` when
/// cancelled.
pub async fn wait_for_room(peer: &Peer, cancel: &CancellationToken) -> bool {
    while peer.backlog() > MAX_BACKLOG {
        tokio::select! {
            () = cancel.cancelled() => return false,
            () = tokio::time::sleep(Duration::from_millis(10)) => {}
        }
    }
    !cancel.is_cancelled()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_oldest_items_are_dropped_and_counted() {
        let q = DropQueue::new(3);
        for i in 0..5 {
            q.push(i);
        }
        q.note_dropped(2);
        assert_eq!(q.take(10), (vec![2, 3, 4], 4));
        assert_eq!(q.take(10), (vec![], 0), "the count resets");
        q.push(9);
        assert_eq!(q.take(0), (vec![], 0));
        assert_eq!(q.len(), 1);
    }

    #[tokio::test]
    async fn subscriptions_end_with_cancel_or_their_connection() {
        let (client, server) = russet_ipc::in_process();
        let hub = SubscriptionHub::default();
        let a = hub.open(&server, "job");
        let b = hub.open(&server, "daemon.logs");
        assert_eq!(hub.list().len(), 2);
        assert_eq!(hub.cancel(&a.id, "c-other").expect_err("not yours").code(), codes::core::NOT_FOUND);
        hub.cancel(&a.id, server.connection_id()).expect("cancelled");
        assert!(a.cancel.is_cancelled());
        drop(client);
        server.close();
        tokio::time::timeout(Duration::from_secs(2), b.cancel.cancelled()).await.expect("ends with the connection");
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(hub.list().is_empty());
    }
}
