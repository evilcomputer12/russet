// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The embedded code-signature superblob (`LC_CODE_SIGNATURE`): code directories, requirements, entitlements
//! (XML and DER) and the CMS signature wrapper. Everything here is big-endian.

use std::collections::BTreeMap;

use crate::MachOError;
use crate::consts::*;
use crate::read::{bytes, cstr, u8_at, u32_be, u64_be, usize_of};

/// A code directory's hash algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum HashType {
    Sha1,
    Sha256,
    /// SHA-256 truncated to 20 bytes.
    Sha256Truncated,
    Sha384,
    Unknown(u8),
}

impl HashType {
    /// From the code directory's `hashType` byte.
    pub fn from_raw(v: u8) -> Self {
        match v {
            CS_HASHTYPE_SHA1 => Self::Sha1,
            CS_HASHTYPE_SHA256 => Self::Sha256,
            CS_HASHTYPE_SHA256_TRUNCATED => Self::Sha256Truncated,
            CS_HASHTYPE_SHA384 => Self::Sha384,
            other => Self::Unknown(other),
        }
    }

    /// The `hashType` byte.
    pub fn raw(self) -> u8 {
        match self {
            Self::Sha1 => CS_HASHTYPE_SHA1,
            Self::Sha256 => CS_HASHTYPE_SHA256,
            Self::Sha256Truncated => CS_HASHTYPE_SHA256_TRUNCATED,
            Self::Sha384 => CS_HASHTYPE_SHA384,
            Self::Unknown(v) => v,
        }
    }

    /// A stable lower-case name (`sha1`, `sha256`, `sha256-truncated`, `sha384`, `unknown-<n>`).
    pub fn name(self) -> String {
        match self {
            Self::Sha1 => "sha1".into(),
            Self::Sha256 => "sha256".into(),
            Self::Sha256Truncated => "sha256-truncated".into(),
            Self::Sha384 => "sha384".into(),
            Self::Unknown(v) => format!("unknown-{v}"),
        }
    }

    /// The size of one hash slot.
    pub fn size(self) -> Option<usize> {
        match self {
            Self::Sha1 | Self::Sha256Truncated => Some(20),
            Self::Sha256 => Some(32),
            Self::Sha384 => Some(48),
            Self::Unknown(_) => None,
        }
    }

    /// The full (untruncated) digest of `data`; `None` for unknown algorithms.
    pub fn digest(self, data: &[u8]) -> Option<Vec<u8>> {
        match self {
            Self::Sha1 => {
                use sha1::Digest as _;
                Some(sha1::Sha1::digest(data).to_vec())
            }
            Self::Sha256 | Self::Sha256Truncated => {
                use sha2::Digest as _;
                Some(sha2::Sha256::digest(data).to_vec())
            }
            Self::Sha384 => {
                use sha2::Digest as _;
                Some(sha2::Sha384::digest(data).to_vec())
            }
            Self::Unknown(_) => None,
        }
    }

    /// The digest of `data` as stored in a hash slot (truncated to the slot size).
    pub fn slot_digest(self, data: &[u8]) -> Option<Vec<u8>> {
        let mut d = self.digest(data)?;
        d.truncate(self.size()?);
        Some(d)
    }
}

/// One entry of the superblob index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlobIndex {
    /// The slot (`CSSLOT_*`).
    pub slot: u32,
    /// The blob's magic (`CSMAGIC_*`).
    pub magic: u32,
    /// Offset inside the superblob.
    pub offset: u32,
    /// Blob length, header included.
    pub length: u32,
}

/// A parsed code directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeDirectory {
    /// The superblob slot it came from (0, or `0x1000…` for alternates).
    pub slot: u32,
    /// For example `0x20400`.
    pub version: u32,
    /// `CS_*` flags (`CS_ADHOC`, `CS_LINKER_SIGNED`, `CS_RUNTIME`, …).
    pub flags: u32,
    pub hash_type: HashType,
    pub hash_size: u8,
    /// Code page size in bytes; 0 means one hash over the whole code range.
    pub page_size: u32,
    pub platform: u8,
    /// The signing identifier (`CFBundleIdentifier` for main executables).
    pub identifier: String,
    /// The team identifier (code directory 0x20200+).
    pub team_id: Option<String>,
    /// End of the hashed range (`codeLimit`, or `codeLimit64` when set).
    pub code_limit: u64,
    pub n_code_slots: u32,
    pub n_special_slots: u32,
    pub exec_seg_base: Option<u64>,
    pub exec_seg_limit: Option<u64>,
    /// `CS_EXECSEG_*` flags (code directory 0x20400+).
    pub exec_seg_flags: Option<u64>,
    /// Hardened-runtime version (code directory 0x20500+).
    pub runtime: Option<u32>,
    /// Special slot → stored hash; all-zero (unused) slots are omitted.
    pub special_slots: BTreeMap<u32, Vec<u8>>,
    /// One hash per code page.
    pub code_hashes: Vec<Vec<u8>>,
    /// The CDHash: the digest of the whole blob with `hash_type`, truncated to 20 bytes.
    pub cdhash: Vec<u8>,
    raw: Vec<u8>,
}

impl CodeDirectory {
    /// Parses a code directory blob (header included).
    pub fn parse(blob: &[u8], slot: u32) -> Result<Self, MachOError> {
        if u32_be(blob, 0)? != CSMAGIC_CODEDIRECTORY {
            return Err(MachOError::Malformed("code directory magic mismatch".into()));
        }
        let length = usize_of(u64::from(u32_be(blob, 4)?), "code directory length")?;
        let blob = bytes(blob, 0, length, "code directory")?;
        let version = u32_be(blob, 8)?;
        let flags = u32_be(blob, 12)?;
        let hash_offset = usize_of(u64::from(u32_be(blob, 16)?), "hash offset")?;
        let ident_offset = usize_of(u64::from(u32_be(blob, 20)?), "identifier offset")?;
        let n_special_slots = u32_be(blob, 24)?;
        let n_code_slots = u32_be(blob, 28)?;
        let code_limit32 = u32_be(blob, 32)?;
        let hash_size = u8_at(blob, 36)?;
        let hash_type = HashType::from_raw(u8_at(blob, 37)?);
        let platform = u8_at(blob, 38)?;
        let page_shift = u8_at(blob, 39)?;
        let page_size = if page_shift == 0 {
            0
        } else if page_shift < 32 {
            1u32 << page_shift
        } else {
            return Err(MachOError::Malformed(format!("code directory page size 2^{page_shift}")));
        };
        let team_id = if version >= CS_SUPPORTSTEAMID {
            let off = u32_be(blob, 48)? as usize;
            if off == 0 { None } else { Some(cstr(blob, off, length, "team id")?) }
        } else {
            None
        };
        let mut code_limit = u64::from(code_limit32);
        if version >= CS_SUPPORTSCODELIMIT64 {
            let l64 = u64_be(blob, 56)?;
            if l64 != 0 {
                code_limit = l64;
            }
        }
        let (exec_seg_base, exec_seg_limit, exec_seg_flags) = if version >= CS_SUPPORTSEXECSEG {
            (Some(u64_be(blob, 64)?), Some(u64_be(blob, 72)?), Some(u64_be(blob, 80)?))
        } else {
            (None, None, None)
        };
        let runtime = if version >= CS_SUPPORTSRUNTIME { Some(u32_be(blob, 88)?) } else { None };
        let identifier = cstr(blob, ident_offset, length, "identifier")?;

        let hs = usize::from(hash_size);
        if let Some(expected) = hash_type.size()
            && expected != hs
        {
            return Err(MachOError::Malformed(format!(
                "code directory hash size {hs} does not match {}",
                hash_type.name()
            )));
        }
        let special_bytes = (n_special_slots as usize)
            .checked_mul(hs)
            .ok_or_else(|| MachOError::Malformed("special slot table overflows".into()))?;
        if special_bytes > hash_offset {
            return Err(MachOError::Malformed("special slots start before the code directory".into()));
        }
        let mut special_slots = BTreeMap::new();
        for k in 1..=n_special_slots {
            let off = hash_offset - (k as usize) * hs;
            let h = bytes(blob, off, hs, "special slot hash")?;
            if h.iter().any(|&b| b != 0) {
                special_slots.insert(k, h.to_vec());
            }
        }
        let mut code_hashes = Vec::with_capacity(n_code_slots.min(1 << 20) as usize);
        for i in 0..n_code_slots as usize {
            let off = i
                .checked_mul(hs)
                .and_then(|o| o.checked_add(hash_offset))
                .ok_or_else(|| MachOError::Malformed("code slot table overflows".into()))?;
            code_hashes.push(bytes(blob, off, hs, "code slot hash")?.to_vec());
        }
        let mut cdhash = hash_type.digest(blob).unwrap_or_default();
        cdhash.truncate(20);
        Ok(Self {
            slot,
            version,
            flags,
            hash_type,
            hash_size,
            page_size,
            platform,
            identifier,
            team_id,
            code_limit,
            n_code_slots,
            n_special_slots,
            exec_seg_base,
            exec_seg_limit,
            exec_seg_flags,
            runtime,
            special_slots,
            code_hashes,
            cdhash,
            raw: blob.to_vec(),
        })
    }

    /// The raw blob, header included.
    pub fn raw(&self) -> &[u8] {
        &self.raw
    }

    /// The untruncated digest of the blob (the full CDHash).
    pub fn full_hash(&self) -> Option<Vec<u8>> {
        self.hash_type.digest(&self.raw)
    }

    /// The stored hash of a special slot (`CSSLOT_INFOSLOT`, `CSSLOT_ENTITLEMENTS`, …).
    pub fn special_slot(&self, slot: u32) -> Option<&[u8]> {
        self.special_slots.get(&slot).map(Vec::as_slice)
    }

    /// Whether a flag of [`Self::flags`] is set.
    pub fn has_flag(&self, flag: u32) -> bool {
        self.flags & flag != 0
    }

    /// Whether an executable-segment flag is set.
    pub fn has_exec_seg_flag(&self, flag: u64) -> bool {
        self.exec_seg_flags.is_some_and(|f| f & flag != 0)
    }

    /// Recomputes the code-page hashes over `image` (the thin slice the directory signs) and returns the indices
    /// of the pages whose hash differs. An unknown hash algorithm reports every page.
    pub fn mismatched_pages(&self, image: &[u8]) -> Vec<u32> {
        let limit = usize::try_from(self.code_limit).unwrap_or(usize::MAX).min(image.len());
        let mut bad = Vec::new();
        let page = if self.page_size == 0 { limit.max(1) } else { self.page_size as usize };
        for (i, stored) in self.code_hashes.iter().enumerate() {
            let start = i.saturating_mul(page);
            let end = start.saturating_add(page).min(limit);
            let ok =
                start <= end && self.hash_type.slot_digest(&image[start.min(limit)..end]).is_some_and(|h| &h == stored);
            if !ok {
                bad.push(u32::try_from(i).unwrap_or(u32::MAX));
            }
        }
        let expected_pages = if self.page_size == 0 { 1 } else { limit.div_ceil(page) };
        if self.code_hashes.len() != expected_pages && bad.is_empty() {
            bad.push(u32::try_from(self.code_hashes.len()).unwrap_or(u32::MAX));
        }
        bad
    }
}

/// The parsed superblob of `LC_CODE_SIGNATURE`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeSignature {
    /// The primary directory (slot 0) first, then the alternates in slot order.
    pub code_directories: Vec<CodeDirectory>,
    /// The XML entitlements blob's payload.
    pub entitlements_xml: Option<String>,
    /// The DER entitlements blob's payload.
    pub entitlements_der: Option<Vec<u8>>,
    /// The requirement-set blob, header included.
    pub requirements: Option<Vec<u8>>,
    /// The CMS wrapper's payload; empty for ad-hoc signatures.
    pub cms: Option<Vec<u8>>,
    /// The superblob index.
    pub blobs: Vec<BlobIndex>,
    raw: Vec<u8>,
}

impl CodeSignature {
    /// Parses the superblob (`data` starts at `LC_CODE_SIGNATURE.dataoff`).
    pub fn parse(data: &[u8]) -> Result<Self, MachOError> {
        if u32_be(data, 0)? != CSMAGIC_EMBEDDED_SIGNATURE {
            return Err(MachOError::Malformed("code signature superblob magic mismatch".into()));
        }
        let length = usize_of(u64::from(u32_be(data, 4)?), "superblob length")?;
        let data = bytes(data, 0, length, "code signature superblob")?;
        let count = u32_be(data, 8)?;
        if (count as usize).saturating_mul(8).saturating_add(12) > length {
            return Err(MachOError::Malformed(format!("superblob claims {count} blobs")));
        }
        let mut sig = Self {
            code_directories: Vec::new(),
            entitlements_xml: None,
            entitlements_der: None,
            requirements: None,
            cms: None,
            blobs: Vec::new(),
            raw: data.to_vec(),
        };
        for i in 0..count as usize {
            let slot = u32_be(data, 12 + i * 8)?;
            let offset = u32_be(data, 16 + i * 8)?;
            let off = offset as usize;
            let magic = u32_be(data, off)?;
            let blen = u32_be(data, off + 4)?;
            if blen < 8 {
                return Err(MachOError::Malformed(format!("blob in slot {slot:#x} is {blen} bytes")));
            }
            let blob = bytes(data, off, blen as usize, "blob")?;
            sig.blobs.push(BlobIndex { slot, magic, offset, length: blen });
            match magic {
                CSMAGIC_CODEDIRECTORY => sig.code_directories.push(CodeDirectory::parse(blob, slot)?),
                CSMAGIC_REQUIREMENTS => sig.requirements = Some(blob.to_vec()),
                CSMAGIC_EMBEDDED_ENTITLEMENTS => {
                    sig.entitlements_xml = Some(String::from_utf8_lossy(&blob[8..]).into_owned());
                }
                CSMAGIC_EMBEDDED_DER_ENTITLEMENTS => sig.entitlements_der = Some(blob[8..].to_vec()),
                CSMAGIC_BLOBWRAPPER => sig.cms = Some(blob[8..].to_vec()),
                _ => {}
            }
        }
        sig.code_directories.sort_by_key(|cd| cd.slot);
        Ok(sig)
    }

    /// The primary code directory (slot 0).
    pub fn code_directory(&self) -> Option<&CodeDirectory> {
        self.code_directories.iter().find(|cd| cd.slot == CSSLOT_CODEDIRECTORY)
    }

    /// The directory with the strongest hash (what the kernel prefers): SHA-384, then SHA-256, then the rest.
    pub fn best_code_directory(&self) -> Option<&CodeDirectory> {
        let rank = |h: HashType| match h {
            HashType::Sha384 => 4,
            HashType::Sha256 => 3,
            HashType::Sha256Truncated => 2,
            HashType::Sha1 => 1,
            HashType::Unknown(_) => 0,
        };
        self.code_directories.iter().max_by_key(|cd| rank(cd.hash_type))
    }

    /// The hash algorithms of every code directory, primary first.
    pub fn hash_types(&self) -> Vec<HashType> {
        self.code_directories.iter().map(|cd| cd.hash_type).collect()
    }

    /// The team identifier of the primary directory.
    pub fn team_id(&self) -> Option<&str> {
        self.code_directory().and_then(|cd| cd.team_id.as_deref())
    }

    /// The signing identifier of the primary directory.
    pub fn identifier(&self) -> Option<&str> {
        self.code_directory().map(|cd| cd.identifier.as_str())
    }

    /// Whether a non-empty CMS signature is present (an ad-hoc signature has an empty one, or none).
    pub fn has_cms_signature(&self) -> bool {
        self.cms.as_ref().is_some_and(|c| !c.is_empty())
    }

    /// Whether the primary directory is ad hoc.
    pub fn is_adhoc(&self) -> bool {
        self.code_directory().is_some_and(|cd| cd.has_flag(CS_ADHOC))
    }

    /// The raw bytes of the blob in `slot` (header included), for special-slot hash checks.
    pub fn blob(&self, slot: u32) -> Option<&[u8]> {
        let b = self.blobs.iter().find(|b| b.slot == slot)?;
        self.raw.get(b.offset as usize..b.offset as usize + b.length as usize)
    }

    /// The raw superblob.
    pub fn raw(&self) -> &[u8] {
        &self.raw
    }
}
