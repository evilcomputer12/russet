// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The `DT_NEEDED` entries of an ELF64 little-endian shared object, read without loading the file: `liblldb.so`
//! is about 230 MB, and the resolver only needs the libpython soname from its dynamic section. Reads the ELF
//! header, the program headers, `PT_DYNAMIC` and the dynamic string table, a few KiB in all.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const DT_NULL: u64 = 0;
const DT_NEEDED: u64 = 1;
const DT_STRTAB: u64 = 5;
const DT_STRSZ: u64 = 10;
/// Upper bounds, so a damaged file cannot make the reader allocate much.
const MAX_DYNAMIC: u64 = 1 << 20;
const MAX_STRTAB: u64 = 16 << 20;

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_owned())
}

fn read_at(f: &mut File, offset: u64, len: u64) -> io::Result<Vec<u8>> {
    let mut buf = vec![0u8; usize::try_from(len).map_err(|_| bad("length out of range"))?];
    f.seek(SeekFrom::Start(offset))?;
    f.read_exact(&mut buf)?;
    Ok(buf)
}

fn u16_at(b: &[u8], at: usize) -> io::Result<u16> {
    b.get(at..at + 2).and_then(|s| s.try_into().ok()).map(u16::from_le_bytes).ok_or_else(|| bad("truncated"))
}

fn u32_at(b: &[u8], at: usize) -> io::Result<u32> {
    b.get(at..at + 4).and_then(|s| s.try_into().ok()).map(u32::from_le_bytes).ok_or_else(|| bad("truncated"))
}

fn u64_at(b: &[u8], at: usize) -> io::Result<u64> {
    b.get(at..at + 8).and_then(|s| s.try_into().ok()).map(u64::from_le_bytes).ok_or_else(|| bad("truncated"))
}

/// The `DT_NEEDED` sonames of an ELF64 little-endian file; `InvalidData` for anything else.
pub fn needed_libraries(path: &Path) -> io::Result<Vec<String>> {
    let mut f = File::open(path)?;
    let header = read_at(&mut f, 0, 64)?;
    if &header[..4] != b"\x7fELF" || header[4] != 2 || header[5] != 1 {
        return Err(bad("not an ELF64 little-endian file"));
    }
    let phoff = u64_at(&header, 0x20)?;
    let phentsize = u64::from(u16_at(&header, 0x36)?);
    let phnum = u64::from(u16_at(&header, 0x38)?);
    if phentsize < 56 || phnum == 0 || phnum > 4096 {
        return Err(bad("unexpected program headers"));
    }
    let phdrs = read_at(&mut f, phoff, phentsize * phnum)?;
    // (offset, vaddr, filesz) of every PT_LOAD, and PT_DYNAMIC's (offset, filesz).
    let mut loads = Vec::new();
    let mut dynamic = None;
    for i in 0..phnum {
        let base = usize::try_from(i * phentsize).map_err(|_| bad("offset out of range"))?;
        let p_type = u32_at(&phdrs, base)?;
        let offset = u64_at(&phdrs, base + 8)?;
        let vaddr = u64_at(&phdrs, base + 16)?;
        let filesz = u64_at(&phdrs, base + 32)?;
        match p_type {
            PT_LOAD => loads.push((offset, vaddr, filesz)),
            PT_DYNAMIC => dynamic = Some((offset, filesz)),
            _ => {}
        }
    }
    let Some((dyn_off, dyn_size)) = dynamic else { return Ok(Vec::new()) };
    let dyn_bytes = read_at(&mut f, dyn_off, dyn_size.min(MAX_DYNAMIC))?;
    let (mut needed, mut strtab, mut strsz) = (Vec::new(), None, None);
    for entry in dyn_bytes.as_chunks::<16>().0 {
        let tag = u64_at(entry, 0)?;
        let val = u64_at(entry, 8)?;
        match tag {
            DT_NULL => break,
            DT_NEEDED => needed.push(val),
            DT_STRTAB => strtab = Some(val),
            DT_STRSZ => strsz = Some(val),
            _ => {}
        }
    }
    let (Some(strtab_vaddr), Some(strsz)) = (strtab, strsz) else { return Ok(Vec::new()) };
    // DT_STRTAB is a virtual address: map it to a file offset through the segment that holds it.
    let strtab_off = loads
        .iter()
        .find(|(_, vaddr, filesz)| strtab_vaddr >= *vaddr && strtab_vaddr < vaddr + filesz)
        .map(|(off, vaddr, _)| off + (strtab_vaddr - vaddr))
        .ok_or_else(|| bad("DT_STRTAB lies outside every PT_LOAD segment"))?;
    let strings = read_at(&mut f, strtab_off, strsz.min(MAX_STRTAB))?;
    Ok(needed
        .into_iter()
        .filter_map(|off| {
            let start = usize::try_from(off).ok()?;
            let rest = strings.get(start..)?;
            let end = rest.iter().position(|b| *b == 0)?;
            String::from_utf8(rest[..end].to_vec()).ok()
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_test_binary_needs_libc() {
        if !cfg!(all(target_os = "linux", target_endian = "little", target_pointer_width = "64")) {
            return;
        }
        let exe = std::env::current_exe().expect("test binary");
        let needed = needed_libraries(&exe).expect("an ELF64 LE executable");
        assert!(needed.iter().any(|n| n.starts_with("libc.so")), "{needed:?}");
    }

    #[test]
    fn non_elf_files_are_rejected() {
        let dir = std::env::temp_dir().join(format!("russet-elf-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let f = dir.join("not-elf");
        std::fs::write(&f, vec![0u8; 128]).expect("write");
        assert_eq!(needed_libraries(&f).map_err(|e| e.kind()).err(), Some(io::ErrorKind::InvalidData));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
