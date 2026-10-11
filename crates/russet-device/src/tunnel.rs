// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The userspace CoreDeviceProxy tunnel with the MSS clamp, and the per-generation RSD service map (device.md §5.3,
//! F1, F2; DEV-04).
//!
//! idevice's `create_software_tunnel` sizes TCP segments from the tunnel MTU (16000 → MSS 15940), but the USB
//! CoreDeviceProxy path drops host-to-device IPv6 packets above about 8 KiB, so every bulk upload stalls (F1).
//! Russet builds the jktcp adapter itself with `MSS = min(mtu − 60, 8132)`. RSD ports change on every tunnel (F2):
//! the map belongs to one tunnel generation and is never cached across tunnels.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use idevice::core_device_proxy::CoreDeviceProxy;
use idevice::rsd::RsdHandshake;
use idevice::{IdeviceService, ReadWrite};
use jktcp::adapter::Adapter;
use jktcp::handle::AdapterHandle;
use russet_core::Result;
use russet_core::error::codes;
use time::OffsetDateTime;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::error::{device_error, from_idevice};
use crate::mux::MuxProvider;

/// How long a closing tunnel keeps its adapter running (see [`Tunnel::close`]).
pub const CLOSE_GRACE: Duration = Duration::from_millis(200);

/// The USB CoreDeviceProxy MSS cap (device.md F1; OQ-7 keeps 8132).
pub const USB_MSS_CAP: usize = 8132;

/// Tunnel settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunnelConfig {
    /// 8132 for USB CoreDeviceProxy (F1).
    pub mss_cap: usize,
    /// TCP connect inside the tunnel (jktcp's SYN budget is 8 s).
    pub connect_timeout: Duration,
    /// The RSD handshake.
    pub rsd_timeout: Duration,
    /// Debug only: `RUSSET_TUNNEL_PCAP`.
    pub pcap: Option<PathBuf>,
}

impl Default for TunnelConfig {
    fn default() -> Self {
        Self {
            mss_cap: USB_MSS_CAP,
            connect_timeout: Duration::from_secs(8),
            rsd_timeout: Duration::from_secs(5),
            pcap: None,
        }
    }
}

/// The MSS for a tunnel MTU: the IPv6 (40) and TCP (20) headers off the MTU, capped (device.md F1).
pub fn select_mss(mtu: u16, cap: usize) -> usize {
    (mtu as usize).saturating_sub(60).min(cap).max(536)
}

/// One RSD service of the current generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RsdEntry {
    pub port: u16,
    pub features: Vec<String>,
}

/// An open tunnel: the jktcp adapter, the RSD map and its generation.
#[derive(Debug)]
pub struct Tunnel {
    handle: AdapterHandle,
    pub generation: u64,
    pub since: OffsetDateTime,
    pub opened: Instant,
    pub mtu: u16,
    pub mss: usize,
    pub services: BTreeMap<String, RsdEntry>,
    pub properties: BTreeMap<String, plist::Value>,
    /// Streams opened through this tunnel that are still alive; idle close waits for zero.
    active: Arc<AtomicUsize>,
}

impl Tunnel {
    /// Opens CoreDeviceProxy over lockdown, clamps the MSS and performs the RSD handshake.
    pub async fn open_usb(provider: &MuxProvider, cfg: &TunnelConfig, generation: u64) -> Result<Self> {
        let proxy =
            CoreDeviceProxy::connect(provider).await.map_err(|e| tunnel_error(from_idevice(e, "CoreDeviceProxy")))?;
        let info = proxy.tunnel_info().clone();
        let ours: std::net::IpAddr = info
            .client_address
            .parse()
            .map_err(|_| device_error(codes::device::TUNNEL_FAILED, "CDTunnel gave an invalid client address"))?;
        let theirs: std::net::IpAddr = info
            .server_address
            .parse()
            .map_err(|_| device_error(codes::device::TUNNEL_FAILED, "CDTunnel gave an invalid server address"))?;
        let stream: Box<dyn ReadWrite> = proxy.into_tunnel().into_inner();
        let mut adapter = Adapter::new(Box::new(stream), ours, theirs);
        let mss = select_mss(info.mtu, cfg.mss_cap);
        adapter.set_mss(mss);
        if let Some(p) = &cfg.pcap {
            adapter
                .pcap(p)
                .await
                .map_err(|e| device_error(codes::device::IO, format!("cannot write the tunnel pcap: {e}")))?;
        }
        let mut handle = adapter.to_async_handle();
        let rsd = tokio::time::timeout(cfg.rsd_timeout, async {
            let s = handle.connect(info.server_rsd_port).await.map_err(|e| {
                device_error(codes::device::TUNNEL_FAILED, format!("cannot reach RSD in the tunnel: {e}"))
            })?;
            RsdHandshake::new(s).await.map_err(|e| tunnel_error(from_idevice(e, "RSD handshake")))
        })
        .await
        .map_err(|_| device_error(codes::device::TUNNEL_FAILED, "the RSD handshake timed out"))??;
        let services = rsd
            .services
            .iter()
            .map(|(name, s)| {
                (name.clone(), RsdEntry { port: s.port, features: s.features.clone().unwrap_or_default() })
            })
            .collect();
        let properties = rsd.properties.into_iter().collect();
        tracing::debug!(generation, mss, mtu = info.mtu, "device tunnel up");
        Ok(Self {
            handle,
            generation,
            since: OffsetDateTime::now_utc(),
            opened: Instant::now(),
            mtu: info.mtu,
            mss,
            services,
            properties,
            active: Arc::new(AtomicUsize::new(0)),
        })
    }

    /// The port of an RSD service in this generation.
    pub fn service_port(&self, name: &str) -> Result<u16> {
        self.services.get(name).map(|s| s.port).ok_or_else(|| {
            device_error(codes::device::SERVICE_UNAVAILABLE, format!("the device does not advertise {name}"))
        })
    }

    /// Connects to a TCP port inside the tunnel.
    pub async fn connect_port(
        &self,
        port: u16,
        timeout: Duration,
    ) -> std::result::Result<Box<dyn ReadWrite>, TunnelConnectError> {
        let mut handle = self.handle.clone();
        match tokio::time::timeout(timeout, handle.connect(port)).await {
            Ok(Ok(s)) => Ok(Box::new(CountedStream::new(s, Arc::clone(&self.active)))),
            Ok(Err(e)) => Err(TunnelConnectError::from_io(e)),
            Err(_) => Err(TunnelConnectError::Timeout),
        }
    }

    /// Streams still open through this tunnel.
    pub fn active_streams(&self) -> usize {
        self.active.load(Ordering::SeqCst)
    }

    /// Stops the adapter at once and keeps the tunnel in place, as if the CDTunnel stream had died: the next connect
    /// fails as dead and the connector rebuilds the tunnel (tests and lab checks of the rebuild path).
    pub async fn kill(&mut self) {
        let _ = self.handle.close().await;
    }

    /// Stops the adapter task; every stream of this generation dies. The adapter keeps running for
    /// [`CLOSE_GRACE`] first, so the FIN exchanges of the streams just closed complete inside the tunnel: a tunnel
    /// cut mid-handshake stays allocated on the device for seconds, and back-to-back open/close cycles then run into
    /// CoreDeviceProxy's limit (measured on iOS 27.0, 2026-10-11).
    pub async fn close(mut self) {
        tokio::time::sleep(CLOSE_GRACE).await;
        let _ = self.handle.close().await;
    }
}

fn tunnel_error(e: russet_core::Error) -> russet_core::Error {
    if e.code() == codes::device::PROTOCOL || e.code() == codes::device::IO {
        device_error(codes::device::TUNNEL_FAILED, e.message().to_owned())
    } else {
        e
    }
}

/// Why a connect inside the tunnel failed.
#[derive(Debug)]
pub enum TunnelConnectError {
    /// The adapter task is gone (the CDTunnel stream died): rebuild the tunnel.
    Dead(std::io::Error),
    /// The device refused or reset the connection.
    Refused(std::io::Error),
    Timeout,
}

impl TunnelConnectError {
    fn from_io(e: std::io::Error) -> Self {
        use std::io::ErrorKind as K;
        match e.kind() {
            K::NetworkUnreachable | K::BrokenPipe => Self::Dead(e),
            _ => Self::Refused(e),
        }
    }

    /// True when the tunnel must be rebuilt.
    pub fn is_dead(&self) -> bool {
        matches!(self, Self::Dead(_))
    }

    /// As a Russet error.
    pub fn into_error(self, what: &str) -> russet_core::Error {
        match self {
            Self::Dead(e) => device_error(codes::device::TUNNEL_FAILED, format!("{what}: the tunnel is down: {e}")),
            Self::Refused(e) if e.kind() == std::io::ErrorKind::TimedOut => {
                device_error(codes::device::TUNNEL_STALLED, format!("{what}: {e}"))
            }
            Self::Refused(e) => device_error(codes::device::SERVICE_UNAVAILABLE, format!("{what}: {e}")),
            Self::Timeout => device_error(codes::device::TUNNEL_STALLED, format!("{what}: the connect timed out")),
        }
    }
}

/// A stream that counts itself in the tunnel's active set until dropped.
#[derive(Debug)]
pub struct CountedStream<S> {
    inner: S,
    active: Arc<AtomicUsize>,
}

impl<S> CountedStream<S> {
    pub fn new(inner: S, active: Arc<AtomicUsize>) -> Self {
        active.fetch_add(1, Ordering::SeqCst);
        Self { inner, active }
    }
}

impl<S> Drop for CountedStream<S> {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for CountedStream<S> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for CountedStream<S> {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mss_is_clamped_for_usb_core_device_proxy() {
        // The device's CDTunnel MTU is 16000: idevice would use 15940, which stalls (F1).
        assert_eq!(select_mss(16000, USB_MSS_CAP), 8132);
        assert_eq!(select_mss(16000, usize::MAX), 15940);
        // Small MTUs keep the header arithmetic; the floor is the IPv6 minimum segment.
        assert_eq!(select_mss(1500, USB_MSS_CAP), 1440);
        assert_eq!(select_mss(100, USB_MSS_CAP), 536);
        assert_eq!(TunnelConfig::default().mss_cap, 8132);
    }

    #[tokio::test]
    async fn counted_streams_track_liveness() {
        let active = Arc::new(AtomicUsize::new(0));
        let (a, _b) = tokio::io::duplex(8);
        let s = CountedStream::new(a, Arc::clone(&active));
        assert_eq!(active.load(Ordering::SeqCst), 1);
        drop(s);
        assert_eq!(active.load(Ordering::SeqCst), 0);
    }
}
