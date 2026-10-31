// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Secret wrappers: values that must never reach logs, `Debug` output or the wire.

use std::fmt;

pub use secrecy::{ExposeSecret, SecretBox, SecretString};

/// Wraps a value so that `Debug` and `Display` print `***`.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Redacted<T>(pub T);

impl<T> fmt::Debug for Redacted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

impl<T> fmt::Display for Redacted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

#[cfg(test)]
mod tests {
    use super::Redacted;

    #[test]
    fn redacted_never_prints_the_value() {
        let r = Redacted("hunter2");
        assert_eq!(format!("{r:?} {r}"), "*** ***");
    }
}
