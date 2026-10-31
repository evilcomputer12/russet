// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Bounds-checked byte readers. Thin Mach-O images are little-endian; fat headers and code-signature blobs are
//! big-endian. Every read checks its range, so malformed input yields [`MachOError::Malformed`], never a panic.

use crate::MachOError;

pub(crate) fn bytes<'a>(b: &'a [u8], off: usize, len: usize, what: &str) -> Result<&'a [u8], MachOError> {
    off.checked_add(len).and_then(|end| b.get(off..end)).ok_or_else(|| {
        MachOError::Malformed(format!("{what} at {off:#x}+{len:#x} is outside the data ({:#x})", b.len()))
    })
}

pub(crate) fn u8_at(b: &[u8], off: usize) -> Result<u8, MachOError> {
    Ok(bytes(b, off, 1, "byte")?[0])
}

pub(crate) fn u32_le(b: &[u8], off: usize) -> Result<u32, MachOError> {
    let s = bytes(b, off, 4, "u32")?;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

pub(crate) fn u32_be(b: &[u8], off: usize) -> Result<u32, MachOError> {
    let s = bytes(b, off, 4, "u32")?;
    Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

pub(crate) fn u64_le(b: &[u8], off: usize) -> Result<u64, MachOError> {
    let s = bytes(b, off, 8, "u64")?;
    let mut a = [0u8; 8];
    a.copy_from_slice(s);
    Ok(u64::from_le_bytes(a))
}

pub(crate) fn u64_be(b: &[u8], off: usize) -> Result<u64, MachOError> {
    let s = bytes(b, off, 8, "u64")?;
    let mut a = [0u8; 8];
    a.copy_from_slice(s);
    Ok(u64::from_be_bytes(a))
}

pub(crate) fn put_u32_le(b: &mut [u8], off: usize, v: u32) -> Result<(), MachOError> {
    let len = b.len();
    let s = off
        .checked_add(4)
        .and_then(|end| b.get_mut(off..end))
        .ok_or_else(|| MachOError::Malformed(format!("write at {off:#x} is outside the data ({len:#x})")))?;
    s.copy_from_slice(&v.to_le_bytes());
    Ok(())
}

pub(crate) fn put_u64_le(b: &mut [u8], off: usize, v: u64) -> Result<(), MachOError> {
    let len = b.len();
    let s = off
        .checked_add(8)
        .and_then(|end| b.get_mut(off..end))
        .ok_or_else(|| MachOError::Malformed(format!("write at {off:#x} is outside the data ({len:#x})")))?;
    s.copy_from_slice(&v.to_le_bytes());
    Ok(())
}

/// A NUL-terminated string starting at `off`, which must end before `end`.
pub(crate) fn cstr(b: &[u8], off: usize, end: usize, what: &str) -> Result<String, MachOError> {
    let end = end.min(b.len());
    let s = b
        .get(off..end)
        .ok_or_else(|| MachOError::Malformed(format!("{what} string at {off:#x} is outside its command")))?;
    let n = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    Ok(String::from_utf8_lossy(&s[..n]).into_owned())
}

/// A fixed-size, NUL-padded name (segment and section names).
pub(crate) fn fixed_name(b: &[u8], off: usize, len: usize) -> Result<String, MachOError> {
    let s = bytes(b, off, len, "name")?;
    let n = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    Ok(String::from_utf8_lossy(&s[..n]).into_owned())
}

pub(crate) fn usize_of(v: u64, what: &str) -> Result<usize, MachOError> {
    usize::try_from(v).map_err(|_| MachOError::Malformed(format!("{what} {v:#x} does not fit in memory")))
}
