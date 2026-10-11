// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Prefixed ULID identifiers (ARCHITECTURE §7.3): `job_…`, `ses_…`, `lse_…`, `sub_…`, `apr_…`.
//! Generation and parsing are W2's (CORE-02); W1 lands the types. Newtype structs serialize as their
//! inner string with serde, schemars and ts-rs alike.

macro_rules! prefixed_id {
    ($(#[$doc:meta])* $name:ident, $prefix:literal) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
        #[cfg_attr(feature = "ts", derive(ts_rs::TS))]
        pub struct $name(String);

        impl $name {
            /// The wire prefix, including the underscore.
            pub const PREFIX: &'static str = $prefix;

            /// A fresh id: the prefix plus a ULID.
            pub fn generate() -> Self {
                todo!("W2 (CORE-02): prefixed ULID")
            }

            /// Parses and validates an id received from the wire.
            pub fn parse(s: &str) -> crate::Result<Self> {
                todo!("W2 (CORE-02): validate the prefix and the 26-character ULID")
            }

            /// The id as sent on the wire.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

prefixed_id!(
    /// A job (`job_…`, ARCHITECTURE §6.2).
    JobId,
    "job_"
);
prefixed_id!(
    /// A run, debug or Flutter session (`ses_…`).
    SessionId,
    "ses_"
);
prefixed_id!(
    /// A lease (`lse_…`, ARCHITECTURE §6.3).
    LeaseId,
    "lse_"
);
prefixed_id!(
    /// A subscription (`sub_…`, ARCHITECTURE §6.4).
    SubscriptionId,
    "sub_"
);
prefixed_id!(
    /// An approval (`apr_…`, `^apr_[0-9A-Z]{26}$`, ARCHITECTURE §6.5).
    ApprovalId,
    "apr_"
);
