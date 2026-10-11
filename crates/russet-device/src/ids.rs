// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Lease and subscription ids for the device plane: prefixed ULIDs (`lse_…`, `sub_…`; ARCHITECTURE §7.3).
//!
//! russet-core's `LeaseId::generate` belongs to W2 (CORE-02). Until it lands, the device plane makes its own
//! ULIDs (48-bit millisecond time + 80 random bits, Crockford base32) and builds the id types through their
//! serde form, which is the bare string. The format is the one W2 validates (`^lse_[0-9A-Z]{26}$`).

use russet_core::ids::{LeaseId, SubscriptionId};

const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// A fresh ULID: 26 Crockford base32 characters.
pub fn ulid() -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
        & 0xFFFF_FFFF_FFFF;
    let random = uuid::Uuid::new_v4().as_u128() & ((1u128 << 80) - 1);
    let value: u128 = (u128::from(millis) << 80) | random;
    let mut out = [0u8; 26];
    for (i, slot) in out.iter_mut().enumerate() {
        let shift = 125 - 5 * i as u32;
        *slot = CROCKFORD[((value >> shift) & 0x1F) as usize];
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A fresh `lse_…` id.
pub fn new_lease_id() -> LeaseId {
    serde_json::from_value(serde_json::Value::String(format!("{}{}", LeaseId::PREFIX, ulid())))
        .unwrap_or_else(|_| unreachable_id())
}

/// A fresh `sub_…` id.
pub fn new_subscription_id() -> SubscriptionId {
    serde_json::from_value(serde_json::Value::String(format!("{}{}", SubscriptionId::PREFIX, ulid())))
        .unwrap_or_else(|_| unreachable_id())
}

/// The id types deserialize from any string; this cannot happen.
fn unreachable_id<T>() -> T {
    panic!("prefixed ids deserialize from strings")
}

/// Parses a lease id received from the wire (`lse_` + 26 Crockford characters).
pub fn parse_lease_id(s: &str) -> russet_core::Result<LeaseId> {
    let valid = s
        .strip_prefix(LeaseId::PREFIX)
        .is_some_and(|rest| rest.len() == 26 && rest.bytes().all(|b| CROCKFORD.contains(&b)));
    if !valid {
        return Err(crate::error::invalid_argument(format!("{s:?} is not a lease id")));
    }
    serde_json::from_value(serde_json::Value::String(s.to_owned()))
        .map_err(|e| crate::error::invalid_argument(format!("{s:?} is not a lease id: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_have_the_wire_shape() {
        let l = new_lease_id();
        assert!(l.as_str().starts_with("lse_"));
        assert_eq!(l.as_str().len(), 30);
        assert!(l.as_str()[4..].bytes().all(|b| CROCKFORD.contains(&b)));
        let s = new_subscription_id();
        assert!(s.as_str().starts_with("sub_"));
        assert_ne!(new_lease_id(), new_lease_id());
        assert_eq!(parse_lease_id(l.as_str()).expect("round trip"), l);
        assert!(parse_lease_id("lse_short").is_err());
    }
}
