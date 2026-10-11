// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Port forwards and debug launch helpers (DEV-08; ARCHITECTURE §10.4).

use std::net::SocketAddr;
#[cfg(unix)]
use std::path::PathBuf;

/// What a forward connects to on the device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForwardTarget {
    /// An RSD service, for example `com.apple.internal.dt.remote.debugproxy`.
    RsdService(String),
    /// Any TCP port on the device's tunnel address.
    TunnelPort(u16),
    /// A classic usbmux connect (iproxy-style), no tunnel.
    UsbmuxPort(u16),
}

/// Where a forward listens locally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindSpec {
    /// Loopback unless explicitly allowed.
    Tcp(SocketAddr),
    /// Created 0600 under the runtime directory.
    #[cfg(unix)]
    Unix(PathBuf),
}

/// A live forward; dropping it closes the forward. W4 defines the fields (lease id, local endpoint).
#[derive(Debug)]
pub struct ForwardHandle {
    _f0: (),
}
