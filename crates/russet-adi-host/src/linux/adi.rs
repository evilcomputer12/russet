// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The ADI calls of `libstoreservicescore.so` (SIGN §7.5). The export names are interoperability facts (omnisette
//! `store_services_core.rs`, Provision); the signatures follow them: buffers ADI returns are owned by ADI and
//! released with `Dispose` after this process copied them.

#![allow(unsafe_code)] // calls into the loaded library through its C ABI; each block says why it holds

use std::ffi::{CString, c_char, c_int, c_void};

use super::shims;

type LoadLibraryWithPath = extern "C" fn(*const c_char) -> c_int;
type SetAndroidId = extern "C" fn(*const c_char, u32) -> c_int;
type SetProvisioningPath = extern "C" fn(*const c_char) -> c_int;
type ProvisioningErase = extern "C" fn(u64) -> c_int;
type ProvisioningDestroy = extern "C" fn(u32) -> c_int;
type ProvisioningEnd = extern "C" fn(u32, *const u8, u32, *const u8, u32) -> c_int;
type ProvisioningStart = extern "C" fn(u64, *const u8, u32, *mut *mut u8, *mut u32, *mut u32) -> c_int;
type GetLoginCode = extern "C" fn(u64) -> c_int;
type Dispose = extern "C" fn(*mut c_void) -> c_int;
type OtpRequest = extern "C" fn(u64, *mut *mut u8, *mut u32, *mut *mut u8, *mut u32) -> c_int;

/// The ADI entry points of a loaded `libstoreservicescore.so`.
pub struct Adi {
    erase: ProvisioningErase,
    destroy: ProvisioningDestroy,
    end: ProvisioningEnd,
    start: ProvisioningStart,
    login_code: GetLoginCode,
    dispose: Dispose,
    otp: OtpRequest,
}

/// An ADI status code (`-45061`: not provisioned / logged out).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdiError(pub i64);

fn check(code: c_int) -> Result<(), AdiError> {
    if code == 0 { Ok(()) } else { Err(AdiError(i64::from(code))) }
}

fn lookup<T: Copy>(handle: usize, name: &str) -> Result<T, String> {
    let a = shims::symbol(handle, name).ok_or_else(|| format!("libstoreservicescore.so lacks {name}"))?;
    debug_assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<usize>());
    // SAFETY: `a` is the export `name`, whose C signature is `T` (see the module docs).
    Ok(unsafe { std::mem::transmute_copy::<usize, T>(&a) })
}

impl Adi {
    /// Loads `<lib_dir>/libstoreservicescore.so`, points it at `lib_dir` (for `libCoreADI.so`), and sets the
    /// Android ID and provisioning directory.
    pub fn load(lib_dir: &str, android_id: &str, provisioning_path: &str) -> Result<Self, String> {
        let dir = if lib_dir.ends_with('/') { lib_dir.to_owned() } else { format!("{lib_dir}/") };
        let handle = shims::load_library(&format!("{dir}libstoreservicescore.so"))?;
        let load_with_path: LoadLibraryWithPath = lookup(handle, "kq56gsgHG6")?;
        let set_android_id: SetAndroidId = lookup(handle, "Sph98paBcz")?;
        let set_path: SetProvisioningPath = lookup(handle, "nf92ngaK92")?;
        let adi = Self {
            erase: lookup(handle, "p435tmhbla")?,
            destroy: lookup(handle, "fy34trz2st")?,
            end: lookup(handle, "uv5t6nhkui")?,
            start: lookup(handle, "rsegvyrt87")?,
            login_code: lookup(handle, "aslgmuibau")?,
            dispose: lookup(handle, "jk24uiwqrg")?,
            otp: lookup(handle, "qi864985u0")?,
        };
        let cdir = CString::new(dir).map_err(|_| "the library path contains NUL".to_owned())?;
        check(load_with_path(cdir.as_ptr())).map_err(|e| format!("LoadLibraryWithPath failed ({})", e.0))?;
        let id = CString::new(android_id).map_err(|_| "the Android ID contains NUL".to_owned())?;
        check(set_android_id(id.as_ptr(), android_id.len() as u32))
            .map_err(|e| format!("SetAndroidID failed ({})", e.0))?;
        let path = CString::new(provisioning_path).map_err(|_| "the provisioning path contains NUL".to_owned())?;
        check(set_path(path.as_ptr())).map_err(|e| format!("SetProvisioningPath failed ({})", e.0))?;
        Ok(adi)
    }

    /// `GetLoginCode(dsid) == 0`.
    pub fn is_provisioned(&self, dsid: i64) -> Result<bool, AdiError> {
        match (self.login_code)(dsid as u64) {
            0 => Ok(true),
            -45061 => Ok(false),
            other => Err(AdiError(i64::from(other))),
        }
    }

    /// Copies and releases a buffer ADI returned.
    fn take(&self, p: *mut u8, len: u32) -> Vec<u8> {
        if p.is_null() {
            return Vec::new();
        }
        // SAFETY: ADI returned `len` bytes at `p`, which stay valid until Dispose.
        let v = unsafe { std::slice::from_raw_parts(p, len as usize) }.to_vec();
        (self.dispose)(p.cast());
        v
    }

    pub fn start_provisioning(&self, dsid: i64, spim: &[u8]) -> Result<(u32, Vec<u8>), AdiError> {
        let mut cpim: *mut u8 = std::ptr::null_mut();
        let mut cpim_len = 0u32;
        let mut session = 0u32;
        check((self.start)(dsid as u64, spim.as_ptr(), spim.len() as u32, &mut cpim, &mut cpim_len, &mut session))?;
        Ok((session, self.take(cpim, cpim_len)))
    }

    pub fn end_provisioning(&self, session: u32, ptm: &[u8], tk: &[u8]) -> Result<(), AdiError> {
        let r = check((self.end)(session, ptm.as_ptr(), ptm.len() as u32, tk.as_ptr(), tk.len() as u32));
        let _ = (self.destroy)(session);
        r
    }

    /// `(mid, otp)`.
    pub fn request_otp(&self, dsid: i64) -> Result<(Vec<u8>, Vec<u8>), AdiError> {
        let (mut mid, mut mid_len, mut otp, mut otp_len) = (std::ptr::null_mut(), 0u32, std::ptr::null_mut(), 0u32);
        check((self.otp)(dsid as u64, &mut mid, &mut mid_len, &mut otp, &mut otp_len))?;
        Ok((self.take(mid, mid_len), self.take(otp, otp_len)))
    }

    pub fn erase(&self, dsid: i64) -> Result<(), AdiError> {
        check((self.erase)(dsid as u64))
    }
}
