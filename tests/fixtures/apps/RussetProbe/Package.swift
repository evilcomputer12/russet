// swift-tools-version: 6.0
// SPDX-License-Identifier: 0BSD
// SPDX-FileCopyrightText: 2026 Russet contributors

// RussetProbe: the Wave-1 vertical-slice fixture (PLAN §4.5, §4.10). One library product holds the app; Russet's
// generated builder package links it into the `RussetProbe-App` executable (build §4.2).

import PackageDescription

let package = Package(
    name: "RussetProbe",
    platforms: [
        .iOS(.v17)
    ],
    products: [
        .library(name: "RussetProbe", targets: ["RussetProbe"])
    ],
    targets: [
        .target(name: "RussetProbe")
    ]
)
