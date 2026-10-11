# russet-plist

XML and binary plist I/O and Xcode build-setting macro expansion (build B01).

- Layer **F** (format/protocol library; permissive; may depend only on F crates) (ARCHITECTURE §5.1–§5.2). Licence: MIT.
- **Owner:** W5 (B01); W16 (the OpenStep writer).
- Reading goes through the `plist` crate (XML, binary and OpenStep); writing is Russet's own, so a property list
  read from an Apple-written file re-serializes byte for byte (`tests/sdk_round_trip.rs` checks the SDK's own
  `SDKSettings.plist`, `SystemVersion.plist`, the platform `Info.plist` and the version plists when
  `RUSSET_TEST_SDK_BUNDLE` points at a Darwin SDK bundle).
- `macros::SettingsStack` expands `$(VAR)`, `${VAR}`, `$VAR`, nested names, `$(inherited)` and the operators
  `lower`, `upper`, `rfc1034identifier`, `c99extidentifier`, `identifier`, `quote`, `dir`, `file`, `base`,
  `suffix`, `standardizepath` and `default=`.
