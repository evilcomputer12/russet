# russet-account

Accounts, identities, DevPortal, provisioning planner and applier, free-team policy.

- Layer **L3** (domain crate; may depend on F and L0–L2 except russet-ipc, and on the L3 edges of ARCHITECTURE §5.2) (ARCHITECTURE §5.1–§5.2). Licence: GPL-3.0-or-later.
- **Owner:** W6 (SA-06 subset, Wave 1), then W12 (Wave 2); `src/free/` is W22's. W1 created the crate as a compiling stub (freeze F0, PLAN §12).

## What it does (Wave 1)

- `AccountStore`: `accounts.toml` (ARCHITECTURE §14.4; kebab-case, mode 0600), `add_asc_key` (checks the key with a
  GET-scoped probe and discovers the team from a certificate's OU, a local identity or a bundle seed ID), and the
  explicit one-time xtool import [D 50]: `XTLAuthToken` (Swift `AuthToken` JSON) and the identity property list
  named after the issuer (PKCS#1 RSA key, DER certificate). The import needs no network (the team comes from the
  certificate), seals both keys, and always returns the `chmod 600` / `xtool auth logout` advice.
- `IdentityStore`: public parts in `data/identities/<team>/<serial>.{json,cer}`, keys sealed as
  `signing-key/<team>/<serial>`; `load` and `import_p12` are async because the secret store is.
- `AscPortal` implements `DevPortal` over russet-asc (per-request GET scopes); its mutations refuse until W12.
- `Provisioner::plan` is GET-only: the identity whose certificate the portal lists, devices, exact bundle IDs,
  additive capabilities, and an existing profile checked for kind, validity, certificate, devices, App ID and
  entitlements — Russet-named first, then any other, so the Wave-1 slice reuses xtool's `XTL-…` profile with 0
  mutations. `apply` re-plans (`provision.planStale` on drift) and caches a zero-mutation plan's profiles; a plan with
  mutations needs an approval (`perm.approvalRequired`) and is applied by W12.
- `ProjectSigning` reads the signing keys of `russet.toml` + `russet.local.toml` and the bundle-ID policies of
  ARCHITECTURE §16.5 (a lenient subset until W3's `ConfigService` and W5's `BundleIdPolicy` land).
- `AccountService` (`DaemonCtx.accounts`) ties it together: plans kept 10 minutes for `provision.apply`,
  `verify`, `plan_project`, and `sign_app` (identity and per-bundle profile from the cache, signing on a blocking
  thread).
- Doctor: `account.configured`, `account.identity`, `account.xtool-files` (files only, no network, no secrets).

## Local, env-gated checks (never in CI)

`tests/local_owner.rs` imports the owner's xtool store into a temporary home (`RUSSET_LOCAL_XTOOL=1`), re-signs a copy
of the M1 `Hello.app` and compares it with xtool's signature (`RUSSET_LOCAL_M1_HELLO=<Hello.app>`), and plans the
`XTL-…` bundle read-only against App Store Connect (`RUSSET_LIVE_ASC=1`). It asserts structure only and prints no
identifier.
