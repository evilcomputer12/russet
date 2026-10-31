// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The Windows named pipe (ARCHITECTURE §7.1; core §7.1).
//!
//! - The name is `\\.\pipe\russet-<user SID>`, plus `-<hash8>` when the runtime directory is not the default
//!   one (`RUSSET_HOME`).
//! - The server sets the explicit DACL `D:P(A;;GA;;;<UserSID>)(A;;GA;;;SY)` (the default pipe DACL would let
//!   Everyone and Anonymous read), `reject_remote_clients` and `first_pipe_instance`. Wine does not enforce
//!   `first_pipe_instance`, so the daemon's lock file stays the primary single-instance guard.
//! - The client checks that the server process runs as the same user, which blocks pipe squatting.
//!
//! This code compiles on Windows from Wave 1; its DACL and squatting tests run on `windows-2025` CI (W15)
//! and the physical PC (W27a).

use std::ffi::c_void;
use std::os::windows::io::AsRawHandle;
use std::time::Duration;

use russet_core::dirs::RussetDirs;
use russet_core::error::{Error, Result, codes};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions};
use windows_sys::Win32::Foundation::{CloseHandle, ERROR_PIPE_BUSY, HANDLE, HLOCAL, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{GetTokenInformation, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows_sys::Win32::System::Pipes::{GetNamedPipeClientProcessId, GetNamedPipeServerProcessId};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};

use crate::{PeerIdentity, TransportKind};

/// `\\.\pipe\russet-<SID>[-<hash8>]`.
pub(crate) fn default_pipe_name(dirs: &RussetDirs) -> String {
    let sid = current_user_sid().unwrap_or_else(|_| "unknown".to_owned());
    let base = format!(r"\\.\pipe\russet-{sid}");
    match RussetDirs::default_runtime() {
        Ok(default) if default == dirs.runtime => base,
        _ => format!("{base}-{}", russet_core::dirs::hash8(dirs.runtime.to_string_lossy().as_bytes())),
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The string SID of the user a process token belongs to.
fn token_user_sid(token: HANDLE) -> Result<String> {
    let mut len = 0u32;
    #[allow(unsafe_code)]
    // SAFETY: a size query with a null buffer; `len` is a valid out pointer.
    unsafe {
        GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut len);
    }
    if len == 0 {
        return Err(Error::new(codes::core::IO, "GetTokenInformation returned no size"));
    }
    let mut buf = vec![0u8; len as usize];
    #[allow(unsafe_code)]
    // SAFETY: `buf` has `len` writable bytes, as the size query asked for.
    let ok = unsafe { GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len) };
    if ok == 0 {
        return Err(Error::io("reading the token user", std::io::Error::last_os_error()));
    }
    #[allow(unsafe_code)]
    // SAFETY: on success the buffer starts with a TOKEN_USER, whose SID points into the same buffer.
    let sid = unsafe { (*(buf.as_ptr().cast::<TOKEN_USER>())).User.Sid };
    let mut out: *mut u16 = std::ptr::null_mut();
    #[allow(unsafe_code)]
    // SAFETY: `sid` is valid while `buf` lives; `out` receives a LocalAlloc'ed string freed below.
    let ok = unsafe { ConvertSidToStringSidW(sid, &mut out) };
    if ok == 0 || out.is_null() {
        return Err(Error::io("converting a SID", std::io::Error::last_os_error()));
    }
    #[allow(unsafe_code)]
    // SAFETY: `out` is a NUL-terminated UTF-16 string from ConvertSidToStringSidW; freed with LocalFree.
    let text = unsafe {
        let mut n = 0;
        while *out.add(n) != 0 {
            n += 1;
        }
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(out, n));
        LocalFree(out as HLOCAL);
        s
    };
    Ok(text)
}

/// The current user's SID string (`S-1-5-21-…`).
pub(crate) fn current_user_sid() -> Result<String> {
    let mut token: HANDLE = std::ptr::null_mut();
    #[allow(unsafe_code)]
    // SAFETY: the pseudo handle of the current process; `token` is a valid out pointer, closed below.
    let ok = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) };
    if ok == 0 {
        return Err(Error::io("opening the process token", std::io::Error::last_os_error()));
    }
    let sid = token_user_sid(token);
    #[allow(unsafe_code)]
    // SAFETY: `token` came from OpenProcessToken.
    unsafe {
        CloseHandle(token);
    }
    sid
}

/// The SID string of the user that runs process `pid`.
fn process_user_sid(pid: u32) -> Result<String> {
    #[allow(unsafe_code)]
    // SAFETY: plain query access; the handle is closed below.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return Err(Error::io("opening the daemon process", std::io::Error::last_os_error()));
    }
    let mut token: HANDLE = std::ptr::null_mut();
    #[allow(unsafe_code)]
    // SAFETY: `process` is a valid handle; `token` is a valid out pointer.
    let ok = unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) };
    let result = if ok == 0 {
        Err(Error::io("opening the daemon's token", std::io::Error::last_os_error()))
    } else {
        let sid = token_user_sid(token);
        #[allow(unsafe_code)]
        // SAFETY: `token` came from OpenProcessToken.
        unsafe {
            CloseHandle(token);
        }
        sid
    };
    #[allow(unsafe_code)]
    // SAFETY: `process` came from OpenProcess.
    unsafe {
        CloseHandle(process);
    }
    result
}

/// A security descriptor from SDDL, freed on drop.
struct SecurityDescriptor(*mut c_void);

// SAFETY: the descriptor is an immutable LocalAlloc'ed buffer, only read by CreateNamedPipeW.
#[allow(unsafe_code)]
unsafe impl Send for SecurityDescriptor {}
// SAFETY: as above; nothing mutates it.
#[allow(unsafe_code)]
unsafe impl Sync for SecurityDescriptor {}

impl SecurityDescriptor {
    fn user_only() -> Result<Self> {
        let sid = current_user_sid()?;
        let sddl = wide(&format!("D:P(A;;GA;;;{sid})(A;;GA;;;SY)"));
        let mut sd: *mut c_void = std::ptr::null_mut();
        #[allow(unsafe_code)]
        // SAFETY: `sddl` is NUL-terminated; `sd` receives a LocalAlloc'ed descriptor freed in Drop.
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut sd,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 || sd.is_null() {
            return Err(Error::io("building the pipe's DACL", std::io::Error::last_os_error()));
        }
        Ok(Self(sd))
    }
}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        #[allow(unsafe_code)]
        // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
        unsafe {
            LocalFree(self.0 as HLOCAL);
        }
    }
}

/// A pipe listener: the next server instance, created before the previous one is handed out.
pub(crate) struct PipeListener {
    name: String,
    next: NamedPipeServer,
    sd: SecurityDescriptor,
}

impl PipeListener {
    pub(crate) fn bind(name: &str) -> Result<Self> {
        let sd = SecurityDescriptor::user_only()?;
        let next = create(name, &sd, true)?;
        Ok(Self { name: name.to_owned(), next, sd })
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) async fn accept(&mut self) -> std::io::Result<(NamedPipeServer, PeerIdentity)> {
        self.next.connect().await?;
        let fresh = create(&self.name, &self.sd, false).map_err(|e| std::io::Error::other(e.to_string()))?;
        let connected = std::mem::replace(&mut self.next, fresh);
        let mut pid = 0u32;
        #[allow(unsafe_code)]
        // SAFETY: a connected pipe handle and a valid out pointer.
        let ok = unsafe { GetNamedPipeClientProcessId(connected.as_raw_handle() as HANDLE, &mut pid) };
        let identity =
            PeerIdentity { uid: None, sid: None, pid: (ok != 0).then_some(pid), transport: TransportKind::NamedPipe };
        Ok((connected, identity))
    }
}

fn create(name: &str, sd: &SecurityDescriptor, first: bool) -> Result<NamedPipeServer> {
    let mut sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.0,
        bInheritHandle: 0,
    };
    #[allow(unsafe_code)]
    // SAFETY: `sa` and the descriptor it points to outlive the call; the pipe copies the security settings.
    let server = unsafe {
        ServerOptions::new()
            .first_pipe_instance(first)
            .reject_remote_clients(true)
            .create_with_security_attributes_raw(name, (&mut sa as *mut SECURITY_ATTRIBUTES).cast())
    };
    server.map_err(|e| {
        Error::new(codes::ipc::DAEMON_UNAVAILABLE, format!("cannot create the pipe {name}: {e}"))
            .with_hint("Another daemon may already listen; use `russet daemon status`.")
            .caused_by(e)
    })
}

/// Opens the pipe, retrying while all instances are busy.
pub(crate) async fn connect(name: &str, timeout: Duration) -> std::io::Result<NamedPipeClient> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        match ClientOptions::new().open(name) {
            Ok(c) => return Ok(c),
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) && tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(e) => return Err(e),
        }
    }
}

/// The client's check: the server process runs as this user (no pipe squatting).
pub(crate) fn check_server(pipe: &NamedPipeClient) -> Result<PeerIdentity> {
    let mut pid = 0u32;
    #[allow(unsafe_code)]
    // SAFETY: a connected pipe handle and a valid out pointer.
    let ok = unsafe { GetNamedPipeServerProcessId(pipe.as_raw_handle() as HANDLE, &mut pid) };
    if ok == 0 {
        return Err(Error::io("reading the daemon's process id", std::io::Error::last_os_error()));
    }
    let server = process_user_sid(pid)?;
    let me = current_user_sid()?;
    if server != me {
        return Err(Error::new(
            codes::perm::AUTH_FAILED,
            format!("the pipe server runs as {server}, not as this user"),
        )
        .with_hint("Another user's process owns the daemon's pipe name; stop it or set RUSSET_HOME."));
    }
    Ok(PeerIdentity { uid: None, sid: Some(server), pid: Some(pid), transport: TransportKind::NamedPipe })
}
