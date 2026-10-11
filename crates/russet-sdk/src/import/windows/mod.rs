// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The Windows materializer for SDK imports: junctions, hardlinks, the copy fallback, the short root and the
//! path-budget check (ARCHITECTURE §16.1, §16.3) [D 69].
//!
//! Pre-created by W1 (PLAN §3 [D 86]); W27b owns it (W16b writes the first implementation, PLAN §5.8).
