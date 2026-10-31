// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Mach-O parsing and in-place edits (ARCHITECTURE §5.3 `russet-macho`, the union of build §4.7 and SIGN §8.5)
//! [D 20]: thin and fat files, `LC_BUILD_VERSION`, rpaths, load commands, UUID, the iOS 27 `LC_SYMTAB`
//! string-pool alignment rule, code-signature blobs, and a last-resort SDK patch for foreign binaries.
//!
//! Layer F: no `russet-*` dependency. Every read is bounds-checked, so malformed input gives
//! [`MachOError::Malformed`] and never a panic. Little-endian images only (every Apple arm64 and x86_64 binary);
//! fat headers and code signatures are big-endian, as on disk.
//!
//! The `fixture` feature adds [`fixture::MachOBuilder`], which writes small synthetic Mach-O images for tests
//! (ARCHITECTURE §19: Mach-O fixtures are built by us at test time, never committed).

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::fmt;
use std::path::Path;
use std::str::FromStr;

pub use uuid::Uuid;

mod codesig;
pub mod consts;
mod edit;
#[cfg(feature = "fixture")]
pub mod fixture;
mod read;

pub use codesig::{BlobIndex, CodeDirectory, CodeSignature, HashType};
pub use edit::{
    align_string_pool, align_string_pool_bytes, delete_rpath, delete_rpath_bytes, patch_sdk_version,
    patch_sdk_version_bytes, thin, thin_bytes,
};

use consts::*;
use read::{bytes, cstr, fixed_name, u32_be, u32_le, u64_be, u64_le, usize_of};

/// Errors of this crate (layer F has no dependency on `russet-core`).
#[derive(Debug, thiserror::Error)]
pub enum MachOError {
    #[error("malformed Mach-O: {0}")]
    Malformed(String),
    #[error("unsupported Mach-O: {0}")]
    Unsupported(String),
    /// What an edit asked for is not in the file (an rpath, an architecture, a version command).
    #[error("not found in the Mach-O: {0}")]
    NotFound(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// A version as Mach-O encodes it (`xxxx.yy.zz` in `LC_BUILD_VERSION`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Version {
    pub major: u16,
    pub minor: u8,
    pub patch: u8,
}

impl Version {
    pub const fn new(major: u16, minor: u8, patch: u8) -> Self {
        Self { major, minor, patch }
    }

    /// Decodes the packed `xxxx.yy.zz` form.
    pub const fn from_packed(v: u32) -> Self {
        Self { major: (v >> 16) as u16, minor: ((v >> 8) & 0xff) as u8, patch: (v & 0xff) as u8 }
    }

    /// The packed `xxxx.yy.zz` form.
    pub const fn packed(self) -> u32 {
        ((self.major as u32) << 16) | ((self.minor as u32) << 8) | self.patch as u32
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.patch == 0 {
            write!(f, "{}.{}", self.major, self.minor)
        } else {
            write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
        }
    }
}

impl FromStr for Version {
    type Err = MachOError;

    /// Parses `27`, `27.0` or `17.0.1`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bad = || MachOError::Malformed(format!("invalid version `{s}`"));
        let mut parts = s.trim().split('.');
        let major = parts.next().ok_or_else(bad)?.parse::<u16>().map_err(|_| bad())?;
        let minor = parts.next().map(str::parse::<u8>).transpose().map_err(|_| bad())?.unwrap_or(0);
        let patch = parts.next().map(str::parse::<u8>).transpose().map_err(|_| bad())?.unwrap_or(0);
        if parts.next().is_some() {
            return Err(bad());
        }
        Ok(Self { major, minor, patch })
    }
}

/// CPU architecture of a slice: the three ARCHITECTURE §5.3 names (F0). Slices of any other CPU (`armv7`,
/// `arm64_32`, `i386`, …) are described by [`Cpu`] and listed by [`MachO::cpu_slices`], so matching on `Arch`
/// stays exhaustive with these three arms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Arch {
    Arm64,
    Arm64e,
    X86_64,
}

impl Arch {
    /// Classifies a CPU type and subtype; `None` for a CPU that is none of the three.
    pub fn from_cpu(cputype: u32, cpusubtype: u32) -> Option<Self> {
        Cpu { cputype, cpusubtype }.arch()
    }

    /// The CPU type.
    pub fn cputype(self) -> u32 {
        match self {
            Self::Arm64 | Self::Arm64e => CPU_TYPE_ARM64,
            Self::X86_64 => CPU_TYPE_X86_64,
        }
    }

    /// The CPU subtype.
    pub fn cpusubtype(self) -> u32 {
        match self {
            Self::Arm64 => CPU_SUBTYPE_ARM64_ALL,
            Self::Arm64e => CPU_SUBTYPE_ARM64E,
            Self::X86_64 => CPU_SUBTYPE_X86_64_ALL,
        }
    }

    /// `arm64`, `arm64e` or `x86_64`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Arm64 => "arm64",
            Self::Arm64e => "arm64e",
            Self::X86_64 => "x86_64",
        }
    }
}

impl fmt::Display for Arch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A CPU type and subtype as a Mach-O or fat header records them: any CPU, including those [`Arch`] does not
/// name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Cpu {
    pub cputype: u32,
    pub cpusubtype: u32,
}

impl Cpu {
    /// The [`Arch`] of this CPU, when it is one of the three.
    pub fn arch(self) -> Option<Arch> {
        match (self.cputype, self.cpusubtype & CPU_SUBTYPE_MASK) {
            (CPU_TYPE_ARM64, CPU_SUBTYPE_ARM64E) => Some(Arch::Arm64e),
            (CPU_TYPE_ARM64, _) => Some(Arch::Arm64),
            (CPU_TYPE_X86_64, _) => Some(Arch::X86_64),
            _ => None,
        }
    }

    /// `arm64`, `arm64e`, `x86_64`, the usual names of a few others (`armv7`, `armv7s`, `arm64_32`, `i386`), or
    /// `cpu-<type>-<subtype>`.
    pub fn name(self) -> String {
        if let Some(a) = self.arch() {
            return a.name().to_owned();
        }
        match (self.cputype, self.cpusubtype & CPU_SUBTYPE_MASK) {
            (CPU_TYPE_ARM, 9) => "armv7".into(),
            (CPU_TYPE_ARM, 11) => "armv7s".into(),
            (CPU_TYPE_ARM, 12) => "armv7k".into(),
            (CPU_TYPE_ARM64_32, _) => "arm64_32".into(),
            (CPU_TYPE_X86, _) => "i386".into(),
            (t, s) => format!("cpu-{t:#x}-{s:#x}"),
        }
    }
}

impl From<Arch> for Cpu {
    fn from(a: Arch) -> Self {
        Self { cputype: a.cputype(), cpusubtype: a.cpusubtype() }
    }
}

impl fmt::Display for Cpu {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name())
    }
}

impl FromStr for Arch {
    type Err = MachOError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "arm64" => Ok(Self::Arm64),
            "arm64e" => Ok(Self::Arm64e),
            "x86_64" => Ok(Self::X86_64),
            other => Err(MachOError::Unsupported(format!("architecture `{other}`"))),
        }
    }
}

/// One architecture slice of a thin or fat file, for the CPUs [`Arch`] names ([`MachO::slices`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slice {
    pub arch: Arch,
    /// Offset of the slice in the file (0 for a thin file).
    pub offset: u64,
    pub size: u64,
}

/// One slice of any CPU ([`MachO::cpu_slices`]); index `i` describes [`MachO::images`]`[i]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CpuSlice {
    pub cpu: Cpu,
    /// Offset of the slice in the file (0 for a thin file).
    pub offset: u64,
    pub size: u64,
}

/// `LC_BUILD_VERSION`: platform, minimum OS, SDK and tool versions. Files with only an `LC_VERSION_MIN_*`
/// command report its platform, version and SDK here, with no tools.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildVersion {
    /// `PLATFORM_*` (2 = iOS).
    pub platform: u32,
    pub minos: Version,
    pub sdk: Version,
    /// `(TOOL_*, version)`.
    pub tools: Vec<(u32, Version)>,
}

/// A dylib load command (`LC_LOAD_DYLIB` and friends).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DylibRef {
    pub name: String,
    /// `LC_LOAD_WEAK_DYLIB`.
    pub weak: bool,
}

/// `LC_ENCRYPTION_INFO_64` (or the 32-bit `LC_ENCRYPTION_INFO`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncInfo {
    pub cryptoff: u32,
    pub cryptsize: u32,
    pub cryptid: u32,
}

/// `LC_SYMTAB`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Symtab {
    pub symoff: u32,
    pub nsyms: u32,
    pub stroff: u32,
    pub strsize: u32,
}

/// One segment load command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub name: String,
    pub vmaddr: u64,
    pub vmsize: u64,
    pub fileoff: u64,
    pub filesize: u64,
    pub maxprot: u32,
    pub initprot: u32,
    pub nsects: u32,
}

/// One parsed thin image (the whole file, or one slice of a fat file).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    /// The header's CPU type and subtype ([`Image::arch`] classifies it).
    pub cpu: Cpu,
    pub is_64: bool,
    /// `MH_EXECUTE`, `MH_DYLIB`, …
    pub filetype: u32,
    pub flags: u32,
    pub ncmds: u32,
    pub sizeofcmds: u32,
    pub build_version: Option<BuildVersion>,
    pub uuid: Option<Uuid>,
    pub rpaths: Vec<String>,
    pub dylibs: Vec<DylibRef>,
    /// `LC_ID_DYLIB`.
    pub install_name: Option<String>,
    pub encryption_info: Option<EncInfo>,
    pub symtab: Option<Symtab>,
    pub segments: Vec<Segment>,
    /// `LC_CODE_SIGNATURE` `(dataoff, datasize)`.
    pub code_signature_range: Option<(u32, u32)>,
    pub code_signature: Option<CodeSignature>,
    /// Why the code signature could not be parsed, when it could not.
    pub code_signature_error: Option<String>,
    /// Free bytes between the end of the load commands and the first section's file data.
    pub header_padding: u64,
}

impl Image {
    /// The image's [`Arch`], when its CPU is one of the three.
    pub fn arch(&self) -> Option<Arch> {
        self.cpu.arch()
    }

    /// Whether this is an `MH_EXECUTE` (an app or app-extension main binary).
    pub fn is_executable(&self) -> bool {
        self.filetype == MH_EXECUTE
    }

    /// The largest power of two (at most 4096) dividing the string table's offset; 4096 when there is no
    /// string table. iOS 27's dyld rejects 64-bit images whose string pool is not 8-byte aligned.
    pub fn string_pool_alignment(&self) -> u32 {
        match self.symtab {
            Some(s) if s.strsize > 0 => alignment_of(u64::from(s.stroff)),
            _ => 4096,
        }
    }

    /// Like [`Self::string_pool_alignment`], for the symbol table (`symoff`).
    pub fn symbol_table_alignment(&self) -> u32 {
        match self.symtab {
            Some(s) if s.nsyms > 0 => alignment_of(u64::from(s.symoff)),
            _ => 4096,
        }
    }

    /// Whether `__LINKEDIT` is the last segment in the file.
    pub fn linkedit_is_last(&self) -> bool {
        let last = self.segments.iter().filter(|s| s.filesize > 0).max_by_key(|s| s.fileoff);
        last.is_some_and(|s| s.name == "__LINKEDIT")
    }

    /// The segment named `name`.
    pub fn segment(&self, name: &str) -> Option<&Segment> {
        self.segments.iter().find(|s| s.name == name)
    }
}

fn alignment_of(off: u64) -> u32 {
    if off == 0 {
        return 4096;
    }
    let tz = off.trailing_zeros().min(12);
    1u32 << tz
}

/// A parsed thin or fat Mach-O view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachO {
    fat: bool,
    /// The slices of the CPUs [`Arch`] names.
    slices: Vec<Slice>,
    /// Every slice, in file order; parallel to `images`.
    cpu_slices: Vec<CpuSlice>,
    images: Vec<Image>,
    primary: usize,
}

impl MachO {
    /// Parses a thin or fat Mach-O file held in memory.
    pub fn parse(bytes: &[u8]) -> Result<Self, MachOError> {
        let magic_be = u32_be(bytes, 0)?;
        if magic_be == FAT_MAGIC || magic_be == FAT_MAGIC_64 {
            return parse_fat(bytes, magic_be == FAT_MAGIC_64);
        }
        let image = parse_image(bytes)?;
        let cpu_slice = CpuSlice { cpu: image.cpu, offset: 0, size: bytes.len() as u64 };
        Ok(Self::assemble(false, vec![cpu_slice], vec![image]))
    }

    fn assemble(fat: bool, cpu_slices: Vec<CpuSlice>, images: Vec<Image>) -> Self {
        let slices = cpu_slices
            .iter()
            .filter_map(|s| Some(Slice { arch: s.cpu.arch()?, offset: s.offset, size: s.size }))
            .collect();
        let primary = images
            .iter()
            .position(|i| i.arch() == Some(Arch::Arm64))
            .or_else(|| images.iter().position(|i| i.arch() == Some(Arch::Arm64e)))
            .unwrap_or(0);
        Self { fat, slices, cpu_slices, images, primary }
    }

    /// Reads and parses a file.
    pub fn open(path: &Path) -> Result<Self, MachOError> {
        Self::parse(&std::fs::read(path)?)
    }

    /// Whether the file is fat (universal).
    pub fn is_fat(&self) -> bool {
        self.fat
    }

    /// The slices of the CPUs [`Arch`] names (arm64, arm64e, x86_64), in file order. [`Self::cpu_slices`] lists
    /// every slice.
    pub fn slices(&self) -> &[Slice] {
        &self.slices
    }

    /// Every slice, whatever its CPU, in file order; index `i` is [`Self::images`]`[i]`.
    pub fn cpu_slices(&self) -> &[CpuSlice] {
        &self.cpu_slices
    }

    /// Every parsed slice, in file order.
    pub fn images(&self) -> &[Image] {
        &self.images
    }

    /// The slice the single-valued accessors describe: arm64 if present, then arm64e, then the first.
    pub fn primary(&self) -> &Image {
        &self.images[self.primary]
    }

    /// The bytes of slice `index` of [`Self::cpu_slices`] (and [`Self::images`]) inside `file`, the data this
    /// view was parsed from.
    pub fn slice_bytes<'a>(&self, file: &'a [u8], index: usize) -> Option<&'a [u8]> {
        let s = self.cpu_slices.get(index)?;
        let start = usize::try_from(s.offset).ok()?;
        let end = start.checked_add(usize::try_from(s.size).ok()?)?;
        file.get(start..end)
    }

    pub fn filetype(&self) -> u32 {
        self.primary().filetype
    }

    pub fn build_version(&self) -> Option<BuildVersion> {
        self.primary().build_version.clone()
    }

    pub fn uuid(&self) -> Option<Uuid> {
        self.primary().uuid
    }

    pub fn rpaths(&self) -> Vec<String> {
        self.primary().rpaths.clone()
    }

    pub fn dylibs(&self) -> Vec<DylibRef> {
        self.primary().dylibs.clone()
    }

    pub fn encryption_info(&self) -> Option<EncInfo> {
        self.primary().encryption_info
    }

    /// Alignment of the `LC_SYMTAB` string pool (iOS 27 requires 8): the smallest over every slice.
    pub fn string_pool_alignment(&self) -> u32 {
        self.images.iter().map(Image::string_pool_alignment).min().unwrap_or(4096)
    }

    pub fn code_signature(&self) -> Option<CodeSignature> {
        self.primary().code_signature.clone()
    }
}

/// Whether `bytes` starts with a little-endian thin Mach-O magic or a plausible fat header.
pub fn is_macho(bytes: &[u8]) -> bool {
    let Ok(le) = u32_le(bytes, 0) else { return false };
    if le == MH_MAGIC || le == MH_MAGIC_64 {
        return true;
    }
    match (u32_be(bytes, 0), u32_be(bytes, 4)) {
        // 0xcafebabe is also the Java class-file magic; fat files have a small architecture count.
        (Ok(FAT_MAGIC | FAT_MAGIC_64), Ok(n)) => (1..=32).contains(&n),
        _ => false,
    }
}

/// Whether the file at `path` starts with a Mach-O magic (reads 8 bytes).
pub fn is_macho_file(path: &Path) -> std::io::Result<bool> {
    use std::io::Read as _;
    let mut f = std::fs::File::open(path)?;
    let mut head = [0u8; 8];
    let mut n = 0;
    while n < head.len() {
        let r = f.read(&mut head[n..])?;
        if r == 0 {
            break;
        }
        n += r;
    }
    Ok(is_macho(&head[..n]))
}

fn parse_fat(bytes: &[u8], is64: bool) -> Result<MachO, MachOError> {
    let n = u32_be(bytes, 4)? as usize;
    if n == 0 || n > 32 {
        return Err(MachOError::Malformed(format!("fat header lists {n} architectures")));
    }
    let entry = if is64 { 32 } else { 20 };
    let mut slices = Vec::with_capacity(n);
    let mut images = Vec::with_capacity(n);
    for i in 0..n {
        let base = 8 + i * entry;
        let cputype = u32_be(bytes, base)?;
        let cpusubtype = u32_be(bytes, base + 4)?;
        let (offset, size) = if is64 {
            (u64_be(bytes, base + 8)?, u64_be(bytes, base + 16)?)
        } else {
            (u64::from(u32_be(bytes, base + 8)?), u64::from(u32_be(bytes, base + 12)?))
        };
        let data = bytes_u64(bytes, offset, size, "fat slice")?;
        let image = parse_image(data)?;
        if image.cpu.cputype != cputype {
            return Err(MachOError::Malformed(format!(
                "fat entry {i} says CPU {cputype:#x} but its slice is {}",
                image.cpu
            )));
        }
        slices.push(CpuSlice { cpu: Cpu { cputype, cpusubtype }, offset, size });
        images.push(image);
    }
    Ok(MachO::assemble(true, slices, images))
}

fn bytes_u64<'a>(b: &'a [u8], off: u64, len: u64, what: &str) -> Result<&'a [u8], MachOError> {
    bytes(b, usize_of(off, what)?, usize_of(len, what)?, what)
}

/// The header fields every edit needs.
pub(crate) struct Header {
    pub is_64: bool,
    pub header_size: usize,
    pub ncmds: u32,
    pub sizeofcmds: u32,
}

pub(crate) fn parse_header(b: &[u8]) -> Result<Header, MachOError> {
    let magic = u32_le(b, 0)?;
    let (is_64, header_size) = match magic {
        MH_MAGIC_64 => (true, HEADER_SIZE_64),
        MH_MAGIC => (false, HEADER_SIZE_32),
        MH_CIGAM | MH_CIGAM_64 => return Err(MachOError::Unsupported("big-endian Mach-O".into())),
        other => return Err(MachOError::Malformed(format!("not a Mach-O (magic {other:#010x})"))),
    };
    let ncmds = u32_le(b, 16)?;
    let sizeofcmds = u32_le(b, 20)?;
    if header_size.saturating_add(sizeofcmds as usize) > b.len() {
        return Err(MachOError::Malformed(format!("load commands ({sizeofcmds:#x} bytes) exceed the file")));
    }
    Ok(Header { is_64, header_size, ncmds, sizeofcmds })
}

/// `(offset, cmd, cmdsize)` of every load command, bounds-checked against `sizeofcmds`.
pub(crate) fn load_commands(b: &[u8], h: &Header) -> Result<Vec<(usize, u32, usize)>, MachOError> {
    let end = h.header_size + h.sizeofcmds as usize;
    let mut out = Vec::with_capacity(h.ncmds.min(4096) as usize);
    let mut off = h.header_size;
    for i in 0..h.ncmds {
        let cmd = u32_le(b, off)?;
        let size = u32_le(b, off + 4)? as usize;
        if size < 8 || off + size > end {
            return Err(MachOError::Malformed(format!("load command {i} ({cmd:#x}) has size {size:#x} at {off:#x}")));
        }
        out.push((off, cmd, size));
        off += size;
    }
    Ok(out)
}

fn parse_image(b: &[u8]) -> Result<Image, MachOError> {
    let h = parse_header(b)?;
    let cputype = u32_le(b, 4)?;
    let cpusubtype = u32_le(b, 8)?;
    let mut img = Image {
        cpu: Cpu { cputype, cpusubtype },
        is_64: h.is_64,
        filetype: u32_le(b, 12)?,
        flags: u32_le(b, 24)?,
        ncmds: h.ncmds,
        sizeofcmds: h.sizeofcmds,
        build_version: None,
        uuid: None,
        rpaths: Vec::new(),
        dylibs: Vec::new(),
        install_name: None,
        encryption_info: None,
        symtab: None,
        segments: Vec::new(),
        code_signature_range: None,
        code_signature: None,
        code_signature_error: None,
        header_padding: 0,
    };
    let lc_end = h.header_size + h.sizeofcmds as usize;
    let mut first_section: Option<u64> = None;
    let mut version_min: Option<BuildVersion> = None;
    for (off, cmd, size) in load_commands(b, &h)? {
        let cmd_end = off + size;
        match cmd {
            LC_SEGMENT_64 | LC_SEGMENT => {
                let is64 = cmd == LC_SEGMENT_64;
                let name = fixed_name(b, off + 8, 16)?;
                let (seg, sect_base, sect_size) = if is64 {
                    let seg = Segment {
                        name,
                        vmaddr: u64_le(b, off + 24)?,
                        vmsize: u64_le(b, off + 32)?,
                        fileoff: u64_le(b, off + 40)?,
                        filesize: u64_le(b, off + 48)?,
                        maxprot: u32_le(b, off + 56)?,
                        initprot: u32_le(b, off + 60)?,
                        nsects: u32_le(b, off + 64)?,
                    };
                    (seg, off + 72, 80)
                } else {
                    let seg = Segment {
                        name,
                        vmaddr: u64::from(u32_le(b, off + 24)?),
                        vmsize: u64::from(u32_le(b, off + 28)?),
                        fileoff: u64::from(u32_le(b, off + 32)?),
                        filesize: u64::from(u32_le(b, off + 36)?),
                        maxprot: u32_le(b, off + 40)?,
                        initprot: u32_le(b, off + 44)?,
                        nsects: u32_le(b, off + 48)?,
                    };
                    (seg, off + 56, 68)
                };
                for s in 0..seg.nsects as usize {
                    let sb = sect_base + s * sect_size;
                    if sb + sect_size > cmd_end {
                        return Err(MachOError::Malformed(format!("section {s} of {} overruns its command", seg.name)));
                    }
                    let (sect_off, flags) = if is64 {
                        (u32_le(b, sb + 48)?, u32_le(b, sb + 64)?)
                    } else {
                        (u32_le(b, sb + 40)?, u32_le(b, sb + 56)?)
                    };
                    // S_ZEROFILL (1), S_GB_ZEROFILL (12), S_THREAD_LOCAL_ZEROFILL (18) have no file data.
                    let zerofill = matches!(flags & 0xff, 1 | 12 | 18);
                    if sect_off != 0 && !zerofill {
                        let o = u64::from(sect_off);
                        first_section = Some(first_section.map_or(o, |f| f.min(o)));
                    }
                }
                img.segments.push(seg);
            }
            LC_SYMTAB => {
                img.symtab = Some(Symtab {
                    symoff: u32_le(b, off + 8)?,
                    nsyms: u32_le(b, off + 12)?,
                    stroff: u32_le(b, off + 16)?,
                    strsize: u32_le(b, off + 20)?,
                });
            }
            LC_UUID => {
                let raw = bytes(b, off + 8, 16, "uuid")?;
                let mut a = [0u8; 16];
                a.copy_from_slice(raw);
                img.uuid = Some(Uuid::from_bytes(a));
            }
            LC_RPATH => {
                let name_off = u32_le(b, off + 8)? as usize;
                img.rpaths.push(cstr(b, off + name_off, cmd_end, "rpath")?);
            }
            LC_LOAD_DYLIB | LC_LOAD_WEAK_DYLIB | LC_REEXPORT_DYLIB | LC_LAZY_LOAD_DYLIB | LC_LOAD_UPWARD_DYLIB => {
                let name_off = u32_le(b, off + 8)? as usize;
                img.dylibs.push(DylibRef {
                    name: cstr(b, off + name_off, cmd_end, "dylib")?,
                    weak: cmd == LC_LOAD_WEAK_DYLIB,
                });
            }
            LC_ID_DYLIB => {
                let name_off = u32_le(b, off + 8)? as usize;
                img.install_name = Some(cstr(b, off + name_off, cmd_end, "install name")?);
            }
            LC_ENCRYPTION_INFO_64 | LC_ENCRYPTION_INFO => {
                img.encryption_info = Some(EncInfo {
                    cryptoff: u32_le(b, off + 8)?,
                    cryptsize: u32_le(b, off + 12)?,
                    cryptid: u32_le(b, off + 16)?,
                });
            }
            LC_BUILD_VERSION => {
                let ntools = u32_le(b, off + 20)? as usize;
                if 24 + ntools.saturating_mul(8) > size {
                    return Err(MachOError::Malformed(format!("LC_BUILD_VERSION lists {ntools} tools")));
                }
                let mut tools = Vec::with_capacity(ntools);
                for t in 0..ntools {
                    let base = off + 24 + t * 8;
                    tools.push((u32_le(b, base)?, Version::from_packed(u32_le(b, base + 4)?)));
                }
                img.build_version = Some(BuildVersion {
                    platform: u32_le(b, off + 8)?,
                    minos: Version::from_packed(u32_le(b, off + 12)?),
                    sdk: Version::from_packed(u32_le(b, off + 16)?),
                    tools,
                });
            }
            LC_VERSION_MIN_IPHONEOS | LC_VERSION_MIN_MACOSX | LC_VERSION_MIN_TVOS | LC_VERSION_MIN_WATCHOS => {
                let platform = match cmd {
                    LC_VERSION_MIN_IPHONEOS => PLATFORM_IOS,
                    LC_VERSION_MIN_MACOSX => PLATFORM_MACOS,
                    LC_VERSION_MIN_TVOS => PLATFORM_TVOS,
                    _ => PLATFORM_WATCHOS,
                };
                version_min = Some(BuildVersion {
                    platform,
                    minos: Version::from_packed(u32_le(b, off + 8)?),
                    sdk: Version::from_packed(u32_le(b, off + 12)?),
                    tools: Vec::new(),
                });
            }
            LC_CODE_SIGNATURE => {
                let dataoff = u32_le(b, off + 8)?;
                let datasize = u32_le(b, off + 12)?;
                img.code_signature_range = Some((dataoff, datasize));
                match bytes(b, dataoff as usize, datasize as usize, "code signature").and_then(CodeSignature::parse) {
                    Ok(sig) => img.code_signature = Some(sig),
                    Err(e) => img.code_signature_error = Some(e.to_string()),
                }
            }
            _ => {}
        }
    }
    if img.build_version.is_none() {
        img.build_version = version_min;
    }
    img.header_padding = first_section.map_or(0, |f| f.saturating_sub(lc_end as u64));
    Ok(img)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn versions_pack_print_and_parse() {
        let v = Version::from_packed(0x001b_0000);
        assert_eq!(v, Version::new(27, 0, 0));
        assert_eq!(v.to_string(), "27.0");
        assert_eq!(Version::new(17, 0, 1).to_string(), "17.0.1");
        assert_eq!("17.0.1".parse::<Version>().unwrap().packed(), 0x0011_0001);
        assert_eq!("27".parse::<Version>().unwrap(), Version::new(27, 0, 0));
        assert!("27.x".parse::<Version>().is_err());
        assert!("1.2.3.4".parse::<Version>().is_err());
    }

    #[test]
    fn arch_classification() {
        assert_eq!(Arch::from_cpu(CPU_TYPE_ARM64, 0), Some(Arch::Arm64));
        assert_eq!(Arch::from_cpu(CPU_TYPE_ARM64, 0x8000_0002), Some(Arch::Arm64e));
        assert_eq!(Arch::from_cpu(CPU_TYPE_X86_64, 3), Some(Arch::X86_64));
        assert_eq!(Arch::from_cpu(CPU_TYPE_ARM, 9), None);
        let armv7 = Cpu { cputype: CPU_TYPE_ARM, cpusubtype: 9 };
        assert_eq!((armv7.arch(), armv7.name()), (None, "armv7".to_owned()));
        assert_eq!(Cpu { cputype: 0x99, cpusubtype: 1 }.to_string(), "cpu-0x99-0x1");
        assert_eq!(Cpu::from(Arch::Arm64e).arch(), Some(Arch::Arm64e));
        assert_eq!("arm64e".parse::<Arch>().unwrap(), Arch::Arm64e);
        assert_eq!(Arch::X86_64.to_string(), "x86_64");
    }

    /// `Arch` keeps exactly the three F0 variants: the build plane matches it exhaustively (ARCHITECTURE §5.3).
    #[test]
    fn arch_keeps_the_f0_variants() {
        let name = |a: Arch| match a {
            Arch::Arm64 => "arm64",
            Arch::Arm64e => "arm64e",
            Arch::X86_64 => "x86_64",
        };
        for a in [Arch::Arm64, Arch::Arm64e, Arch::X86_64] {
            assert_eq!(name(a), a.name());
        }
    }

    #[test]
    fn garbage_is_rejected_without_panicking() {
        for input in [&b""[..], b"\x00", b"\xcf\xfa\xed\xfe", b"\xca\xfe\xba\xbe\x00\x00\x00\x09", b"hello world!!"] {
            assert!(MachO::parse(input).is_err(), "{input:?}");
        }
        assert!(!is_macho(b"\xca\xfe\xba\xbe\x00\x00\x00\x2d")); // a Java class file
        assert!(is_macho(b"\xcf\xfa\xed\xfe\x0c\x00\x00\x01"));
    }

    #[test]
    fn alignment_helper() {
        assert_eq!(alignment_of(0x4004), 4);
        assert_eq!(alignment_of(0x4008), 8);
        assert_eq!(alignment_of(0x4000), 4096);
        assert_eq!(alignment_of(0), 4096);
        assert_eq!(alignment_of(3), 1);
    }
}
