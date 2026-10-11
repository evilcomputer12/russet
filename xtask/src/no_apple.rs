// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `cargo xtask check-no-apple`: the `guard apple-files` scan (ARCHITECTURE §19, K4). It fails on files with
//! Mach-O (thin or fat), BOMStore/CAR, NIBArchive, xar, UDIF `koly`, IMG4 or pbzx magic; on the Apple file
//! extensions `.tbd`, `.xip`, `.ipsw`, `.dmg`, `.mobileprovision`, `.p8` and `.p12`; and on Apple copyright
//! headers in the first lines of text files.
//!
//! By default it scans the git tree (tracked files plus untracked files that are not ignored); `--path DIR`
//! scans directories instead (caches, upload artifacts). The provenance allowlist for generated fixtures
//! [D 88] is [`crate::guard::provenance_allows`] (W8). Owner: W1.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::workstreams;

/// Flags of `cargo xtask check-no-apple`.
#[derive(Debug, Default, clap::Args)]
pub struct Args {
    /// Scan these directories instead of the git tree
    #[arg(long = "path", value_name = "DIR")]
    pub paths: Vec<PathBuf>,
}

/// One flagged file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// Repository-relative for the git tree; as given otherwise.
    pub path: String,
    pub reasons: Vec<&'static str>,
}

pub fn run(args: Args) -> Result<()> {
    let root = workstreams::repo_root();
    let files: Vec<(String, PathBuf)> = if args.paths.is_empty() {
        git_files(&root)?.into_iter().map(|rel| (rel.clone(), root.join(rel))).collect()
    } else {
        let mut out = Vec::new();
        for p in &args.paths {
            walk(p, &mut out).with_context(|| format!("scanning {}", p.display()))?;
        }
        out.into_iter().map(|p| (p.display().to_string(), p)).collect()
    };
    let mut findings = Vec::new();
    for (shown, path) in &files {
        let reasons = scan_file(path).with_context(|| format!("reading {shown}"))?;
        if !reasons.is_empty() {
            let finding = Finding { path: shown.clone(), reasons };
            if !crate::guard::provenance_allows(&root, &finding) {
                findings.push(finding);
            }
        }
    }
    if findings.is_empty() {
        println!("check-no-apple: {} files scanned, no Apple files", files.len());
        return Ok(());
    }
    for f in &findings {
        println!("{}: {}", f.path, f.reasons.join(", "));
    }
    bail!("check-no-apple: {} file(s) look like Apple files (ARCHITECTURE §19, K4)", findings.len())
}

/// Extensions of Apple files that never enter the repository.
const APPLE_EXTENSIONS: &[&str] = &["tbd", "xip", "ipsw", "dmg", "mobileprovision", "p8", "p12"];

/// Magic numbers at offset 0.
const MAGIC_AT_START: &[(&[u8], &str)] = &[
    (&[0xfe, 0xed, 0xfa, 0xce], "Mach-O magic"),
    (&[0xce, 0xfa, 0xed, 0xfe], "Mach-O magic"),
    (&[0xfe, 0xed, 0xfa, 0xcf], "Mach-O magic"),
    (&[0xcf, 0xfa, 0xed, 0xfe], "Mach-O magic"),
    (&[0xca, 0xfe, 0xba, 0xbe], "fat Mach-O magic"),
    (&[0xbe, 0xba, 0xfe, 0xca], "fat Mach-O magic"),
    (&[0xca, 0xfe, 0xba, 0xbf], "fat Mach-O magic"),
    (&[0xbf, 0xba, 0xfe, 0xca], "fat Mach-O magic"),
    (b"BOMStore", "BOMStore (BOM/CAR) magic"),
    (b"NIBArchive", "NIBArchive magic"),
    (b"xar!", "xar magic"),
    (b"pbzx", "pbzx magic"),
];

/// IMG4, IM4P and IM4M are DER sequences whose first element is this IA5String.
const IMG4_TAGS: &[&[u8]] = &[b"\x16\x04IMG4", b"\x16\x04IM4P", b"\x16\x04IM4M"];

/// How many leading lines of a text file may carry a copyright header.
const HEADER_LINES: usize = 40;

/// The reasons `path` looks like an Apple file (empty when it does not).
pub fn scan_file(path: &Path) -> std::io::Result<Vec<&'static str>> {
    let mut reasons = Vec::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str())
        && APPLE_EXTENSIONS.iter().any(|a| a.eq_ignore_ascii_case(ext))
    {
        reasons.push("Apple file extension");
    }
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let mut head = Vec::with_capacity(8192);
    (&mut file).take(8192).read_to_end(&mut head)?;
    for (magic, reason) in MAGIC_AT_START {
        if head.starts_with(magic) && !reasons.contains(reason) {
            reasons.push(reason);
        }
    }
    if head.len() >= 2
        && head[0] == 0x30
        && IMG4_TAGS.iter().any(|t| head[..head.len().min(16)].windows(t.len()).any(|w| w == *t))
    {
        reasons.push("IMG4 magic");
    }
    if len >= 512 {
        let mut trailer = [0u8; 4];
        file.seek(SeekFrom::Start(len - 512))?;
        file.read_exact(&mut trailer)?;
        if &trailer == b"koly" {
            reasons.push("UDIF koly trailer");
        }
    }
    let is_text = !head.contains(&0);
    if is_text && has_apple_copyright_header(&head) {
        reasons.push("Apple copyright header");
    }
    Ok(reasons)
}

/// A copyright line naming Apple Inc. (or Apple Computer) among the first [`HEADER_LINES`] lines.
fn has_apple_copyright_header(head: &[u8]) -> bool {
    // Spelled in pieces so this file does not match its own scan.
    let owners = [concat!("apple", " inc"), concat!("apple", " computer")];
    String::from_utf8_lossy(head).lines().take(HEADER_LINES).any(|line| {
        let line = line.to_ascii_lowercase();
        line.find("copyright").is_some_and(|at| owners.iter().any(|o| line[at..].contains(o)))
    })
}

/// Tracked files plus untracked files that are not ignored, repository-relative; deleted files are skipped.
fn git_files(root: &Path) -> Result<Vec<String>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-z", "--cached", "--others", "--exclude-standard"])
        .output()
        .context("running git ls-files")?;
    if !out.status.success() {
        bail!("git ls-files failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let mut files: Vec<String> = out
        .stdout
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .filter(|rel| root.join(rel).is_file())
        .collect();
    files.sort();
    files.dedup();
    Ok(files)
}

/// Every regular file under `dir`, without following symlinks.
fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(dir)?;
    if meta.is_file() {
        out.push(dir.to_path_buf());
        return Ok(());
    }
    if !meta.is_dir() {
        return Ok(());
    }
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.collect::<std::io::Result<_>>()?;
    entries.sort_by_key(|e| e.path());
    for entry in entries {
        walk(&entry.path(), out)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let p = dir.join(name);
        let mut f = File::create(&p).expect("create");
        f.write_all(bytes).expect("write");
        p
    }

    #[test]
    fn a_seeded_mach_o_header_is_caught() {
        let dir = tempfile::tempdir().expect("tempdir");
        // MH_MAGIC_64, CPU_TYPE_ARM64, MH_EXECUTE: the first bytes of an arm64 executable.
        let p = write(dir.path(), "probe", &[0xcf, 0xfa, 0xed, 0xfe, 0x0c, 0x00, 0x00, 0x01, 0, 0, 0, 0, 2, 0, 0, 0]);
        assert_eq!(scan_file(&p).expect("scan"), vec!["Mach-O magic"]);
        let mut found = Vec::new();
        walk(dir.path(), &mut found).expect("walk");
        assert_eq!(found, vec![p]);
    }

    #[test]
    fn every_magic_and_extension_is_caught() {
        let dir = tempfile::tempdir().expect("tempdir");
        let d = dir.path();
        assert_eq!(
            scan_file(&write(d, "fat", &[0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 2])).expect("scan"),
            vec!["fat Mach-O magic"]
        );
        assert_eq!(
            scan_file(&write(d, "Assets.car", b"BOMStore\0\0\0\x01")).expect("scan"),
            vec!["BOMStore (BOM/CAR) magic"]
        );
        assert_eq!(scan_file(&write(d, "x.nib", b"NIBArchive\x01\0")).expect("scan"), vec!["NIBArchive magic"]);
        assert_eq!(scan_file(&write(d, "a.pkg", b"xar!\0\x1c\0\x01")).expect("scan"), vec!["xar magic"]);
        assert_eq!(scan_file(&write(d, "Payload", b"pbzx\0\0\0\0\x01\0\0\0")).expect("scan"), vec!["pbzx magic"]);
        assert_eq!(
            scan_file(&write(d, "img", b"\x30\x83\x01\x02\x03\x16\x04IM4P\x16\x04krnl")).expect("scan"),
            vec!["IMG4 magic"]
        );
        let mut udif = vec![0u8; 4096];
        udif[4096 - 512..4096 - 508].copy_from_slice(b"koly");
        assert_eq!(scan_file(&write(d, "image.bin", &udif)).expect("scan"), vec!["UDIF koly trailer"]);
        assert_eq!(
            scan_file(&write(d, "AuthKey.p8", b"-----BEGIN PRIVATE KEY-----\n")).expect("scan"),
            vec!["Apple file extension"]
        );
        assert_eq!(scan_file(&write(d, "x.DMG", b"")).expect("scan"), vec!["Apple file extension"]);
        let header = format!("// {} (c) 2019 Apple Inc. All rights reserved.\nint x;\n", "Copyright");
        assert_eq!(scan_file(&write(d, "x.h", header.as_bytes())).expect("scan"), vec!["Apple copyright header"]);
    }

    #[test]
    fn ordinary_files_pass() {
        let dir = tempfile::tempdir().expect("tempdir");
        let d = dir.path();
        // Spelled in pieces so `reuse lint` does not read this test's string as this file's licence.
        let source = concat!("// SPDX-", "License-Identifier: MIT\nfn main() {}\n");
        assert!(scan_file(&write(d, "lib.rs", source.as_bytes())).expect("scan").is_empty());
        assert!(
            scan_file(&write(d, "NOTICE.md", b"Apple and Xcode are trademarks of Apple Inc.\n"))
                .expect("scan")
                .is_empty()
        );
        assert!(scan_file(&write(d, "zeros", &[0u8; 1024])).expect("scan").is_empty());
        assert!(scan_file(&write(d, "empty", b"")).expect("scan").is_empty());
    }
}
