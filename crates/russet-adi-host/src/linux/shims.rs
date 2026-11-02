// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Import resolution for the Android libraries (SIGN §7.5). On x86_64 Linux bionic and glibc share the SysV ABI
//! and the kernel's `struct stat`, `errno` values and `open` flags, so most of `libc.so`, `libm.so` and `libdl.so`
//! resolve to the host's glibc. The rest are Rust shims:
//!
//! - bionic-only names (`__errno`, the `_chk` helpers glibc lacks, `__system_property_get`, `__sF`,
//!   `arc4random`, `sysconf` with bionic's constants, `__assert2`, C++ `operator new/delete`);
//! - `liblog` (`__android_log_*`): swallowed, never printed (it could carry identifiers);
//! - `libdl` (`dlopen`/`dlsym`/`dlclose`/`dlerror`): served by this loader, so `libstoreservicescore.so` reaches
//!   `libCoreADI.so` through it;
//! - anything else gets a trap stub that names the import on stderr and exits with status 70, so a library update
//!   that needs a new shim fails loudly instead of corrupting memory.

#![allow(unsafe_code)] // C ABI shims and generated trap stubs; each block says why it holds

use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::sync::Mutex;

use super::elf::{self, Image};

/// Loaded Android libraries, by `dlopen` handle (index + 1).
static LIBRARIES: Mutex<Vec<Image>> = Mutex::new(Vec::new());
/// Names of unresolved imports, by trap index.
static TRAPS: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// The handle `dlopen` returns for the host's own libc, libdl, libm and liblog.
const HOST_HANDLE: usize = 0x7fff_0000;

/// Whether `RUSSET_ADI_TRACE=1` asked for import tracing (names only).
fn tracing() -> bool {
    std::env::var_os("RUSSET_ADI_TRACE").is_some_and(|v| v == "1")
}

/// Leaves the process without running the `atexit` destructors the Android libraries registered (they may call
/// imports Russet does not provide).
pub fn exit_now(code: c_int) -> ! {
    // SAFETY: _exit ends the process at once; stdout is flushed after every answer.
    unsafe { libc::_exit(code) }
}

extern "C" fn errno_shim() -> *mut c_int {
    // SAFETY: glibc's per-thread errno location.
    unsafe { libc::__errno_location() }
}

extern "C" fn system_property_get(_name: *const c_char, value: *mut c_char) -> c_int {
    if !value.is_null() {
        // SAFETY: bionic's contract: `value` holds PROP_VALUE_MAX bytes.
        unsafe { *value = 0 };
    }
    0
}

extern "C" fn arc4random_shim() -> u32 {
    let mut b = [0u8; 4];
    arc4random_buf_shim(b.as_mut_ptr().cast(), 4);
    u32::from_ne_bytes(b)
}

extern "C" fn arc4random_buf_shim(buf: *mut c_void, n: usize) {
    let mut done = 0;
    while done < n {
        // SAFETY: `buf` holds `n` bytes per arc4random_buf's contract.
        let r = unsafe { libc::getrandom(buf.cast::<u8>().add(done).cast(), n - done, 0) };
        if r <= 0 {
            std::process::abort();
        }
        done += r as usize;
    }
}

extern "C" fn arc4random_uniform_shim(upper: u32) -> u32 {
    if upper < 2 {
        return 0;
    }
    let min = upper.wrapping_neg() % upper;
    loop {
        let r = arc4random_shim();
        if r >= min {
            return r % upper;
        }
    }
}

extern "C" fn android_log(_prio: c_int, _tag: *const c_char, _msg: *const c_char) -> c_int {
    0
}

extern "C" fn android_log_assert(_cond: *const c_char, _tag: *const c_char, _fmt: *const c_char) {
    eprintln!("russet-adi-host: an ADI library asserted");
    exit_now(70);
}

extern "C" fn assert2(_file: *const c_char, _line: c_int, _func: *const c_char, _msg: *const c_char) {
    eprintln!("russet-adi-host: an ADI library failed an assertion");
    exit_now(70);
}

extern "C" fn sysconf_shim(name: c_int) -> libc::c_long {
    // bionic's <unistd.h> numbering differs from glibc's.
    // SAFETY: plain libc calls.
    unsafe {
        match name {
            0x06 => libc::sysconf(libc::_SC_CLK_TCK),
            0x27 | 0x28 => libc::sysconf(libc::_SC_PAGESIZE),
            0x60 => libc::sysconf(libc::_SC_NPROCESSORS_CONF),
            0x61 => libc::sysconf(libc::_SC_NPROCESSORS_ONLN),
            _ => {
                *libc::__errno_location() = libc::EINVAL;
                -1
            }
        }
    }
}

extern "C" fn strlen_chk(s: *const c_char, _n: usize) -> usize {
    // SAFETY: bionic's FORTIFY wrapper of strlen.
    unsafe { libc::strlen(s) }
}

extern "C" fn strchr_chk(s: *const c_char, c: c_int, _n: usize) -> *mut c_char {
    // SAFETY: bionic's FORTIFY wrapper of strchr.
    unsafe { libc::strchr(s, c) }
}

extern "C" fn strrchr_chk(s: *const c_char, c: c_int, _n: usize) -> *mut c_char {
    // SAFETY: bionic's FORTIFY wrapper of strrchr.
    unsafe { libc::strrchr(s, c) }
}

extern "C" fn strncpy_chk2(d: *mut c_char, s: *const c_char, n: usize, _dl: usize, _sl: usize) -> *mut c_char {
    // SAFETY: bionic's FORTIFY wrapper of strncpy.
    unsafe { libc::strncpy(d, s, n) }
}

extern "C" fn umask_chk(mode: libc::mode_t) -> libc::mode_t {
    // SAFETY: plain umask.
    unsafe { libc::umask(mode) }
}

extern "C" fn operator_new(n: usize) -> *mut c_void {
    // SAFETY: malloc; C++ new aborts instead of returning null.
    let p = unsafe { libc::malloc(n.max(1)) };
    if p.is_null() {
        std::process::abort();
    }
    p
}

extern "C" fn operator_delete(p: *mut c_void) {
    // SAFETY: memory from operator_new.
    unsafe { libc::free(p) }
}

extern "C" fn pure_virtual() {
    eprintln!("russet-adi-host: an ADI library called a pure virtual function");
    exit_now(70);
}

extern "C" fn set_abort_message(_msg: *const c_char) {}

static SF: [u8; 3 * 152] = [0; 3 * 152];
static STACK_CHK_GUARD: usize = 0x5275_7373_6574_4144;

static DLERROR: &CStr = c"russet-adi-host: library not found";

extern "C" fn dlopen_shim(path: *const c_char, _flags: c_int) -> *mut c_void {
    if path.is_null() {
        return HOST_HANDLE as *mut c_void;
    }
    // SAFETY: dlopen's contract: a NUL-terminated path.
    let path = unsafe { CStr::from_ptr(path) }.to_string_lossy().into_owned();
    let base = path.rsplit('/').next().unwrap_or(&path).to_owned();
    if matches!(base.as_str(), "libc.so" | "libdl.so" | "libm.so" | "liblog.so") {
        return HOST_HANDLE as *mut c_void;
    }
    match load_library(&path) {
        Ok(handle) => handle as *mut c_void,
        Err(e) => {
            eprintln!("russet-adi-host: dlopen of {base} failed: {e}");
            std::ptr::null_mut()
        }
    }
}

extern "C" fn dlsym_shim(handle: *mut c_void, name: *const c_char) -> *mut c_void {
    if name.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: dlsym's contract: a NUL-terminated name.
    let name = unsafe { CStr::from_ptr(name) }.to_string_lossy().into_owned();
    let h = handle as usize;
    let found = match h {
        0 | HOST_HANDLE => resolve_name(&name),
        _ => LIBRARIES.lock().ok().and_then(|libs| libs.get(h - 1).and_then(|i| i.symbol(&name))),
    };
    found.unwrap_or(0) as *mut c_void
}

extern "C" fn dlclose_shim(_handle: *mut c_void) -> c_int {
    0
}

extern "C" fn dlerror_shim() -> *const c_char {
    DLERROR.as_ptr()
}

/// The shims, by import name.
fn hooks() -> &'static HashMap<&'static str, usize> {
    static TABLE: std::sync::OnceLock<HashMap<&'static str, usize>> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let mut t: HashMap<&'static str, usize> = HashMap::new();
        t.insert("__errno", errno_shim as *const () as usize);
        t.insert("__system_property_get", system_property_get as *const () as usize);
        t.insert("arc4random", arc4random_shim as *const () as usize);
        t.insert("arc4random_buf", arc4random_buf_shim as *const () as usize);
        t.insert("arc4random_uniform", arc4random_uniform_shim as *const () as usize);
        t.insert("__android_log_print", android_log as *const () as usize);
        t.insert("__android_log_write", android_log as *const () as usize);
        t.insert("__android_log_vprint", android_log as *const () as usize);
        t.insert("__android_log_buf_print", android_log as *const () as usize);
        t.insert("__android_log_assert", android_log_assert as *const () as usize);
        t.insert("__assert2", assert2 as *const () as usize);
        t.insert("android_set_abort_message", set_abort_message as *const () as usize);
        t.insert("sysconf", sysconf_shim as *const () as usize);
        t.insert("__strlen_chk", strlen_chk as *const () as usize);
        t.insert("__strchr_chk", strchr_chk as *const () as usize);
        t.insert("__strrchr_chk", strrchr_chk as *const () as usize);
        t.insert("__strncpy_chk2", strncpy_chk2 as *const () as usize);
        t.insert("__umask_chk", umask_chk as *const () as usize);
        t.insert("_Znwm", operator_new as *const () as usize);
        t.insert("_Znam", operator_new as *const () as usize);
        t.insert("_ZdlPv", operator_delete as *const () as usize);
        t.insert("_ZdaPv", operator_delete as *const () as usize);
        t.insert("__cxa_pure_virtual", pure_virtual as *const () as usize);
        t.insert("__sF", SF.as_ptr() as usize);
        t.insert("__stack_chk_guard", &STACK_CHK_GUARD as *const usize as usize);
        t.insert("dlopen", dlopen_shim as *const () as usize);
        t.insert("android_dlopen_ext", dlopen_shim as *const () as usize);
        t.insert("dlsym", dlsym_shim as *const () as usize);
        t.insert("dlclose", dlclose_shim as *const () as usize);
        t.insert("dlerror", dlerror_shim as *const () as usize);
        t
    })
}

/// `libm.so.6`, opened before the sandbox closes the filesystem (the Rust binary may not link every libm symbol).
static LIBM: std::sync::OnceLock<usize> = std::sync::OnceLock::new();

/// Opens the host libraries imports may need; call before the sandbox is applied.
pub fn preload_host() {
    LIBM.get_or_init(|| {
        // SAFETY: dlopen of the system libm.
        unsafe { libc::dlopen(c"libm.so.6".as_ptr(), libc::RTLD_NOW | libc::RTLD_GLOBAL) as usize }
    });
    let _ = hooks();
}

/// An import's address: a shim, an export of a loaded Android library, or the host's glibc.
fn resolve_name(name: &str) -> Option<usize> {
    if let Some(a) = hooks().get(name) {
        return Some(*a);
    }
    if let Ok(libs) = LIBRARIES.lock()
        && let Some(a) = libs.iter().find_map(|l| l.symbol(name))
    {
        return Some(a);
    }
    let c = CString::new(name).ok()?;
    // SAFETY: dlsym over the global namespace (glibc, and libm opened by preload_host).
    let a = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c.as_ptr()) } as usize;
    (a != 0).then_some(a)
}

/// An import of a library being loaded: resolved, 0 for a missing weak one, else a trap stub.
pub fn resolve_import(lib: &str, name: &str, weak: bool) -> usize {
    match resolve_name(name) {
        Some(a) => {
            if tracing() {
                eprintln!("russet-adi-host: {lib}: {name} resolved");
            }
            a
        }
        None if weak => 0,
        None => {
            if tracing() {
                eprintln!("russet-adi-host: {lib}: {name} unresolved (trap)");
            }
            trap(name)
        }
    }
}

extern "C" fn trapped(index: usize) -> ! {
    let name = TRAPS.lock().ok().and_then(|t| t.get(index).cloned()).unwrap_or_default();
    eprintln!("russet-adi-host: an ADI library called `{name}`, which Russet does not provide yet");
    exit_now(70)
}

/// A stub that reports `name` and exits: `mov rdi, index; mov rax, trapped; jmp rax`.
fn trap(name: &str) -> usize {
    static PAGES: Mutex<(usize, usize)> = Mutex::new((0, 0)); // (current page, used bytes)
    const STUB: usize = 32;
    let Ok(mut traps) = TRAPS.lock() else { return 0 };
    let index = traps.len();
    traps.push(name.to_owned());
    let Ok(mut pages) = PAGES.lock() else { return 0 };
    if pages.0 == 0 || pages.1 + STUB > 4096 {
        // SAFETY: a fresh anonymous page for stubs.
        let p = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                4096,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        if p == libc::MAP_FAILED {
            return 0;
        }
        *pages = (p as usize, 0);
    }
    let at = pages.0 + pages.1;
    let mut code = Vec::with_capacity(STUB);
    code.extend_from_slice(&[0x48, 0xbf]);
    code.extend_from_slice(&(index as u64).to_le_bytes());
    code.extend_from_slice(&[0x48, 0xb8]);
    code.extend_from_slice(&(trapped as *const () as usize as u64).to_le_bytes());
    code.extend_from_slice(&[0xff, 0xe0]);
    code.resize(STUB, 0xcc);
    // SAFETY: `at` lies in our stub page; the page is made writable, filled, then executable again.
    unsafe {
        libc::mprotect(pages.0 as *mut c_void, 4096, libc::PROT_READ | libc::PROT_WRITE);
        std::ptr::copy_nonoverlapping(code.as_ptr(), at as *mut u8, STUB);
        libc::mprotect(pages.0 as *mut c_void, 4096, libc::PROT_READ | libc::PROT_EXEC);
    }
    pages.1 += STUB;
    at
}

/// Loads the library at `path` (once) and returns its `dlopen` handle.
pub fn load_library(path: &str) -> Result<usize, String> {
    let base = path.rsplit('/').next().unwrap_or(path).to_owned();
    if let Ok(libs) = LIBRARIES.lock()
        && let Some(i) = libs.iter().position(|l| l.name == base)
    {
        return Ok(i + 1);
    }
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {base}: {e}"))?;
    let traps_before = TRAPS.lock().map(|t| t.len()).unwrap_or(0);
    let mut resolve = |name: &str, weak: bool| resolve_import(&base, name, weak);
    let mut image = elf::load(&base, &bytes, &mut resolve)?;
    let missing = TRAPS.lock().map(|t| t.len()).unwrap_or(0) - traps_before;
    // Initializers of a library with missing imports could only reach code that is not there (Apple's
    // libstoreservicescore.so constructs mediaplatform work queues); the ADI entry points do not need them.
    if missing == 0 {
        image.run_initializers();
    } else if image.initializers() > 0 {
        eprintln!(
            "russet-adi-host: {base}: {missing} imports are not provided; its {} initializers are not run",
            image.initializers()
        );
    }
    let mut libs = LIBRARIES.lock().map_err(|_| "the library table is poisoned".to_owned())?;
    libs.push(image);
    Ok(libs.len())
}

/// Export `name` of the library loaded under handle `handle`.
pub fn symbol(handle: usize, name: &str) -> Option<usize> {
    LIBRARIES.lock().ok()?.get(handle.checked_sub(1)?)?.symbol(name)
}
