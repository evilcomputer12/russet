// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The helper's sandbox (SIGN §7.5), applied before any Apple code is mapped:
//!
//! - **seccomp-bpf**: no network (`socket`, `connect`, `bind`, `sendto`, …), no `execve`, no `ptrace` or
//!   cross-process memory access, no namespaces, mounts, BPF, kernel modules or keyrings; x32 syscalls are refused
//!   and a foreign architecture kills the process. Everything else is allowed (a strict allowlist needs a trace
//!   of the real libraries' syscalls; tightening it is a W22 follow-up).
//! - **Landlock** (when the kernel has it): read and execute the libraries' directory, read and write the
//!   provisioning directory, read `/dev/urandom`; nothing else of the filesystem. WSL kernels usually lack
//!   Landlock; the helper says so on stderr and in `status`, and seccomp still applies.

#![allow(unsafe_code)] // raw seccomp and Landlock syscalls; each block says why it holds

use std::ffi::CString;
use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd};
use std::path::Path;

#[repr(C)]
struct SockFilter {
    code: u16,
    jt: u8,
    jf: u8,
    k: u32,
}

#[repr(C)]
struct SockFprog {
    len: u16,
    filter: *const SockFilter,
}

const BPF_LD_W_ABS: u16 = 0x20;
const BPF_JEQ_K: u16 = 0x15;
const BPF_JGE_K: u16 = 0x35;
const BPF_RET_K: u16 = 0x06;
const SECCOMP_RET_KILL_PROCESS: u32 = 0x8000_0000;
const SECCOMP_RET_ERRNO: u32 = 0x0005_0000;
const SECCOMP_RET_ALLOW: u32 = 0x7fff_0000;
const AUDIT_ARCH_X86_64: u32 = 0xc000_003e;
const AUDIT_ARCH_AARCH64: u32 = 0xc000_00b7;

/// The syscalls the helper refuses with `EPERM`.
fn denied() -> Vec<i64> {
    vec![
        libc::SYS_socket,
        libc::SYS_socketpair,
        libc::SYS_connect,
        libc::SYS_accept,
        libc::SYS_accept4,
        libc::SYS_bind,
        libc::SYS_listen,
        libc::SYS_sendto,
        libc::SYS_sendmsg,
        libc::SYS_sendmmsg,
        libc::SYS_recvfrom,
        libc::SYS_recvmsg,
        libc::SYS_recvmmsg,
        libc::SYS_execve,
        libc::SYS_execveat,
        libc::SYS_ptrace,
        libc::SYS_process_vm_readv,
        libc::SYS_process_vm_writev,
        libc::SYS_mount,
        libc::SYS_umount2,
        libc::SYS_pivot_root,
        libc::SYS_chroot,
        libc::SYS_unshare,
        libc::SYS_setns,
        libc::SYS_bpf,
        libc::SYS_perf_event_open,
        libc::SYS_init_module,
        libc::SYS_finit_module,
        libc::SYS_delete_module,
        libc::SYS_kexec_load,
        libc::SYS_reboot,
        libc::SYS_keyctl,
        libc::SYS_add_key,
        libc::SYS_request_key,
        libc::SYS_userfaultfd,
        libc::SYS_io_uring_setup,
    ]
}

/// The BPF program of [`apply_seccomp`].
fn program() -> Vec<SockFilter> {
    let arch = if cfg!(target_arch = "x86_64") { AUDIT_ARCH_X86_64 } else { AUDIT_ARCH_AARCH64 };
    let deny = denied();
    let n = deny.len();
    let mut p = vec![
        SockFilter { code: BPF_LD_W_ABS, jt: 0, jf: 0, k: 4 },
        SockFilter { code: BPF_JEQ_K, jt: 1, jf: 0, k: arch },
        SockFilter { code: BPF_RET_K, jt: 0, jf: 0, k: SECCOMP_RET_KILL_PROCESS },
        SockFilter { code: BPF_LD_W_ABS, jt: 0, jf: 0, k: 0 },
    ];
    // Instructions after this one: the x32 check, n comparisons, ALLOW, ERRNO.
    let errno_at = p.len() + 1 + n + 1;
    let x32_at = p.len();
    p.push(SockFilter { code: BPF_JGE_K, jt: (errno_at - x32_at - 1) as u8, jf: 0, k: 0x4000_0000 });
    for (i, nr) in deny.into_iter().enumerate() {
        let at = x32_at + 1 + i;
        p.push(SockFilter { code: BPF_JEQ_K, jt: (errno_at - at - 1) as u8, jf: 0, k: nr as u32 });
    }
    p.push(SockFilter { code: BPF_RET_K, jt: 0, jf: 0, k: SECCOMP_RET_ALLOW });
    p.push(SockFilter { code: BPF_RET_K, jt: 0, jf: 0, k: SECCOMP_RET_ERRNO | libc::EPERM as u32 });
    p
}

/// Installs the seccomp filter on every thread.
pub fn apply_seccomp() -> Result<(), String> {
    let prog = program();
    let fprog = SockFprog { len: prog.len() as u16, filter: prog.as_ptr() };
    // SAFETY: no_new_privs is required for unprivileged filters; `fprog` points at a live, well-formed program.
    unsafe {
        if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
            return Err(format!("no_new_privs: {}", std::io::Error::last_os_error()));
        }
        let r = libc::syscall(
            libc::SYS_seccomp,
            1, /* SET_MODE_FILTER */
            1, /* FLAG_TSYNC */
            &fprog as *const SockFprog,
        );
        if r != 0 {
            return Err(format!("seccomp: {}", std::io::Error::last_os_error()));
        }
    }
    Ok(())
}

const LANDLOCK_CREATE_RULESET_VERSION: u32 = 1;
const LANDLOCK_RULE_PATH_BENEATH: i32 = 1;
const ACCESS_EXECUTE: u64 = 1 << 0;
const ACCESS_WRITE_FILE: u64 = 1 << 1;
const ACCESS_READ_FILE: u64 = 1 << 2;
const ACCESS_READ_DIR: u64 = 1 << 3;
const ACCESS_REMOVE_DIR: u64 = 1 << 4;
const ACCESS_REMOVE_FILE: u64 = 1 << 5;
const ACCESS_MAKE_DIR: u64 = 1 << 7;
const ACCESS_MAKE_REG: u64 = 1 << 8;
const ACCESS_TRUNCATE: u64 = 1 << 14;

#[repr(C)]
struct RulesetAttr {
    handled_access_fs: u64,
}

#[repr(C, packed)]
struct PathBeneath {
    allowed_access: u64,
    parent_fd: i32,
}

fn open_path(p: &Path) -> Result<OwnedFd, String> {
    let c = CString::new(p.as_os_str().as_encoded_bytes()).map_err(|_| "a path contains NUL".to_owned())?;
    // SAFETY: an O_PATH descriptor of an existing path; ownership moves into OwnedFd.
    let fd = unsafe { libc::open(c.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(format!("cannot open {}: {}", p.display(), std::io::Error::last_os_error()));
    }
    // SAFETY: `fd` was just returned by open and is owned by nobody else.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Restricts the filesystem to `libs` (read, execute), `state` (read, write) and `/dev/urandom` (read).
/// `Ok(false)`: the kernel has no Landlock.
pub fn apply_landlock(libs: &Path, state: &Path) -> Result<bool, String> {
    // SAFETY: the version query takes no attribute.
    let abi = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<RulesetAttr>(),
            0usize,
            LANDLOCK_CREATE_RULESET_VERSION,
        )
    };
    if abi < 1 {
        return Ok(false);
    }
    let mut handled = (1u64 << 13) - 1;
    if abi >= 3 {
        handled |= ACCESS_TRUNCATE;
    }
    let attr = RulesetAttr { handled_access_fs: handled };
    // SAFETY: a ruleset for `handled`; the returned descriptor is owned below.
    let fd = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            &attr as *const RulesetAttr,
            std::mem::size_of::<RulesetAttr>(),
            0u32,
        )
    };
    if fd < 0 {
        return Err(format!("landlock ruleset: {}", std::io::Error::last_os_error()));
    }
    // SAFETY: `fd` was just created and is owned by nobody else.
    let ruleset = unsafe { OwnedFd::from_raw_fd(fd as i32) };
    let rule = |path: &Path, access: u64| -> Result<(), String> {
        let dir = open_path(path)?;
        let pb = PathBeneath { allowed_access: access & handled, parent_fd: dir.as_raw_fd() };
        // SAFETY: a path-beneath rule over a live O_PATH descriptor.
        let r = unsafe {
            libc::syscall(
                libc::SYS_landlock_add_rule,
                ruleset.as_raw_fd(),
                LANDLOCK_RULE_PATH_BENEATH,
                &pb as *const PathBeneath,
                0u32,
            )
        };
        if r != 0 {
            return Err(format!("landlock rule for {}: {}", path.display(), std::io::Error::last_os_error()));
        }
        Ok(())
    };
    rule(libs, ACCESS_EXECUTE | ACCESS_READ_FILE | ACCESS_READ_DIR)?;
    rule(
        state,
        ACCESS_READ_FILE
            | ACCESS_READ_DIR
            | ACCESS_WRITE_FILE
            | ACCESS_REMOVE_FILE
            | ACCESS_REMOVE_DIR
            | ACCESS_MAKE_REG
            | ACCESS_MAKE_DIR
            | ACCESS_TRUNCATE,
    )?;
    rule(Path::new("/dev/urandom"), ACCESS_READ_FILE)?;
    // SAFETY: no_new_privs (also set for seccomp) and the ruleset built above.
    unsafe {
        if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
            return Err(format!("no_new_privs: {}", std::io::Error::last_os_error()));
        }
        if libc::syscall(libc::SYS_landlock_restrict_self, ruleset.as_raw_fd(), 0u32) != 0 {
            return Err(format!("landlock restrict: {}", std::io::Error::last_os_error()));
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_filter_jumps_land_on_errno() {
        let p = program();
        let errno_at = p.len() - 1;
        assert_eq!(p[errno_at].k, SECCOMP_RET_ERRNO | libc::EPERM as u32);
        assert_eq!(p[errno_at - 1].k, SECCOMP_RET_ALLOW);
        for (i, ins) in p.iter().enumerate().skip(4).take(p.len() - 6) {
            assert_eq!(i + 1 + ins.jt as usize, errno_at, "instruction {i}");
        }
        assert!(p.len() < 255);
    }
}
