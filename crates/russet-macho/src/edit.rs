// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! In-place edits: rpath deletion, the iOS 27 string-pool alignment fix, thinning and the last-resort SDK patch.
//!
//! Every edit works on the bytes of each slice and rebuilds the fat header when a slice changes size. File edits
//! write a temporary file next to the original and rename it over the original, keeping its permissions. An edit
//! invalidates an existing code signature; they run before signing (SIGN §8.5).

use std::borrow::Cow;
use std::path::Path;

use crate::consts::*;
use crate::read::{bytes, cstr, put_u32_le, put_u64_le, u32_be, u32_le, u64_be, u64_le, usize_of};
use crate::{Arch, MachO, MachOError, Version, load_commands, parse_header};

/// Deletes every `LC_RPATH` equal to `rpath` from every slice, in place.
pub fn delete_rpath(path: &Path, rpath: &str) -> Result<(), MachOError> {
    let data = std::fs::read(path)?;
    let out = delete_rpath_bytes(&data, rpath)?;
    write_in_place(path, &out)
}

/// [`delete_rpath`] on bytes held in memory. Fails with [`MachOError::NotFound`] when no slice has the rpath.
pub fn delete_rpath_bytes(data: &[u8], rpath: &str) -> Result<Vec<u8>, MachOError> {
    match map_slices(data, |s| delete_rpath_thin(s, rpath))? {
        Some(out) => Ok(out),
        None => Err(MachOError::NotFound(format!("LC_RPATH {rpath}"))),
    }
}

/// Fixes the iOS 27 8-byte string-pool rule in place; returns whether anything changed.
pub fn align_string_pool(path: &Path) -> Result<bool, MachOError> {
    let data = std::fs::read(path)?;
    match align_string_pool_bytes(&data)? {
        Some(out) => {
            write_in_place(path, &out)?;
            Ok(true)
        }
        None => Ok(false),
    }
}

/// [`align_string_pool`] on bytes held in memory; `None` when every slice is already aligned.
///
/// The string table moves to the next 8-byte boundary (`pad` zero bytes before it) and grows by `16 - pad` zero
/// bytes at its end, so everything after it in `__LINKEDIT` (usually only the code signature) moves by exactly
/// 16 bytes and keeps its alignment. Symbol entries address strings relative to the table, so they stay valid.
pub fn align_string_pool_bytes(data: &[u8]) -> Result<Option<Vec<u8>>, MachOError> {
    map_slices(data, align_thin)
}

/// Thins a fat file to one architecture in place. A thin file of that architecture is left alone.
pub fn thin(path: &Path, arch: Arch) -> Result<(), MachOError> {
    let data = std::fs::read(path)?;
    let out = thin_bytes(&data, arch)?;
    if out.len() != data.len() || out != data {
        write_in_place(path, &out)?;
    }
    Ok(())
}

/// [`thin`] on bytes held in memory.
pub fn thin_bytes(data: &[u8], arch: Arch) -> Result<Vec<u8>, MachOError> {
    let m = MachO::parse(data)?;
    let i = m
        .cpu_slices()
        .iter()
        .position(|s| s.cpu.arch() == Some(arch))
        .ok_or_else(|| MachOError::NotFound(format!("architecture {arch}")))?;
    let slice = m.slice_bytes(data, i).ok_or_else(|| MachOError::Malformed("slice outside the file".into()))?;
    Ok(slice.to_vec())
}

/// Rewrites the SDK version of a foreign binary. Never the default (ARCHITECTURE §16.4).
pub fn patch_sdk_version(path: &Path, sdk: Version) -> Result<(), MachOError> {
    let data = std::fs::read(path)?;
    let out = patch_sdk_version_bytes(&data, sdk)?;
    write_in_place(path, &out)
}

/// [`patch_sdk_version`] on bytes held in memory. Fails with [`MachOError::NotFound`] when a slice has neither
/// `LC_BUILD_VERSION` nor `LC_VERSION_MIN_*`.
pub fn patch_sdk_version_bytes(data: &[u8], sdk: Version) -> Result<Vec<u8>, MachOError> {
    let out = map_slices(data, |s| patch_sdk_thin(s, sdk).map(Some))?;
    out.ok_or_else(|| MachOError::NotFound("a version load command".into()))
}

fn delete_rpath_thin(b: &[u8], rpath: &str) -> Result<Option<Vec<u8>>, MachOError> {
    let h = parse_header(b)?;
    let cmds = load_commands(b, &h)?;
    let mut keep = Vec::with_capacity(h.sizeofcmds as usize);
    let mut removed = 0u32;
    for &(off, cmd, size) in &cmds {
        if cmd == LC_RPATH {
            let name_off = u32_le(b, off + 8)? as usize;
            if cstr(b, off + name_off, off + size, "rpath")? == rpath {
                removed += 1;
                continue;
            }
        }
        keep.extend_from_slice(bytes(b, off, size, "load command")?);
    }
    if removed == 0 {
        return Ok(None);
    }
    let lc_start = h.header_size;
    let lc_end = lc_start + h.sizeofcmds as usize;
    let new_size = keep.len();
    keep.resize(h.sizeofcmds as usize, 0);
    let mut out = b.to_vec();
    out[lc_start..lc_end].copy_from_slice(&keep);
    put_u32_le(&mut out, 16, h.ncmds - removed)?;
    put_u32_le(&mut out, 20, u32::try_from(new_size).map_err(|_| MachOError::Malformed("sizeofcmds".into()))?)?;
    Ok(Some(out))
}

fn patch_sdk_thin(b: &[u8], sdk: Version) -> Result<Vec<u8>, MachOError> {
    let h = parse_header(b)?;
    let mut out = b.to_vec();
    let mut found = false;
    for (off, cmd, _) in load_commands(b, &h)? {
        match cmd {
            LC_BUILD_VERSION => {
                put_u32_le(&mut out, off + 16, sdk.packed())?;
                found = true;
            }
            LC_VERSION_MIN_IPHONEOS | LC_VERSION_MIN_MACOSX | LC_VERSION_MIN_TVOS | LC_VERSION_MIN_WATCHOS => {
                put_u32_le(&mut out, off + 12, sdk.packed())?;
                found = true;
            }
            _ => {}
        }
    }
    if found { Ok(out) } else { Err(MachOError::NotFound("LC_BUILD_VERSION or LC_VERSION_MIN_*".into())) }
}

/// The shift applied to `__LINKEDIT` data after the string table.
const STRING_POOL_SHIFT: u32 = 16;

fn align_thin(b: &[u8]) -> Result<Option<Vec<u8>>, MachOError> {
    let h = parse_header(b)?;
    if !h.is_64 {
        return Ok(None);
    }
    let cmds = load_commands(b, &h)?;
    let Some(&(symtab, _, _)) = cmds.iter().find(|c| c.1 == LC_SYMTAB) else {
        return Ok(None);
    };
    let stroff = u32_le(b, symtab + 16)?;
    let strsize = u32_le(b, symtab + 20)?;
    if strsize == 0 || stroff % 8 == 0 {
        return Ok(None);
    }
    let pad_before = 8 - stroff % 8;
    let pad_after = STRING_POOL_SHIFT - pad_before;
    let str_end =
        stroff.checked_add(strsize).ok_or_else(|| MachOError::Malformed("string table end overflows".into()))?;

    // The string table must live in __LINKEDIT, the last segment, which must end the file.
    let mut linkedit = None;
    let mut last_fileoff = 0u64;
    for &(off, cmd, _) in &cmds {
        if cmd == LC_SEGMENT_64 {
            let name = crate::read::fixed_name(b, off + 8, 16)?;
            let fileoff = u64_le(b, off + 40)?;
            let filesize = u64_le(b, off + 48)?;
            if filesize > 0 {
                last_fileoff = last_fileoff.max(fileoff);
            }
            if name == "__LINKEDIT" {
                linkedit = Some((off, fileoff, filesize));
            }
        }
    }
    let (le_cmd, le_off, le_size) = linkedit.ok_or_else(|| MachOError::Unsupported("no __LINKEDIT segment".into()))?;
    if le_off != last_fileoff {
        return Err(MachOError::Unsupported("__LINKEDIT is not the last segment".into()));
    }
    let le_end = le_off.checked_add(le_size).ok_or_else(|| MachOError::Malformed("__LINKEDIT size".into()))?;
    if u64::from(stroff) < le_off || u64::from(str_end) > le_end {
        return Err(MachOError::Unsupported("the string table is outside __LINKEDIT".into()));
    }
    if le_end != b.len() as u64 {
        return Err(MachOError::Unsupported("data follows __LINKEDIT".into()));
    }

    let mut out = Vec::with_capacity(b.len() + STRING_POOL_SHIFT as usize);
    let (s, e) = (stroff as usize, str_end as usize);
    out.extend_from_slice(&b[..s]);
    out.resize(out.len() + pad_before as usize, 0);
    out.extend_from_slice(&b[s..e]);
    out.resize(out.len() + pad_after as usize, 0);
    out.extend_from_slice(&b[e..]);

    // Offsets at or after the end of the string table move by STRING_POOL_SHIFT; an offset strictly inside it
    // would mean overlapping data, which we refuse to guess about.
    let shift = |v: u32, what: &str| -> Result<u32, MachOError> {
        if v == 0 || v < stroff {
            Ok(v)
        } else if v >= str_end {
            v.checked_add(STRING_POOL_SHIFT).ok_or_else(|| MachOError::Malformed(format!("{what} overflows")))
        } else {
            Err(MachOError::Unsupported(format!("{what} points inside the string table")))
        }
    };
    let fix = |out: &mut Vec<u8>, at: usize, what: &str| -> Result<(), MachOError> {
        let v = u32_le(out, at)?;
        put_u32_le(out, at, shift(v, what)?)
    };
    for &(off, cmd, _) in &cmds {
        match cmd {
            LC_SYMTAB => {
                fix(&mut out, off + 8, "symoff")?;
                put_u32_le(&mut out, off + 16, stroff + pad_before)?;
                put_u32_le(&mut out, off + 20, strsize + pad_after)?;
            }
            LC_DYSYMTAB => {
                for (at, what) in [
                    (32, "tocoff"),
                    (40, "modtaboff"),
                    (48, "extrefsymoff"),
                    (56, "indirectsymoff"),
                    (64, "extreloff"),
                    (72, "locreloff"),
                ] {
                    fix(&mut out, off + at, what)?;
                }
            }
            LC_DYLD_INFO | LC_DYLD_INFO_ONLY => {
                for (at, what) in [
                    (8, "rebase_off"),
                    (16, "bind_off"),
                    (24, "weak_bind_off"),
                    (32, "lazy_bind_off"),
                    (40, "export_off"),
                ] {
                    fix(&mut out, off + at, what)?;
                }
            }
            LC_CODE_SIGNATURE
            | LC_SEGMENT_SPLIT_INFO
            | LC_FUNCTION_STARTS
            | LC_DATA_IN_CODE
            | LC_DYLIB_CODE_SIGN_DRS
            | LC_LINKER_OPTIMIZATION_HINT
            | LC_DYLD_EXPORTS_TRIE
            | LC_DYLD_CHAINED_FIXUPS
            | LC_ATOM_INFO => {
                fix(&mut out, off + 8, "dataoff")?;
            }
            _ => {}
        }
    }
    let new_size = le_size + u64::from(STRING_POOL_SHIFT);
    put_u64_le(&mut out, le_cmd + 48, new_size)?;
    let vmsize = u64_le(&out, le_cmd + 32)?;
    if vmsize < new_size {
        put_u64_le(&mut out, le_cmd + 32, new_size.div_ceil(0x4000) * 0x4000)?;
    }
    Ok(Some(out))
}

/// One fat entry's CPU type, subtype and alignment (log2).
struct FatEntry {
    cputype: u32,
    cpusubtype: u32,
    align: u32,
}

/// Applies `f` to every thin slice and returns the new file when any slice changed (`None` otherwise).
fn map_slices(
    data: &[u8],
    mut f: impl FnMut(&[u8]) -> Result<Option<Vec<u8>>, MachOError>,
) -> Result<Option<Vec<u8>>, MachOError> {
    let magic = u32_be(data, 0)?;
    if magic != FAT_MAGIC && magic != FAT_MAGIC_64 {
        return f(data);
    }
    let is64 = magic == FAT_MAGIC_64;
    let n = u32_be(data, 4)? as usize;
    if n == 0 || n > 32 {
        return Err(MachOError::Malformed(format!("fat header lists {n} architectures")));
    }
    let entry = if is64 { 32 } else { 20 };
    let mut entries = Vec::with_capacity(n);
    let mut slices: Vec<Cow<'_, [u8]>> = Vec::with_capacity(n);
    let mut changed = false;
    for i in 0..n {
        let base = 8 + i * entry;
        let (offset, size, align) = if is64 {
            (u64_be(data, base + 8)?, u64_be(data, base + 16)?, u32_be(data, base + 24)?)
        } else {
            (u64::from(u32_be(data, base + 8)?), u64::from(u32_be(data, base + 12)?), u32_be(data, base + 16)?)
        };
        entries.push(FatEntry { cputype: u32_be(data, base)?, cpusubtype: u32_be(data, base + 4)?, align });
        let slice = bytes(data, usize_of(offset, "slice")?, usize_of(size, "slice")?, "fat slice")?;
        match f(slice)? {
            Some(new) => {
                changed = true;
                slices.push(Cow::Owned(new));
            }
            None => slices.push(Cow::Borrowed(slice)),
        }
    }
    if !changed {
        return Ok(None);
    }
    build_fat(is64, &entries, &slices).map(Some)
}

fn build_fat(is64: bool, entries: &[FatEntry], slices: &[Cow<'_, [u8]>]) -> Result<Vec<u8>, MachOError> {
    let entry = if is64 { 32 } else { 20 };
    let mut cur = (8 + entries.len() * entry) as u64;
    let mut offsets = Vec::with_capacity(entries.len());
    for (e, s) in entries.iter().zip(slices) {
        let align = 1u64 << e.align.min(20);
        cur = cur.div_ceil(align) * align;
        offsets.push(cur);
        cur += s.len() as u64;
    }
    let total = usize_of(cur, "fat file size")?;
    let mut out = vec![0u8; total];
    out[0..4].copy_from_slice(&(if is64 { FAT_MAGIC_64 } else { FAT_MAGIC }).to_be_bytes());
    out[4..8].copy_from_slice(&(entries.len() as u32).to_be_bytes());
    for (i, ((e, s), &off)) in entries.iter().zip(slices).zip(&offsets).enumerate() {
        let base = 8 + i * entry;
        out[base..base + 4].copy_from_slice(&e.cputype.to_be_bytes());
        out[base + 4..base + 8].copy_from_slice(&e.cpusubtype.to_be_bytes());
        if is64 {
            out[base + 8..base + 16].copy_from_slice(&off.to_be_bytes());
            out[base + 16..base + 24].copy_from_slice(&(s.len() as u64).to_be_bytes());
            out[base + 24..base + 28].copy_from_slice(&e.align.to_be_bytes());
        } else {
            let off32 = u32::try_from(off).map_err(|_| MachOError::Unsupported("slice beyond 4 GiB".into()))?;
            let len32 = u32::try_from(s.len()).map_err(|_| MachOError::Unsupported("slice over 4 GiB".into()))?;
            out[base + 8..base + 12].copy_from_slice(&off32.to_be_bytes());
            out[base + 12..base + 16].copy_from_slice(&len32.to_be_bytes());
            out[base + 16..base + 20].copy_from_slice(&e.align.to_be_bytes());
        }
        let start = usize_of(off, "slice offset")?;
        out[start..start + s.len()].copy_from_slice(s);
    }
    Ok(out)
}

/// Writes `data` to a temporary file next to `path`, copies `path`'s permissions and renames it over `path`.
fn write_in_place(path: &Path, data: &[u8]) -> Result<(), MachOError> {
    let perms = std::fs::metadata(path)?.permissions();
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!(".{name}.russet-macho-{}", std::process::id()));
    let result = (|| {
        std::fs::write(&tmp, data)?;
        std::fs::set_permissions(&tmp, perms)?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map_err(MachOError::from)
}
