// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Archive, dSYMs, the store IPA, validation, metadata stamping, upload, TestFlight, App Store versions and
//! review, and the Apple-hardware mode (SIGN §9).
//!
//! W12 (Wave 2) implements the archive half: [`create_archive`] lays out an `.xcarchive` (SIGN §9.1) a Mac user
//! can open in Xcode Organizer, [`generate_dsyms`] runs `dsymutil` on the unstripped binaries (SIGN §9.1), and
//! [`package_ipa`] writes the store/development IPA through the deterministic `russet-ipa` writer (SIGN §9.3).
//! Both the archive and the IPA are byte-reproducible for identical inputs. W23 (Wave 4) fills the rest
//! (validation, stamping, upload, TestFlight, metadata, review, Apple-hardware CI).

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::sync::Arc;

mod archive;
mod dsym;
mod ipa;

pub use archive::{Archive, ArchiveRequest, archive_app, create_archive};
pub use dsym::generate_dsyms;
pub use ipa::{IpaInfo, package_ipa};

use camino::Utf8Path;
use russet_core::Result;
use russet_core::error::{Error, codes};

/// How an archive is exported (SIGN §9.2). The wire value is the kebab form (`development`, `ad-hoc`,
/// `app-store-connect`, `enterprise`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportMethod {
    Development,
    AdHoc,
    AppStoreConnect,
    Enterprise,
}

impl ExportMethod {
    /// The wire value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Development => "development",
            Self::AdHoc => "ad-hoc",
            Self::AppStoreConnect => "app-store-connect",
            Self::Enterprise => "enterprise",
        }
    }

    /// Parses a wire value (also accepts `app-store` and `appstore` for `app-store-connect`).
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "development" => Some(Self::Development),
            "ad-hoc" | "adhoc" => Some(Self::AdHoc),
            "app-store-connect" | "app-store" | "appstore" => Some(Self::AppStoreConnect),
            "enterprise" | "in-house" | "inhouse" => Some(Self::Enterprise),
            _ => None,
        }
    }

    /// Whether this method deflates (store) or stores (development installs) the IPA entries (SIGN §9.3).
    pub fn is_store(self) -> bool {
        !matches!(self, Self::Development)
    }
}

/// An invalid-input error of this crate (`ship.ipaInvalid`).
pub(crate) fn invalid(message: impl Into<String>) -> Error {
    Error::new(codes::ship::IPA_INVALID, message)
        .with_hint("Pass a built `.app` directory or an `.xcarchive` produced by `russet ship archive`.")
}

/// An archive-creation error (`ship.archiveFailed`).
pub(crate) fn archive_failed(message: impl Into<String>) -> Error {
    Error::new(codes::ship::ARCHIVE_FAILED, message)
        .with_hint("Check that the build product and its dSYMs exist and are readable.")
}

/// Reads a bundle-aware `.app` and returns a usable UTF-8 path, mapping a non-UTF-8 path to an error.
pub(crate) fn utf8(path: &std::path::Path) -> Result<&Utf8Path> {
    Utf8Path::from_path(path).ok_or_else(|| invalid(format!("{} is not a UTF-8 path", path.display())))
}

/// The default reproducible timestamp for archives and IPAs: `SOURCE_DATE_EPOCH` when set, else Apple's
/// reference date 2001-01-01 (SIGN §9.1, §9.3). Re-exported from `russet-ipa` so callers need not depend on it.
pub fn default_epoch() -> std::time::SystemTime {
    russet_ipa::default_epoch()
}

/// Doctor checks of this crate, for russetd's doctor seam.
pub fn checks() -> Vec<Arc<dyn russet_doctor::Check>> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::time::{Duration, UNIX_EPOCH};

    use camino::{Utf8Path, Utf8PathBuf};

    use crate::{ArchiveRequest, ExportMethod, archive_app, create_archive, package_ipa};

    /// Builds a minimal `.app` under `dir`: an `Info.plist`, a Mach-O-magic executable, a resource and a symlink.
    fn make_app(dir: &Utf8Path, bundle_id: &str) -> Utf8PathBuf {
        let app = dir.join("Hello.app");
        std::fs::create_dir_all(&app).unwrap();
        let mut info = plist::Dictionary::new();
        info.insert("CFBundleIdentifier".into(), plist::Value::String(bundle_id.into()));
        info.insert("CFBundleExecutable".into(), plist::Value::String("Hello".into()));
        info.insert("CFBundleShortVersionString".into(), plist::Value::String("1.2".into()));
        info.insert("CFBundleVersion".into(), plist::Value::String("34".into()));
        info.insert("CFBundlePackageType".into(), plist::Value::String("APPL".into()));
        plist::Value::Dictionary(info).to_file_xml(app.join("Info.plist")).unwrap();
        // A thin arm64 Mach-O magic header so the IPA writer marks it executable.
        std::fs::write(app.join("Hello"), [0xCF, 0xFA, 0xED, 0xFE, 0, 0, 0, 1, b'h', b'i']).unwrap();
        std::fs::write(app.join("data.txt"), b"resource\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(app.join("Hello"), std::fs::Permissions::from_mode(0o755)).unwrap();
            std::os::unix::fs::symlink("Hello", app.join("Current")).unwrap();
        }
        app
    }

    fn tmp() -> (tempfile::TempDir, Utf8PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let p = Utf8PathBuf::from_path_buf(d.path().to_path_buf()).unwrap();
        (d, p)
    }

    #[test]
    fn archive_has_the_xcarchive_layout() {
        let (_g, root) = tmp();
        let app = make_app(&root, "com.evilcomputer12.russet.hello");
        let out = root.join("out");
        std::fs::create_dir_all(&out).unwrap();

        let mut req = ArchiveRequest::new(app, "Hello", ExportMethod::AppStoreConnect);
        req.created = UNIX_EPOCH + Duration::from_secs(978_307_200);
        req.team_id = Some("ABCDE12345".into());
        let archive = create_archive(&req, &out).unwrap();

        assert!(archive.path.join("Info.plist").is_file());
        assert!(archive.path.join("Products/Applications/Hello.app/Info.plist").is_file());
        assert!(archive.path.join("Products/Applications/Hello.app/Hello").is_file());
        assert_eq!(archive.app_rel, "Products/Applications/Hello.app");
        assert_eq!(archive.bundle_id, "com.evilcomputer12.russet.hello");
        assert_eq!(archive.short_version, "1.2");
        assert_eq!(archive.build_version, "34");

        let info = plist::Value::from_file(archive.path.join("Info.plist")).unwrap();
        let d = info.as_dictionary().unwrap();
        assert_eq!(d.get("ArchiveVersion").and_then(plist::Value::as_signed_integer), Some(2));
        let props = d.get("ApplicationProperties").and_then(plist::Value::as_dictionary).unwrap();
        assert_eq!(
            props.get("CFBundleIdentifier").and_then(plist::Value::as_string),
            Some("com.evilcomputer12.russet.hello")
        );
        assert_eq!(props.get("Team").and_then(plist::Value::as_string), Some("ABCDE12345"));
    }

    #[test]
    fn archive_digest_is_identical_across_two_runs() {
        // SIGN acceptance: the archive SHA-256 is identical across two runs.
        let (_g, root) = tmp();
        let app = make_app(&root, "com.evilcomputer12.russet.hello");
        let fixed = UNIX_EPOCH + Duration::from_secs(978_307_200);
        let run = |name: &str| {
            let out = root.join(name);
            std::fs::create_dir_all(&out).unwrap();
            let mut req = ArchiveRequest::new(app.clone(), "Hello", ExportMethod::AppStoreConnect);
            req.created = fixed;
            create_archive(&req, &out).unwrap().digest
        };
        assert_eq!(run("a"), run("b"), "the archive digest must be reproducible");

        // Replacing an existing archive is a no-op for the digest.
        let expected = run("b");
        let out = root.join("a");
        let mut req = ArchiveRequest::new(app, "Hello", ExportMethod::AppStoreConnect);
        req.created = fixed;
        assert_eq!(create_archive(&req, &out).unwrap().digest, expected);
    }

    #[test]
    fn archive_includes_dsyms() {
        let (_g, root) = tmp();
        let app = make_app(&root, "com.example.app");
        let dsym = root.join("Hello.app.dSYM");
        std::fs::create_dir_all(dsym.join("Contents/Resources/DWARF")).unwrap();
        std::fs::write(dsym.join("Contents/Resources/DWARF/Hello"), b"dwarf").unwrap();

        let out = root.join("out");
        std::fs::create_dir_all(&out).unwrap();
        let mut req = ArchiveRequest::new(app, "Hello", ExportMethod::Development);
        req.dsyms = vec![dsym];
        let archive = create_archive(&req, &out).unwrap();
        assert_eq!(archive.dsyms, vec![Utf8PathBuf::from("dSYMs/Hello.app.dSYM")]);
        assert!(archive.path.join("dSYMs/Hello.app.dSYM/Contents/Resources/DWARF/Hello").is_file());
    }

    #[test]
    fn ipa_is_reproducible_and_reads_the_bundle() {
        let (_g, root) = tmp();
        let app = make_app(&root, "com.example.app");

        let a = package_ipa(&app, ExportMethod::AppStoreConnect, &root.join("a.ipa")).unwrap();
        let b = package_ipa(&app, ExportMethod::AppStoreConnect, &root.join("b.ipa")).unwrap();
        assert_eq!(a.sha256, b.sha256, "the IPA must be byte-reproducible");
        assert_eq!(a.bundle_id, "com.example.app");
        assert_eq!(a.short_version, "1.2");
        assert_eq!(a.build_version, "34");
        assert!(a.size > 0);

        let dev = package_ipa(&app, ExportMethod::Development, &root.join("dev.ipa")).unwrap();
        assert_ne!(dev.sha256, a.sha256);
    }

    #[test]
    fn archive_app_finds_the_single_app() {
        let (_g, root) = tmp();
        let app = make_app(&root, "com.example.app");
        let out = root.join("out");
        std::fs::create_dir_all(&out).unwrap();
        let archive = create_archive(&ArchiveRequest::new(app, "Hello", ExportMethod::Development), &out).unwrap();
        let found = archive_app(&archive.path).unwrap();
        assert_eq!(found.file_name(), Some("Hello.app"));
    }

    #[test]
    fn export_method_round_trips() {
        for (s, m) in [
            ("development", ExportMethod::Development),
            ("ad-hoc", ExportMethod::AdHoc),
            ("app-store-connect", ExportMethod::AppStoreConnect),
            ("enterprise", ExportMethod::Enterprise),
        ] {
            assert_eq!(ExportMethod::parse(s), Some(m));
            assert_eq!(m.as_str(), s);
        }
        assert_eq!(ExportMethod::parse("app-store"), Some(ExportMethod::AppStoreConnect));
        assert!(ExportMethod::parse("nope").is_none());
        assert!(!ExportMethod::Development.is_store());
        assert!(ExportMethod::AppStoreConnect.is_store());
    }
}
