// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! What identifies a Mach-O build: its `LC_UUID` (how LLDB binds the local file to the device's image; RD §21
//! `app.uuid`) and the CDHash of its strongest code directory (the install ledger's key, ARCHITECTURE §14.8).
//!
//! - The CDHash is the hash of the CodeDirectory blob, computed with the directory's own hash type and
//!   truncated to 20 bytes (`codesign -dvvv`'s `CDHash=`). A file with several code directories (SHA-1 plus a
//!   SHA-256 alternate) is identified by the strongest one, as the kernel picks it: SHA-384, then SHA-256, then
//!   truncated SHA-256, then SHA-1.
//! - The code directory covers the code pages, `Info.plist`, the resource seal (`_CodeSignature/CodeResources`,
//!   which also lists the nested code), the requirements and the entitlements, but not the CMS signature, whose
//!   signing time changes on every signing. So re-signing an unchanged build keeps the CDHash, and any change to
//!   the app gives a new one.
//! - Universal files: the arm64 slice (what an iPhone runs), else arm64e, else the first slice.
//!
//! The reader seeks to the parts it needs, and bounds every size it reads, so a large or hostile file costs
//! little.

use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use sha1::Digest as _;
use sha2::Digest as _;

const MH_MAGIC: u32 = 0xfeed_face;
const MH_MAGIC_64: u32 = 0xfeed_facf;
const MH_CIGAM: u32 = 0xcefa_edfe;
const MH_CIGAM_64: u32 = 0xcffa_edfe;
const FAT_MAGIC: u32 = 0xcafe_babe;
const FAT_MAGIC_64: u32 = 0xcafe_babf;
/// Java class files share `0xcafebabe`; their next word (the class version) is at least 45.
const MAX_FAT_ARCHS: u32 = 32;
const LC_UUID: u32 = 0x1b;
const LC_CODE_SIGNATURE: u32 = 0x1d;
const CPU_TYPE_ARM64: u32 = 0x0100_000c;
const CPU_SUBTYPE_MASK: u32 = 0x00ff_ffff;
const CPU_SUBTYPE_ARM64E: u32 = 2;
/// `sizeofcmds` above this is not a real file.
const MAX_LOAD_COMMANDS_SIZE: u32 = 16 << 20;
/// An embedded signature above this is not a real file (a 4 GiB binary needs about 64 MiB of SHA-256 slots).
const MAX_SIGNATURE_SIZE: u32 = 128 << 20;

const CSMAGIC_EMBEDDED_SIGNATURE: u32 = 0xfade_0cc0;
const CSMAGIC_CODEDIRECTORY: u32 = 0xfade_0c02;
const CSSLOT_CODEDIRECTORY: u32 = 0;
const CSSLOT_ALTERNATE_CODEDIRECTORIES: u32 = 0x1000;
const CSSLOT_ALTERNATE_CODEDIRECTORY_MAX: u32 = 5;
/// The CodeDirectory fields up to `spare2` (version 0x20001's layout, which every later version extends).
const CODEDIRECTORY_MIN_LEN: usize = 44;
/// The offset of `hashType` in a CodeDirectory.
const CODEDIRECTORY_HASH_TYPE: usize = 37;

/// `CS_HASHTYPE_SHA1`.
pub const HASH_SHA1: u8 = 1;
/// `CS_HASHTYPE_SHA256`.
pub const HASH_SHA256: u8 = 2;
/// `CS_HASHTYPE_SHA256_TRUNCATED`.
pub const HASH_SHA256_TRUNCATED: u8 = 3;
/// `CS_HASHTYPE_SHA384`.
pub const HASH_SHA384: u8 = 4;
/// `CS_CDHASH_LEN`: a CDHash is 20 bytes whatever the hash type.
pub const CDHASH_LEN: usize = 20;

/// What identifies one Mach-O image.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MachIdentity {
    /// `LC_UUID`, upper-case and dashed (`4C4C44A9-…`), as debugserver and LLDB print it.
    pub uuid: Option<String>,
    /// The CDHash of the strongest code directory: 40 lower-case hex digits. `None` for an unsigned file.
    pub cdhash: Option<String>,
    /// The hash type of that code directory ([`HASH_SHA256`], …).
    pub hash_type: Option<u8>,
}

/// Reads the identity of the Mach-O file at `path`; `Ok(None)` when the file is not a Mach-O.
pub fn identity(path: &Path) -> io::Result<Option<MachIdentity>> {
    let mut f = std::fs::File::open(path)?;
    read_identity(&mut f)
}

/// [`identity`] over any seekable source.
pub fn read_identity<R: Read + Seek>(r: &mut R) -> io::Result<Option<MachIdentity>> {
    let Some(head) = read_at(r, 0, 8)? else { return Ok(None) };
    let be = u32::from_be_bytes([head[0], head[1], head[2], head[3]]);
    if be == FAT_MAGIC || be == FAT_MAGIC_64 {
        let count = u32::from_be_bytes([head[4], head[5], head[6], head[7]]);
        if count == 0 || count > MAX_FAT_ARCHS {
            return Ok(None);
        }
        return match pick_slice(r, be == FAT_MAGIC_64, count)? {
            Some(offset) => read_thin(r, offset),
            None => Ok(None),
        };
    }
    read_thin(r, 0)
}

/// The offset of the slice an iPhone runs: arm64, else arm64e, else the first.
fn pick_slice<R: Read + Seek>(r: &mut R, wide: bool, count: u32) -> io::Result<Option<u64>> {
    let entry = if wide { 32 } else { 20 };
    let Some(table) = read_at(r, 8, entry * count as usize)? else { return Ok(None) };
    let mut slices = Vec::new();
    for e in table.chunks_exact(entry) {
        let cputype = u32::from_be_bytes([e[0], e[1], e[2], e[3]]);
        let subtype = u32::from_be_bytes([e[4], e[5], e[6], e[7]]) & CPU_SUBTYPE_MASK;
        let offset = if wide {
            u64::from_be_bytes([e[8], e[9], e[10], e[11], e[12], e[13], e[14], e[15]])
        } else {
            u64::from(u32::from_be_bytes([e[8], e[9], e[10], e[11]]))
        };
        slices.push((cputype, subtype, offset));
    }
    let arm64 = slices.iter().find(|(t, s, _)| *t == CPU_TYPE_ARM64 && *s != CPU_SUBTYPE_ARM64E);
    let arm64e = slices.iter().find(|(t, s, _)| *t == CPU_TYPE_ARM64 && *s == CPU_SUBTYPE_ARM64E);
    Ok(arm64.or(arm64e).or(slices.first()).map(|(_, _, o)| *o))
}

/// One thin image at `base`.
fn read_thin<R: Read + Seek>(r: &mut R, base: u64) -> io::Result<Option<MachIdentity>> {
    let Some(head) = read_at(r, base, 28)? else { return Ok(None) };
    let le = u32::from_le_bytes([head[0], head[1], head[2], head[3]]);
    let (wide, little) = match le {
        MH_MAGIC_64 => (true, true),
        MH_MAGIC => (false, true),
        MH_CIGAM_64 => (true, false),
        MH_CIGAM => (false, false),
        _ => return Ok(None),
    };
    let word = |b: &[u8], at: usize| -> Option<u32> {
        let w: [u8; 4] = b.get(at..at + 4)?.try_into().ok()?;
        Some(if little { u32::from_le_bytes(w) } else { u32::from_be_bytes(w) })
    };
    let mut id = MachIdentity::default();
    let (Some(ncmds), Some(sizeofcmds)) = (word(&head, 16), word(&head, 20)) else { return Ok(Some(id)) };
    if sizeofcmds > MAX_LOAD_COMMANDS_SIZE {
        return Ok(Some(id));
    }
    let header_len = if wide { 32 } else { 28 };
    let Some(cmds) = read_at(r, base + header_len, sizeofcmds as usize)? else { return Ok(Some(id)) };
    let mut signature = None;
    let mut at = 0usize;
    for _ in 0..ncmds {
        let (Some(cmd), Some(size)) = (word(&cmds, at), word(&cmds, at + 4)) else { break };
        let size = size as usize;
        if size < 8 || at + size > cmds.len() {
            break;
        }
        let body = &cmds[at..at + size];
        match cmd {
            LC_UUID if size >= 24 => id.uuid = Some(format_uuid(&body[8..24])),
            LC_CODE_SIGNATURE if size >= 16 => {
                if let (Some(off), Some(len)) = (word(body, 8), word(body, 12)) {
                    signature = Some((off, len));
                }
            }
            _ => {}
        }
        at += size;
    }
    if let Some((off, len)) = signature
        && len <= MAX_SIGNATURE_SIZE
        && let Some(blob) = read_at(r, base + u64::from(off), len as usize)?
        && let Some((hash_type, cdhash)) = strongest_cdhash(&blob)
    {
        id.cdhash = Some(hex(&cdhash));
        id.hash_type = Some(hash_type);
    }
    Ok(Some(id))
}

/// The CDHash of the strongest code directory in an embedded-signature SuperBlob.
pub fn strongest_cdhash(superblob: &[u8]) -> Option<(u8, [u8; CDHASH_LEN])> {
    let be = |at: usize| -> Option<u32> { Some(u32::from_be_bytes(superblob.get(at..at + 4)?.try_into().ok()?)) };
    if be(0)? != CSMAGIC_EMBEDDED_SIGNATURE {
        return None;
    }
    let length = (be(4)? as usize).min(superblob.len());
    let count = be(8)? as usize;
    let mut best: Option<(u8, &[u8])> = None;
    for i in 0..count {
        let entry = 12 + i * 8;
        if entry + 8 > length {
            break;
        }
        let (slot, offset) = (be(entry)?, be(entry + 4)? as usize);
        let is_cd = slot == CSSLOT_CODEDIRECTORY
            || (CSSLOT_ALTERNATE_CODEDIRECTORIES
                ..CSSLOT_ALTERNATE_CODEDIRECTORIES + CSSLOT_ALTERNATE_CODEDIRECTORY_MAX)
                .contains(&slot);
        if !is_cd || offset + 8 > length || be(offset)? != CSMAGIC_CODEDIRECTORY {
            continue;
        }
        let blob_len = be(offset + 4)? as usize;
        if blob_len < CODEDIRECTORY_MIN_LEN || offset + blob_len > length {
            continue;
        }
        let blob = &superblob[offset..offset + blob_len];
        let hash_type = blob[CODEDIRECTORY_HASH_TYPE];
        if priority(hash_type) > best.map_or(0, |(t, _)| priority(t)) {
            best = Some((hash_type, blob));
        }
    }
    let (hash_type, blob) = best?;
    Some((hash_type, cdhash_of(hash_type, blob)?))
}

/// The kernel's preference among hash types (higher is stronger); 0 for an unknown type.
fn priority(hash_type: u8) -> u8 {
    match hash_type {
        HASH_SHA384 => 4,
        HASH_SHA256 => 3,
        HASH_SHA256_TRUNCATED => 2,
        HASH_SHA1 => 1,
        _ => 0,
    }
}

/// The CDHash of one CodeDirectory blob with its hash type.
pub fn cdhash_of(hash_type: u8, code_directory: &[u8]) -> Option<[u8; CDHASH_LEN]> {
    let full: Vec<u8> = match hash_type {
        HASH_SHA1 => sha1::Sha1::digest(code_directory).to_vec(),
        HASH_SHA256 | HASH_SHA256_TRUNCATED => sha2::Sha256::digest(code_directory).to_vec(),
        HASH_SHA384 => sha2::Sha384::digest(code_directory).to_vec(),
        _ => return None,
    };
    full.get(..CDHASH_LEN)?.try_into().ok()
}

/// Reads `len` bytes at `offset`; `None` when the file ends first.
fn read_at<R: Read + Seek>(r: &mut R, offset: u64, len: usize) -> io::Result<Option<Vec<u8>>> {
    r.seek(SeekFrom::Start(offset))?;
    let mut buf = Vec::new();
    let read = r.by_ref().take(len as u64).read_to_end(&mut buf)?;
    Ok((read == len).then_some(buf))
}

fn format_uuid(b: &[u8]) -> String {
    let h = hex(b).to_ascii_uppercase();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::io::Cursor;

    use russet_testkit::debug::macho::{TestCodeDirectory, signed_test_macho, synthetic_ios_executable, universal};

    use super::*;

    const UUID: [u8; 16] = [0x4c, 0x4c, 0x44, 0xa9, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];

    fn expected(hash_type: u8, cd: &[u8]) -> String {
        hex(&cdhash_of(hash_type, cd).unwrap())
    }

    #[test]
    fn a_signed_thin_file_gives_its_uuid_and_the_cdhash_of_its_code_directory() {
        let built = signed_test_macho(UUID, b"code v1", &[TestCodeDirectory::new(HASH_SHA256, b"slots v1")], b"cms 1");
        let id = read_identity(&mut Cursor::new(&built.bytes)).unwrap().unwrap();
        assert_eq!(id.uuid.as_deref(), Some("4C4C44A9-0001-0203-0405-060708090A0B"));
        assert_eq!(id.hash_type, Some(HASH_SHA256));
        let cdhash = id.cdhash.unwrap();
        assert_eq!(cdhash, expected(HASH_SHA256, &built.code_directories[0]));
        assert_eq!(cdhash.len(), 2 * CDHASH_LEN);
        assert!(cdhash.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)));
    }

    #[test]
    fn re_signing_changes_the_file_but_not_the_cdhash_and_a_code_change_does() {
        let cd = [TestCodeDirectory::new(HASH_SHA256, b"slots v1")];
        let first = signed_test_macho(UUID, b"code v1", &cd, b"signed at 09:00:00");
        let again = signed_test_macho(UUID, b"code v1", &cd, b"signed at 09:00:07");
        assert_ne!(first.bytes, again.bytes);
        let id = |b: &[u8]| read_identity(&mut Cursor::new(b)).unwrap().unwrap().cdhash;
        assert_eq!(id(&first.bytes), id(&again.bytes));
        let changed = signed_test_macho(UUID, b"code v2", &[TestCodeDirectory::new(HASH_SHA256, b"slots v2")], b"x");
        assert_ne!(id(&first.bytes), id(&changed.bytes));
    }

    #[test]
    fn the_strongest_code_directory_wins() {
        let built = signed_test_macho(
            UUID,
            b"code",
            &[
                TestCodeDirectory::new(HASH_SHA1, b"sha1 slots"),
                TestCodeDirectory::new(HASH_SHA256, b"sha256 slots"),
                TestCodeDirectory::new(HASH_SHA256_TRUNCATED, b"truncated slots"),
            ],
            b"cms",
        );
        let id = read_identity(&mut Cursor::new(&built.bytes)).unwrap().unwrap();
        assert_eq!(id.hash_type, Some(HASH_SHA256));
        assert_eq!(id.cdhash.unwrap(), expected(HASH_SHA256, &built.code_directories[1]));
        let sha384 = signed_test_macho(
            UUID,
            b"code",
            &[TestCodeDirectory::new(HASH_SHA256, b"a"), TestCodeDirectory::new(HASH_SHA384, b"b")],
            b"cms",
        );
        let id = read_identity(&mut Cursor::new(&sha384.bytes)).unwrap().unwrap();
        assert_eq!(id.hash_type, Some(HASH_SHA384));
        assert_eq!(id.cdhash.unwrap(), expected(HASH_SHA384, &sha384.code_directories[1]));
        let sha1_only = signed_test_macho(UUID, b"code", &[TestCodeDirectory::new(HASH_SHA1, b"a")], b"cms");
        let id = read_identity(&mut Cursor::new(&sha1_only.bytes)).unwrap().unwrap();
        assert_eq!(id.cdhash.unwrap(), expected(HASH_SHA1, &sha1_only.code_directories[0]));
    }

    #[test]
    fn universal_files_use_the_arm64_slice() {
        let x86 = signed_test_macho(UUID, b"x86", &[TestCodeDirectory::new(HASH_SHA256, b"x86")], b"c");
        let arm = signed_test_macho([7; 16], b"arm", &[TestCodeDirectory::new(HASH_SHA256, b"arm")], b"c");
        for wide in [false, true] {
            let fat = universal(&[(0x0100_0007, 3, &x86.bytes), (CPU_TYPE_ARM64, 0, &arm.bytes)], wide);
            let id = read_identity(&mut Cursor::new(&fat)).unwrap().unwrap();
            assert_eq!(id.uuid.as_deref(), Some("07070707-0707-0707-0707-070707070707"), "wide: {wide}");
            assert_eq!(id.cdhash.unwrap(), expected(HASH_SHA256, &arm.code_directories[0]));
        }
    }

    #[test]
    fn unsigned_foreign_and_broken_files() {
        let unsigned = synthetic_ios_executable(UUID);
        let id = read_identity(&mut Cursor::new(&unsigned)).unwrap().unwrap();
        assert!(id.uuid.is_some() && id.cdhash.is_none());
        assert!(read_identity(&mut Cursor::new(b"#!/bin/sh\necho hi\n")).unwrap().is_none());
        assert!(read_identity(&mut Cursor::new(b"")).unwrap().is_none());
        // A Java class file shares the universal magic.
        assert!(read_identity(&mut Cursor::new(b"\xca\xfe\xba\xbe\x00\x00\x00\x34rest")).unwrap().is_none());
        // Truncated anywhere: never a panic, never a CDHash from half a signature.
        let built = signed_test_macho(UUID, b"code", &[TestCodeDirectory::new(HASH_SHA256, b"s")], b"cms");
        for cut in 0..built.bytes.len() {
            let id = read_identity(&mut Cursor::new(&built.bytes[..cut])).unwrap();
            assert!(id.is_none_or(|i| i.cdhash.is_none()), "cut at {cut}");
        }
        // An unknown hash type is skipped.
        let odd = signed_test_macho(UUID, b"code", &[TestCodeDirectory::new(9, b"s")], b"cms");
        assert!(read_identity(&mut Cursor::new(&odd.bytes)).unwrap().unwrap().cdhash.is_none());
    }

    #[test]
    fn identity_reads_files() {
        let dir = std::env::temp_dir().join(format!("russet-macho-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Probe");
        let built = signed_test_macho(UUID, b"code", &[TestCodeDirectory::new(HASH_SHA256, b"s")], b"cms");
        std::fs::write(&path, &built.bytes).unwrap();
        let id = identity(&path).unwrap().unwrap();
        assert_eq!(id.cdhash.unwrap(), expected(HASH_SHA256, &built.code_directories[0]));
        assert!(identity(&dir.join("missing")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
