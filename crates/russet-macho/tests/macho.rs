// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Parsing and edits on synthetic Mach-O images built at test time (ARCHITECTURE §19; build T-MACHO).

#![allow(clippy::unwrap_used)]

use russet_macho::consts::*;
use russet_macho::fixture::{MachOBuilder, fat};
use russet_macho::{Arch, CodeSignature, HashType, MachO, MachOError, Version};

fn strings_of(bytes: &[u8]) -> Vec<String> {
    // Resolves every symbol name through LC_SYMTAB, to prove the string table is still addressable.
    let m = MachO::parse(bytes).unwrap();
    let s = m.primary().symtab.unwrap();
    (0..s.nsyms as usize)
        .map(|i| {
            let e = s.symoff as usize + i * 16;
            let strx = u32::from_le_bytes(bytes[e..e + 4].try_into().unwrap()) as usize;
            let start = s.stroff as usize + strx;
            let end = start + bytes[start..].iter().position(|&c| c == 0).unwrap();
            String::from_utf8(bytes[start..end].to_vec()).unwrap()
        })
        .collect()
}

#[test]
fn parses_an_executable() {
    let bytes = MachOBuilder::executable()
        .rpath("@executable_path/Frameworks")
        .rpath("/home/someone/.build/PackageFrameworks")
        .dylib_dependency("@rpath/Kit.framework/Kit", true)
        .encryption_info()
        .build();
    let m = MachO::parse(&bytes).unwrap();
    assert!(!m.is_fat());
    assert_eq!(m.slices().len(), 1);
    assert_eq!(m.slices()[0].arch, Arch::Arm64);
    assert_eq!(m.filetype(), MH_EXECUTE);
    let bv = m.build_version().unwrap();
    assert_eq!((bv.platform, bv.minos, bv.sdk), (PLATFORM_IOS, Version::new(17, 0, 0), Version::new(27, 0, 0)));
    assert_eq!(bv.tools, vec![(TOOL_LD, Version::new(1100, 0, 0))]);
    assert_eq!(m.uuid().unwrap().as_bytes(), &[0x52; 16]);
    assert_eq!(m.rpaths(), vec!["@executable_path/Frameworks", "/home/someone/.build/PackageFrameworks"]);
    let dylibs = m.dylibs();
    assert_eq!(dylibs.len(), 2);
    assert_eq!(dylibs[0].name, "/usr/lib/libSystem.B.dylib");
    assert!(dylibs[1].weak);
    assert_eq!(m.encryption_info().unwrap().cryptid, 0);
    assert!(m.string_pool_alignment() >= 8);
    assert!(m.code_signature().is_none());
    let img = m.primary();
    assert!(img.is_executable() && img.is_64 && img.linkedit_is_last());
    assert!(img.header_padding > 3000, "room for LC_CODE_SIGNATURE: {}", img.header_padding);
    assert_eq!(img.segment("__TEXT").unwrap().fileoff, 0);
    assert_eq!(strings_of(&bytes), vec!["_main"]);
}

#[test]
fn parses_a_dylib_and_a_fat_file() {
    let dylib = MachOBuilder::dylib("@rpath/Kit.framework/Kit").build();
    let m = MachO::parse(&dylib).unwrap();
    assert_eq!(m.filetype(), MH_DYLIB);
    assert_eq!(m.primary().install_name.as_deref(), Some("@rpath/Kit.framework/Kit"));

    let x86 = MachOBuilder::dylib("@rpath/Kit.framework/Kit").arch(Arch::X86_64).platform(7).build();
    let both = fat(&[x86, dylib.clone()]);
    let m = MachO::parse(&both).unwrap();
    assert!(m.is_fat());
    assert_eq!(m.slices().iter().map(|s| s.arch).collect::<Vec<_>>(), vec![Arch::X86_64, Arch::Arm64]);
    // The single-valued accessors describe the arm64 slice even when it is not first.
    assert_eq!(m.primary().arch, Arch::Arm64);
    assert_eq!(m.build_version().unwrap().platform, PLATFORM_IOS);
    assert_eq!(m.slice_bytes(&both, 1).unwrap(), &dylib[..]);
    assert!(russet_macho::is_macho(&both));
}

#[test]
fn reads_a_linker_signature_and_detects_tampering() {
    let mut bytes = MachOBuilder::executable().linker_signed("Hello").build();
    let m = MachO::parse(&bytes).unwrap();
    let sig = m.code_signature().unwrap();
    let cd = sig.code_directory().unwrap();
    assert_eq!(cd.version, CS_SUPPORTSEXECSEG);
    assert_eq!(cd.hash_type, HashType::Sha256);
    assert_eq!(cd.page_size, 4096);
    assert_eq!(cd.identifier, "Hello");
    assert!(cd.has_flag(CS_ADHOC) && cd.has_flag(CS_LINKER_SIGNED));
    assert!(cd.has_exec_seg_flag(CS_EXECSEG_MAIN_BINARY));
    assert_eq!(cd.cdhash.len(), 20);
    assert!(!sig.has_cms_signature() && sig.is_adhoc());
    assert!(cd.mismatched_pages(&bytes).is_empty());
    bytes[0x1000] ^= 0xff;
    assert_eq!(cd.mismatched_pages(&bytes), vec![1]);
}

fn be(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_be_bytes()).collect()
}

/// A code directory with `nspecial` special slots (slot k hashes to `[k; 32]`) and a team id.
fn code_directory(hash: u8, hash_size: u8, ident: &str, team: &str, nspecial: u32) -> Vec<u8> {
    let header = 88u32;
    let ident_off = header;
    let team_off = ident_off + ident.len() as u32 + 1;
    let hash_off = team_off + team.len() as u32 + 1 + nspecial * u32::from(hash_size);
    let len = hash_off + u32::from(hash_size); // one code slot
    let mut cd = be(&[CSMAGIC_CODEDIRECTORY, len, 0x20400, 0, hash_off, ident_off, nspecial, 1, 4096]);
    cd.extend_from_slice(&[hash_size, hash, 0, 12]);
    cd.extend_from_slice(&be(&[0, 0, team_off, 0]));
    cd.extend_from_slice(&0u64.to_be_bytes());
    for v in [0u64, 0x4000, 0x11] {
        cd.extend_from_slice(&v.to_be_bytes());
    }
    cd.extend_from_slice(ident.as_bytes());
    cd.push(0);
    cd.extend_from_slice(team.as_bytes());
    cd.push(0);
    for k in (1..=nspecial).rev() {
        cd.extend_from_slice(&vec![k as u8; usize::from(hash_size)]);
    }
    cd.extend_from_slice(&vec![0xaa; usize::from(hash_size)]);
    assert_eq!(cd.len(), len as usize);
    cd
}

#[test]
fn parses_every_superblob_blob_kind() {
    let xml = "<?xml version=\"1.0\"?><plist><dict><key>get-task-allow</key><true/></dict></plist>";
    let mut ents = be(&[CSMAGIC_EMBEDDED_ENTITLEMENTS, 8 + xml.len() as u32]);
    ents.extend_from_slice(xml.as_bytes());
    let mut der = be(&[CSMAGIC_EMBEDDED_DER_ENTITLEMENTS, 12]);
    der.extend_from_slice(&[0x70, 0x02, 0x02, 0x01]);
    let reqs = be(&[CSMAGIC_REQUIREMENTS, 12, 0]);
    let cms = be(&[CSMAGIC_BLOBWRAPPER, 8]);
    let cd1 = code_directory(CS_HASHTYPE_SHA1, 20, "dev.russet.test.probe", "TEAMID1234", 7);
    let cd2 = code_directory(CS_HASHTYPE_SHA256, 32, "dev.russet.test.probe", "TEAMID1234", 7);
    let blobs: Vec<(u32, Vec<u8>)> = vec![
        (CSSLOT_CODEDIRECTORY, cd1),
        (CSSLOT_REQUIREMENTS, reqs),
        (CSSLOT_ENTITLEMENTS, ents),
        (CSSLOT_DER_ENTITLEMENTS, der),
        (CSSLOT_ALTERNATE_CODEDIRECTORIES, cd2),
        (CSSLOT_SIGNATURESLOT, cms),
    ];
    let mut off = 12 + 8 * blobs.len() as u32;
    let mut index = Vec::new();
    let mut body = Vec::new();
    for (slot, b) in &blobs {
        index.extend_from_slice(&be(&[*slot, off]));
        off += b.len() as u32;
        body.extend_from_slice(b);
    }
    let mut sb = be(&[CSMAGIC_EMBEDDED_SIGNATURE, off, blobs.len() as u32]);
    sb.extend_from_slice(&index);
    sb.extend_from_slice(&body);

    let sig = CodeSignature::parse(&sb).unwrap();
    assert_eq!(sig.hash_types(), vec![HashType::Sha1, HashType::Sha256]);
    assert_eq!(sig.best_code_directory().unwrap().hash_type, HashType::Sha256);
    assert_eq!(sig.team_id(), Some("TEAMID1234"));
    assert_eq!(sig.identifier(), Some("dev.russet.test.probe"));
    let cd = sig.code_directory().unwrap();
    assert_eq!(cd.exec_seg_flags, Some(0x11));
    assert_eq!(cd.special_slot(CSSLOT_ENTITLEMENTS), Some(&[5u8; 20][..]));
    assert_eq!(cd.special_slot(CSSLOT_DER_ENTITLEMENTS), Some(&[7u8; 20][..]));
    assert_eq!(cd.n_special_slots, 7);
    assert!(sig.entitlements_xml.as_deref().unwrap().contains("get-task-allow"));
    assert_eq!(sig.entitlements_der.as_deref(), Some(&[0x70, 0x02, 0x02, 0x01][..]));
    assert!(sig.requirements.is_some());
    assert_eq!(sig.cms.as_deref(), Some(&[][..]));
    assert!(!sig.has_cms_signature());
    assert_eq!(sig.blob(CSSLOT_REQUIREMENTS).unwrap().len(), 12);
    // Truncations anywhere fail cleanly.
    for n in 0..sb.len() {
        let _ = CodeSignature::parse(&sb[..n]);
    }
}

#[test]
fn deletes_rpaths_in_thin_and_fat_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("App");
    let bytes = MachOBuilder::executable()
        .rpath("/home/u/.build/PackageFrameworks")
        .rpath("@executable_path/Frameworks")
        .build();
    std::fs::write(&path, &bytes).unwrap();
    let before = MachO::open(&path).unwrap();
    russet_macho::delete_rpath(&path, "/home/u/.build/PackageFrameworks").unwrap();
    let after_bytes = std::fs::read(&path).unwrap();
    assert_eq!(after_bytes.len(), bytes.len(), "deletion keeps the file size");
    let after = MachO::parse(&after_bytes).unwrap();
    assert_eq!(after.rpaths(), vec!["@executable_path/Frameworks"]);
    assert_eq!(after.primary().ncmds, before.primary().ncmds - 1);
    assert!(after.primary().sizeofcmds < before.primary().sizeofcmds);
    assert_eq!(after.dylibs(), before.dylibs());
    assert_eq!(after.build_version(), before.build_version());
    assert_eq!(after_bytes[0x1000..], bytes[0x1000..], "section data and __LINKEDIT are untouched");
    assert!(matches!(
        russet_macho::delete_rpath(&path, "/home/u/.build/PackageFrameworks"),
        Err(MachOError::NotFound(_))
    ));

    let thin = MachOBuilder::dylib("@rpath/libA.dylib").rpath("/abs/path").build();
    let both =
        fat(&[thin.clone(), MachOBuilder::dylib("@rpath/libA.dylib").arch(Arch::X86_64).rpath("/abs/path").build()]);
    let out = russet_macho::delete_rpath_bytes(&both, "/abs/path").unwrap();
    let m = MachO::parse(&out).unwrap();
    assert!(m.images().iter().all(|i| i.rpaths.is_empty()));
}

#[test]
fn aligns_a_misaligned_string_pool() {
    let bytes = MachOBuilder::executable().symbol("_helper").symbol("_other").misaligned_string_pool().build();
    let m = MachO::parse(&bytes).unwrap();
    assert_eq!(m.string_pool_alignment(), 4);
    let names = strings_of(&bytes);

    let fixed = russet_macho::align_string_pool_bytes(&bytes).unwrap().unwrap();
    assert_eq!(fixed.len(), bytes.len() + 16);
    let m2 = MachO::parse(&fixed).unwrap();
    assert!(m2.string_pool_alignment() >= 8);
    assert_eq!(strings_of(&fixed), names);
    let le = m2.primary().segment("__LINKEDIT").unwrap();
    assert_eq!(le.fileoff + le.filesize, fixed.len() as u64);
    assert!(le.vmsize >= le.filesize);
    assert!(russet_macho::align_string_pool_bytes(&fixed).unwrap().is_none(), "idempotent");

    // An aligned file is left alone, and the file edit reports what it did.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lib.dylib");
    std::fs::write(&path, MachOBuilder::dylib("@rpath/lib.dylib").build()).unwrap();
    assert!(!russet_macho::align_string_pool(&path).unwrap());
    std::fs::write(&path, &bytes).unwrap();
    assert!(russet_macho::align_string_pool(&path).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), fixed);
}

#[test]
fn aligning_moves_a_trailing_signature_and_keeps_it_aligned() {
    let bytes = MachOBuilder::executable().misaligned_string_pool().linker_signed("Hello").build();
    let (old_off, _) = MachO::parse(&bytes).unwrap().primary().code_signature_range.unwrap();
    let fixed = russet_macho::align_string_pool_bytes(&bytes).unwrap().unwrap();
    let m = MachO::parse(&fixed).unwrap();
    let (new_off, size) = m.primary().code_signature_range.unwrap();
    assert_eq!(new_off, old_off + 16);
    assert_eq!(new_off % 16, 0);
    assert_eq!((new_off + size) as usize, fixed.len());
    // The signature blob moved intact (its page hashes are stale until the app is re-signed).
    assert!(m.code_signature().is_some());
}

#[test]
fn aligns_one_slice_of_a_fat_file() {
    let bad = MachOBuilder::dylib("@rpath/libA.dylib").misaligned_string_pool().build();
    let good = MachOBuilder::dylib("@rpath/libA.dylib").arch(Arch::X86_64).build();
    let both = fat(&[good.clone(), bad]);
    assert_eq!(MachO::parse(&both).unwrap().string_pool_alignment(), 4);
    let fixed = russet_macho::align_string_pool_bytes(&both).unwrap().unwrap();
    let m = MachO::parse(&fixed).unwrap();
    assert!(m.string_pool_alignment() >= 8);
    assert_eq!(m.slice_bytes(&fixed, 0).unwrap(), &good[..]);
    assert_eq!(m.slices()[1].offset % 0x4000, 0);
}

#[test]
fn thins_and_patches_sdk_versions() {
    let arm = MachOBuilder::executable().build();
    let both = fat(&[MachOBuilder::executable().arch(Arch::X86_64).platform(7).build(), arm.clone()]);
    assert_eq!(russet_macho::thin_bytes(&both, Arch::Arm64).unwrap(), arm);
    assert!(matches!(russet_macho::thin_bytes(&arm, Arch::X86_64), Err(MachOError::NotFound(_))));

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("App");
    std::fs::write(&path, &both).unwrap();
    russet_macho::thin(&path, Arch::Arm64).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), arm);
    russet_macho::thin(&path, Arch::Arm64).unwrap(); // already thin: a no-op

    russet_macho::patch_sdk_version(&path, Version::new(26, 2, 0)).unwrap();
    let bv = MachO::open(&path).unwrap().build_version().unwrap();
    assert_eq!((bv.minos, bv.sdk), (Version::new(17, 0, 0), Version::new(26, 2, 0)));
    let patched = russet_macho::patch_sdk_version_bytes(&both, Version::new(27, 1, 0)).unwrap();
    assert!(
        MachO::parse(&patched)
            .unwrap()
            .images()
            .iter()
            .all(|i| i.build_version.as_ref().unwrap().sdk == Version::new(27, 1, 0))
    );
}

#[cfg(unix)]
#[test]
fn edits_keep_permissions() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("App");
    std::fs::write(&path, MachOBuilder::executable().rpath("/x").build()).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    russet_macho::delete_rpath(&path, "/x").unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o755);
    let leftovers: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
    assert_eq!(leftovers.len(), 1, "no temporary file is left behind");
}

#[test]
fn corrupted_images_never_panic() {
    let base = MachOBuilder::executable().rpath("@executable_path/Frameworks").linker_signed("Hello").build();
    let both = fat(&[base.clone(), MachOBuilder::executable().arch(Arch::X86_64).build()]);
    for input in [&base, &both] {
        // Truncations at every load-command boundary region, and single-byte flips across the header area.
        for n in (0..0x400).chain((0x4000..input.len()).step_by(7)) {
            let _ = MachO::parse(&input[..n.min(input.len())]);
        }
        for i in 0..0x400.min(input.len()) {
            let mut m = input.clone();
            m[i] ^= 0xff;
            let _ = MachO::parse(&m);
            let _ = russet_macho::align_string_pool_bytes(&m);
            let _ = russet_macho::delete_rpath_bytes(&m, "@executable_path/Frameworks");
        }
    }
}
