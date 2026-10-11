# russet-sign

Bundle walk, provisioning profiles, entitlements, signing through apple-codesign, verify, lints.

- Layer **L3** (domain crate; may depend on F and L0–L2 except russet-ipc, and on the L3 edges of ARCHITECTURE §5.2) (ARCHITECTURE §5.1–§5.2). Licence: GPL-3.0-or-later.
- **Owner:** W6 (SA-04, SA-05). W1 created the crate as a compiling stub (freeze F0, PLAN §12).
- `apple-codesign` 0.29.0 with `default-features = false` (ARCHITECTURE §16.7). Its `KeyInfoSigner` trait comes from `x509-certificate` 0.24, which apple-codesign does not re-export; `russet_sign::KeyInfoSigner` re-exports it.

## What it does

- `ProvisioningProfile::parse` reads the CMS payload with `cryptographic-message-syntax` (the parser apple-codesign
  uses); the kind is derived from `get-task-allow`, the device list and `ProvisionsAllDevices`.
- `EntitlementsResolver` is the SIGN §8.4 merge (variables, required keys, glob rules per value type,
  `aps-environment` from the profile, least privilege or profile-all); `capability_for_entitlement` is the SIGN
  §6.5 map.
- `sign` copies the app (`SignOutput::CopyTo`) or signs in place; runs the pre-sign fixes (chmod 0755, string-pool
  alignment when sdk ≥ 27.0, stale `_CodeSignature`/`.DS_Store`/`__MACOSX` removal, opt-in thinning); checks each
  app-like bundle's profile (bundle, team, certificate, distribution, expiry); embeds it; signs inside-out with one
  `SigningSettings` (key and team global, entitlements per bundle path; frameworks and dylibs get none); then
  verifies, and runs the lint profile if asked. `.app` inputs only; `verify`, `lint` and `inspect` also read
  `.ipa` files (unpacked safely to a temporary directory).
- `verify` = apple-codesign's verifier (primary code directory, CMS, embedded slots) + every alternate code
  directory's pages + the Info.plist and CodeResources slots + every sealed resource and coverage + L-SIG-001…007.
- `lint` adds L-MACHO-001…004 and L-BND-001…010 (`device-install`, `app-store`); a build product that is unsigned
  or only linker-signed skips the L-SIG rules with an info finding.
- `inspect` reports every slice with team IDs masked and no certificate subject.
- The `test-pki` feature provides synthetic identities and CMS-signed profiles for tests.
