// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! [`RecordingConnector`]: wraps any [`ServiceConnector`] and records a [`Trace`] of everything that passes
//! through it (QA §3.5.1), without forking idevice: facade calls become `call` records, and service streams are
//! tapped ([`TapStream`]) so the bytes in both directions become `data` records.
//!
//! The recorder applies the `default-v1` redaction ([`crate::redaction`]) to facade arguments and results
//! before storing them, and the trace lint is the safety net. Timestamps are a logical step counter, so a trace
//! recorded from a scripted scenario is reproducible (QA §2 T10); set a fixed `recorded_at` in the header for a
//! committed fixture.
//!
//! Wrapping a [`crate::ScriptedConnector`] lets the whole record → replay path be exercised on Linux and
//! Windows CI with no device at all (QA-06).

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use async_trait::async_trait;
use futures::stream::{BoxStream, StreamExt};
use russet_core::error::Result;
use russet_device::io::{FacadeCall, ServiceConnector};
use russet_device::{DeviceEvent, Udid};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::rdt::{CallError, Dir, Header, Record, Trace, Via, base64_encode};
use crate::redaction::redact_value;

/// Services whose payloads the recorder never stores, by default: an Apple file flows through them (QA §3.5.2).
/// The payload becomes an [`Record::Elided`] (a length and a SHA-256).
pub fn default_elided_services() -> BTreeMap<String, &'static str> {
    [
        ("com.apple.mobile.mobile_image_mounter", "apple-file:ddi"),
        ("com.apple.dt.fetchsymbols", "apple-file:symbols"),
        ("com.apple.mobile.MCInstall", "apple-file:profile"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect()
}

/// A connector that records a trace of another connector's traffic.
pub struct RecordingConnector<C: ServiceConnector> {
    inner: Arc<C>,
    log: Arc<Log>,
}

struct Log {
    header: Header,
    step: AtomicU64,
    next_conn: AtomicU64,
    records: Mutex<Vec<Record>>,
    /// Services whose payloads are elided (QA §3.5.2), with the elision reason.
    elide: BTreeMap<String, &'static str>,
}

impl Log {
    /// The next logical timestamp (a step counter, so traces are reproducible).
    fn t(&self) -> f64 {
        self.step.fetch_add(1, Ordering::SeqCst) as f64
    }

    fn push(&self, rec: Record) {
        self.records.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(rec);
    }
}

impl<C: ServiceConnector> RecordingConnector<C> {
    /// Wraps `inner`, recording into a trace with `header`. The default set of Apple-file services is elided.
    pub fn new(inner: C, header: Header) -> Self {
        let log = Log {
            header,
            step: AtomicU64::new(0),
            next_conn: AtomicU64::new(0),
            records: Mutex::new(Vec::new()),
            elide: default_elided_services(),
        };
        Self { inner: Arc::new(inner), log: Arc::new(log) }
    }

    /// Also elide `service` (its payload becomes a length and a hash, with `reason`).
    pub fn elide_service(mut self, service: &str, reason: &'static str) -> Self {
        Arc::get_mut(&mut self.log).expect("elide_service before cloning").elide.insert(service.to_owned(), reason);
        self
    }

    /// The trace recorded so far.
    pub fn trace(&self) -> Trace {
        let records = self.log.records.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        Trace { header: self.log.header.clone(), records }
    }

    /// Writes the recorded trace to a file (by extension: plain `*.rdt.jsonl` or `*.rdt.jsonl.zst`).
    pub fn write(&self, path: &std::path::Path) -> Result<()> {
        self.trace().write(path)
    }

    fn tap(&self, via: Via, service: &str, stream: Box<dyn idevice::ReadWrite>) -> Box<dyn idevice::ReadWrite> {
        let conn = self.log.next_conn.fetch_add(1, Ordering::SeqCst) + 1;
        self.log.push(Record::Conn {
            id: conn,
            t: self.log.t(),
            via: Some(via),
            service: Some(service.into()),
            open: true,
        });
        let elide = self.log.elide.get(service).copied();
        Box::new(TapStream {
            inner: stream,
            conn,
            log: Arc::clone(&self.log),
            elide,
            c2d: Elided::default(),
            d2c: Elided::default(),
        })
    }
}

#[async_trait]
impl<C: ServiceConnector> ServiceConnector for RecordingConnector<C> {
    async fn watch(&self) -> Result<BoxStream<'static, DeviceEvent>> {
        let stream = self.inner.watch().await?;
        // Pass events through untouched; discovery is carried by the header, so events need no records.
        Ok(stream.boxed())
    }

    async fn call(&self, call: FacadeCall) -> Result<Value> {
        let api = call.api.clone();
        let mut args = call.args.clone();
        redact_value(&mut args);
        let t = self.log.t();
        match self.inner.call(call).await {
            Ok(mut result) => {
                redact_value(&mut result);
                self.log.push(Record::Call { t, api, args, result: Some(result.clone()), error: None });
                Ok(result)
            }
            Err(e) => {
                self.log.push(Record::Call {
                    t,
                    api,
                    args,
                    result: None,
                    error: Some(CallError {
                        code: e.code().as_str().to_owned(),
                        message: Some(e.message().to_owned()),
                    }),
                });
                Err(e)
            }
        }
    }

    async fn lockdown_service(&self, udid: &Udid, name: &str) -> Result<Box<dyn idevice::ReadWrite>> {
        let stream = self.inner.lockdown_service(udid, name).await?;
        Ok(self.tap(Via::Lockdown, name, stream))
    }

    async fn rsd_service(&self, udid: &Udid, name: &str) -> Result<Box<dyn idevice::ReadWrite>> {
        let stream = self.inner.rsd_service(udid, name).await?;
        Ok(self.tap(Via::Rsd, name, stream))
    }
}

/// A running elision of one direction of an elided stream: the length and hash, never the bytes.
#[derive(Default)]
struct Elided {
    len: u64,
    hasher: Option<Sha256>,
}

impl Elided {
    fn feed(&mut self, bytes: &[u8]) {
        self.len += bytes.len() as u64;
        self.hasher.get_or_insert_with(Sha256::new).update(bytes);
    }

    fn finish(self) -> Option<(u64, String)> {
        self.hasher.map(|h| (self.len, hex(&h.finalize())))
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A service stream that records the bytes flowing through it (QA §3.5.1), or, for an elided service, only their
/// length and hash (QA §3.5.2).
struct TapStream {
    inner: Box<dyn idevice::ReadWrite>,
    conn: u64,
    log: Arc<Log>,
    /// The elision reason when this service's payload is not stored.
    elide: Option<&'static str>,
    c2d: Elided,
    d2c: Elided,
}

impl std::fmt::Debug for TapStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TapStream").field("conn", &self.conn).finish_non_exhaustive()
    }
}

impl AsyncRead for TapStream {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let poll = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = &poll {
            let new = buf.filled()[before..].to_vec();
            if !new.is_empty() {
                if self.elide.is_some() {
                    self.d2c.feed(&new);
                } else {
                    let rec =
                        Record::Data { conn: self.conn, t: self.log.t(), dir: Dir::D2c, b64: base64_encode(&new) };
                    self.log.push(rec);
                }
            }
        }
        poll
    }
}

impl AsyncWrite for TapStream {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        let poll = Pin::new(&mut self.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(n)) = &poll
            && *n > 0
        {
            if self.elide.is_some() {
                let chunk = buf[..*n].to_vec();
                self.c2d.feed(&chunk);
            } else {
                let rec =
                    Record::Data { conn: self.conn, t: self.log.t(), dir: Dir::C2d, b64: base64_encode(&buf[..*n]) };
                self.log.push(rec);
            }
        }
        poll
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

impl Drop for TapStream {
    fn drop(&mut self) {
        if let Some(reason) = self.elide {
            for (dir, elided) in [(Dir::C2d, std::mem::take(&mut self.c2d)), (Dir::D2c, std::mem::take(&mut self.d2c))]
            {
                if let Some((len, sha256)) = elided.finish() {
                    self.log.push(Record::Elided {
                        conn: Some(self.conn),
                        t: self.log.t(),
                        dir,
                        len,
                        sha256,
                        reason: reason.into(),
                    });
                }
            }
        }
        self.log.push(Record::Conn { id: self.conn, t: self.log.t(), via: None, service: None, open: false });
    }
}
