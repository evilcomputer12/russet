// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The error-code registry (ARCHITECTURE §13.2–§13.3): one module per family, each owned by one workstream.
//!
//! Seam file: lead-only after W1 merges (PLAN §0 rule 2). Workstreams add codes only in their own family
//! file; the list of families and [`all_codes`] live here so nobody edits another card's file.

use std::sync::LazyLock;

use super::ErrorCode;

pub mod account;
pub mod agents;
pub mod anisette;
pub mod app;
pub mod assets;
pub mod build;
pub mod config;
pub mod core;
pub mod crash;
pub mod debug;
pub mod device;
pub mod flutter_build;
pub mod flutter_run;
pub mod gsa;
pub mod ipc;
pub mod mcp;
pub mod perf;
pub mod perm;
pub mod provision;
pub mod screen;
pub mod sdk;
pub mod secrets;
pub mod ship;
pub mod sign;
pub mod toolchain;
pub mod xcode;
pub mod xctest;

/// One code family: its module, `russetCode` prefix, number ranges (ARCHITECTURE §13.2) and codes.
#[derive(Debug, Clone, Copy)]
pub struct Family {
    /// Module name under `russet_core::error::codes`.
    pub module: &'static str,
    /// `russetCode` prefix before the dot; `flutter_build` and `flutter_run` share `flutter`.
    pub prefix: &'static str,
    /// Inclusive number ranges. The `assets`/`flutter`/`xcode` split of 4100–4199 and the `gsa`/`anisette`
    /// split of 5250–5299 follow the codes ARCHITECTURE §13.3 assigns.
    pub ranges: &'static [(u16, u16)],
    /// The family's `ALL` array.
    pub codes: &'static [ErrorCode],
}

/// Every family, in range order.
pub const FAMILIES: &[Family] = &[
    Family { module: "core", prefix: "core", ranges: &[(1000, 1099)], codes: self::core::ALL },
    Family { module: "ipc", prefix: "ipc", ranges: &[(1100, 1199)], codes: ipc::ALL },
    Family { module: "perm", prefix: "perm", ranges: &[(1200, 1299)], codes: perm::ALL },
    Family { module: "config", prefix: "config", ranges: &[(1300, 1399)], codes: config::ALL },
    Family { module: "secrets", prefix: "secrets", ranges: &[(1400, 1499)], codes: secrets::ALL },
    Family { module: "toolchain", prefix: "toolchain", ranges: &[(2000, 2099)], codes: toolchain::ALL },
    Family { module: "sdk", prefix: "sdk", ranges: &[(2100, 2199)], codes: sdk::ALL },
    Family { module: "device", prefix: "device", ranges: &[(3000, 3099), (3200, 3299)], codes: device::ALL },
    Family { module: "app", prefix: "app", ranges: &[(3100, 3199)], codes: app::ALL },
    Family { module: "build", prefix: "build", ranges: &[(4000, 4099)], codes: build::ALL },
    Family { module: "assets", prefix: "assets", ranges: &[(4100, 4119)], codes: assets::ALL },
    Family { module: "flutter_build", prefix: "flutter", ranges: &[(4120, 4149)], codes: flutter_build::ALL },
    Family { module: "xcode", prefix: "xcode", ranges: &[(4150, 4199)], codes: xcode::ALL },
    Family { module: "sign", prefix: "sign", ranges: &[(5000, 5099)], codes: sign::ALL },
    Family { module: "account", prefix: "account", ranges: &[(5100, 5199)], codes: account::ALL },
    Family { module: "provision", prefix: "provision", ranges: &[(5200, 5249)], codes: provision::ALL },
    Family { module: "gsa", prefix: "gsa", ranges: &[(5250, 5269)], codes: gsa::ALL },
    Family { module: "anisette", prefix: "anisette", ranges: &[(5270, 5299)], codes: anisette::ALL },
    Family { module: "debug", prefix: "debug", ranges: &[(6000, 6099)], codes: debug::ALL },
    Family { module: "crash", prefix: "crash", ranges: &[(6100, 6149)], codes: crash::ALL },
    Family { module: "flutter_run", prefix: "flutter", ranges: &[(6150, 6199)], codes: flutter_run::ALL },
    Family { module: "ship", prefix: "ship", ranges: &[(7000, 7199)], codes: ship::ALL },
    Family { module: "screen", prefix: "screen", ranges: &[(8000, 8099)], codes: screen::ALL },
    Family { module: "perf", prefix: "perf", ranges: &[(8100, 8199)], codes: perf::ALL },
    Family { module: "xctest", prefix: "xctest", ranges: &[(8200, 8299)], codes: xctest::ALL },
    Family { module: "mcp", prefix: "mcp", ranges: &[(9000, 9099)], codes: mcp::ALL },
    Family { module: "agents", prefix: "agents", ranges: &[(9100, 9199)], codes: agents::ALL },
];

/// Every registered code: the concatenation of the families' `ALL` arrays, in range order.
pub fn all_codes() -> &'static [ErrorCode] {
    static ALL_CODES: LazyLock<Vec<ErrorCode>> =
        LazyLock::new(|| FAMILIES.iter().flat_map(|f| f.codes.iter().copied()).collect());
    &ALL_CODES
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// Exit codes of ARCHITECTURE §13.4 (0 and 2 never come from an error code).
    const EXIT_CODES: &[u8] = &[1, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 130];

    #[test]
    fn numbers_and_names_are_unique() {
        let mut numbers = BTreeSet::new();
        let mut names = BTreeSet::new();
        for c in all_codes() {
            assert!(numbers.insert(c.number()), "duplicate error number {}", c.number());
            assert!(names.insert(c.as_str()), "duplicate russetCode {}", c.as_str());
        }
        assert_eq!(numbers.len(), all_codes().len());
    }

    #[test]
    fn codes_sit_in_their_family_range_and_prefix() {
        for f in FAMILIES {
            for c in f.codes {
                let n = u16::try_from(c.number()).expect("codes are positive and below 65536");
                assert!(
                    f.ranges.iter().any(|(lo, hi)| (*lo..=*hi).contains(&n)),
                    "{} ({}) is outside the {} ranges {:?}",
                    c.as_str(),
                    n,
                    f.module,
                    f.ranges
                );
                let (prefix, rest) = c.as_str().split_once('.').expect("russetCode is family.name");
                assert_eq!(prefix, f.prefix, "{} has the wrong prefix for family {}", c.as_str(), f.module);
                assert!(
                    rest.chars().next().is_some_and(|ch| ch.is_ascii_lowercase())
                        && rest.chars().all(|ch| ch.is_ascii_alphanumeric()),
                    "{} is not lowerCamelCase after the dot",
                    c.as_str()
                );
            }
        }
    }

    #[test]
    fn family_arrays_are_sorted_and_ranges_disjoint() {
        for f in FAMILIES {
            let numbers: Vec<i32> = f.codes.iter().map(|c| c.number()).collect();
            let mut sorted = numbers.clone();
            sorted.sort_unstable();
            assert_eq!(numbers, sorted, "{}::ALL is not in number order", f.module);
        }
        let mut ranges: Vec<(u16, u16, &str)> =
            FAMILIES.iter().flat_map(|f| f.ranges.iter().map(move |(lo, hi)| (*lo, *hi, f.module))).collect();
        ranges.sort_unstable();
        for w in ranges.windows(2) {
            assert!(w[0].1 < w[1].0, "ranges of {} and {} overlap", w[0].2, w[1].2);
        }
    }

    #[test]
    fn exit_codes_follow_the_table() {
        for c in all_codes() {
            assert!(EXIT_CODES.contains(&c.exit_code()), "{} has exit code {}", c.as_str(), c.exit_code());
            if c.as_str().ends_with(".notFound") {
                assert_eq!(c.exit_code(), 3, "{}: *.notFound exits 3", c.as_str());
            }
        }
        assert_eq!(self::core::CANCELLED.exit_code(), 130);
        assert_eq!(self::core::DISK_FULL.exit_code(), 4);
        assert_eq!(self::core::INTERNAL.exit_code(), 1);
        assert_eq!(app::LAUNCH_FAILED.exit_code(), 9);
    }

    #[test]
    fn registry_holds_every_architecture_code() {
        // ARCHITECTURE §13.3 lists 163 codes; a family file may add more, never fewer.
        assert!(all_codes().len() >= 163, "only {} codes registered", all_codes().len());
        assert_eq!(device::LIVE_BACKEND_REFUSED.number(), 3019);
        assert_eq!(self::core::NOT_IMPLEMENTED.as_str(), "core.notImplemented");
        assert_eq!(debug::JIT_PAGE_MISSED.number(), 6016);
    }
}
