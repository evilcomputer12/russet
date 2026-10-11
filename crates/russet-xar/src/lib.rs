// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Read-only Apple container formats for SDK and DDI import [D 62, D 69]: xar (TOC checksum and the prehashed
//! RSA signature chained to the runtime-pinned Apple Root CA), pbzx, gzip, cpio, UDIF and a read-only HFS+
//! reader (ARCHITECTURE §5.1, §16.3). Streaming, size-capped, never writes through a link.
//!
//! Layer F (MIT). F0 stub (W1); W16b fills it. ARCHITECTURE §5.3 names no cross-area signature for this crate.
