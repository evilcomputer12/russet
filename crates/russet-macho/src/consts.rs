// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Mach-O and code-signing constants (`<mach-o/loader.h>`, `<mach-o/fat.h>`, `<kern/cs_blobs.h>` field values,
//! written from the public format descriptions; no Apple header is copied).

/// 32-bit thin header magic (little-endian on disk).
pub const MH_MAGIC: u32 = 0xfeed_face;
/// 64-bit thin header magic (little-endian on disk).
pub const MH_MAGIC_64: u32 = 0xfeed_facf;
/// Byte-swapped 32-bit magic (a big-endian file read as little-endian).
pub const MH_CIGAM: u32 = 0xcefa_edfe;
/// Byte-swapped 64-bit magic.
pub const MH_CIGAM_64: u32 = 0xcffa_edfe;
/// Fat header magic (big-endian on disk), 32-bit offsets.
pub const FAT_MAGIC: u32 = 0xcafe_babe;
/// Fat header magic (big-endian on disk), 64-bit offsets.
pub const FAT_MAGIC_64: u32 = 0xcafe_babf;

/// `mach_header_64` size.
pub const HEADER_SIZE_64: usize = 32;
/// `mach_header` size.
pub const HEADER_SIZE_32: usize = 28;

/// CPU type flag for 64-bit ABIs.
pub const CPU_ARCH_ABI64: u32 = 0x0100_0000;
/// CPU type flag for ILP32 on 64-bit hardware.
pub const CPU_ARCH_ABI64_32: u32 = 0x0200_0000;
/// `CPU_TYPE_ARM`.
pub const CPU_TYPE_ARM: u32 = 12;
/// `CPU_TYPE_ARM64`.
pub const CPU_TYPE_ARM64: u32 = CPU_TYPE_ARM | CPU_ARCH_ABI64;
/// `CPU_TYPE_ARM64_32`.
pub const CPU_TYPE_ARM64_32: u32 = CPU_TYPE_ARM | CPU_ARCH_ABI64_32;
/// `CPU_TYPE_X86`.
pub const CPU_TYPE_X86: u32 = 7;
/// `CPU_TYPE_X86_64`.
pub const CPU_TYPE_X86_64: u32 = CPU_TYPE_X86 | CPU_ARCH_ABI64;
/// Mask of the subtype bits (the top byte carries capability flags such as pointer authentication).
pub const CPU_SUBTYPE_MASK: u32 = 0x00ff_ffff;
/// `CPU_SUBTYPE_ARM64_ALL`.
pub const CPU_SUBTYPE_ARM64_ALL: u32 = 0;
/// `CPU_SUBTYPE_ARM64E`.
pub const CPU_SUBTYPE_ARM64E: u32 = 2;
/// `CPU_SUBTYPE_X86_64_ALL`.
pub const CPU_SUBTYPE_X86_64_ALL: u32 = 3;

/// Relocatable object file.
pub const MH_OBJECT: u32 = 0x1;
/// Demand-paged executable (apps and app extensions).
pub const MH_EXECUTE: u32 = 0x2;
/// Dynamic library (frameworks and `.dylib`s).
pub const MH_DYLIB: u32 = 0x6;
/// The dynamic linker.
pub const MH_DYLINKER: u32 = 0x7;
/// Loadable bundle.
pub const MH_BUNDLE: u32 = 0x8;
/// Debug companion (dSYM).
pub const MH_DSYM: u32 = 0xa;

/// Load commands that dyld must understand carry this bit.
pub const LC_REQ_DYLD: u32 = 0x8000_0000;
pub const LC_SEGMENT: u32 = 0x1;
pub const LC_SYMTAB: u32 = 0x2;
pub const LC_DYSYMTAB: u32 = 0xb;
pub const LC_LOAD_DYLIB: u32 = 0xc;
pub const LC_ID_DYLIB: u32 = 0xd;
pub const LC_LOAD_WEAK_DYLIB: u32 = 0x18 | LC_REQ_DYLD;
pub const LC_SEGMENT_64: u32 = 0x19;
pub const LC_UUID: u32 = 0x1b;
pub const LC_RPATH: u32 = 0x1c | LC_REQ_DYLD;
pub const LC_CODE_SIGNATURE: u32 = 0x1d;
pub const LC_SEGMENT_SPLIT_INFO: u32 = 0x1e;
pub const LC_REEXPORT_DYLIB: u32 = 0x1f | LC_REQ_DYLD;
pub const LC_LAZY_LOAD_DYLIB: u32 = 0x20;
pub const LC_ENCRYPTION_INFO: u32 = 0x21;
pub const LC_DYLD_INFO: u32 = 0x22;
pub const LC_DYLD_INFO_ONLY: u32 = 0x22 | LC_REQ_DYLD;
pub const LC_LOAD_UPWARD_DYLIB: u32 = 0x23 | LC_REQ_DYLD;
pub const LC_VERSION_MIN_MACOSX: u32 = 0x24;
pub const LC_VERSION_MIN_IPHONEOS: u32 = 0x25;
pub const LC_FUNCTION_STARTS: u32 = 0x26;
pub const LC_MAIN: u32 = 0x28 | LC_REQ_DYLD;
pub const LC_DATA_IN_CODE: u32 = 0x29;
pub const LC_DYLIB_CODE_SIGN_DRS: u32 = 0x2b;
pub const LC_ENCRYPTION_INFO_64: u32 = 0x2c;
pub const LC_LINKER_OPTIMIZATION_HINT: u32 = 0x2e;
pub const LC_VERSION_MIN_TVOS: u32 = 0x2f;
pub const LC_VERSION_MIN_WATCHOS: u32 = 0x30;
pub const LC_BUILD_VERSION: u32 = 0x32;
pub const LC_DYLD_EXPORTS_TRIE: u32 = 0x33 | LC_REQ_DYLD;
pub const LC_DYLD_CHAINED_FIXUPS: u32 = 0x34 | LC_REQ_DYLD;
pub const LC_ATOM_INFO: u32 = 0x36;

/// `LC_BUILD_VERSION` platforms.
pub const PLATFORM_MACOS: u32 = 1;
pub const PLATFORM_IOS: u32 = 2;
pub const PLATFORM_TVOS: u32 = 3;
pub const PLATFORM_WATCHOS: u32 = 4;
pub const PLATFORM_BRIDGEOS: u32 = 5;
pub const PLATFORM_MACCATALYST: u32 = 6;
pub const PLATFORM_IOSSIMULATOR: u32 = 7;
pub const PLATFORM_TVOSSIMULATOR: u32 = 8;
pub const PLATFORM_WATCHOSSIMULATOR: u32 = 9;
pub const PLATFORM_DRIVERKIT: u32 = 10;
pub const PLATFORM_VISIONOS: u32 = 11;
pub const PLATFORM_VISIONOSSIMULATOR: u32 = 12;

/// `LC_BUILD_VERSION` tool ids.
pub const TOOL_CLANG: u32 = 1;
pub const TOOL_SWIFT: u32 = 2;
pub const TOOL_LD: u32 = 3;
pub const TOOL_LLD: u32 = 4;

/// Code-signature superblob (`CSMAGIC_EMBEDDED_SIGNATURE`).
pub const CSMAGIC_EMBEDDED_SIGNATURE: u32 = 0xfade_0cc0;
/// Code directory blob.
pub const CSMAGIC_CODEDIRECTORY: u32 = 0xfade_0c02;
/// Requirement set blob.
pub const CSMAGIC_REQUIREMENTS: u32 = 0xfade_0c01;
/// Single requirement blob.
pub const CSMAGIC_REQUIREMENT: u32 = 0xfade_0c00;
/// XML entitlements blob.
pub const CSMAGIC_EMBEDDED_ENTITLEMENTS: u32 = 0xfade_7171;
/// DER entitlements blob.
pub const CSMAGIC_EMBEDDED_DER_ENTITLEMENTS: u32 = 0xfade_7172;
/// CMS signature wrapper.
pub const CSMAGIC_BLOBWRAPPER: u32 = 0xfade_0b01;

/// Superblob slots.
pub const CSSLOT_CODEDIRECTORY: u32 = 0;
pub const CSSLOT_INFOSLOT: u32 = 1;
pub const CSSLOT_REQUIREMENTS: u32 = 2;
pub const CSSLOT_RESOURCEDIR: u32 = 3;
pub const CSSLOT_APPLICATION: u32 = 4;
pub const CSSLOT_ENTITLEMENTS: u32 = 5;
pub const CSSLOT_REP_SPECIFIC: u32 = 6;
pub const CSSLOT_DER_ENTITLEMENTS: u32 = 7;
pub const CSSLOT_ALTERNATE_CODEDIRECTORIES: u32 = 0x1000;
pub const CSSLOT_ALTERNATE_CODEDIRECTORY_LIMIT: u32 = 5;
pub const CSSLOT_SIGNATURESLOT: u32 = 0x10000;

/// Code directory hash types.
pub const CS_HASHTYPE_SHA1: u8 = 1;
pub const CS_HASHTYPE_SHA256: u8 = 2;
pub const CS_HASHTYPE_SHA256_TRUNCATED: u8 = 3;
pub const CS_HASHTYPE_SHA384: u8 = 4;

/// Code directory flags.
pub const CS_ADHOC: u32 = 0x0000_0002;
pub const CS_HARD: u32 = 0x0000_0100;
pub const CS_KILL: u32 = 0x0000_0200;
pub const CS_RESTRICT: u32 = 0x0000_0800;
pub const CS_ENFORCEMENT: u32 = 0x0000_1000;
pub const CS_REQUIRE_LV: u32 = 0x0000_2000;
pub const CS_RUNTIME: u32 = 0x0001_0000;
pub const CS_LINKER_SIGNED: u32 = 0x0002_0000;

/// Executable-segment flags (code directory 0x20400+).
pub const CS_EXECSEG_MAIN_BINARY: u64 = 0x1;
pub const CS_EXECSEG_ALLOW_UNSIGNED: u64 = 0x10;
pub const CS_EXECSEG_DEBUGGER: u64 = 0x20;
pub const CS_EXECSEG_JIT: u64 = 0x40;
pub const CS_EXECSEG_SKIP_LV: u64 = 0x80;
pub const CS_EXECSEG_CAN_LOAD_CDHASH: u64 = 0x100;
pub const CS_EXECSEG_CAN_EXEC_CDHASH: u64 = 0x200;

/// Code directory versions that add fields.
pub const CS_SUPPORTSSCATTER: u32 = 0x20100;
pub const CS_SUPPORTSTEAMID: u32 = 0x20200;
pub const CS_SUPPORTSCODELIMIT64: u32 = 0x20300;
pub const CS_SUPPORTSEXECSEG: u32 = 0x20400;
pub const CS_SUPPORTSRUNTIME: u32 = 0x20500;
pub const CS_SUPPORTSLINKAGE: u32 = 0x20600;
