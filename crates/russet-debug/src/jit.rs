// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The JIT servicer and touch plans (RD-10; RD §7).
//!
//! W7 lands the pure [`touch_plan`] (RD §7.5 S2), which the host JIT demo (H3) already services with; W18 builds
//! the servicer, the arming sequence and the statistics on it.
//!
//! A debugger write is what makes XNU associate a page of a dual-mapped JIT region with the debugger (on TXM
//! devices, `txm_associate_debug_region`), so every page of an announced region needs at least one written
//! byte. The plan writes `IHELPED!` at the start (Dart's diagnostic), then one two-byte write across each
//! second page boundary, so `n` pages take `1 + ⌈(n − 1) / 2⌉` writes, all inside the region.

use russet_gdbremote::codec::hex_encode;

/// Dart's diagnostic marker on page 0 (RD §7.1).
pub const MARKER: &[u8; 8] = b"IHELPED!";

/// One write of a plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TouchWrite {
    pub addr: u64,
    pub len: u64,
}

/// The writes that touch every page overlapping `[base, base + size)` with pages of `page` bytes. Every byte
/// written lies inside the region; for page-aligned input there are `1 + ⌈(n − 1) / 2⌉` writes for `n` pages.
pub fn touch_writes(base: u64, size: u64, page: u64) -> Vec<TouchWrite> {
    if size == 0 || page == 0 {
        return Vec::new();
    }
    let end = base.saturating_add(size);
    let first = base - base % page;
    let n = (end - first).div_ceil(page);
    let mut out = vec![TouchWrite { addr: base, len: size.min(MARKER.len() as u64) }];
    let mut k = 1u64;
    while k < n {
        let boundary = first + (k + 1) * page;
        if k + 1 < n {
            // The last byte of page k and the first byte of page k + 1; page k + 1 starts before `end`.
            out.push(TouchWrite { addr: boundary - 1, len: 2 });
            k += 2;
        } else {
            // An odd tail: one byte in the last page, inside the region.
            out.push(TouchWrite { addr: (boundary - 1).min(end - 1), len: 1 });
            k += 1;
        }
    }
    out
}

/// The `M` packets of [`touch_writes`], writing `fill` bytes (zeros in production: fresh pages are zero, so the
/// writes change nothing; the host demo verifies with a non-zero fill) and the marker on page 0.
pub fn touch_plan_with(base: u64, size: u64, page: u64, fill: u8) -> Vec<Vec<u8>> {
    touch_writes(base, size, page)
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let data: Vec<u8> = if i == 0 { MARKER[..w.len as usize].to_vec() } else { vec![fill; w.len as usize] };
            format!("M{:x},{:x}:{}", w.addr, w.len, hex_encode(&data)).into_bytes()
        })
        .collect()
}

/// RD §17.2's `touch_plan`: the production plan (zero fill).
pub fn touch_plan(base: u64, size: u64, page: u64) -> Vec<Vec<u8>> {
    touch_plan_with(base, size, page, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: u64 = 16384;

    /// Checks the invariants of RD §23.1 for one region.
    fn check(base: u64, size: u64) {
        let writes = touch_writes(base, size, PAGE);
        let end = base + size;
        for w in &writes {
            assert!(w.addr >= base && w.addr + w.len <= end, "write {w:?} outside [{base:#x}, {end:#x})");
        }
        let first = base / PAGE;
        let last = (end - 1) / PAGE;
        for p in first..=last {
            let (lo, hi) = (p * PAGE, (p + 1) * PAGE);
            assert!(
                writes.iter().any(|w| w.addr < hi && w.addr + w.len > lo),
                "page {p} untouched for {base:#x}+{size:#x}"
            );
        }
        if base.is_multiple_of(PAGE) && size.is_multiple_of(PAGE) {
            let n = size / PAGE;
            assert_eq!(writes.len() as u64, 1 + (n - 1).div_ceil(2), "write count for {n} pages");
        }
    }

    #[test]
    fn aligned_regions_of_1_to_64_pages() {
        for n in 1..=64 {
            check(0x1_2800_0000, n * PAGE);
        }
    }

    #[test]
    fn unaligned_bases_and_sizes() {
        for n in 1..=64u64 {
            for (shift, trim) in [(1, 0), (0, 1), (PAGE - 3, 5), (100, PAGE / 2), (PAGE - 1, PAGE - 2)] {
                let size = n * PAGE - trim;
                if size > 0 {
                    check(0x1_2800_0000 + shift, size);
                }
            }
        }
        // Tiny regions, also across a page boundary.
        for size in 1..=9 {
            check(0x1_2800_4000 - 4, size);
            check(0x1_2800_4000, size);
        }
    }

    #[test]
    fn a_512_kib_region_is_17_packets_with_the_marker_first() {
        let plan = touch_plan(0x1_2800_0000, 512 * 1024, PAGE);
        assert_eq!(plan.len(), 17);
        assert_eq!(plan[0], b"M128000000,8:4948454c50454421");
        assert_eq!(plan[1], b"M128007fff,2:0000");
        assert_eq!(plan[16], b"M12807ffff,1:00");
        assert_eq!(touch_plan_with(0x4000, PAGE * 2, PAGE, 0x5a)[1], b"Mbfff,1:5a");
        assert_eq!(touch_plan_with(0x1000, PAGE * 2, PAGE, 0x5a)[1], b"M7fff,2:5a5a");
    }

    #[test]
    fn degenerate_input() {
        assert!(touch_writes(0x1000, 0, PAGE).is_empty());
        assert!(touch_writes(0x1000, 10, 0).is_empty());
    }
}
