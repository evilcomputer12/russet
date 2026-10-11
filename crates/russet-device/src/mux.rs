// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Russet's usbmuxd client (device.md §6.1, §10; DEV-02). usbmuxd on Linux (`/var/run/usbmuxd`), the Apple Mobile
//! Device Service on Windows (`127.0.0.1:27015`), or netmuxd; `USBMUXD_SOCKET_ADDRESS` and `[device] mux`
//! override the default.
//!
//! Russet speaks the plist mux protocol itself instead of idevice's `UsbmuxdConnection`, which carries two known
//! problems in 0.1.68 (ARCHITECTURE §16.7 [D 22], device.md U8/U10): a declared packet size below 16 underflows,
//! any larger size is allocated as is (up to about 4 GiB), and the `Listen` stream is not `Send`. Here a header
//! declaring fewer than 16 or more than [`MAX_PACKET_BYTES`] bytes is rejected *before* any allocation, and
//! [`MuxClient::listen`] returns a `Send + 'static` stream. [`MuxProvider`] implements idevice's
//! `IdeviceProvider` on top of it, so every lockdown service Russet opens goes through this code.

use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
#[cfg(unix)]
use std::path::PathBuf;
use std::pin::Pin;
use std::str::FromStr;
use std::time::Duration;

use futures::stream::BoxStream;
use idevice::pairing_file::PairingFile;
use idevice::provider::IdeviceProvider;
use idevice::{Idevice, IdeviceError, ReadWrite};
use russet_core::Result;
use russet_core::error::codes;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::error::{device_error, invalid_argument};
use crate::model::{Transport, Udid};

/// The largest mux packet Russet accepts: 16 MiB including the 16-byte header (ARCHITECTURE §16.7). Pair records
/// and device lists are a few KiB.
pub const MAX_PACKET_BYTES: u32 = 16 * 1024 * 1024;
/// The mux header: length, version, message type, tag (four little-endian u32).
pub const HEADER_BYTES: u32 = 16;
/// Protocol version 1 = XML plists.
const VERSION_PLIST: u32 = 1;
/// Message type 8 = plist.
const MESSAGE_PLIST: u32 = 8;
/// The client name usbmuxd logs.
const CLIENT_NAME: &str = "russet";
/// How long one mux request may take before it counts as unavailable.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Where the mux daemon listens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MuxEndpoint {
    #[cfg(unix)]
    Unix(PathBuf),
    Tcp(SocketAddr),
}

impl MuxEndpoint {
    /// Linux and macOS: `/var/run/usbmuxd`; Windows: the Apple Mobile Device Service on `127.0.0.1:27015`.
    pub fn platform_default() -> Self {
        #[cfg(unix)]
        {
            Self::Unix(PathBuf::from("/var/run/usbmuxd"))
        }
        #[cfg(not(unix))]
        {
            Self::Tcp(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 27015))
        }
    }

    /// Parses `USBMUXD_SOCKET_ADDRESS`-style text: `host:port` is TCP, anything else a Unix socket path.
    pub fn parse_address(s: &str) -> Result<Self> {
        let s = s.trim();
        if let Ok(addr) = SocketAddr::from_str(s) {
            return Ok(Self::Tcp(addr));
        }
        #[cfg(unix)]
        {
            if !s.is_empty() && !s.contains(':') {
                return Ok(Self::Unix(PathBuf::from(s)));
            }
        }
        Err(invalid_argument(format!("not a mux address: {s:?}")))
    }

    /// The `[device] mux` setting (ARCHITECTURE §14.3: `auto | usbmuxd:<path> | amds | netmuxd:<addr>`). `auto` uses
    /// `USBMUXD_SOCKET_ADDRESS` when set (both idevice and pymobiledevice3 honour it, QA V12), else the default.
    pub fn from_setting(setting: &str) -> Result<Self> {
        let setting = setting.trim();
        match setting {
            "" | "auto" => match std::env::var("USBMUXD_SOCKET_ADDRESS") {
                Ok(v) if !v.trim().is_empty() => Self::parse_address(&v),
                _ => Ok(Self::platform_default()),
            },
            "amds" => Ok(Self::Tcp(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 27015))),
            other => {
                if let Some(path) = other.strip_prefix("usbmuxd:") {
                    Self::parse_address(path)
                } else if let Some(addr) = other.strip_prefix("netmuxd:") {
                    Self::parse_address(addr)
                } else {
                    Err(invalid_argument(format!(
                        "`[device] mux = {other:?}`: expected auto, usbmuxd:<path>, amds or netmuxd:<addr>"
                    )))
                }
            }
        }
    }

    /// For messages; never secret.
    pub fn describe(&self) -> String {
        match self {
            #[cfg(unix)]
            Self::Unix(p) => p.display().to_string(),
            Self::Tcp(a) => a.to_string(),
        }
    }

    async fn open(&self) -> Result<Box<dyn ReadWrite>> {
        let res: std::io::Result<Box<dyn ReadWrite>> = match self {
            #[cfg(unix)]
            Self::Unix(p) => tokio::net::UnixStream::connect(p).await.map(|s| Box::new(s) as Box<dyn ReadWrite>),
            Self::Tcp(a) => tokio::net::TcpStream::connect(a).await.map(|s| Box::new(s) as Box<dyn ReadWrite>),
        };
        res.map_err(|e| {
            device_error(codes::device::USBMUX_UNAVAILABLE, format!("cannot reach the mux at {}: {e}", self.describe()))
                .caused_by(e)
        })
    }
}

/// One mux packet header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// The whole packet, header included.
    pub length: u32,
    pub version: u32,
    pub message: u32,
    pub tag: u32,
}

impl Header {
    /// Encodes the header.
    pub fn encode(&self) -> [u8; 16] {
        let mut out = [0u8; 16];
        out[0..4].copy_from_slice(&self.length.to_le_bytes());
        out[4..8].copy_from_slice(&self.version.to_le_bytes());
        out[8..12].copy_from_slice(&self.message.to_le_bytes());
        out[12..16].copy_from_slice(&self.tag.to_le_bytes());
        out
    }

    /// Decodes and checks a header: a declared length below 16 or above [`MAX_PACKET_BYTES`] is a protocol error,
    /// so nothing is ever allocated for a bogus size (device.md U10).
    pub fn decode(b: [u8; 16]) -> Result<Self> {
        let word = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        let h = Self { length: word(0), version: word(4), message: word(8), tag: word(12) };
        if h.length < HEADER_BYTES {
            return Err(device_error(
                codes::device::PROTOCOL,
                format!("mux packet declares {} bytes, less than its 16-byte header", h.length),
            ));
        }
        if h.length > MAX_PACKET_BYTES {
            return Err(device_error(
                codes::device::PROTOCOL,
                format!("mux packet declares {} bytes, more than the {MAX_PACKET_BYTES}-byte limit", h.length),
            ));
        }
        Ok(h)
    }

    /// The payload length.
    pub fn body_len(&self) -> usize {
        (self.length - HEADER_BYTES) as usize
    }
}

/// Encodes one plist request.
pub fn encode_request(dict: &plist::Dictionary, tag: u32) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    plist::to_writer_xml(&mut body, dict)
        .map_err(|e| device_error(codes::device::PROTOCOL, format!("cannot encode a mux request: {e}")))?;
    let length = u32::try_from(body.len() + HEADER_BYTES as usize)
        .ok()
        .filter(|l| *l <= MAX_PACKET_BYTES)
        .ok_or_else(|| device_error(codes::device::PROTOCOL, "mux request too large"))?;
    let mut out = Header { length, version: VERSION_PLIST, message: MESSAGE_PLIST, tag }.encode().to_vec();
    out.extend_from_slice(&body);
    Ok(out)
}

/// Reads one plist packet from `r`, enforcing the size limits of [`Header::decode`].
pub async fn read_packet<R: tokio::io::AsyncRead + Unpin + ?Sized>(r: &mut R) -> Result<(Header, plist::Dictionary)> {
    let mut hb = [0u8; 16];
    r.read_exact(&mut hb).await.map_err(|e| mux_io(e, "reading a mux header"))?;
    let h = Header::decode(hb)?;
    let mut body = vec![0u8; h.body_len()];
    r.read_exact(&mut body).await.map_err(|e| mux_io(e, "reading a mux packet"))?;
    let dict: plist::Dictionary = plist::from_bytes(&body)
        .map_err(|e| device_error(codes::device::PROTOCOL, format!("malformed mux plist: {e}")))?;
    Ok((h, dict))
}

fn mux_io(e: std::io::Error, what: &str) -> russet_core::Error {
    let code = match e.kind() {
        std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe => {
            codes::device::USBMUX_UNAVAILABLE
        }
        _ => codes::device::IO,
    };
    device_error(code, format!("{what}: {e}")).caused_by(e)
}

/// A device the mux reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MuxDevice {
    /// The mux's own id (changes on every attach).
    pub device_id: u32,
    pub udid: Udid,
    pub transport: Transport,
    /// For network devices.
    pub address: Option<IpAddr>,
}

/// A mux `Listen` event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MuxEvent {
    Attached(MuxDevice),
    /// By mux device id.
    Detached(u32),
}

/// Parses an `Attached` record (`ListDevices` entries and `Listen` events have the same shape).
pub fn parse_device(record: &plist::Dictionary) -> Option<MuxDevice> {
    let props = record.get("Properties").and_then(|p| p.as_dictionary())?;
    let device_id = record
        .get("DeviceID")
        .or_else(|| props.get("DeviceID"))
        .and_then(|v| v.as_unsigned_integer())
        .and_then(|v| u32::try_from(v).ok())?;
    let udid = props.get("SerialNumber").and_then(|v| v.as_string())?.to_owned();
    let connection = props.get("ConnectionType").and_then(|v| v.as_string()).unwrap_or("USB");
    let (transport, address) = if connection.eq_ignore_ascii_case("network") {
        (Transport::Wifi, props.get("NetworkAddress").and_then(|v| v.as_data()).and_then(parse_sockaddr))
    } else {
        (Transport::Usb, None)
    };
    Some(MuxDevice { device_id, udid: Udid(udid), transport, address })
}

/// usbmuxd's `NetworkAddress`: a raw `sockaddr` (family byte at 1: 2 = IPv4, 30 or 10 = IPv6).
fn parse_sockaddr(b: &[u8]) -> Option<IpAddr> {
    match b.get(1).copied()? {
        2 if b.len() >= 8 => Some(IpAddr::V4(Ipv4Addr::new(b[4], b[5], b[6], b[7]))),
        10 | 23 | 30 if b.len() >= 24 => {
            let mut a = [0u8; 16];
            a.copy_from_slice(&b[8..24]);
            Some(IpAddr::V6(Ipv6Addr::from(a)))
        }
        _ => None,
    }
}

/// The `Result` code of a mux reply: 0 ok, 1 bad command, 2 bad device, 3 connection refused, 6 bad version.
fn result_number(reply: &plist::Dictionary) -> Option<u64> {
    reply.get("Number").and_then(|v| v.as_unsigned_integer())
}

fn result_error(number: u64, what: &str) -> russet_core::Error {
    match number {
        2 => device_error(codes::device::DISCONNECTED, format!("{what}: the mux no longer knows this device")),
        3 => device_error(codes::device::SERVICE_UNAVAILABLE, format!("{what}: the device refused the connection")),
        n => device_error(codes::device::PROTOCOL, format!("{what}: the mux answered error {n}")),
    }
}

/// One connection to the mux daemon. Each request waits for its reply; `Connect` and `Listen` consume it.
#[derive(Debug)]
pub struct MuxClient {
    stream: Box<dyn ReadWrite>,
    endpoint: MuxEndpoint,
    tag: u32,
}

impl MuxClient {
    /// Opens a connection.
    pub async fn connect(endpoint: &MuxEndpoint) -> Result<Self> {
        let stream = endpoint.open().await?;
        Ok(Self { stream, endpoint: endpoint.clone(), tag: 0 })
    }

    /// Wraps an already-open stream (tests, fake muxes).
    pub fn from_stream(stream: Box<dyn ReadWrite>, endpoint: MuxEndpoint) -> Self {
        Self { stream, endpoint, tag: 0 }
    }

    fn request(message_type: &str) -> plist::Dictionary {
        let mut d = plist::Dictionary::new();
        d.insert("MessageType".into(), message_type.into());
        d.insert("ClientVersionString".into(), CLIENT_NAME.into());
        d.insert("ProgName".into(), CLIENT_NAME.into());
        d.insert("kLibUSBMuxVersion".into(), 3.into());
        d
    }

    async fn send(&mut self, dict: &plist::Dictionary) -> Result<()> {
        self.tag = self.tag.wrapping_add(1);
        let bytes = encode_request(dict, self.tag)?;
        self.stream.write_all(&bytes).await.map_err(|e| mux_io(e, "writing a mux request"))?;
        self.stream.flush().await.map_err(|e| mux_io(e, "writing a mux request"))
    }

    async fn roundtrip(&mut self, dict: &plist::Dictionary, what: &str) -> Result<plist::Dictionary> {
        let fut = async {
            self.send(dict).await?;
            // Skip unsolicited packets (for example Attached events on a reused connection).
            loop {
                let (h, reply) = read_packet(&mut self.stream).await?;
                if h.tag == self.tag
                    || h.tag == 0 && reply.get("MessageType").and_then(|v| v.as_string()) == Some("Result")
                {
                    return Ok(reply);
                }
            }
        };
        match tokio::time::timeout(REQUEST_TIMEOUT, fut).await {
            Ok(r) => r,
            Err(_) => Err(device_error(codes::device::USBMUX_UNAVAILABLE, format!("{what}: the mux did not answer"))),
        }
    }

    /// `ListDevices`.
    pub async fn list_devices(&mut self) -> Result<Vec<MuxDevice>> {
        let reply = self.roundtrip(&Self::request("ListDevices"), "ListDevices").await?;
        let list = reply
            .get("DeviceList")
            .and_then(|v| v.as_array())
            .ok_or_else(|| device_error(codes::device::PROTOCOL, "ListDevices: the reply has no DeviceList"))?;
        Ok(list.iter().filter_map(|v| v.as_dictionary()).filter_map(parse_device).collect())
    }

    /// `ReadBUID`: the host's system BUID.
    pub async fn read_buid(&mut self) -> Result<String> {
        let reply = self.roundtrip(&Self::request("ReadBUID"), "ReadBUID").await?;
        reply
            .get("BUID")
            .and_then(|v| v.as_string())
            .map(str::to_owned)
            .ok_or_else(|| device_error(codes::device::PROTOCOL, "ReadBUID: the reply has no BUID"))
    }

    /// `ReadPairRecord`: the raw pair record plist, or `device.notPaired` when the mux has none.
    pub async fn read_pair_record(&mut self, udid: &Udid) -> Result<Vec<u8>> {
        let mut req = Self::request("ReadPairRecord");
        req.insert("PairRecordID".into(), udid.as_str().into());
        let reply = self.roundtrip(&req, "ReadPairRecord").await?;
        match reply.get("PairRecordData").and_then(|v| v.as_data()) {
            Some(d) => Ok(d.to_vec()),
            None => Err(device_error(
                codes::device::NOT_PAIRED,
                format!("no pair record for {} in the mux", udid.redacted()),
            )),
        }
    }

    /// `SavePairRecord`, so usbmuxd (or AMDS) owns the record (device.md §6.2).
    pub async fn save_pair_record(&mut self, udid: &Udid, device_id: u32, record: Vec<u8>) -> Result<()> {
        let mut req = Self::request("SavePairRecord");
        req.insert("PairRecordID".into(), udid.as_str().into());
        req.insert("PairRecordData".into(), plist::Value::Data(record));
        req.insert("DeviceID".into(), device_id.into());
        let reply = self.roundtrip(&req, "SavePairRecord").await?;
        match result_number(&reply) {
            Some(0) => Ok(()),
            Some(n) => Err(result_error(n, "SavePairRecord")),
            None => Err(device_error(codes::device::PROTOCOL, "SavePairRecord: no result")),
        }
    }

    /// `Connect` to a TCP port on the device (host byte order); the connection becomes the raw stream.
    pub async fn connect_port(mut self, device_id: u32, port: u16) -> Result<Box<dyn ReadWrite>> {
        let mut req = Self::request("Connect");
        req.insert("DeviceID".into(), device_id.into());
        // usbmuxd wants the port in network byte order inside the integer.
        req.insert("PortNumber".into(), u64::from(port.to_be()).into());
        let reply = self.roundtrip(&req, "Connect").await?;
        match result_number(&reply) {
            Some(0) => Ok(self.stream),
            Some(n) => Err(result_error(n, &format!("Connect to device port {port}"))),
            None => Err(device_error(codes::device::PROTOCOL, "Connect: no result")),
        }
    }

    /// `Listen`: attach and detach events, as a `Send + 'static` stream. Devices already present are reported as
    /// `Attached` first (usbmuxd does this within 0.1 ms, device.md §2.2).
    pub async fn listen(mut self) -> Result<BoxStream<'static, Result<MuxEvent>>> {
        let reply = self.roundtrip(&Self::request("Listen"), "Listen").await?;
        match result_number(&reply) {
            Some(0) => {}
            Some(n) => return Err(result_error(n, "Listen")),
            None => return Err(device_error(codes::device::PROTOCOL, "Listen: no result")),
        }
        let stream = futures::stream::try_unfold(self.stream, |mut s| async move {
            loop {
                let (_, msg) = read_packet(&mut s).await?;
                match msg.get("MessageType").and_then(|v| v.as_string()) {
                    Some("Attached") => {
                        if let Some(d) = parse_device(&msg) {
                            return Ok(Some((MuxEvent::Attached(d), s)));
                        }
                    }
                    Some("Detached") => {
                        if let Some(id) = msg.get("DeviceID").and_then(|v| v.as_unsigned_integer()) {
                            return Ok(Some((MuxEvent::Detached(id as u32), s)));
                        }
                    }
                    // Paired and other notifications are not needed.
                    _ => {}
                }
            }
        });
        Ok(Box::pin(stream))
    }

    /// The endpoint this client talks to.
    pub fn endpoint(&self) -> &MuxEndpoint {
        &self.endpoint
    }
}

/// idevice's `IdeviceProvider` over [`MuxClient`]: every lockdown connection Russet opens uses the hardened mux
/// code above.
#[derive(Debug, Clone)]
pub struct MuxProvider {
    pub endpoint: MuxEndpoint,
    pub device_id: u32,
    pub udid: Udid,
    pub label: String,
}

fn to_idevice_err(e: russet_core::Error) -> IdeviceError {
    use std::io::{Error as IoError, ErrorKind};
    let kind = match e.code() {
        c if c == codes::device::USBMUX_UNAVAILABLE => ErrorKind::ConnectionRefused,
        c if c == codes::device::DISCONNECTED => ErrorKind::NotConnected,
        c if c == codes::device::TIMEOUT => ErrorKind::TimedOut,
        c if c == codes::device::NOT_PAIRED => return IdeviceError::InvalidHostID,
        c if c == codes::device::NOT_FOUND => return IdeviceError::DeviceNotFound,
        c if c == codes::device::SERVICE_UNAVAILABLE => return IdeviceError::ServiceNotFound,
        _ => ErrorKind::Other,
    };
    IdeviceError::Socket(IoError::new(kind, e.message().to_owned()))
}

impl MuxProvider {
    /// A raw connection to a device port.
    pub async fn connect_raw(&self, port: u16) -> Result<Box<dyn ReadWrite>> {
        let client = MuxClient::connect(&self.endpoint).await?;
        client.connect_port(self.device_id, port).await
    }

    /// The device's pair record from the mux.
    pub async fn pairing_file(&self) -> Result<PairingFile> {
        let mut client = MuxClient::connect(&self.endpoint).await?;
        let bytes = client.read_pair_record(&self.udid).await?;
        PairingFile::from_bytes(&bytes).map_err(|e| {
            device_error(
                codes::device::NOT_PAIRED,
                format!("the pair record of {} is unreadable", self.udid.redacted()),
            )
            .caused_by(crate::error::Cause(crate::error::render_idevice(&e)))
        })
    }
}

impl IdeviceProvider for MuxProvider {
    fn connect(&self, port: u16) -> Pin<Box<dyn Future<Output = std::result::Result<Idevice, IdeviceError>> + Send>> {
        let this = self.clone();
        Box::pin(async move {
            let stream = this.connect_raw(port).await.map_err(to_idevice_err)?;
            let mut dev = Idevice::new(stream, this.label.clone());
            dev.set_udid(this.udid.as_str());
            Ok(dev)
        })
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn get_pairing_file(&self) -> Pin<Box<dyn Future<Output = std::result::Result<PairingFile, IdeviceError>> + Send>> {
        let this = self.clone();
        Box::pin(async move { this.pairing_file().await.map_err(to_idevice_err) })
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::AsyncWriteExt;

    use super::*;

    #[test]
    fn headers_round_trip_and_bad_sizes_are_rejected() {
        let h = Header { length: 300, version: 1, message: 8, tag: 7 };
        assert_eq!(Header::decode(h.encode()).expect("valid"), h);
        let short = Header { length: 15, ..h }.encode();
        assert_eq!(Header::decode(short).expect_err("short").code(), codes::device::PROTOCOL);
        let huge = Header { length: MAX_PACKET_BYTES + 1, ..h }.encode();
        assert_eq!(Header::decode(huge).expect_err("huge").code(), codes::device::PROTOCOL);
        let max = Header { length: MAX_PACKET_BYTES, ..h }.encode();
        assert_eq!(Header::decode(max).expect("16 MiB is allowed").body_len(), (MAX_PACKET_BYTES - 16) as usize);
    }

    /// The card's unit test: a declared usbmuxd size above 16 MiB is rejected without allocation. The reader gets
    /// only the 16-byte header (declaring 4 GiB − 1); if it tried to allocate and read the body it would block on
    /// the never-arriving bytes instead of failing at once.
    #[tokio::test]
    async fn a_declared_size_above_16_mib_is_rejected_before_reading_the_body() {
        let (mut client, mut server) = tokio::io::duplex(64);
        let header = Header { length: u32::MAX, version: 1, message: 8, tag: 1 }.encode();
        server.write_all(&header).await.expect("write header");
        let res = tokio::time::timeout(Duration::from_secs(2), read_packet(&mut client)).await.expect("no hang");
        let err = res.expect_err("rejected");
        assert_eq!(err.code(), codes::device::PROTOCOL);
        assert!(err.message().contains("more than"), "{}", err.message());
        drop(server);
    }

    fn attached(device_id: u64, udid: &str, conn: &str) -> plist::Dictionary {
        let mut props = plist::Dictionary::new();
        props.insert("ConnectionType".into(), conn.into());
        props.insert("DeviceID".into(), device_id.into());
        props.insert("SerialNumber".into(), udid.into());
        let mut d = plist::Dictionary::new();
        d.insert("MessageType".into(), "Attached".into());
        d.insert("DeviceID".into(), device_id.into());
        d.insert("Properties".into(), props.into());
        d
    }

    async fn reply(server: &mut tokio::io::DuplexStream, tag: u32, dict: &plist::Dictionary) {
        let bytes = encode_request(dict, tag).expect("encode");
        server.write_all(&bytes).await.expect("reply");
    }

    #[tokio::test]
    async fn list_listen_and_connect_speak_the_mux_protocol() {
        let (client, mut server) = tokio::io::duplex(1 << 16);
        let ep = MuxEndpoint::Tcp("127.0.0.1:1".parse().expect("addr"));
        let fake = tokio::spawn(async move {
            // ListDevices
            let (h, req) = read_packet(&mut server).await.expect("request");
            assert_eq!(req.get("MessageType").and_then(|v| v.as_string()), Some("ListDevices"));
            let mut list = plist::Dictionary::new();
            list.insert(
                "DeviceList".into(),
                plist::Value::Array(vec![attached(1, "00008101-000A1B2C3D4E5F60", "USB").into()]),
            );
            reply(&mut server, h.tag, &list).await;
            // Listen
            let (h, req) = read_packet(&mut server).await.expect("listen");
            assert_eq!(req.get("MessageType").and_then(|v| v.as_string()), Some("Listen"));
            let mut ok = plist::Dictionary::new();
            ok.insert("MessageType".into(), "Result".into());
            ok.insert("Number".into(), 0.into());
            reply(&mut server, h.tag, &ok).await;
            reply(&mut server, 0, &attached(1, "00008101-000A1B2C3D4E5F60", "USB")).await;
            let mut detached = plist::Dictionary::new();
            detached.insert("MessageType".into(), "Detached".into());
            detached.insert("DeviceID".into(), 1.into());
            reply(&mut server, 0, &detached).await;
        });
        let mut mux = MuxClient::from_stream(Box::new(client), ep);
        let devices = mux.list_devices().await.expect("list");
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].device_id, 1);
        assert_eq!(devices[0].transport, Transport::Usb);
        let mut events = mux.listen().await.expect("listen");
        use futures::StreamExt;
        let first = events.next().await.expect("event").expect("ok");
        assert!(matches!(first, MuxEvent::Attached(ref d) if d.device_id == 1));
        let second = events.next().await.expect("event").expect("ok");
        assert_eq!(second, MuxEvent::Detached(1));
        fake.await.expect("fake mux");
    }

    #[test]
    fn mux_settings_parse() {
        assert_eq!(
            MuxEndpoint::from_setting("amds").expect("amds"),
            MuxEndpoint::Tcp("127.0.0.1:27015".parse().expect("addr"))
        );
        assert_eq!(
            MuxEndpoint::from_setting("netmuxd:127.0.0.1:27016").expect("netmuxd"),
            MuxEndpoint::Tcp("127.0.0.1:27016".parse().expect("addr"))
        );
        #[cfg(unix)]
        assert_eq!(
            MuxEndpoint::from_setting("usbmuxd:/run/usbmuxd").expect("usbmuxd"),
            MuxEndpoint::Unix(PathBuf::from("/run/usbmuxd"))
        );
        assert!(MuxEndpoint::from_setting("bogus").is_err());
    }

    #[test]
    fn network_addresses_parse() {
        let mut v4 = vec![0u8; 16];
        v4[1] = 2;
        v4[4..8].copy_from_slice(&[192, 168, 1, 7]);
        assert_eq!(parse_sockaddr(&v4), Some(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 7))));
        assert_eq!(parse_sockaddr(&[0, 99]), None);
    }
}
