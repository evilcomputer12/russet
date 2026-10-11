// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Synthetic Mach-O images for tests (feature `fixture`). ARCHITECTURE §19: Mach-O fixtures are built by Russet's
//! own code at test time and never committed.
//!
//! A built image is a small but structurally complete thin arm64 (or x86_64) file: `__PAGEZERO` (executables),
//! `__TEXT` with one `__text` section, `__LINKEDIT` with a symbol and string table, `LC_SYMTAB`, `LC_DYSYMTAB`,
//! `LC_BUILD_VERSION`, `LC_UUID`, `LC_MAIN` or `LC_ID_DYLIB`, dylib and rpath commands and, on request,
//! `LC_ENCRYPTION_INFO_64`. apple-codesign can sign it, so signer tests run on it.

use crate::consts::*;
use crate::{Arch, Version};

const PAGE: u64 = 0x4000;
const TEXT_SECTION_OFFSET: u64 = 0x1000;

/// Builds one thin image.
#[derive(Debug, Clone)]
pub struct MachOBuilder {
    arch: Arch,
    filetype: u32,
    platform: u32,
    minos: Version,
    sdk: Version,
    uuid: [u8; 16],
    install_name: String,
    dylibs: Vec<(String, bool)>,
    rpaths: Vec<String>,
    encryption: bool,
    symbols: Vec<String>,
    misaligned_string_pool: bool,
    text_size: u64,
    linker_signed: Option<String>,
}

impl MachOBuilder {
    /// An iOS arm64 executable (`MH_EXECUTE`), minos 17.0, sdk 27.0.
    pub fn executable() -> Self {
        Self {
            arch: Arch::Arm64,
            filetype: MH_EXECUTE,
            platform: PLATFORM_IOS,
            minos: Version::new(17, 0, 0),
            sdk: Version::new(27, 0, 0),
            uuid: [0x52; 16],
            install_name: String::new(),
            dylibs: vec![("/usr/lib/libSystem.B.dylib".into(), false)],
            rpaths: Vec::new(),
            encryption: false,
            symbols: vec!["_main".into()],
            misaligned_string_pool: false,
            text_size: 64,
            linker_signed: None,
        }
    }

    /// An iOS arm64 dynamic library (`MH_DYLIB`) with install name `install_name`.
    pub fn dylib(install_name: &str) -> Self {
        Self {
            filetype: MH_DYLIB,
            install_name: install_name.into(),
            symbols: vec!["_fixture_function".into()],
            ..Self::executable()
        }
    }

    pub fn arch(mut self, arch: Arch) -> Self {
        self.arch = arch;
        self
    }

    pub fn platform(mut self, platform: u32) -> Self {
        self.platform = platform;
        self
    }

    pub fn minos(mut self, v: Version) -> Self {
        self.minos = v;
        self
    }

    pub fn sdk(mut self, v: Version) -> Self {
        self.sdk = v;
        self
    }

    pub fn uuid(mut self, uuid: [u8; 16]) -> Self {
        self.uuid = uuid;
        self
    }

    pub fn rpath(mut self, rpath: &str) -> Self {
        self.rpaths.push(rpath.into());
        self
    }

    pub fn dylib_dependency(mut self, name: &str, weak: bool) -> Self {
        self.dylibs.push((name.into(), weak));
        self
    }

    /// Adds `LC_ENCRYPTION_INFO_64` (cryptid 0), as lld writes for iOS executables.
    pub fn encryption_info(mut self) -> Self {
        self.encryption = true;
        self
    }

    pub fn symbol(mut self, name: &str) -> Self {
        self.symbols.push(name.into());
        self
    }

    /// Places the string table at an offset that is 4 but not 8-byte aligned (what pre-LLVM-23 strip tools
    /// produce), so iOS 27's dyld would reject the image.
    pub fn misaligned_string_pool(mut self) -> Self {
        self.misaligned_string_pool = true;
        self
    }

    /// Adds an ad-hoc, linker-signed code signature (one SHA-256 code directory, flags `CS_ADHOC |
    /// CS_LINKER_SIGNED`, 4 KiB pages), as lld writes for arm64 Darwin targets.
    pub fn linker_signed(mut self, identifier: &str) -> Self {
        self.linker_signed = Some(identifier.into());
        self
    }

    /// The size of the `__text` section (default 64 bytes; at most 0x3000).
    pub fn text_size(mut self, n: u64) -> Self {
        self.text_size = n.clamp(4, PAGE - TEXT_SECTION_OFFSET);
        self
    }

    /// Writes the image.
    pub fn build(&self) -> Vec<u8> {
        let exec = self.filetype == MH_EXECUTE;
        let base: u64 = if exec { 0x1_0000_0000 } else { 0 };

        // __LINKEDIT content: nlist_64 entries, then the string table.
        let mut strtab = vec![0u8];
        let mut nlist = Vec::new();
        for name in &self.symbols {
            let strx = strtab.len() as u32;
            strtab.extend_from_slice(name.as_bytes());
            strtab.push(0);
            nlist.extend_from_slice(&strx.to_le_bytes());
            nlist.push(0x0f); // N_SECT | N_EXT
            nlist.push(1); // section 1: __text
            nlist.extend_from_slice(&0u16.to_le_bytes());
            nlist.extend_from_slice(&(base + TEXT_SECTION_OFFSET).to_le_bytes());
        }
        while strtab.len() % 8 != 0 {
            strtab.push(0);
        }
        let symoff = PAGE;
        let mut stroff = symoff + nlist.len() as u64;
        let mut linkedit = nlist.clone();
        if self.misaligned_string_pool {
            stroff += 4;
            linkedit.extend_from_slice(&[0; 4]);
        }
        linkedit.extend_from_slice(&strtab);
        // The linker signature, if any, starts 16-byte aligned after the string table.
        let sig_layout = self.linker_signed.as_ref().map(|ident| {
            while linkedit.len() % 16 != 0 {
                linkedit.push(0);
            }
            let sig_off = PAGE + linkedit.len() as u64;
            let n_pages = sig_off.div_ceil(4096);
            let ident_len = ident.len() as u64 + 1;
            let cd_len = 88 + ident_len + n_pages * 32;
            let sb_len = (20 + cd_len).div_ceil(16) * 16;
            (ident.clone(), sig_off, n_pages, cd_len, sb_len)
        });
        if let Some((_, _, _, _, sb_len)) = &sig_layout {
            linkedit.resize(linkedit.len() + *sb_len as usize, 0);
        }
        let linkedit_size = linkedit.len() as u64;

        let mut cmds: Vec<Vec<u8>> = Vec::new();
        if exec {
            cmds.push(segment("__PAGEZERO", 0, 0x1_0000_0000, 0, 0, 0, &[]));
        }
        let text_sect = section("__text", "__TEXT", base + TEXT_SECTION_OFFSET, self.text_size, TEXT_SECTION_OFFSET);
        cmds.push(segment("__TEXT", base, PAGE, 0, PAGE, 5, &[text_sect]));
        cmds.push(segment("__LINKEDIT", base + PAGE, linkedit_size.div_ceil(PAGE) * PAGE, PAGE, linkedit_size, 1, &[]));
        let nsyms = self.symbols.len() as u32;
        cmds.push(words(&[LC_SYMTAB, 24, symoff as u32, nsyms, stroff as u32, strtab.len() as u32]));
        let mut dysym = vec![LC_DYSYMTAB, 80, 0, 0, 0, nsyms, nsyms, 0];
        dysym.extend_from_slice(&[0; 12]);
        cmds.push(words(&dysym));
        if exec {
            cmds.push(string_cmd(0xe, 12, "/usr/lib/dyld", &[]));
        }
        let mut bv = words(&[LC_BUILD_VERSION, 32, self.platform, self.minos.packed(), self.sdk.packed(), 1]);
        bv.extend_from_slice(&TOOL_LD.to_le_bytes());
        bv.extend_from_slice(&Version::new(1100, 0, 0).packed().to_le_bytes());
        cmds.push(bv);
        let mut uuid = words(&[LC_UUID, 24]);
        uuid.extend_from_slice(&self.uuid);
        cmds.push(uuid);
        if exec {
            let mut main = words(&[LC_MAIN, 24]);
            main.extend_from_slice(&TEXT_SECTION_OFFSET.to_le_bytes());
            main.extend_from_slice(&0u64.to_le_bytes());
            cmds.push(main);
        } else {
            cmds.push(string_cmd(LC_ID_DYLIB, 24, &self.install_name, &[2, 0x0001_0000, 0x0001_0000]));
        }
        for (name, weak) in &self.dylibs {
            let cmd = if *weak { LC_LOAD_WEAK_DYLIB } else { LC_LOAD_DYLIB };
            cmds.push(string_cmd(cmd, 24, name, &[2, 0x0001_0000, 0x0001_0000]));
        }
        for rpath in &self.rpaths {
            cmds.push(string_cmd(LC_RPATH, 12, rpath, &[]));
        }
        if self.encryption {
            cmds.push(words(&[LC_ENCRYPTION_INFO_64, 24, PAGE as u32, 0, 0, 0]));
        }
        if let Some((_, sig_off, _, _, sb_len)) = &sig_layout {
            cmds.push(words(&[LC_CODE_SIGNATURE, 16, *sig_off as u32, *sb_len as u32]));
        }

        let sizeofcmds: usize = cmds.iter().map(Vec::len).sum();
        let mut out = Vec::with_capacity((PAGE + linkedit_size) as usize);
        let flags = if exec { 0x0020_0085 } else { 0x0010_0085 }; // NOUNDEFS|DYLDLINK|TWOLEVEL + PIE / APP_EXT_SAFE
        out.extend_from_slice(&words(&[
            MH_MAGIC_64,
            self.arch.cputype(),
            self.arch.cpusubtype(),
            self.filetype,
            cmds.len() as u32,
            sizeofcmds as u32,
            flags,
            0,
        ]));
        for c in &cmds {
            out.extend_from_slice(c);
        }
        assert!(out.len() as u64 <= TEXT_SECTION_OFFSET, "fixture load commands overflow the header page");
        out.resize(TEXT_SECTION_OFFSET as usize, 0);
        // `__text`: NOPs ending in RET (arm64 encodings; harmless bytes elsewhere).
        for i in 0..self.text_size / 4 {
            let insn: u32 = if i + 1 == self.text_size / 4 { 0xd65f_03c0 } else { 0xd503_201f };
            out.extend_from_slice(&insn.to_le_bytes());
        }
        out.resize(PAGE as usize, 0);
        out.extend_from_slice(&linkedit);
        if let Some((ident, sig_off, n_pages, cd_len, sb_len)) = sig_layout {
            let sig_off = sig_off as usize;
            let mut cd = Vec::with_capacity(cd_len as usize);
            let hash_offset = 88 + ident.len() as u32 + 1;
            for w in [
                CSMAGIC_CODEDIRECTORY,
                cd_len as u32,
                CS_SUPPORTSEXECSEG,
                CS_ADHOC | CS_LINKER_SIGNED,
                hash_offset,
                88,
                0,
                n_pages as u32,
                sig_off as u32,
            ] {
                cd.extend_from_slice(&w.to_be_bytes());
            }
            cd.extend_from_slice(&[32, CS_HASHTYPE_SHA256, 0, 12]);
            cd.extend_from_slice(&[0; 16]); // spare2, scatterOffset, teamOffset, spare3
            cd.extend_from_slice(&0u64.to_be_bytes()); // codeLimit64
            let exec_flags = if exec { CS_EXECSEG_MAIN_BINARY } else { 0 };
            for v in [0u64, PAGE, exec_flags] {
                cd.extend_from_slice(&v.to_be_bytes());
            }
            cd.extend_from_slice(ident.as_bytes());
            cd.push(0);
            for page in out[..sig_off].chunks(4096) {
                use sha2::Digest as _;
                cd.extend_from_slice(&sha2::Sha256::digest(page));
            }
            let mut sb = Vec::with_capacity(sb_len as usize);
            for w in [CSMAGIC_EMBEDDED_SIGNATURE, sb_len as u32, 1, CSSLOT_CODEDIRECTORY, 20] {
                sb.extend_from_slice(&w.to_be_bytes());
            }
            sb.extend_from_slice(&cd);
            sb.resize(sb_len as usize, 0);
            out[sig_off..sig_off + sb.len()].copy_from_slice(&sb);
        }
        out
    }
}

/// Builds a fat file from thin images (16 KiB slice alignment, as `lipo` does for arm64).
pub fn fat(slices: &[Vec<u8>]) -> Vec<u8> {
    let header = 8 + slices.len() * 20;
    let mut cur = (header as u64).div_ceil(PAGE) * PAGE;
    let mut out = vec![0u8; header];
    out[0..4].copy_from_slice(&FAT_MAGIC.to_be_bytes());
    out[4..8].copy_from_slice(&(slices.len() as u32).to_be_bytes());
    let mut placed = Vec::new();
    for (i, s) in slices.iter().enumerate() {
        let cputype = u32::from_le_bytes([s[4], s[5], s[6], s[7]]);
        let cpusubtype = u32::from_le_bytes([s[8], s[9], s[10], s[11]]);
        let base = 8 + i * 20;
        out[base..base + 4].copy_from_slice(&cputype.to_be_bytes());
        out[base + 4..base + 8].copy_from_slice(&cpusubtype.to_be_bytes());
        out[base + 8..base + 12].copy_from_slice(&(cur as u32).to_be_bytes());
        out[base + 12..base + 16].copy_from_slice(&(s.len() as u32).to_be_bytes());
        out[base + 16..base + 20].copy_from_slice(&14u32.to_be_bytes());
        placed.push(cur);
        cur = (cur + s.len() as u64).div_ceil(PAGE) * PAGE;
    }
    for (s, off) in slices.iter().zip(placed) {
        out.resize(off as usize, 0);
        out.extend_from_slice(s);
    }
    out
}

fn words(w: &[u32]) -> Vec<u8> {
    w.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// A load command with a trailing NUL-terminated string at `str_off`, padded to 8 bytes.
fn string_cmd(cmd: u32, str_off: u32, s: &str, extra: &[u32]) -> Vec<u8> {
    let raw = s.len() + 1 + str_off as usize;
    let size = raw.div_ceil(8) * 8;
    let mut v = words(&[cmd, size as u32, str_off]);
    v.extend(extra.iter().flat_map(|x| x.to_le_bytes()));
    v.resize(str_off as usize, 0);
    v.extend_from_slice(s.as_bytes());
    v.resize(size, 0);
    v
}

fn name16(s: &str) -> [u8; 16] {
    let mut n = [0u8; 16];
    n[..s.len().min(16)].copy_from_slice(&s.as_bytes()[..s.len().min(16)]);
    n
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
    let mut v = words(&[LC_SEGMENT_64, (72 + 80 * sections.len()) as u32]);
    v.extend_from_slice(&name16(name));
    for x in [vmaddr, vmsize, fileoff, filesize] {
        v.extend_from_slice(&x.to_le_bytes());
    }
    v.extend_from_slice(&words(&[prot, prot, sections.len() as u32, 0]));
    for s in sections {
        v.extend_from_slice(s);
    }
    v
}

fn section(sect: &str, seg: &str, addr: u64, size: u64, offset: u64) -> Vec<u8> {
    let mut v = Vec::with_capacity(80);
    v.extend_from_slice(&name16(sect));
    v.extend_from_slice(&name16(seg));
    v.extend_from_slice(&addr.to_le_bytes());
    v.extend_from_slice(&size.to_le_bytes());
    // offset, align (2^2), reloff, nreloc, flags (PURE_INSTRUCTIONS | SOME_INSTRUCTIONS), reserved1..3
    v.extend_from_slice(&words(&[offset as u32, 2, 0, 0, 0x8000_0400, 0, 0, 0]));
    v
}
