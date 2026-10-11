// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! A tiny arm64 iOS Mach-O executable built at test time (no Apple file is committed, ARCHITECTURE §19), and
//! the reader the fake debugserver uses to serve it from "device memory" (RD V5).
//!
//! The file has `__PAGEZERO`, `__TEXT` (one `__text` section with `_main: mov x0, #0; ret`), `__LINKEDIT`
//! (symbol and string tables), `LC_SYMTAB`, `LC_UUID`, `LC_BUILD_VERSION` (iOS, minos 17.0, sdk 27.0) and
//! `LC_MAIN`. LLDB loads it with `target create --arch arm64-apple-ios` and binds it to the fake process by UUID.

use std::io;

pub const MH_MAGIC_64: u32 = 0xFEED_FACF;
pub const CPU_TYPE_ARM64: u32 = 0x0100_000C;
pub const MH_EXECUTE: u32 = 2;
const LC_SEGMENT_64: u32 = 0x19;
const LC_SYMTAB: u32 = 0x2;
const LC_UUID: u32 = 0x1B;
const LC_BUILD_VERSION: u32 = 0x32;
const LC_MAIN: u32 = 0x8000_0028;

/// The unslid `__TEXT` address of the synthetic executable.
pub const TEXT_VMADDR: u64 = 0x1_0000_0000;
/// File offset (and `__TEXT`-relative offset) of `_main`.
pub const MAIN_OFFSET: u64 = 0x1000;
const PAGE: u64 = 0x4000;

/// One segment as read from a Mach-O file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub name: String,
    pub vmaddr: u64,
    pub vmsize: u64,
    pub fileoff: u64,
    pub filesize: u64,
    pub maxprot: u32,
}

/// What the fake debugserver needs from an executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachOInfo {
    pub cputype: u32,
    pub cpusubtype: u32,
    pub filetype: u32,
    pub flags: u32,
    pub segments: Vec<Segment>,
    /// Upper-case, dashed, as debugserver reports it.
    pub uuid: Option<String>,
    /// `LC_MAIN.entryoff`.
    pub entryoff: Option<u64>,
}

/// Builds the synthetic executable with this UUID.
pub fn synthetic_ios_executable(uuid: [u8; 16]) -> Vec<u8> {
    // __text: _main = mov x0, #0; ret
    let text: [u32; 2] = [0xD280_0000, 0xD65F_03C0];
    let strtab: &[u8] = b"\0_main\0";
    let nsyms = 1u32;

    let mut cmds = Vec::new();
    let mut ncmds = 0u32;
    let mut push = |cmds: &mut Vec<u8>, cmd: Vec<u8>| {
        cmds.extend_from_slice(&cmd);
        ncmds += 1;
    };
    push(&mut cmds, segment("__PAGEZERO", 0, TEXT_VMADDR, 0, 0, 0, &[]));
    let section = text_section(TEXT_VMADDR + MAIN_OFFSET, (text.len() * 4) as u64, MAIN_OFFSET as u32);
    push(&mut cmds, segment("__TEXT", TEXT_VMADDR, PAGE, 0, PAGE, 5, &[section]));
    let linkedit_off = PAGE;
    let symoff = linkedit_off as u32;
    let stroff = symoff + nsyms * 16;
    let linkedit_size = u64::from(nsyms * 16) + strtab.len() as u64;
    push(&mut cmds, segment("__LINKEDIT", TEXT_VMADDR + PAGE, PAGE, linkedit_off, linkedit_size, 1, &[]));
    let mut symtab = Vec::new();
    for v in [LC_SYMTAB, 24, symoff, nsyms, stroff, strtab.len() as u32] {
        symtab.extend_from_slice(&v.to_le_bytes());
    }
    push(&mut cmds, symtab);
    let mut uuid_cmd = Vec::new();
    uuid_cmd.extend_from_slice(&LC_UUID.to_le_bytes());
    uuid_cmd.extend_from_slice(&24u32.to_le_bytes());
    uuid_cmd.extend_from_slice(&uuid);
    push(&mut cmds, uuid_cmd);
    let mut build = Vec::new();
    // platform 2 (iOS), minos 17.0, sdk 27.0, no tools.
    for v in [LC_BUILD_VERSION, 24, 2, 0x0011_0000, 0x001B_0000, 0] {
        build.extend_from_slice(&v.to_le_bytes());
    }
    push(&mut cmds, build);
    let mut main = Vec::new();
    main.extend_from_slice(&LC_MAIN.to_le_bytes());
    main.extend_from_slice(&24u32.to_le_bytes());
    main.extend_from_slice(&MAIN_OFFSET.to_le_bytes());
    main.extend_from_slice(&0u64.to_le_bytes());
    push(&mut cmds, main);

    let mut out = Vec::new();
    // MH_NOUNDEFS | MH_DYLDLINK | MH_TWOLEVEL | MH_PIE
    for v in [MH_MAGIC_64, CPU_TYPE_ARM64, 0, MH_EXECUTE, ncmds, cmds.len() as u32, 0x0020_0085, 0] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&cmds);
    out.resize(MAIN_OFFSET as usize, 0);
    for w in text {
        out.extend_from_slice(&w.to_le_bytes());
    }
    out.resize(linkedit_off as usize, 0);
    // nlist_64: n_strx 1, N_SECT|N_EXT, section 1, desc 0, value = _main.
    out.extend_from_slice(&1u32.to_le_bytes());
    out.push(0x0F);
    out.push(1);
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(TEXT_VMADDR + MAIN_OFFSET).to_le_bytes());
    out.extend_from_slice(strtab);
    out
}

fn fixed16(name: &str) -> [u8; 16] {
    let mut b = [0u8; 16];
    let n = name.len().min(16);
    b[..n].copy_from_slice(&name.as_bytes()[..n]);
    b
}

fn segment(
    name: &str,
    vmaddr: u64,
    vmsize: u64,
    fileoff: u64,
    filesize: u64,
    prot: u32,
    sections: &[Vec<u8>],
) -> Vec<u8> {
    let mut c = Vec::new();
    c.extend_from_slice(&LC_SEGMENT_64.to_le_bytes());
    c.extend_from_slice(&(72 + 80 * sections.len() as u32).to_le_bytes());
    c.extend_from_slice(&fixed16(name));
    for v in [vmaddr, vmsize, fileoff, filesize] {
        c.extend_from_slice(&v.to_le_bytes());
    }
    for v in [prot, prot, sections.len() as u32, 0] {
        c.extend_from_slice(&v.to_le_bytes());
    }
    for s in sections {
        c.extend_from_slice(s);
    }
    c
}

fn text_section(addr: u64, size: u64, offset: u32) -> Vec<u8> {
    let mut s = Vec::new();
    s.extend_from_slice(&fixed16("__text"));
    s.extend_from_slice(&fixed16("__TEXT"));
    s.extend_from_slice(&addr.to_le_bytes());
    s.extend_from_slice(&size.to_le_bytes());
    // offset, align (2^2), reloff, nreloc, flags (pure + some instructions), reserved1..3
    for v in [offset, 2, 0, 0, 0x8000_0400, 0, 0, 0] {
        s.extend_from_slice(&v.to_le_bytes());
    }
    s
}

/// A code directory for [`signed_test_macho`]: its hash type and the bytes that stand for its hash slots (the
/// testkit has no hash functions; readers of the CDHash hash the whole directory, whatever its slots hold).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestCodeDirectory {
    pub hash_type: u8,
    pub slots: Vec<u8>,
}

impl TestCodeDirectory {
    pub fn new(hash_type: u8, slots: &[u8]) -> Self {
        Self { hash_type, slots: slots.to_vec() }
    }
}

/// A signed test file and the exact code-directory blobs inside it, in slot order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedTestMachO {
    pub bytes: Vec<u8>,
    pub code_directories: Vec<Vec<u8>>,
}

const LC_CODE_SIGNATURE: u32 = 0x1D;

/// A thin arm64 file with `LC_UUID`, `code` and an embedded signature (`LC_CODE_SIGNATURE`): a SuperBlob with the
/// code directories (slot 0, then the alternate slots `0x1000…`), an empty requirements blob and a CMS blob
/// wrapper holding `cms`, which stands for the signature that changes on every signing. Not runnable; it is what
/// signature readers parse.
pub fn signed_test_macho(uuid: [u8; 16], code: &[u8], cds: &[TestCodeDirectory], cms: &[u8]) -> SignedTestMachO {
    let be = |v: u32| v.to_be_bytes();
    let mut directories = Vec::new();
    for cd in cds {
        let ident = b"dev.russet.test.probe\0";
        let hash_size = cd.slots.len().min(255) as u8;
        let ident_offset = 44u32;
        let hash_offset = ident_offset + ident.len() as u32;
        let len = hash_offset + cd.slots.len() as u32;
        let mut b = Vec::new();
        // magic, length, version 0x20001, flags, hashOffset, identOffset, nSpecialSlots, nCodeSlots, codeLimit
        for v in [0xFADE_0C02, len, 0x0002_0001, 0, hash_offset, ident_offset, 0, 1, code.len() as u32] {
            b.extend_from_slice(&be(v));
        }
        // hashSize, hashType, platform, pageSize (2^12), spare2
        b.extend_from_slice(&[hash_size, cd.hash_type, 0, 12]);
        b.extend_from_slice(&be(0));
        b.extend_from_slice(ident);
        b.extend_from_slice(&cd.slots);
        directories.push(b);
    }
    let mut blobs: Vec<(u32, Vec<u8>)> = Vec::new();
    for (i, d) in directories.iter().enumerate() {
        let slot = if i == 0 { 0 } else { 0x1000 + i as u32 - 1 };
        blobs.push((slot, d.clone()));
    }
    let mut requirements = Vec::new();
    for v in [0xFADE_0C01, 12, 0] {
        requirements.extend_from_slice(&be(v));
    }
    blobs.insert(1.min(blobs.len()), (2, requirements));
    let mut wrapper = Vec::new();
    wrapper.extend_from_slice(&be(0xFADE_0B01));
    wrapper.extend_from_slice(&be(8 + cms.len() as u32));
    wrapper.extend_from_slice(cms);
    blobs.push((0x10000, wrapper));
    let header = 12 + 8 * blobs.len();
    let total = header + blobs.iter().map(|(_, b)| b.len()).sum::<usize>();
    let mut sig = Vec::new();
    for v in [0xFADE_0CC0, total as u32, blobs.len() as u32] {
        sig.extend_from_slice(&be(v));
    }
    let mut offset = header;
    for (slot, b) in &blobs {
        sig.extend_from_slice(&be(*slot));
        sig.extend_from_slice(&be(offset as u32));
        offset += b.len();
    }
    for (_, b) in &blobs {
        sig.extend_from_slice(b);
    }

    let cmds_len = 24 + 16;
    let code_at = 32 + cmds_len;
    let sig_at = (code_at + code.len()).next_multiple_of(16);
    let mut out = Vec::new();
    for v in [MH_MAGIC_64, CPU_TYPE_ARM64, 0, MH_EXECUTE, 2, cmds_len as u32, 0x0020_0085, 0] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    for v in [LC_UUID, 24] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&uuid);
    for v in [LC_CODE_SIGNATURE, 16, sig_at as u32, sig.len() as u32] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(code);
    out.resize(sig_at, 0);
    out.extend_from_slice(&sig);
    SignedTestMachO { bytes: out, code_directories: directories }
}

/// A universal (fat) file of `slices` (`cputype`, `cpusubtype`, file), 16 KiB-aligned; `wide` writes the 64-bit
/// table (`FAT_MAGIC_64`).
pub fn universal(slices: &[(u32, u32, &[u8])], wide: bool) -> Vec<u8> {
    const ALIGN: usize = 0x4000;
    let mut out = Vec::new();
    out.extend_from_slice(&(if wide { 0xCAFE_BABF_u32 } else { 0xCAFE_BABE }).to_be_bytes());
    out.extend_from_slice(&(slices.len() as u32).to_be_bytes());
    let mut at = ALIGN;
    let mut placed = Vec::new();
    for (cputype, subtype, bytes) in slices {
        out.extend_from_slice(&cputype.to_be_bytes());
        out.extend_from_slice(&subtype.to_be_bytes());
        if wide {
            out.extend_from_slice(&(at as u64).to_be_bytes());
            out.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
            out.extend_from_slice(&14u32.to_be_bytes());
            out.extend_from_slice(&0u32.to_be_bytes());
        } else {
            out.extend_from_slice(&(at as u32).to_be_bytes());
            out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
            out.extend_from_slice(&14u32.to_be_bytes());
        }
        placed.push(at);
        at = (at + bytes.len()).next_multiple_of(ALIGN);
    }
    for ((_, _, bytes), at) in slices.iter().zip(placed) {
        out.resize(at, 0);
        out.extend_from_slice(bytes);
    }
    out
}

/// Reads the header, segments, UUID and entry point of a thin 64-bit Mach-O.
pub fn parse(data: &[u8]) -> io::Result<MachOInfo> {
    let bad = |what: &str| io::Error::new(io::ErrorKind::InvalidData, format!("Mach-O: {what}"));
    let u32_at = |o: usize| data.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let u64_at =
        |o: usize| data.get(o..o + 8).map(|b| u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]));
    if u32_at(0) != Some(MH_MAGIC_64) {
        return Err(bad("not a thin 64-bit little-endian file"));
    }
    let mut info = MachOInfo {
        cputype: u32_at(4).ok_or_else(|| bad("short header"))?,
        cpusubtype: u32_at(8).ok_or_else(|| bad("short header"))?,
        filetype: u32_at(12).ok_or_else(|| bad("short header"))?,
        flags: u32_at(24).ok_or_else(|| bad("short header"))?,
        segments: Vec::new(),
        uuid: None,
        entryoff: None,
    };
    let ncmds = u32_at(16).ok_or_else(|| bad("short header"))?;
    let mut off = 32usize;
    for _ in 0..ncmds {
        let cmd = u32_at(off).ok_or_else(|| bad("truncated load command"))?;
        let size = u32_at(off + 4).ok_or_else(|| bad("truncated load command"))? as usize;
        if size < 8 {
            return Err(bad("load command smaller than 8 bytes"));
        }
        match cmd {
            LC_SEGMENT_64 => {
                let name = data.get(off + 8..off + 24).ok_or_else(|| bad("truncated segment"))?;
                let end = name.iter().position(|&b| b == 0).unwrap_or(16);
                info.segments.push(Segment {
                    name: String::from_utf8_lossy(&name[..end]).into_owned(),
                    vmaddr: u64_at(off + 24).ok_or_else(|| bad("truncated segment"))?,
                    vmsize: u64_at(off + 32).ok_or_else(|| bad("truncated segment"))?,
                    fileoff: u64_at(off + 40).ok_or_else(|| bad("truncated segment"))?,
                    filesize: u64_at(off + 48).ok_or_else(|| bad("truncated segment"))?,
                    maxprot: u32_at(off + 56).ok_or_else(|| bad("truncated segment"))?,
                });
            }
            LC_UUID => {
                let u = data.get(off + 8..off + 24).ok_or_else(|| bad("truncated LC_UUID"))?;
                info.uuid = Some(format_uuid(u));
            }
            LC_MAIN => info.entryoff = u64_at(off + 8),
            _ => {}
        }
        off += size;
    }
    Ok(info)
}

/// `4C4C44A9-5555-3144-A1B2-60BFA66E4425` from 16 bytes.
pub fn format_uuid(u: &[u8]) -> String {
    let hex: String = u.iter().map(|b| format!("{b:02X}")).collect();
    if hex.len() != 32 {
        return hex;
    }
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn the_synthetic_executable_parses() {
        let uuid = [0x4C, 0x4C, 0x44, 0xA9, 0x55, 0x55, 0x31, 0x44, 0xA1, 0xB2, 0x60, 0xBF, 0xA6, 0x6E, 0x44, 0x25];
        let bytes = synthetic_ios_executable(uuid);
        let info = parse(&bytes).unwrap();
        assert_eq!((info.cputype, info.filetype), (CPU_TYPE_ARM64, MH_EXECUTE));
        assert_eq!(info.uuid.as_deref(), Some("4C4C44A9-5555-3144-A1B2-60BFA66E4425"));
        assert_eq!(info.entryoff, Some(MAIN_OFFSET));
        let names: Vec<&str> = info.segments.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["__PAGEZERO", "__TEXT", "__LINKEDIT"]);
        let text = &info.segments[1];
        assert_eq!((text.vmaddr, text.maxprot), (TEXT_VMADDR, 5));
        let main = &bytes[MAIN_OFFSET as usize..MAIN_OFFSET as usize + 8];
        assert_eq!(main, [0x00, 0x00, 0x80, 0xD2, 0xC0, 0x03, 0x5F, 0xD6]);
        assert!(parse(b"\x7fELF").is_err());
    }

    #[test]
    fn signed_test_files_carry_their_uuid_and_blobs() {
        let cds = [TestCodeDirectory::new(1, b"sha1"), TestCodeDirectory::new(2, b"sha256 slots")];
        let built = signed_test_macho([3; 16], b"code", &cds, b"cms");
        assert_eq!(parse(&built.bytes).unwrap().uuid.as_deref(), Some("03030303-0303-0303-0303-030303030303"));
        assert_eq!(built.code_directories.len(), 2);
        for cd in &built.code_directories {
            assert!(built.bytes.windows(cd.len()).any(|w| w == cd.as_slice()));
            assert_eq!(&cd[..4], &[0xFA, 0xDE, 0x0C, 0x02]);
        }
        assert_eq!(built.code_directories[1][37], 2, "hashType");
        let fat = universal(&[(CPU_TYPE_ARM64, 0, &built.bytes)], false);
        assert_eq!(&fat[..4], &[0xCA, 0xFE, 0xBA, 0xBE]);
        assert_eq!(&fat[0x4000..0x4000 + built.bytes.len()], built.bytes.as_slice());
    }
}
