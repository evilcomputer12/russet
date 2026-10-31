// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! A clean-room ELF64 loader for Android shared objects (SIGN §7.5), written from the ELF gABI and the x86-64
//! psABI: map the `PT_LOAD` segments, apply `DT_RELA`/`DT_JMPREL`/`DT_RELR` relocations
//! (`R_X86_64_RELATIVE/GLOB_DAT/JUMP_SLOT/64/IRELATIVE`), resolve imports through the caller, set each page's
//! protection, run `DT_INIT`/`DT_INIT_ARRAY`, and look exports up by name. Metadata is read from the file image
//! (translated through the `PT_LOAD` table); only relocations and init calls touch the mapped memory.
//!
//! Not supported (a clear error, never a crash): other machines, TLS relocations and Android's packed
//! relocations (`DT_ANDROID_REL[A]`). `russet-adi-host` loads only what Apple ships in the Apple Music APK.

#![allow(unsafe_code)] // mapping and relocating native code is what this module is for; each block says why it holds

use std::collections::HashMap;

const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PF_R: u32 = 4;

const DT_NULL: i64 = 0;
const DT_PLTRELSZ: i64 = 2;
const DT_HASH: i64 = 4;
const DT_STRTAB: i64 = 5;
const DT_SYMTAB: i64 = 6;
const DT_RELA: i64 = 7;
const DT_RELASZ: i64 = 8;
const DT_STRSZ: i64 = 10;
const DT_INIT: i64 = 12;
const DT_REL: i64 = 17;
const DT_JMPREL: i64 = 23;
const DT_INIT_ARRAY: i64 = 25;
const DT_INIT_ARRAYSZ: i64 = 27;
const DT_RELRSZ: i64 = 35;
const DT_RELR: i64 = 36;
const DT_ANDROID_REL: i64 = 0x6000_000f;
const DT_ANDROID_RELA: i64 = 0x6000_0011;
const DT_ANDROID_RELR: i64 = 0x6fff_e000;
const DT_ANDROID_RELRSZ: i64 = 0x6fff_e001;
const DT_GNU_HASH: i64 = 0x6fff_fef5;

const R_X86_64_64: u32 = 1;
const R_X86_64_GLOB_DAT: u32 = 6;
const R_X86_64_JUMP_SLOT: u32 = 7;
const R_X86_64_RELATIVE: u32 = 8;
const R_X86_64_IRELATIVE: u32 = 37;

const STB_WEAK: u8 = 2;
const STT_GNU_IFUNC: u8 = 10;

const PAGE: usize = 4096;

/// A loaded library.
#[derive(Debug)]
pub struct Image {
    pub name: String,
    exports: HashMap<String, usize>,
}

impl Image {
    /// The address of export `name`.
    pub fn symbol(&self, name: &str) -> Option<usize> {
        self.exports.get(name).copied()
    }
}

/// How the loader asks for an import: `(name, weak)` → address (0 for an absent weak symbol).
pub type Resolver<'a> = &'a mut dyn FnMut(&str, bool) -> usize;

struct File<'a> {
    b: &'a [u8],
    loads: Vec<Phdr>,
}

#[derive(Debug, Clone, Copy)]
struct Phdr {
    kind: u32,
    flags: u32,
    offset: u64,
    vaddr: u64,
    filesz: u64,
    memsz: u64,
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

impl File<'_> {
    /// The file offset of virtual address `v`.
    fn off(&self, v: u64) -> Option<usize> {
        let p = self.loads.iter().find(|p| v >= p.vaddr && v < p.vaddr + p.filesz)?;
        usize::try_from(p.offset + (v - p.vaddr)).ok()
    }

    fn u32v(&self, v: u64) -> Option<u32> {
        u32_at(self.b, self.off(v)?)
    }

    fn u64v(&self, v: u64) -> Option<u64> {
        u64_at(self.b, self.off(v)?)
    }

    fn cstr(&self, v: u64) -> Option<String> {
        let start = self.off(v)?;
        let rest = self.b.get(start..)?;
        let end = rest.iter().position(|c| *c == 0)?;
        String::from_utf8(rest[..end].to_vec()).ok()
    }
}

fn page_floor(x: usize) -> usize {
    x & !(PAGE - 1)
}

fn page_ceil(x: usize) -> usize {
    (x + PAGE - 1) & !(PAGE - 1)
}

/// Loads `bytes` (an ELF shared object named `name`), resolving imports with `resolve`.
pub fn load(name: &str, bytes: &[u8], resolve: Resolver<'_>) -> Result<Image, String> {
    if bytes.get(..4) != Some(b"\x7fELF") || bytes.get(4) != Some(&2) || bytes.get(5) != Some(&1) {
        return Err(format!("{name} is not a little-endian ELF64 file"));
    }
    if u16_at(bytes, 16) != Some(3) {
        return Err(format!("{name} is not a shared object"));
    }
    if !cfg!(target_arch = "x86_64") || u16_at(bytes, 18) != Some(62) {
        return Err(format!("{name} is not an x86_64 library, or this host is not x86_64"));
    }
    let phoff = u64_at(bytes, 32).ok_or("truncated header")? as usize;
    let phentsize = u16_at(bytes, 54).ok_or("truncated header")? as usize;
    let phnum = u16_at(bytes, 56).ok_or("truncated header")? as usize;
    let mut phdrs = Vec::new();
    for i in 0..phnum {
        let at = phoff + i * phentsize;
        phdrs.push(Phdr {
            kind: u32_at(bytes, at).ok_or("truncated program header")?,
            flags: u32_at(bytes, at + 4).ok_or("truncated program header")?,
            offset: u64_at(bytes, at + 8).ok_or("truncated program header")?,
            vaddr: u64_at(bytes, at + 16).ok_or("truncated program header")?,
            filesz: u64_at(bytes, at + 32).ok_or("truncated program header")?,
            memsz: u64_at(bytes, at + 40).ok_or("truncated program header")?,
        });
    }
    let loads: Vec<Phdr> = phdrs.iter().copied().filter(|p| p.kind == PT_LOAD).collect();
    if loads.is_empty() {
        return Err(format!("{name} has no loadable segment"));
    }
    for p in &loads {
        if p.offset.checked_add(p.filesz).is_none_or(|end| end > bytes.len() as u64) || p.filesz > p.memsz {
            return Err(format!("{name} has a segment outside the file"));
        }
    }
    let f = File { b: bytes, loads: loads.clone() };
    let lo = page_floor(loads.iter().map(|p| p.vaddr).min().unwrap_or(0) as usize);
    let hi = page_ceil(loads.iter().map(|p| p.vaddr + p.memsz).max().unwrap_or(0) as usize);
    let span = hi - lo;

    // SAFETY: an anonymous private mapping of `span` bytes; nothing else refers to it yet.
    let base = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            span,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    if base == libc::MAP_FAILED {
        return Err(format!("cannot map {span} bytes for {name}"));
    }
    let bias = base as usize - lo;
    for p in &loads {
        let src = &bytes[p.offset as usize..(p.offset + p.filesz) as usize];
        // SAFETY: the destination lies inside the mapping (lo <= vaddr, vaddr + filesz <= hi) and is writable.
        unsafe { std::ptr::copy_nonoverlapping(src.as_ptr(), (bias + p.vaddr as usize) as *mut u8, src.len()) };
    }

    // The dynamic section.
    let dynamic =
        phdrs.iter().find(|p| p.kind == PT_DYNAMIC).ok_or_else(|| format!("{name} has no dynamic section"))?;
    let mut dynv: HashMap<i64, u64> = HashMap::new();
    let mut at = dynamic.offset as usize;
    loop {
        let tag = u64_at(bytes, at).ok_or("truncated dynamic section")? as i64;
        let val = u64_at(bytes, at + 8).ok_or("truncated dynamic section")?;
        if tag == DT_NULL {
            break;
        }
        dynv.entry(tag).or_insert(val);
        at += 16;
    }
    if dynv.contains_key(&DT_ANDROID_REL) || dynv.contains_key(&DT_ANDROID_RELA) || dynv.contains_key(&DT_REL) {
        return Err(format!("{name} uses packed or REL relocations, which this loader does not read"));
    }
    let symtab = *dynv.get(&DT_SYMTAB).ok_or("no DT_SYMTAB")?;
    let strtab = *dynv.get(&DT_STRTAB).ok_or("no DT_STRTAB")?;
    let _strsz = dynv.get(&DT_STRSZ).copied().unwrap_or(0);
    let nsyms = symbol_count(&f, &dynv).ok_or_else(|| format!("{name} has neither DT_HASH nor DT_GNU_HASH"))?;

    struct Sym {
        name: String,
        info: u8,
        shndx: u16,
        value: u64,
    }
    let sym = |i: u32| -> Option<Sym> {
        let v = symtab + u64::from(i) * 24;
        let off = f.off(v)?;
        Some(Sym {
            name: f.cstr(strtab + u64::from(u32_at(bytes, off)?))?,
            info: *bytes.get(off + 4)?,
            shndx: u16_at(bytes, off + 6)?,
            value: u64_at(bytes, off + 8)?,
        })
    };

    // Relocations.
    let mut cache: HashMap<u32, usize> = HashMap::new();
    let mut apply = |table: u64, size: u64| -> Result<(), String> {
        let mut off = 0;
        while off + 24 <= size {
            let r_offset = f.u64v(table + off).ok_or("truncated relocation")?;
            let r_info = f.u64v(table + off + 8).ok_or("truncated relocation")?;
            let r_addend = f.u64v(table + off + 16).ok_or("truncated relocation")? as i64;
            off += 24;
            let kind = (r_info & 0xffff_ffff) as u32;
            let index = (r_info >> 32) as u32;
            let where_ = (bias + r_offset as usize) as *mut u64;
            let value = match kind {
                0 => continue,
                R_X86_64_RELATIVE => (bias as i64 + r_addend) as u64,
                R_X86_64_IRELATIVE => {
                    // SAFETY: the resolver is a function of the library, already relocated before IRELATIVE
                    // entries (they come last); it takes no arguments and returns an address.
                    let f: extern "C" fn() -> usize = unsafe { std::mem::transmute((bias as i64 + r_addend) as usize) };
                    f() as u64
                }
                R_X86_64_64 | R_X86_64_GLOB_DAT | R_X86_64_JUMP_SLOT => {
                    let s = match cache.get(&index) {
                        Some(a) => *a,
                        None => {
                            let s = sym(index).ok_or("a relocation names a missing symbol")?;
                            let a = if s.shndx != 0 && s.info & 0xf != STT_GNU_IFUNC {
                                bias + s.value as usize
                            } else {
                                resolve(&s.name, s.info >> 4 == STB_WEAK)
                            };
                            cache.insert(index, a);
                            a
                        }
                    };
                    if kind == R_X86_64_64 { (s as i64 + r_addend) as u64 } else { s as u64 }
                }
                other => return Err(format!("{name} needs relocation type {other}, which this loader does not apply")),
            };
            // SAFETY: r_offset is inside the image (checked against the span), 8-byte slots per the psABI.
            if r_offset as usize + 8 > hi || (r_offset as usize) < lo {
                return Err(format!("{name} relocates outside its image"));
            }
            unsafe { where_.write_unaligned(value) };
        }
        Ok(())
    };
    if let (Some(t), Some(s)) = (dynv.get(&DT_RELA), dynv.get(&DT_RELASZ)) {
        apply(*t, *s)?;
    }
    if let (Some(t), Some(s)) = (dynv.get(&DT_JMPREL), dynv.get(&DT_PLTRELSZ)) {
        apply(*t, *s)?;
    }
    let relr =
        dynv.get(&DT_RELR).zip(dynv.get(&DT_RELRSZ)).or(dynv.get(&DT_ANDROID_RELR).zip(dynv.get(&DT_ANDROID_RELRSZ)));
    if let Some((t, s)) = relr {
        let mut addr = 0usize;
        let mut i = 0;
        while i + 8 <= *s {
            let e = f.u64v(t + i).ok_or("truncated DT_RELR")?;
            i += 8;
            if e & 1 == 0 {
                addr = bias + e as usize;
                // SAFETY: a relative slot of the image, as DT_RELR names it.
                unsafe { (addr as *mut usize).write_unaligned((addr as *const usize).read_unaligned() + bias) };
                addr += 8;
            } else {
                for bit in 1..64 {
                    if (e >> bit) & 1 == 1 {
                        let slot = addr + (bit - 1) * 8;
                        // SAFETY: as above, for each bit of the bitmap entry.
                        unsafe { (slot as *mut usize).write_unaligned((slot as *const usize).read_unaligned() + bias) };
                    }
                }
                addr += 63 * 8;
            }
        }
    }

    // Page protections: the union of the flags of every segment touching a page.
    let mut prot = vec![0u32; span / PAGE];
    for p in &loads {
        let start = page_floor(p.vaddr as usize) - lo;
        let end = page_ceil((p.vaddr + p.memsz) as usize) - lo;
        for page in prot.iter_mut().take(end / PAGE).skip(start / PAGE) {
            *page |= p.flags;
        }
    }
    for (i, flags) in prot.iter().enumerate() {
        let mut pr = 0;
        if flags & PF_R != 0 {
            pr |= libc::PROT_READ;
        }
        if flags & PF_W != 0 {
            pr |= libc::PROT_WRITE;
        }
        if flags & PF_X != 0 {
            pr |= libc::PROT_EXEC;
        }
        // SAFETY: a page of our own mapping.
        unsafe { libc::mprotect((base as usize + i * PAGE) as *mut libc::c_void, PAGE, pr) };
    }

    // Exports.
    let mut exports = HashMap::new();
    for i in 1..nsyms {
        if let Some(s) = sym(i)
            && s.shndx != 0
            && !s.name.is_empty()
            && matches!(s.info >> 4, 1 | 2 | 10)
        {
            exports.insert(s.name, bias + s.value as usize);
        }
    }

    // Initializers, with bionic's (argc, argv, envp).
    static EMPTY: [usize; 1] = [0];
    let argv = EMPTY.as_ptr() as *const *const libc::c_char;
    let mut inits = Vec::new();
    if let Some(i) = dynv.get(&DT_INIT) {
        inits.push(bias + *i as usize);
    }
    if let (Some(a), Some(n)) = (dynv.get(&DT_INIT_ARRAY), dynv.get(&DT_INIT_ARRAYSZ)) {
        for k in 0..(*n as usize / 8) {
            // SAFETY: DT_INIT_ARRAY lies in the relocated image.
            inits.push(unsafe { ((bias + *a as usize + k * 8) as *const usize).read_unaligned() });
        }
    }
    for f in inits.into_iter().filter(|a| *a != 0 && *a != usize::MAX) {
        // SAFETY: an initializer of the library with the ELF/bionic init signature.
        let init: extern "C" fn(i32, *const *const libc::c_char, *const *const libc::c_char) =
            unsafe { std::mem::transmute(f) };
        init(0, argv, argv);
    }
    Ok(Image { name: name.to_owned(), exports })
}

/// The number of dynamic symbols, from `DT_HASH` (nchain) or `DT_GNU_HASH` (the highest chain end).
fn symbol_count(f: &File<'_>, dynv: &HashMap<i64, u64>) -> Option<u32> {
    if let Some(h) = dynv.get(&DT_HASH) {
        return f.u32v(h + 4);
    }
    let h = *dynv.get(&DT_GNU_HASH)?;
    let nbuckets = f.u32v(h)?;
    let symoffset = f.u32v(h + 4)?;
    let bloom = f.u32v(h + 8)?;
    let buckets = h + 16 + u64::from(bloom) * 8;
    let chains = buckets + u64::from(nbuckets) * 4;
    let mut max = 0;
    for i in 0..nbuckets {
        max = max.max(f.u32v(buckets + u64::from(i) * 4)?);
    }
    if max < symoffset {
        return Some(symoffset);
    }
    let mut i = max;
    loop {
        let c = f.u32v(chains + u64::from(i - symoffset) * 4)?;
        i += 1;
        if c & 1 == 1 {
            return Some(i);
        }
    }
}
