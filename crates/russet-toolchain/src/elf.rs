// SPDX-License-Identifier: AGPL-3.0-only
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
/// The size of an `Elf64_Phdr`; any other `e_phentsize` is refused.
const PHDR_SIZE: u64 = 56;
/// Upper bounds, so a damaged file cannot make the reader allocate much.
const MAX_PHNUM: u64 = 4096;
const MAX_DYNAMIC: u64 = 1 << 20;
const MAX_STRTAB: u64 = 16 << 20;

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_owned())
}

/// Reads `len` bytes at `offset`, refusing ranges that overflow or lie beyond the end of the file before
/// allocating anything.
fn read_at(f: &mut File, file_len: u64, offset: u64, len: u64) -> io::Result<Vec<u8>> {
    let end = offset.checked_add(len).ok_or_else(|| bad("range overflows"))?;
    if end > file_len {
        return Err(bad("range beyond the end of the file"));
    }
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

/// The `DT_NEEDED` sonames of an ELF64 little-endian file; `InvalidData` for anything else, including a damaged
/// file (offsets past its end, overflowing ranges, an unexpected program-header size): never a panic, and never
/// more than a few MiB read.
pub fn needed_libraries(path: &Path) -> io::Result<Vec<String>> {
    let mut f = File::open(path)?;
    let file_len = f.metadata()?.len();
    let header = read_at(&mut f, file_len, 0, 64)?;
    if &header[..4] != b"\x7fELF" || header[4] != 2 || header[5] != 1 {
        return Err(bad("not an ELF64 little-endian file"));
    }
    let phoff = u64_at(&header, 0x20)?;
    let phentsize = u64::from(u16_at(&header, 0x36)?);
    let phnum = u64::from(u16_at(&header, 0x38)?);
    if phentsize != PHDR_SIZE || phnum == 0 || phnum > MAX_PHNUM {
        return Err(bad("unexpected program headers"));
    }
    let phdrs = read_at(&mut f, file_len, phoff, PHDR_SIZE * phnum)?;
    // (offset, vaddr, filesz) of every PT_LOAD, and PT_DYNAMIC's (offset, filesz).
    let mut loads = Vec::new();
    let mut dynamic = None;
    for i in 0..phnum {
        let base = usize::try_from(i * PHDR_SIZE).map_err(|_| bad("offset out of range"))?;
        let p_type = u32_at(&phdrs, base)?;
        let offset = u64_at(&phdrs, base + 8)?;
        let vaddr = u64_at(&phdrs, base + 16)?;
        let filesz = u64_at(&phdrs, base + 32)?;
        match p_type {
            PT_LOAD if vaddr.checked_add(filesz).is_none() || offset.checked_add(filesz).is_none() => {
                return Err(bad("a PT_LOAD segment's range overflows"));
            }
            PT_LOAD => loads.push((offset, vaddr, filesz)),
            PT_DYNAMIC => dynamic = Some((offset, filesz)),
            _ => {}
        }
    }
    let Some((dyn_off, dyn_size)) = dynamic else { return Ok(Vec::new()) };
    let dyn_bytes = read_at(&mut f, file_len, dyn_off, dyn_size.min(MAX_DYNAMIC))?;
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
    // DT_STRTAB is a virtual address: map it to a file offset through the segment that holds it (subtracting
    // first, so a huge `p_vaddr + p_filesz` cannot overflow).
    let strtab_off = loads
        .iter()
        .find(|(_, vaddr, filesz)| strtab_vaddr >= *vaddr && strtab_vaddr - vaddr < *filesz)
        .and_then(|(off, vaddr, _)| off.checked_add(strtab_vaddr - vaddr))
        .ok_or_else(|| bad("DT_STRTAB lies outside every PT_LOAD segment"))?;
    let available = file_len.saturating_sub(strtab_off);
    let strings = read_at(&mut f, file_len, strtab_off, strsz.min(MAX_STRTAB).min(available))?;
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
        let dir = tempfile::tempdir().expect("dir");
        let f = dir.path().join("not-elf");
        std::fs::write(&f, vec![0u8; 128]).expect("write");
        assert_eq!(needed_libraries(&f).map_err(|e| e.kind()).err(), Some(io::ErrorKind::InvalidData));
    }

    /// A minimal ELF64 LE file: the header, `phdrs` program headers at offset 64, then `tail`.
    fn elf(phentsize: u16, phdrs: &[[u64; 4]], tail: &[u8]) -> Vec<u8> {
        let mut b = vec![0u8; 64];
        b[..4].copy_from_slice(b"\x7fELF");
        b[4] = 2; // ELFCLASS64
        b[5] = 1; // little-endian
        b[0x20..0x28].copy_from_slice(&64u64.to_le_bytes());
        b[0x36..0x38].copy_from_slice(&phentsize.to_le_bytes());
        b[0x38..0x3a].copy_from_slice(&u16::try_from(phdrs.len()).expect("few").to_le_bytes());
        // Each entry: p_type, p_offset, p_vaddr, p_filesz (the other fields stay zero).
        for [p_type, offset, vaddr, filesz] in phdrs {
            let mut ph = vec![0u8; 56];
            ph[..4].copy_from_slice(&u32::try_from(*p_type).expect("type").to_le_bytes());
            ph[8..16].copy_from_slice(&offset.to_le_bytes());
            ph[16..24].copy_from_slice(&vaddr.to_le_bytes());
            ph[32..40].copy_from_slice(&filesz.to_le_bytes());
            b.extend_from_slice(&ph);
        }
        b.extend_from_slice(tail);
        b
    }

    fn dynamic(entries: &[(u64, u64)]) -> Vec<u8> {
        entries.iter().flat_map(|(t, v)| t.to_le_bytes().into_iter().chain(v.to_le_bytes())).collect()
    }

    #[test]
    fn damaged_files_are_errors_never_panics_or_huge_reads() {
        let dir = tempfile::tempdir().expect("dir");
        let check = |name: &str, bytes: Vec<u8>| {
            let f = dir.path().join(name);
            std::fs::write(&f, bytes).expect("write");
            needed_libraries(&f)
        };
        // A program-header size other than Elf64_Phdr's (the old reader would read 65535 × n bytes).
        let r = check("phentsize", elf(u16::MAX, &[[PT_LOAD.into(), 0, 0, 64]], &[]));
        assert_eq!(r.map_err(|e| e.kind()).err(), Some(io::ErrorKind::InvalidData));
        // A PT_LOAD whose vaddr + filesz overflows, and a DT_STRTAB that only that overflow would "contain".
        let dyn_at = 64 + 2 * 56;
        let dyn_bytes = dynamic(&[(DT_NEEDED, 0), (DT_STRTAB, u64::MAX - 1), (DT_STRSZ, 8), (DT_NULL, 0)]);
        let phdrs = [[PT_LOAD.into(), 0, u64::MAX - 8, u64::MAX], [PT_DYNAMIC.into(), dyn_at, 0, 64]];
        let r = check("overflow", elf(56, &phdrs, &dyn_bytes));
        assert_eq!(r.map_err(|e| e.kind()).err(), Some(io::ErrorKind::InvalidData));
        // Offsets past the end of the file.
        let phdrs = [[PT_LOAD.into(), 0, 0, 4096], [PT_DYNAMIC.into(), 1 << 40, 0, 1 << 20]];
        let r = check("past-end", elf(56, &phdrs, &[]));
        assert_eq!(r.map_err(|e| e.kind()).err(), Some(io::ErrorKind::InvalidData));
        // A well-formed file still works: the string table follows the dynamic section.
        let strtab_at = dyn_at + 4 * 16;
        let strtab = b"\0libpython3.12.so.1.0\0";
        let strsz = u64::try_from(strtab.len()).expect("small");
        let dyn_bytes = dynamic(&[(DT_NEEDED, 1), (DT_STRTAB, strtab_at), (DT_STRSZ, strsz), (DT_NULL, 0)]);
        let mut tail = dyn_bytes;
        tail.extend_from_slice(strtab);
        let phdrs = [[PT_LOAD.into(), 0, 0, 4096], [PT_DYNAMIC.into(), dyn_at, 0, 64]];
        let needed = check("good", elf(56, &phdrs, &tail)).expect("parses");
        assert_eq!(needed, ["libpython3.12.so.1.0"]);
    }
}
