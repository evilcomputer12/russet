// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Stack adapters (build §9.1): detect, prepare, compile, resources. Detection order: `russet.toml [project]
//! stack`, then Flutter, Xcode, React Native, MAUI, SwiftPM.

/// Flutter (W17; `aot.rs` W24).
pub mod flutter;
/// .NET MAUI (W29; study only for the first build, Q30).
pub mod maui;
/// React Native / Expo (W29).
pub mod rn;
/// SwiftPM through Swift Build (W5).
pub mod swiftpm;
/// Xcode projects (W28).
pub mod xcode;
