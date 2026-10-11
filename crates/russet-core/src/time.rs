// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Time and randomness (testability contract T2). `ManualClock` and `SeededEntropy` live in
//! `russet-testkit`.

use std::time::{Instant, SystemTime};

/// Wall-clock and monotonic time.
pub trait Clock: Send + Sync {
    fn now(&self) -> SystemTime;
    fn instant(&self) -> Instant;
}

/// Randomness source.
pub trait Entropy: Send + Sync {
    fn fill(&self, b: &mut [u8]);
}
