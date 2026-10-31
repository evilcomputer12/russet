// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! What this binary is (core §5.1 `build_info`): its version, its build id `<version>+g<sha>[-dirty]` and
//! target triple, and whether it is a development build (ARCHITECTURE §3.3 item 3).

/// The package version, for example `0.1.0`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// `<version>+g<sha>[-dirty]`, from `RUSSET_BUILD_ID` or git at build time (`build.rs`).
pub const BUILD_ID: &str = env!("RUSSET_BUILD_ID");

/// The target triple this binary was built for.
pub const TARGET: &str = env!("RUSSET_TARGET");

/// Development builds (`debug_assertions`, or a build id ending in `-dirty`) compare the daemon's build id and
/// executable identity, not just the protocol version, and refuse the `live` device backend without
/// `RUSSET_LANE=A` (ARCHITECTURE §3.3 items 3 and 7).
pub fn is_development() -> bool {
    cfg!(debug_assertions) || BUILD_ID.ends_with("-dirty")
}

/// The packaging that built this binary (`deb`, `rpm`, `arch`, `msi`, `winget`), set with `RUSSET_PACKAGED` at
/// build time; package-managed installs never update themselves (core §17).
pub fn packaged_by() -> Option<&'static str> {
    option_env!("RUSSET_PACKAGED").filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_build_id_starts_with_the_version() {
        assert!(BUILD_ID.starts_with(VERSION), "{BUILD_ID}");
        assert!(BUILD_ID.contains('+'), "{BUILD_ID}");
        assert!(!TARGET.is_empty());
        // Tests are built with debug assertions.
        assert!(is_development());
    }
}
