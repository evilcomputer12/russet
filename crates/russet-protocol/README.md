# russet-protocol

Layer **L1** (ARCHITECTURE §5.1–§5.2). Licence: AGPL-3.0-only.

- **Owner:** W1 created it. `Cargo.toml`, `src/lib.rs`, `src/registry.rs` and `src/macros.rs` are seam files,
  lead-only after W1 merges (PLAN §0 rule 2).
- **Namespace modules** `src/<namespace>.rs` belong to the area that owns the namespace (PLAN §3). W1
  pre-registered every method, notification and client-side request of ARCHITECTURE §8–§9 with placeholder
  DTOs (`Unspecified`); the owner replaces each `…Params`/`…Result` alias with a real struct.
- **DTO rules:** derive `Serialize, Deserialize, JsonSchema, TS` with `#[serde(rename_all = "camelCase")]`;
  unit enums kebab-case; data-carrying enums `#[serde(tag = "kind", rename_all = "kebab-case")]` with struct
  variants only; never `deny_unknown_fields` on inbound DTOs (ARCHITECTURE §5.2, §5.3, §7.3).
- **Contract:** ARCHITECTURE §5.3 `russet-protocol`, §7, §8, §9.
