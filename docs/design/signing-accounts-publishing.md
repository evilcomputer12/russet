# Signing, accounts, provisioning and publishing

> **Lead note (2026-10-11):** where this document conflicts with [docs/ARCHITECTURE.md](../ARCHITECTURE.md), ARCHITECTURE.md wins; §20 there lists every resolved conflict (names, codes, files, crates), and §20.1 lists the superseded fields of this document. Work is scheduled in [docs/PLAN.md](../PLAN.md). Review round 1 (same day) rewrote §9.9 (the Apple-hardware template and its signing modes) and added the App Store and TestFlight methods (ARCHITECTURE §8.4, [D 64], [D 76]).

**Status:** design draft v1, 2026-10-11. **Area:** code signing, Apple accounts (paid and free),
provisioning, secret storage, archiving and publishing. **Crates owned:** `russet-secrets`,
`russet-asc`, `russet-account`, `russet-gsa`, `russet-anisette` (+ `russet-adi-host` binary),
`russet-macho`, `russet-sign`, `russet-ship`.

**Binding inputs:** [decisions.md](../decisions.md), [verified-facts.md](../verified-facts.md),
[demo/m1-baseline.sh](../../demo/m1-baseline.sh), and [landscape report](../research/landscape-2026-10.md)
§2.6, §2.7, §2.12, §2.13, §5.1 and risks R3, R4, R5, R9, R10.

**How to read this.** Implementation agents: start with §19 (ordered tasks), then read the section
each task points to. Markers used throughout:

- **[V n]**: verified hands-on in this session; see the table in §2, row n.
- **[S]**: external source, linked inline or in §22.
- **[L sg-xx]**: a fact-checked record of the landscape report.
- **[A]**: assumption. A task in §19 must verify it before relying on it.

Nothing in this document contains key material, team IDs, UDIDs or personal names. Identifiers seen
during verification are redacted.

---

## 0. Summary

1. **Signing (`russet-sign`).** It uses **apple-codesign 0.29.0** (MPL-2.0) as a library with
   `default-features = false`, so no AWS SDK gets pulled in. Russet adds what rcodesign lacks for
   iOS:
   - parsing and selecting provisioning profiles;
   - merging entitlements per bundle, from the project and the profile;
   - signing inside-out: app, extensions, frameworks and dylibs;
   - fixes before signing: exec bits and LINKEDIT string-pool alignment;
   - verification after signing;
   - iOS 26/27 lints.

   apple-codesign already sets `CS_EXECSEG_MAIN_BINARY` on every `MH_EXECUTE` (app and appex). It
   sets `ALLOW_UNSIGNED` only when `get-task-allow` is true. The team ID propagates to all nested
   code [V9].
2. **Paid accounts (the default; `russet-asc` + `russet-account`).** Russet has its own async App
   Store Connect client and does not use the `app-store-connect` 0.7.0 crate [V10].
   - The client covers a curated, typed subset of Apple's OpenAPI spec 4.5.1 [V6].
   - Enums are open, because Apple returns capability types that are missing from the spec [V5].
   - Tokens are ES256 JWTs. GET-only **scoped** tokens are used for read-only work and for AI
     agents. A mismatched scope is rejected with `403 FORBIDDEN.REQUEST_DOES_NOT_MATCH_SCOPE` [V3].
   - The provisioning engine is idempotent and uses **plan → approve → apply**. Paid teams keep
     their **exact** bundle IDs.
3. **Free Apple ID (opt-in; `russet-gsa` + `russet-anisette`).**
   - `russet-gsa` handles GrandSlam SRP-6a, 2FA and the Xcode app token. The client identity is
     data-driven, because Apple changed it on 2026-09-10 [S xtool #257].
   - `russet-anisette` generates anisette locally from Apple's Android `libstoreservicescore.so` and
     `libCoreADI.so`. They are fetched at runtime from Apple's CDN with HTTP range requests, about
     2 MB instead of the 142 MB APK [V12]. They run inside a sandboxed helper process, using our own
     clean-room ELF loader. The existing Rust loader, `android-loader`, has **no licence** [V13].
   - developerservices2 is driven through the same typed client as the public API, which is
     xtool's approach [S].
4. **Secrets (`russet-secrets`).**
   - Backend: keyring 4.2.0 (Secret Service or Windows Credential Manager).
   - Envelope encryption covers blobs larger than the Windows 2,560-byte credential limit [V16].
   - Headless Linux gets a passphrase-protected file store. CI gets environment-variable injection.
   - Key material is never printed. A scrubber redacts tokens in all logs.
5. **Publishing (`russet-ship`).**
   - Builds an `.xcarchive`-layout archive with dSYMs (llvm `dsymutil`) and writes the IPA natively.
   - Runs about 45 offline App Store checks.
   - Offers **opt-in, visible** metadata stamping in two tiers: SDK facts, and Xcode identity.
   - Uploads through the documented build-upload API [V7].
   - Drives TestFlight and metadata through the API. Screenshots go through the App Asset Library,
     because the legacy screenshot resources are deprecated [V8].
   - **Apple-hardware release mode:**
     - A GitHub Actions macOS runner (`xcode-27`, public preview [V18]) archives the app **unsigned**.
       By default Russet then signs, validates and uploads it locally, so no Apple credentials go to
       GitHub (§9.9, ARCHITECTURE [D 76]).
     - Xcode Cloud is supported only for teams that already have a product. Product creation needs
       Xcode on a Mac, and `ciProducts` is GET-only [V6].
6. **Safety.** Every change to the developer account goes through plan → approve → apply. AI agents
   can read and plan; only a human can approve. Revocations always need a typed confirmation.

---

## 1. Context and boundaries

### 1.1 What exists today (M1 baseline)

- `xtool dev run --usb` provisions, signs and installs with the owner's App Store Connect team key.
  It renames the bundle to `XTL-<issuer prefix>.<bundle id>` and creates a new development
  certificate without revoking old ones (verified-facts, 2026-10-11).
- xtool shells out to the `zip` binary. Russet must write IPAs natively (verified-facts).
- The xtool-signed `Hello.app` installed and launched on the iPhone 12 (iOS 27.0). Its signature is
  the "golden" reference for our signer [V11].

### 1.2 This area owns

- Turning a built `.app` (unsigned, linker-signed or xtool-signed) into a device-installable signed
  bundle or IPA.
- Everything that touches the Apple developer account: certificates, devices, bundle IDs,
  capabilities and profiles.
- App Store Connect: uploads, TestFlight, metadata and CI.
- Secret storage for all Russet credentials.

### 1.3 Interfaces consumed from other areas

| From | What | Rust type here (owned by the other area) |
|---|---|---|
| Build | Built `.app` path(s), unstripped binaries for dSYMs, requested entitlements per target (from the project editors), version and build number | `BuildProducts { app: PathBuf, debug_binaries: Vec<PathBuf>, targets: Vec<TargetEntitlements> }` |
| SDK import | Metadata of the user's imported Xcode/SDK, used for stamping and lints | `SdkMetadata { sdk_canonical_name, sdk_version, sdk_build, xcode_version, xcode_build }` |
| Device daemon | Connected devices (registration) and install of the signed product | `DeviceIdentity { udid, name, product_type, os_version }` |
| IDE extension | Wizard UI, approval dialogs | JSON-RPC (§10) |
| MCP server | Agent access | Tool schemas (§11) |

`SdkMetadata` values come from the user's own SDK bundle, never from constants. On this host they
are `iphoneos27.0`, SDK build `24A430`, Xcode `27.0` / `27A266a` [V17].

---

## 2. Facts verified hands-on in this session

All App Store Connect calls were **GET-only**, and every JWT carried a GET-only `scope` claim, so
Apple itself would reject any mutation. Key material and identifiers were never printed.

| # | Fact | How it was verified |
|---|---|---|
| V1 | The owner's team key is ES256 P-256 (`AuthKey_<10-char id>.p8`, mode 0600). xtool keeps the same key's PEM **inline** in `~/.config/xtool/data/XTLAuthToken` with mode **0644**. xtool's development identity (private key and certificate, binary plist) is also 0644. | `stat` and a JSON/plist **shape** dump (values not printed) |
| V2 | `GET /v1/certificates`, `/devices`, `/bundleIds?include=bundleIdCapabilities`, `/profiles`, `/apps`, `/betaGroups`, `/builds`, `/ciProducts` and `/users` all return 200 with GET-scoped tokens. The response header `X-Rate-Limit: user-hour-lim:3600;user-hour-rem:3599;` shows a limit of 3600 requests per hour. | Python probe (`cryptography` ES256, urllib) |
| V3 | A token scoped to `GET /v1/certificates` and used on `GET /v1/devices` gets **403 `FORBIDDEN.REQUEST_DOES_NOT_MATCH_SCOPE`**. A scoped GET token with a 60-minute lifetime on `/v1/certificates` gets **401 `NOT_AUTHORIZED`**, which confirms the 20-minute cap. | Probe |
| V4 | The team holds 10 certificates: 4 `DEVELOPMENT`, one of them API-created with the display name **"Created via API"**; 2 `DISTRIBUTION`; 2 `DEVELOPER_ID_APPLICATION`; 2 `MAC_INSTALLER_DISTRIBUTION`. The team also has 7 devices, with modern UDIDs of 25 characters and an old Mac UDID of 36. 10 bundle IDs, all `platform=UNIVERSAL`, including xtool's, which xtool creates with `IOS`. 1 active profile, 1 app record, 4 beta groups and 0 Xcode Cloud products. | Probe; xtool source `DeveloperServicesAddAppOperation.swift` |
| V5 | `include=bundleIdCapabilities` returned the capability types `DRIVERKIT_FAMILY_NETWORKING_PUB`, `DRIVERKIT_PUBLIC` and `DRIVERKIT_USBTRANSPORT_PUB`, which are **not** in the spec's `CapabilityType` enum, so enums must be open. `filter[identifier]=com.x` is a **prefix** match: it returns `com.x` and `com.x.y`. | Probe |
| V6 | The ASC OpenAPI spec is **4.5.1** (zip `Last-Modified: 2026-10-06`), with 1,001 paths. There is no `POST /v1/apps` (app records are web-only) and no app-group or iCloud-container resources. `ciProducts` is GET/DELETE only, while `ciWorkflows` POST and `ciBuildRuns` POST exist. `buildUploads`/`buildUploadFiles` exist, with `assetType` ∈ {ASSET, ASSET_DESCRIPTION, ASSET_SPI} and `sourceFileChecksums.file.algorithm` ∈ {MD5, SHA_256}. | `curl -I` on Apple's zip URL; the spec parsed locally |
| V7 | Apple's current docs (fetched 2026-10-11) still describe the build-upload flow: create a build upload, reserve a file, upload, commit with `PATCH`, then the `BUILD_UPLOAD_STATE_UPDATED` webhook. The 4.1 release notes introduced it. The help page "Upload builds" requires the role Account Holder, Admin, App Manager or Developer. | docs JSON via `curl` |
| V8 | Apple's docs mark the `appScreenshots`, `appScreenshotSets`, `appPreviews` and `appPreviewSets` resources as **deprecated** in favour of the **App Asset Library** and its placements (`/v1/apps/{id}/assetLibrary`, `appAssetLibraryImages`, `appAssetLibraryPlacements`). | docs JSON via `curl` |
| V9 | apple-codesign 0.29.0 source:<br>• `create_code_directory` sets `MAIN_BINARY` on every `MH_EXECUTE` and code-signing page size 4096;<br>• `plist_to_executable_segment_flags` maps `get-task-allow`/`run-unsigned-code` to `ALLOW_UNSIGNED`;<br>• the team ID is global (cloned into nested settings);<br>• entitlements are scoped per `SettingsScope::Path`, so frameworks get none;<br>• DER entitlements are emitted;<br>• auto DR for WWDR certificates is `identifier … and anchor apple generic and certificate leaf[subject.CN] = … and certificate 1[field.1.2.840.113635.100.6.2.1]`. | `cargo info` + source read |
| V10 | The `app-store-connect` 0.7.0 crate (Apache-2.0 OR MIT):<br>• blocking reqwest;<br>• a token minted once ("TODO need to handle token expiration");<br>• covers bundle IDs, certificates, devices, profiles and notary only;<br>• `generate_signing_certificate` writes the private key to a PEM file. | Source read |
| V11 | The xtool-signed `Hello` that ran on iOS 27.0:<br>• CodeDirectory v`0x20400`, with both a SHA-1 and a SHA-256 CD;<br>• `execSegFlags=0x11` (MAIN_BINARY \| ALLOW_UNSIGNED);<br>• signed entitlements `application-identifier`, `com.apple.developer.team-identifier`, `get-task-allow` and `keychain-access-groups=[<TEAM>.<bundle id>]` (the profile's `<TEAM>.*` resolved);<br>• a DER entitlements blob is present;<br>• `LC_BUILD_VERSION sdk 17.0` equals `minos`;<br>• `LC_SYMTAB stroff % 8 == 0`;<br>• no `DT*` keys.<br>Its API-created development profile has TTL 365 days, `Platform=[iOS, xrOS, visionOS]`, `PPQCheck=true` and a `DER-Encoded-Profile`, and covers 4 devices. Profile entitlements: `application-identifier`, `keychain-access-groups=[<TEAM>.*, com.apple.token]`, `get-task-allow`, team ID. | Our own Mach-O signature parser over the M1 output (`orchard-lab/Hello`) |
| V12 | The Apple Music APK at `apps.mzstatic.com/content/android-apple-music-apk/applemusic.apk`:<br>• 142,139,820 bytes, `x-apple-version-number: 4.9.6.1447`, `Last-Modified 2025-04-15`;<br>• **range requests return 206**;<br>• its central directory (272,130 bytes, 4,468 entries) lists `lib/{arm64-v8a,armeabi-v7a,x86,x86_64}/{libCoreADI.so,libstoreservicescore.so}`; x86_64 compressed 1,093,336 + 901,029 bytes, arm64 1,352,399 + 932,080 bytes.<br>Only these two libraries are needed (Provision README; xtool `XADIProvider.swift`). | `curl -I`; a 64 KiB tail range; central-directory range; nothing else downloaded |
| V13 | Licences:<br>• `Dadoum/android-loader`: **none** (GitHub `license=null`);<br>• `Dadoum/Provision`: LGPL-2.1 (`COPYING-LGPLv2`);<br>• `xtool-org/xadi`: LGPL-2.1, Linux and macOS only;<br>• SideStore `apple-private-apis` (omnisette): MPL-2.0, depends on android-loader;<br>• Impactor: MIT (`plume_core` MPL-2.0);<br>• isideload: MIT;<br>• `Dadoum/anisette-v3-server`: none;<br>• `apple-codesign-quick`: LGPL-2.1-or-later. | `gh api repos/...`, crate metadata |
| V14 | Impactor vendors apple-codesign 0.29.0 with only three patches: `chmod 0755` on Mach-O files before signing; no hard failure on bundles without `CFBundleIdentifier`; public `macho_universal::*` and mutable Mach-O iterators. | `diff` against the crates.io source |
| V15 | `gsa.apple.com` chains to **Apple Root CA**, which is not in the system store (`openssl` verify code 19). Its SHA-256 is `B0:B1:73:0E:CB:C7:FF:45:05:14:2C:49:F1:29:5E:6E:DA:6B:CA:ED:7E:2C:68:C5:BE:91:B5:A1:10:01:F0:24`, valid to 2035-02-09, and identical to `apple.com/appleca/AppleIncRootCertificate.cer`. `api.appstoreconnect.apple.com` and `developerservices2.apple.com` chain to DigiCert public roots (verify OK). | `openssl s_client`, `openssl x509 -fingerprint` |
| V16 | Windows `CREDENTIAL.CredentialBlobSize` is at most `CRED_MAX_CREDENTIAL_BLOB_SIZE` (5×512 = 2,560 bytes). isideload's keyring code notes that Linux binary secrets misbehave and that base64 blobs are too long on Windows. | Microsoft Learn; isideload `keyring_storage.rs` |
| V17 | The local SDK bundle `~/.swiftpm/swift-sdks/darwin.artifactbundle` holds:<br>• `iPhoneOS27.0.sdk`: `SDKSettings.json` CanonicalName `iphoneos27.0`, Version `27.0`; `SystemVersion.plist` ProductBuildVersion `24A430`;<br>• `Xcode.app/Contents/version.plist`: CFBundleShortVersionString `27.0`, ProductBuildVersion `27A266a`. | `plistlib` read |
| V18 | GitHub Actions runner labels (re-checked 2026-10-11): `xcode-27` (arm64, **public preview**; macOS 27.0.1, default Xcode 27.0 27A266a, plus 27.1 and 27.2 beta 2) and `xcode-27-xlarge`; `macos-26`/`macos-latest` (arm64, Xcode 26.0.1–26.6 only); no `macos-27` label. Standard macOS runners have 3 M1 cores, 7 GB RAM and 14 GB SSD. | `actions/runner-images` README and image readmes; #14404 |
| V19 | The host's Secret Service is KDE `ksecretd` (`org.freedesktop.secrets` on the session bus). | `busctl --user list` |
| V20 | The Mac oracle (`ssh mac`, 192.168.1.8:22) was **unreachable** (two connection timeouts). Every Mac-dependent check is deferred to tasks SA-05 and SA-16. | `ssh` |
| V21 | `keyring` 4.2.0 (MIT OR Apache-2.0, on keyring-core 1.0.0; the `v1` feature uses zbus Secret Service, Windows native and Apple keychain); `jsonwebtoken` 11.1.0 (MIT); `p12-keystore` 0.4.0; `rcgen` 0.14.10; `zip` 9.0.1; `plist` 1.10.1; `reqwest` 0.13.5. apple-codesign has had no release since **0.29.0 (2024-11-29)**; its main branch receives only dependency bumps (last commit 2026-08-09). | `cargo +stable info`, `gh api` |

Note for the toolchain area: `rustup` on this host has no default toolchain. `cargo +stable` works.

---

## 3. Architecture

### 3.1 Crates and dependencies

```text
russet-cli / russetd (daemon) / Russet IDE extension / MCP server
        |                  JSON-RPC (§10), MCP (§11)
        v
russet-ship ───────────────┬───────────── russet-account ──────────── russet-gsa ── russet-anisette ─┐
  archive, ipa, stamp,     │                plan/apply provisioning,    SRP, 2FA,    LocalAdi | RemoteV3 │
  validate, upload,        │                identities (CSR, p12),      app tokens,               spawns │
  TestFlight, metadata,    │                devices, capabilities,      DS2 (QH65B2 + v1)          v     │
  apple-hardware CI        │                free-team policy                                russet-adi-host│
        │                  │                          │                                    (ELF loader,   │
        v                  v                          v                                     sandboxed)    │
russet-sign ── apple-codesign 0.29 (MPL-2.0)     russet-asc  (typed ASC client: JWT | DS2 auth)           │
  profiles, entitlements, bundle walk,                                                                     │
  sign, verify, lints                                                                                      │
        │                                                                                                  │
        v                                                                                                  │
russet-macho (LC_BUILD_VERSION, LC_SYMTAB alignment, signature parsing) ───────── russet-secrets ◄─────────┘
                                                                                  keyring 4.2 + envelope store
```

| Crate | Licence | Purpose | Main dependencies |
|---|---|---|---|
| `russet-secrets` | AGPL-3.0-only | Secret storage abstraction | `keyring` 4.2 (`v1` feature), `aes-gcm`, `argon2`, `zeroize`, `windows-sys` (DPAPI, optional) |
| `russet-asc` | AGPL-3.0-only | Typed async ASC client; JWT; scopes; DS2 transport hook | `reqwest` 0.13 (rustls), `p256` (ES256), `serde`, `tokio`, `url`, `base64` |
| `russet-account` | AGPL-3.0-only | Accounts, identities, provisioning engine, free-team policy | `russet-asc`, `russet-gsa`, `russet-secrets`, `russet-sign` (profile type), `rsa` 0.9, `x509-cert` 0.3, `p12-keystore` 0.4 |
| `russet-gsa` | AGPL-3.0-only | GrandSlam auth, 2FA, app tokens, developerservices2 | `num-bigint`, `sha2`, `hmac`, `pbkdf2`, `aes`, `cbc`, `aes-gcm`, `plist`, `reqwest` (custom root store) |
| `russet-anisette` | AGPL-3.0-only | Anisette providers, library acquisition, helper IPC | `reqwest`, `zip` (range-fed), `sha2`, `serde_json`, `tokio-tungstenite` (RemoteV3 only) |
| `russet-adi-host` (bin) | AGPL-3.0-only | Clean-room ELF loader + ADI calls in a separate, sandboxed process | `object` or `goblin` (ELF parsing), `libc`, `landlock`/`seccompiler` (Linux), `windows-sys` (Windows) |
| `russet-macho` | AGPL-3.0-only | Small Mach-O reader/patcher shared by sign, ship, build and debug | `object` or `goblin` |
| `russet-sign` | AGPL-3.0-only | Bundle model, profiles, entitlements merge, signing, verify, lints | `apple-codesign` 0.29.0 (`default-features=false`), `plist` 1.10, `cms`/`x509-cert` |
| `russet-ship` | AGPL-3.0-only | Archive, IPA, stamping, validation, upload, TestFlight, metadata, CI | `russet-sign`, `russet-asc`, `zip` 9, `md-5`, `sha2`, `reqwest` |

apple-codesign is MPL-2.0. Linking it into an AGPL-3.0-only program is permitted under MPL-2.0
§3.3 "Larger Work" (landscape §3.6). Russet does not fork it unless that becomes necessary.
`apple-codesign-quick` (LGPL-2.1+, 3.5k LOC, RustCrypto stack) is the documented fallback signer if
apple-codesign blocks us (R-12).

### 3.2 Process model

- `russetd` owns accounts, token caches, GSA sessions, the secrets handle, long-running jobs (upload
  and processing polls) and the `russet-adi-host` child.
- The CLI connects to `russetd`. For CI and headless use, `russet --in-process` runs the same library
  code without the daemon.
- `russet-adi-host` is spawned on demand (§7.5). It has no network access; the parent performs all
  HTTP.

### 3.3 State locations

| Item | Linux | Windows |
|---|---|---|
| Config | `$XDG_CONFIG_HOME/russet/` (`accounts.toml`, `policy.toml`, `gsa-profile.json`) | `%APPDATA%\Russet\` |
| Data | `$XDG_DATA_HOME/russet/` (`identities/`, `profiles/`, `anisette/`, `archives/`) | `%LOCALAPPDATA%\Russet\` |
| State | `$XDG_STATE_HOME/russet/` (`uploads/` journals, `audit.log`) | `%LOCALAPPDATA%\Russet\state\` |
| Secrets | Secret Service (service `dev.russet`) + `$XDG_DATA_HOME/russet/secrets/*.bin` (envelope) | Credential Manager (`dev.russet`) + `%LOCALAPPDATA%\Russet\secrets\*.bin` |
| Project-local | `.russet/signing.lock.json` (gitignored), `russet.toml` `[signing]`/`[ship]` (committed) | same |

All directories are created 0700 and files 0600 on Unix. On Windows, the default user-profile ACLs
apply.

---

## 4. `russet-secrets`

### 4.1 Requirements

- Never print or log secret bytes. `SecretBytes` has a redacting `Debug`, no `Display` and no
  `Serialize`, and is zeroized on drop.
- Store small secrets natively in the OS keyring. Store large ones as AES-256-GCM blobs on disk,
  under a data-encryption key (DEK) that lives in the keyring. This covers the Windows 2,560-byte
  limit and the Linux binary-secret quirk [V16].
- Work headless: on Linux without a Secret Service, use a passphrase-protected file store (Argon2id).
  In CI, take secrets from environment variables.

### 4.2 Public API

```rust
pub struct SecretStore { /* backend + index */ }

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct SecretId { pub kind: SecretKind, pub name: String } // e.g. (AscKey, "<key id>")

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum SecretKind {
    AscKey,          // .p8 PEM (~257 B)
    SigningKey,      // PKCS#8 DER of an RSA-2048 code-signing key (~1.2 KiB)
    AppleIdSession,  // adsid + GS token + expiry (CBOR)
    AppleIdPassword, // only with explicit "remember password" opt-in
    AnisetteState,   // adi.pb + local user UUID (envelope)
    GithubToken,     // Apple-hardware mode setup
    Dek,             // internal: 32-byte data-encryption key
}

pub struct SecretBytes(/* Zeroizing<Vec<u8>> */);

pub enum Backend { OsKeyring, PassphraseFile { path: PathBuf }, Env, Memory /* tests */ }

impl SecretStore {
    pub fn open(cfg: &SecretsConfig) -> Result<Self, SecretError>;
    pub fn backend(&self) -> BackendInfo;                 // shown by `russet doctor`
    pub fn put(&self, id: &SecretId, v: &SecretBytes) -> Result<(), SecretError>;
    pub fn get(&self, id: &SecretId) -> Result<Option<SecretBytes>, SecretError>;
    pub fn delete(&self, id: &SecretId) -> Result<(), SecretError>;
    pub fn list(&self, kind: SecretKind) -> Result<Vec<SecretId>, SecretError>; // from local index
}

pub enum SecretError { NoBackend { hint: String }, Locked, Denied, TooLarge, Corrupt, Io(std::io::Error), Backend(String) }
```

### 4.3 Layout

- **Keyring entries:** `Entry::new("dev.russet", "<kind>/<name>")`. Examples: `asc-key/<keyid>`,
  `signing-key/<team>/<cert-serial>`, `apple-id/<dsid>`, `dek/v1`.
- **Size rule:** values of 1,024 bytes or less go into the keyring directly. Larger values go into
  `secrets/<hex(sha256(kind/name))>.bin`, encrypted as
  `{version:1, nonce, ciphertext}` with AAD `"<kind>/<name>"` under the DEK.
- **Index:** `secrets/index.json` holds names only, no values. It exists because keyring v1 cannot
  enumerate entries.
- **Linux detection:** if `org.freedesktop.secrets` is absent, or `Entry::store_status()` fails, fall
  back to `PassphraseFile`. The passphrase comes from `RUSSET_SECRETS_PASSPHRASE` or an interactive
  prompt, and is stretched with Argon2id (m=64 MiB, t=3) into the DEK. `russet doctor` explains how
  to install a Secret Service.
- **Windows:** Credential Manager holds the DEK and small items. Optionally, signing keys can go into
  the CurrentUser certificate store as non-exportable CNG keys, which apple-codesign can sign with
  (`windows_store_find_code_signing_certificates`) [S apple-codesign `windows.rs`].
- **CI (`Env` backend):** `RUSSET_ASC_KEY_ID`, `RUSSET_ASC_ISSUER_ID`, `RUSSET_ASC_KEY_P8` (PEM
  text or base64), `RUSSET_SIGNING_P12_B64` + `RUSSET_SIGNING_P12_PASSWORD`, and
  `RUSSET_PROFILE_<n>_B64`. The env backend is read-only and never writes back.

### 4.4 Migrating from xtool (opt-in)

`russet account import --from-xtool` reads `~/.config/xtool/data/XTLAuthToken` (the ASC key) and
the development-identity plist. It moves them into the keyring and, if the user agrees, recommends
`chmod 600` or deleting the xtool copies after `xtool auth logout`. Both files are world-readable
today [V1].

This also lets Russet **reuse** the existing API-created development certificate (it expires
2027-10-10) instead of creating a fifth one.

---

## 5. `russet-asc`: App Store Connect client

### 5.1 Decision

| Option | Verdict |
|---|---|
| `app-store-connect` 0.7.0 (apple-platform-rs) | Rejected as the main client. It is blocking, never refreshes tokens, covers provisioning and notary only, and writes keys to plaintext PEM files [V10]. Its JWT and CSR code is a useful reference (Apache-2.0/MIT). |
| Full codegen from Apple's spec (1,001 paths, 7.5 MB JSON) | Rejected. Compile time is huge, and committing generated code derived wholesale from Apple's spec file is questionable under our "no Apple files" policy. |
| **Own client with a curated resource description** | **Chosen.** `crates/russet-asc/api/*.yaml` is written by us and lists only the resources and fields we use. `xtask asc-gen` generates `src/generated/*.rs`. `xtask asc-drift` downloads Apple's current spec zip at dev or CI time and reports drift for covered fields. The spec is never committed. |

### 5.2 Authentication

- **ES256 JWT.**
  - Header `{alg: ES256, kid, typ: JWT}`.
  - Team-key claims: `{iss, iat, exp, aud: "appstoreconnect-v1", scope?}`. Individual keys use
    `sub: "user"` instead of `iss` [S Apple, *Generating tokens*].
  - Lifetime is at most 20 minutes; Russet mints 19 minutes and refreshes when 60 seconds remain.
  - Exception: a **GET-only scope** on Xcode Cloud and metrics resources allows tokens of up to
    6 months [S]. Russet uses this only for CI monitoring tokens handed to a dashboard, and never
    persists such a token.
- **Scoped tokens** (`TokenFactory::scoped`) are used for:
  - every read-only operation requested by MCP or AI agents;
  - `russet account verify`;
  - the live test suite.

  Apple enforces the scope [V3]. It ignores `limit`, `cursor` and `sort` when matching.
- Individual API keys cannot use the provisioning endpoints [S Apple, *Creating API keys*]. Russet
  detects `sub=user` keys and disables provisioning features with `E_ASC_INDIVIDUAL_KEY_NO_PROVISIONING`.

```rust
pub struct AscKey { pub key_id: String, pub issuer: Issuer, pem: SecretBytes }
pub enum Issuer { Team { issuer_id: String }, Individual }

pub struct TokenFactory { /* AscKey + Mutex<Option<(String, Instant)>> */ }
impl TokenFactory {
    pub fn new(key: AscKey) -> Result<Self, AscError>;     // parses the P-256 key; never logs it
    pub fn bearer(&self) -> Result<String, AscError>;      // cached 19 min
    pub fn scoped(&self, scope: &[ScopeEntry], lifetime: Duration) -> Result<String, AscError>;
}
pub struct ScopeEntry { pub path: String, pub query: Option<String> } // method is always GET
```

### 5.3 Transport and pluggable auth

```rust
#[async_trait::async_trait]
pub trait Auth: Send + Sync {
    fn base_url(&self) -> &url::Url;                      // https://api.appstoreconnect.apple.com
    fn flavor(&self) -> ApiFlavor;
    async fn authorize(&self, req: &mut reqwest::Request) -> Result<(), AscError>;
}
pub enum ApiFlavor { Public, XcodeDs2 { team_id: String } }  // Ds2 implemented in russet-gsa (§7.4)

pub struct AscClient { /* reqwest::Client, Arc<dyn Auth>, RateLimiter, RetryPolicy, redacting tracer */ }
impl AscClient {
    pub fn new(auth: std::sync::Arc<dyn Auth>) -> Self;
    pub fn certificates(&self) -> CertificatesApi<'_>;
    pub fn devices(&self) -> DevicesApi<'_>;
    pub fn bundle_ids(&self) -> BundleIdsApi<'_>;
    pub fn capabilities(&self) -> CapabilitiesApi<'_>;
    pub fn profiles(&self) -> ProfilesApi<'_>;
    pub fn apps(&self) -> AppsApi<'_>;
    pub fn builds(&self) -> BuildsApi<'_>;
    pub fn build_uploads(&self) -> BuildUploadsApi<'_>;
    pub fn testflight(&self) -> TestFlightApi<'_>;     // betaGroups, betaTesters, betaBuildLocalizations, ...
    pub fn store(&self) -> StoreApi<'_>;               // appStoreVersions(+Localizations), reviewSubmissions
    pub fn asset_library(&self) -> AssetLibraryApi<'_>;
    pub fn ci(&self) -> CiApi<'_>;                     // ciProducts, ciWorkflows, ciBuildRuns, ciArtifacts
    pub fn raw_get(&self, path_and_query: &str) -> impl Future<Output = Result<serde_json::Value, AscError>>; // allowlisted GET (§11)
}
```

**JSON:API helpers:**

- `Document<T>` / `Resource<T> { id, attributes, relationships }`.
- `Paged<T>` follows `links.next`, and keeps `limit` together with `cursor` on later pages; xtool
  found that dropping `limit` breaks the cursor [S xtool `DeveloperServices+OpenAPI.swift`].
- `Open<E>` (serde untagged `Known(E) | Unknown(String)`) is used for **every** enum [V5].

**Exact-match filters.** `filter[identifier]` is a prefix match [V5]. Every `find_*` helper
post-filters for equality and raises `E_ASC_AMBIGUOUS` on duplicates.

**Rate limiting.** A token bucket allows 3,600 requests per hour per key [V2]. The client reads
`X-Rate-Limit` and slows down when fewer than 200 requests remain. On 429 it backs off
exponentially (1, 2, 4, … 60 s) with jitter.

**Retries.** GETs retry on 5xx and transport errors (3 attempts). Mutations are **not** retried
blindly. They use reconciliation: re-read the resource and decide (§9.6 step 6).

**Errors:**

```rust
pub enum AscError {
    Unauthorized { detail: String },                          // 401 NOT_AUTHORIZED (bad/expired token, >20 min)
    ScopeMismatch { detail: String },                         // 403 FORBIDDEN.REQUEST_DOES_NOT_MATCH_SCOPE [V3]
    Forbidden { code: String, detail: String },               // 403 FORBIDDEN_ERROR: role lacks permission
    NotFound { path: String },
    Conflict { code: String, detail: String, pointer: Option<String> }, // 409 ENTITY_ERROR.*, STATE_ERROR
    RateLimited { retry_after: Option<Duration> },
    Server { status: u16 },
    Transport(String), Decode { path: String, err: String },
}
```

The exact `code` strings for duplicates and certificate limits are [A]. They are captured during
SA-06 live runs (with approval) into `russet-asc/tests/fixtures/`.

### 5.4 Coverage (MVP, M5)

| Resource | Operations | Used by |
|---|---|---|
| `certificates` | list (filter type/serial), get, create (CSR), delete (revoke) | identities |
| `devices` | list (filter udid/platform/status), create, patch (status/name) | device registration |
| `bundleIds` | list (filter identifier, include capabilities/profiles), create, get | provisioning |
| `bundleIdCapabilities` | create, patch, delete | capabilities |
| `profiles` | list (filter name/type/state), get (`profileContent`), create, delete | provisioning |
| `merchantIds` | list, create | Apple Pay capability |
| `apps` | list (filter bundleId), get, `assetLibrary` | upload, metadata |
| `buildUploads`, `buildUploadFiles` | create, get, patch (`uploaded`, checksums), delete | upload |
| `builds` | list (filter app/version/processingState), get, patch (`usesNonExemptEncryption`, `expired`), betaGroups relationship | processing, TestFlight |
| `betaGroups`, `betaTesters`, `betaBuildLocalizations`, `betaAppReviewSubmissions`, `buildBetaNotifications`, `betaAppReviewDetails` | CRUD as needed | TestFlight |
| `apps/{id}/betaFeedbackCrashSubmissions`, `betaFeedbackCrashSubmissions/{id}/crashLog`, `builds/{id}/diagnosticSignatures`, `diagnosticSignatures/{id}/logs` | read | crash panel |
| `appStoreVersions`, `appStoreVersionLocalizations`, `appInfos`, `reviewSubmissions`, `reviewSubmissionItems` | create, patch, read | metadata, submission (M5+) |
| `appAssetLibraryRefData`, `apps/{id}/assetLibrary`, `appAssetLibraryImages` (+ upload ops), `appAssetLibraryPlacements`, `appAssetLibraryPlacementOrderingRequests` | read, create, delete | screenshots [V8] |
| `ciProducts`, `ciWorkflows`, `ciBuildRuns`, `ciBuildActions`, `ciArtifacts`, `ciIssues`, `ciTestResults`, `scmRepositories`, `scmGitReferences` | read; `ciBuildRuns` create; `ciWorkflows` create/patch | Xcode Cloud (§9.10) |

---

## 6. `russet-account`: accounts, identities and provisioning

### 6.1 Account model

```rust
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum AccountId { AscKey(String /* key id */), AppleId(String /* dsid */) }

pub struct Account {
    pub id: AccountId,
    pub label: String,
    pub kind: AccountKind,              // AscTeamKey | AscIndividualKey | AppleIdFree | AppleIdPaid
    pub team: Option<Team>,
    pub added_at: time::OffsetDateTime,
}
pub struct Team { pub team_id: String, pub name: Option<String>, pub tier: TeamTier }
pub enum TeamTier { Paid, Free, Enterprise, Unknown }

pub struct AccountStore { /* accounts.toml + Arc<SecretStore> */ }
impl AccountStore {
    pub fn load(paths: &Paths, secrets: std::sync::Arc<SecretStore>) -> anyhow::Result<Self>;
    pub fn list(&self) -> Vec<Account>;
    pub fn default_account(&self) -> Option<&Account>;
    pub async fn add_asc_key(&mut self, req: AddAscKey) -> Result<Account, AccountError>;
    pub async fn begin_apple_id_login(&mut self, req: AppleIdLogin) -> Result<LoginStep, AccountError>;
    pub async fn continue_apple_id_login(&mut self, s: LoginSessionId, input: LoginInput) -> Result<LoginStep, AccountError>;
    pub fn remove(&mut self, id: &AccountId, wipe_secrets: bool) -> Result<(), AccountError>;
    pub fn set_default(&mut self, id: &AccountId) -> Result<(), AccountError>;
    pub async fn portal(&self, id: &AccountId, team: Option<&str>) -> Result<std::sync::Arc<dyn DevPortal>, AccountError>;
}
pub struct AddAscKey { pub key_id: String, pub issuer_id: Option<String> /* None = individual */, pub p8_pem: SecretBytes, pub label: Option<String> }
```

**Adding a paid key** (`add_asc_key`):

1. Parse the PEM; it must be EC P-256.
2. Call `GET /v1/certificates?limit=1` with a scoped token. A 401 means a wrong issuer or key ID; a
   403 means the key's role lacks permission.
3. **Discover the team ID.** Use the subject `OU` of any team certificate's `certificateContent`.
   Fall back to the `seedId` of any bundle ID, which is the App ID prefix and usually equals the
   team ID [V4]. If neither exists, ask the user.
4. Store the PEM in the keyring and write `accounts.toml`.
5. Keep the user's original `.p8` file. Apple lets a key be downloaded only once, so the wizard
   tells the user to keep a backup [S Apple, *Creating API keys*].

### 6.2 `DevPortal`: one surface for paid and free

```rust
#[async_trait::async_trait]
pub trait DevPortal: Send + Sync {
    fn team(&self) -> &Team;
    fn flavor(&self) -> PortalFlavor;                             // PublicApi | XcodeDs2
    async fn list_certificates(&self, f: CertFilter) -> Result<Vec<CertInfo>, PortalError>;
    async fn create_certificate(&self, kind: CertKind, csr_pem: &str) -> Result<CertInfo, PortalError>;
    async fn revoke_certificate(&self, id: &str, approval: &Approval) -> Result<(), PortalError>;
    async fn list_devices(&self) -> Result<Vec<DeviceRecord>, PortalError>;
    async fn register_device(&self, udid: &str, name: &str) -> Result<DeviceRecord, PortalError>;
    async fn find_bundle_id(&self, identifier: &str) -> Result<Option<BundleIdRecord>, PortalError>; // exact
    async fn register_bundle_id(&self, identifier: &str, name: &str) -> Result<BundleIdRecord, PortalError>;
    async fn capabilities(&self, b: &BundleIdRecord) -> Result<Vec<CapabilityRecord>, PortalError>;
    async fn enable_capability(&self, b: &BundleIdRecord, c: &CapabilitySpec) -> Result<CapabilityRecord, PortalError>;
    async fn disable_capability(&self, id: &str, approval: &Approval) -> Result<(), PortalError>;
    async fn list_profiles(&self, f: ProfileFilter) -> Result<Vec<ProfileRecord>, PortalError>;
    async fn create_profile(&self, r: CreateProfile) -> Result<ProfileRecord, PortalError>;
    async fn delete_profile(&self, id: &str, approval: &Approval) -> Result<(), PortalError>;
    /// DS2 only: the public API has no app-group resources [V6]; PublicApi returns Unsupported.
    async fn ensure_app_groups(&self, b: &BundleIdRecord, groups: &[String]) -> Result<Vec<String>, PortalError>;
}
pub enum CertKind { Development /* DEVELOPMENT */, Distribution /* DISTRIBUTION */ }
pub enum ProfileKind { Development, AdHoc, AppStore, InHouse }  // IOS_APP_DEVELOPMENT|ADHOC|STORE|INHOUSE
```

- `AscPortal` implements it over `russet-asc` with JWT auth.
- `Ds2Portal` implements it over the same `russet-asc` typed API with `Ds2Auth`, plus the QH65B2
  legacy actions for teams and app groups (§7.4).

### 6.3 Signing identities

```rust
pub struct IdentityStore { /* data dir + Arc<SecretStore> */ }
pub struct SigningIdentity {
    pub id: IdentityId,                  // "<team>/<serial>"
    pub team_id: String, pub kind: CertKind,
    pub serial: String, pub asc_certificate_id: Option<String>,
    pub common_name: String, pub not_after: time::OffsetDateTime,
    pub label: String,                   // "Russet on <hostname> (<date>)"; Apple names API certs "Created via API" [V4]
    pub cert_der: Vec<u8>,               // public, stored as identities/<team>/<serial>.cer
}
pub struct LoadedIdentity { /* SigningIdentity + key (zeroized) implementing apple_codesign::KeyInfoSigner */ }

impl IdentityStore {
    pub fn list(&self, team: Option<&str>) -> Result<Vec<SigningIdentity>, IdentityError>;
    pub fn new_csr(&self, team: &str, kind: CertKind) -> Result<(PendingKey, String /* CSR PEM */), IdentityError>;
    pub fn commit(&self, pending: PendingKey, cert_der: &[u8], asc_id: Option<String>) -> Result<SigningIdentity, IdentityError>;
    pub fn import_p12(&self, p12: &[u8], password: &SecretBytes) -> Result<SigningIdentity, IdentityError>;
    pub fn export_p12(&self, id: &IdentityId, password: &SecretBytes) -> Result<Vec<u8>, IdentityError>;
    pub fn import_xtool(&self, xtool_data_dir: &std::path::Path) -> Result<Vec<SigningIdentity>, IdentityError>;
    pub fn load(&self, id: &IdentityId) -> Result<LoadedIdentity, IdentityError>;
    pub fn forget(&self, id: &IdentityId) -> Result<(), IdentityError>;   // local only, never revokes
}
```

- **Key and CSR.** The key is RSA-2048, generated with `rsa` and `OsRng`. The CSR is PKCS#10 with
  SHA-256/RSA and subject `CN=Russet <hostname>`, built with `x509-cert`'s builder. Apple ignores the
  CN for API-created certificates [V4].
- **Storage.** The private key, as PKCS#8 DER, goes into the keyring as `SigningKey`. The `.cer` goes
  to disk. A pending key survives a crash between CSR and `commit`, under the name
  `signing-key/<team>/pending-<uuid>`; it is garbage-collected after 1 hour.
- **Lookup.** A team certificate is mapped to a local identity by serial number
  (`filter[serialNumber]`), with leading zeros normalised as omarchy does [S omarchy `asc.py`].

### 6.4 Provisioning engine

```rust
pub struct ProvisionRequest {
    pub bundles: Vec<BundleSpec>,                 // app first, then extensions/app clip/watch
    pub distribution: Distribution,               // Development | AdHoc | AppStore | InHouse
    pub devices: DeviceSelection,                 // Connected(Vec<DeviceIdentity>) | AllEnabled | Udids(Vec<String>)
    pub identity: IdentityPreference,             // Auto | Specific(IdentityId)
}
pub struct BundleSpec { pub rel_path: String, pub role: BundleRole, pub bundle_id: String, pub requested: plist::Dictionary }

pub struct ProvisionPlan {
    pub plan_id: uuid::Uuid, pub digest: [u8; 32],          // the approval binds to this digest
    pub portal: PortalFlavor, pub team_id: String,
    pub mutations: Vec<Mutation>, pub reads_only: bool,
    pub warnings: Vec<PlanWarning>,                         // e.g. "device slots 11..100 can take 24–72 h"
}
pub enum Mutation {
    CreateCertificate { kind: CertKind },
    RevokeCertificate { id: String, display: String, expires: String },     // never auto-approved
    RegisterDevice { udid_redacted: String, name: String },
    RegisterBundleId { identifier: String, consumes_free_quota: bool },
    EnableCapability { bundle: String, capability: String, settings: serde_json::Value },
    DisableCapability { bundle: String, capability: String },               // never auto-approved
    CreateProfile { name: String, kind: ProfileKind, bundle: String, devices: usize },
    DeleteProfile { id: String, name: String, russet_owned: bool },
    EnsureAppGroups { bundle: String, groups: Vec<String> },
}
pub struct ProvisionOutcome { pub identity: SigningIdentity, pub bundles: Vec<BundleProvision> }
pub struct BundleProvision {
    pub rel_path: String, pub bundle_id_requested: String, pub bundle_id_registered: String,
    pub profile: russet_sign::ProvisioningProfile, pub profile_path: std::path::PathBuf,
}

pub struct Provisioner { /* Arc<dyn DevPortal>, IdentityStore, ProvisionPolicy */ }
impl Provisioner {
    pub async fn plan(&self, req: &ProvisionRequest) -> Result<ProvisionPlan, ProvisionError>;    // GET-only
    pub async fn apply(&self, plan: &ProvisionPlan, approval: &Approval) -> Result<ProvisionOutcome, ProvisionError>;
    pub async fn ensure(&self, req: &ProvisionRequest, approver: &dyn Approver) -> Result<ProvisionOutcome, ProvisionError>;
}
pub struct Approval { pub plan_id: uuid::Uuid, pub digest: [u8; 32], pub source: ApprovalSource, pub token: String }
pub enum ApprovalSource { CliTty, CliYesFlag, IdeDialog { user: String }, Policy { rule: String } }
```

**Planning algorithm** (all reads first; `apply` re-validates that the state still matches the digest):

1. **Identity.**
   - Development: use a local identity of kind `Development` for this team whose certificate is
     listed by the portal and not expired. Otherwise plan `CreateCertificate{Development}`.
   - Paid teams **never** plan a revocation automatically. Revocation is a separate command (§6.7).
   - Free teams: see §7.6.
2. **Devices** (Development and AdHoc only).
   - For each selected UDID, look it up with `filter[udid]`. If missing, plan `RegisterDevice`.
   - Warn that devices 11–100 can take 24–72 hours to become eligible on newer memberships
     [S Apple, *Device registration updates*].
   - Warn that the limit is 100 devices per family per membership year
     [S Apple, *Devices overview*].
3. **Bundle IDs.** For each `BundleSpec`, call `find_bundle_id(exact)`. If missing, plan
   `RegisterBundleId`.
   - Name: `Russet <identifier with . - _ replaced by spaces>`. Xcode uses `XC …` and xtool uses
     `XTool …` [V4].
   - Paid teams keep the **exact** identifier.
4. **Capabilities.** Map the requested entitlements to capability types (§6.5) and diff them against
   the existing capabilities.
   - Paid: **additive only**. Plan `EnableCapability` for missing ones and never disable automatically.
     Other team members, or Xcode-managed apps, may depend on them. xtool deletes unrequested
     capabilities, which is unsafe on shared teams [S xtool `DeveloperServicesAddAppOperation.swift`].
   - Free: full sync is allowed.
5. **Profiles.** Search for an `ACTIVE` profile named `Russet <Kind> <bundle id>` with the right
   type. Decode its `profileContent` and accept it only if all of these hold:
   - it is not expiring (more than 24 hours left for Development, 7 days for AdHoc and App Store);
   - it contains the identity certificate;
   - for Development and AdHoc, it contains every selected UDID;
   - its entitlements cover the requested ones (§8.4).

   Otherwise plan `DeleteProfile` (Russet-owned only) plus `CreateProfile`.
   - Development and AdHoc profiles get all `ENABLED` devices of class IPHONE, IPAD or IPOD (xtool
     does the same), or the explicit subset for ad hoc.
   - Profiles cannot be patched, because the API has no PATCH, so they are always recreated.
6. **App groups and iCloud containers.**
   - Public API: these are unsupported [V6]. Emit `PlanWarning::PortalStepRequired` with a deep link
     to `https://developer.apple.com/account/resources/identifiers/list/applicationGroup`, then
     re-plan after the user confirms.
   - Optional: an Apple ID session on the same paid team can do it via DS2 (§7.4). This is opt-in
     and uses a private API.

**Default auto-approval policy** (`policy.toml`, editable):

| Mutation | Paid team | Free team |
|---|---|---|
| RegisterDevice (the connected device) | auto | ask (3 devices/week [L sg-10]) |
| RegisterBundleId | auto | **ask** (10 App IDs per 7 days [S AltStore `Team.swift` `maximumFreeAppIDs = 10`]) |
| EnableCapability | auto | auto (sync) |
| Create/DeleteProfile (Russet-owned) | auto | auto |
| CreateCertificate | ask once per machine and team | ask (may need revocation) |
| RevokeCertificate, DisableCapability, DeleteProfile (foreign) | typed confirmation | typed confirmation |

**Profile cache.** Profiles are stored as `profiles/<uuid>.mobileprovision`. The index records the
team, bundle ID, kind, expiry and certificate serials. `.russet/signing.lock.json` pins the UUIDs per
project and machine.

**PPQ.** Development and ad hoc apps on teams created after 2021-06-06 contact `ppq.apple.com` at
first launch [S Apple, *Provisioning profile updates*]. The owner's profile has `PPQCheck=true` [V11].
`russet doctor` warns when the device is offline.

### 6.5 Entitlement → capability map

The free column follows Apple's "Supported capabilities (iOS)", which lists nine capabilities for
free accounts [S]. Capability types are open strings [V5].

| Entitlement key(s) | Capability type | Settings | Free | Notes |
|---|---|---|---|---|
| `com.apple.security.application-groups` | `APP_GROUPS` | none | yes | Group IDs: portal or DS2 only |
| `aps-environment` | `PUSH_NOTIFICATIONS` | none | no | Signed value comes from the profile (§8.4) |
| `com.apple.developer.associated-domains` | `ASSOCIATED_DOMAINS` | none | no | Profile value is `*` |
| `com.apple.developer.icloud-container-identifiers`, `…icloud-services`, `…ubiquity-container-identifiers`, `…ubiquity-kvstore-identifier` | `ICLOUD` | `ICLOUD_VERSION=XCODE_6` | no | Containers: portal only |
| `com.apple.developer.in-app-payments` | `APPLE_PAY` | none | no | Merchant IDs through the API |
| `com.apple.developer.healthkit`, `…healthkit.access` | `HEALTHKIT` | none | yes | |
| `com.apple.developer.homekit` | `HOMEKIT` | none | yes | |
| `com.apple.developer.networking.networkextension` | `NETWORK_EXTENSIONS` | none | no | |
| `com.apple.developer.networking.vpn.api` | `PERSONAL_VPN` | none | no | |
| `com.apple.developer.networking.wifi-info` | `ACCESS_WIFI_INFORMATION` | none | no | |
| `com.apple.developer.networking.HotspotConfiguration` | `HOT_SPOT` | none | no | |
| `com.apple.developer.networking.multipath` | `MULTIPATH` | none | no | |
| `com.apple.developer.nfc.readersession.formats` | `NFC_TAG_READING` | none | no | |
| `com.apple.developer.siri` | `SIRIKIT` | none | no | |
| `com.apple.developer.applesignin` | `APPLE_ID_AUTH` | `APPLE_ID_AUTH_APP_CONSENT=PRIMARY_APP_CONSENT` | no | |
| `com.apple.developer.game-center` | `GAME_CENTER` | none | no | |
| `com.apple.developer.default-data-protection` | `DATA_PROTECTION` | level option | yes | |
| `com.apple.external-accessory.wireless-configuration` | `WIRELESS_ACCESSORY_CONFIGURATION` | none | yes | |
| `inter-app-audio` | `INTER_APP_AUDIO` | none | yes | |
| `com.apple.developer.ClassKit-environment` | `CLASSKIT` | none | no | |
| `com.apple.developer.authentication-services.autofill-credential-provider` | `AUTOFILL_CREDENTIAL_PROVIDER` | none | no | |
| `com.apple.developer.pass-type-identifiers` | `WALLET` | none | no | |
| `com.apple.developer.coremedia.hls.low-latency` | `COREMEDIA_HLS_LOW_LATENCY` | none | no | |
| `keychain-access-groups` | none | none | yes | Profile grants `<TEAM>.*` [V11] |
| `get-task-allow`, `application-identifier`, `com.apple.developer.team-identifier`, `beta-reports-active` | none | none | n/a | Required keys (§8.4) |
| Anything else, for example `…kernel.increased-memory-limit` | `Unknown(String)` | none | ? | [A] Try `EnableCapability` with Xcode's type string, or tell the user to use the portal |

Background modes are Info.plist keys, not entitlements. Maps routing (`MAPS`) has no entitlement.

### 6.6 Distribution certificates

The team already has 2 `DISTRIBUTION` certificates, whose private keys are presumably on the Mac
[V4]. Apple's help page says distribution certificates belong to the team and are limited, and that
only Account Holder or Admin may create them in the portal [S *Certificates overview*]. Yet
omarchy's `asc.py identity` created an Apple Distribution certificate through the API
[S omarchy FINDINGS 33]. Its receipt of the same day records an "App Manager" team key
[S omarchy receipt 2026-10-04]. Whether App Manager suffices without the "Certificates, Identifiers
& Profiles" access flag is [A].

The wizard decides in this order:

1. A local `Distribution` identity exists and its certificate is active: use it.
2. Otherwise, offer these options:
   - **(a) Import a `.p12`.** Export it from the machine that holds the key; on a Mac, Keychain
     Access → Export.
   - **(b) Create a new certificate.** Generate the key and CSR, then `POST /v1/certificates`. On a
     409 or a limit error, show the limit and stop. It never revokes automatically [A: exact error
     code].
   - **(c) Use Apple-hardware release mode** (§9.9) with signing mode `xcode-cloud`. Xcode's
     cloud-managed signing on the runner needs no local distribution key, but it needs an
     **Admin-role** team key stored on GitHub. In Apple's roles matrix only Account Holder and Admin
     may create cloud-managed certificates (App Manager and Developer only when granted separately in
     Users and Access, which a team key cannot carry: a key has only a role), and CI reports from
     Aug–Oct 2026 show App Manager team keys failing with
     `DeveloperAPIServiceErrorDomain Code=5 "Cloud signing permission error"` while Admin keys work
     [S Apple roles; S CI reports]. The default Apple-hardware mode `russet` uses option (a) or (b)
     instead.
3. Revoking a distribution certificate is §6.7, with a typed confirmation. Apple's stated effects:
   - App Store apps already live are unaffected.
   - New uploads signed with the revoked certificate fail.
   - Unsubmitted builds "may be marked as Invalid Binary".
   - Profiles that use it become invalid [S *Certificates overview*].

### 6.7 Revocation UX (paid and free)

The command is `russet account certs revoke <id>`, or the IDE dialog. The dialog shows:

- certificate type, display name, serial suffix, created and expiry dates;
- which local identity (if any) and which Russet profiles use it;
- the consequences:
  - development certificate: apps signed with it stop launching on devices once the revocation is
    seen;
  - free teams: AltStore, SideStore or Sideloadly installs signed by it stop working.

The user must type `REVOKE <last 6 of serial>`. MCP/agents cannot revoke (§11).

---

## 7. Free Apple ID: `russet-gsa`, `russet-anisette`, `russet-adi-host`

### 7.1 Status, consent and risk

- **Off by default.** Enabling it shows a consent screen covering:
  - Russet will use private Apple endpoints that Xcode uses.
  - Apple may block or lock accounts (landscape R3).
  - Use a secondary Apple ID.
  - Two Apple libraries (about 2 MB) are downloaded from Apple's CDN to your machine and never
    redistributed.
- **Known breakage.**
  - On 2026-09-10 GSA rejected the old Xcode client identity, and every client had to switch
    [L sg-14]. xtool commits `893cf4f` (#244) and `450223e` (#257) show three successive identity
    strings within a week.
  - Free-account installs fail with `0xe8008015` ("A valid provisioning profile for this executable
    was not found") on iOS 26.5 and 27 in several tools. iloader#570 is still **open**, with the
    last report on 2026-09-20 for iOS 27.0 [S].
  - The device rule in M1 stays: paid team first.

### 7.2 GrandSlam authentication (protocol facts from xtool XKit and isideload)

These facts come from xtool XKit (MIT), isideload (MIT) and Impactor `plume_core` (MPL-2.0), reread
on 2026-10-11 [S]. They are interoperability facts; Russet's code is written from this description.

1. **Lookup.** `GET https://gsa.apple.com/grandslam/GsService2/lookup` returns a plist
   `urls{gsService, secondaryAuth, trustedDeviceSecondaryAuth, validateCode, midStartProvisioning, midFinishProvisioning}`.
2. **SRP init.** `POST gsService` with a `text/x-xml-plist` body
   `{Header:{Version:"1.0.1"}, Request:{A2k, cpd, o:"init", ps:["s2k","s2k_fo"], u:<apple id lowercased>}}`.
   It returns `{sp, i, s, B, c}`.
   - The SRP-6a group is 2048-bit (RFC 5054) with `g=2`.
   - `x = SHA256(salt ‖ SHA256(":" ‖ PBKDF2-SHA256(pw', salt, i, 32)))`, where `pw' = SHA256(password)`.
     For `s2k_fo`, `pw'` is its lowercase hex string.
3. **SRP complete.** `{M1, c, cpd, o:"complete", u}` returns `{M2, spd, np, Status}`.
   - `spd` is AES-256-CBC. The key and IV are `HMAC-SHA256(K, "extra data key:")` and
     `HMAC-SHA256(K, "extra data iv:")` (first 16 bytes for the IV).
   - Russet verifies `M2` (HAMK) and `np`.
   - `Status.hsc == 409` means 2FA is required, and `au` selects `trustedDeviceSecondaryAuth` or
     `secondaryAuth`.
4. **2FA.**
   - **Trusted device:** `GET <trustedDeviceSecondaryAuth>`, then `GET <validateCode>` with header
     `security-code: <6 digits>`.
   - **SMS:** `GET https://gsa.apple.com/auth` (trusted numbers),
     `PUT https://gsa.apple.com/auth/verify/phone` `{phoneNumber:{id}, mode:"sms"}`, and
     `POST …/phone/securitycode` `{securityCode:{code}, phoneNumber:{id}, mode:"sms"}`.
   - Headers carry `X-Apple-Identity-Token: base64(adsid:GsIdmsToken)`, the anisette headers and
     `X-Apple-I-MD-RINFO`.
   - After 2FA, Russet repeats the SRP login once, as xtool does.
5. **App token.** `POST gsService` with
   `{Header:{Version:"1.0.1"}, Request:{o:"apptokens", u:<adsid>, app:["com.apple.gs.xcode.auth"], c, t:<GsIdmsToken>, checksum, cpd}}`.
   - `sk`, `c`, `adsid` and `GsIdmsToken` all come from the decrypted `spd`.
   - `checksum = HMAC-SHA256(sk, "apptokens" ‖ adsid ‖ app)`.
   - The response `et` is AES-256-GCM with a 16-byte nonce under `sk`. It decrypts to a plist
     `{status-code: 200, t: {"com.apple.gs.xcode.auth": {token, duration, expiry}}}`
     [S isideload `apple_account.rs`].
6. **TLS.** `gsa.apple.com` chains to Apple Root CA, which is not in the system store [V15].
   - `russet-gsa` builds its reqwest client with a root store holding **only** Apple Root CA.
   - Russet downloads the certificate at first use from
     `https://www.apple.com/appleca/AppleIncRootCertificate.cer` and pins its SHA-256 [V15].
     Russet thus ships a hash, not the Apple file.
   - Connections are not kept alive (`Connection: close`); GSA dislikes reused connections
     [S xtool `GrandSlamClient.swift`].

**Client identity profile** (`gsa-profile.json`, versioned, user-overridable, refreshable from
Russet releases; §21 Q9):

```json
{
  "version": 3,
  "clientInfo": "<{model}> <macOS;27.0;26A5378j> <com.apple.AuthKit/1 (com.apple.akd/1.0)>",
  "userAgent": "AuthKit/1 (Macintosh; OS X 27.0) (com.apple.dt.Xcode/26.5)",
  "xcodeVersion": "26.5 (17F42)",
  "appInfo": "com.apple.gs.xcode.auth",
  "anisetteClientInfoOverride": null,
  "source": "xtool 450223eb0 (2026-09-10); isideload uses X-Xcode-Version 27.0 (27A5218g)"
}
```

### 7.3 `russet-gsa` API

```rust
pub struct GsaClient { /* http (Apple-root-only), Arc<dyn AnisetteProvider>, ClientProfile, endpoints */ }
pub enum LoginState { Authenticated(GsSession), NeedsTwoFactor(PendingTwoFactor) }
pub enum TwoFactorVia { TrustedDevice, Sms { phone_id: u32 } }
pub struct GsSession { pub adsid: String, pub dsid: String, /* sk, c, GsIdmsToken: SecretBytes */ }
pub struct XcodeToken { pub adsid: String, pub token: SecretBytes, pub expires_at: time::OffsetDateTime }

impl GsaClient {
    pub fn new(anisette: std::sync::Arc<dyn russet_anisette::AnisetteProvider>, profile: ClientProfile, apple_root_der: &[u8]) -> Result<Self, GsaError>;
    pub async fn login(&self, apple_id: &str, password: &SecretBytes) -> Result<LoginState, GsaError>;
    pub async fn trusted_numbers(&self, p: &PendingTwoFactor) -> Result<Vec<TrustedNumber>, GsaError>;
    pub async fn request_code(&self, p: &PendingTwoFactor, via: TwoFactorVia) -> Result<(), GsaError>;
    pub async fn submit_code(&self, p: PendingTwoFactor, via: TwoFactorVia, code: &str, password: &SecretBytes) -> Result<GsSession, GsaError>;
    pub async fn xcode_token(&self, s: &GsSession) -> Result<XcodeToken, GsaError>;
}
pub enum GsaError {
    BadCredentials, TwoFactorRequired, BadCode, TooManyCodes, AccountLocked { url: Option<String> },
    ServiceRejected { status: u16 },        // 503/429 storms like 2026-09-10 → E_GSA_IDENTITY_REJECTED
    Anisette(russet_anisette::AnisetteError), Tls(String), Protocol(String),
}
```

### 7.4 developerservices2 (DS2)

Two dialects, both POST:

1. **Legacy QH65B2.** URL
   `https://developerservices2.apple.com/services/QH65B2/<action>.action?clientId=XABBG36SBA`.
   - The body is an XML plist with `requestId` (UUID), `clientId`, `protocolVersion: "QH65B2"`,
     `userLocale` and parameters. The response has `resultCode` (0 means OK) and
     `userString`/`resultString`.
   - Actions used:
     - `listTeams` returns `teams[{teamId, status, name, memberships[{name, platform}]}]`. A team
       counts as **free** if no iOS membership name contains "Apple Developer Program"
       [S xtool `DeveloperServicesTeam.swift`].
     - `ios/listApplicationGroups`.
     - `ios/addApplicationGroup {teamId, name, identifier}`.
     - `ios/assignApplicationGroupToAppId {teamId, appIdId, applicationGroups}`.
2. **v1 mirror of the public API.** Base `https://developerservices2.apple.com/services` plus the
   public path, for example `/v1/certificates` [S xtool `DeveloperAPIXcodeAuthMiddleware`].
   `Ds2Auth: russet_asc::Auth` rewrites each request:
   - **GET and DELETE** become POST with `X-HTTP-Method-Override: GET|DELETE`, and the body
     `{"urlEncodedQueryParams":"<original query>&teamId=<team>"}`.
   - **POST and PATCH** bodies get `data.attributes.teamId` injected.
   - **Headers:**
     - `Accept`/`Content-Type: application/vnd.api+json`
     - `User-Agent: Xcode`, `X-Xcode-Version`, `X-MMe-Client-Info`, `X-Mme-Device-Id`
     - `X-Apple-App-Info: com.apple.gs.xcode.auth`
     - `X-Apple-I-Identity-Id: <adsid>`, `X-Apple-GS-Token: <token>`
     - anisette headers
   - A `bundleIdCapabilities` create needs the extra relationship
     `capability: {data: {type: "capabilities", id: <CapabilityType>}}` on DS2 only
     [S xtool `DeveloperAPI/patch.js`].

### 7.5 Anisette

**Providers:**

```rust
#[async_trait::async_trait]
pub trait AnisetteProvider: Send + Sync {
    fn kind(&self) -> AnisetteKind;                       // Local | Remote { url }
    async fn headers(&self) -> Result<AnisetteHeaders, AnisetteError>;
    async fn client_info(&self) -> Result<String, AnisetteError>;
    async fn reset(&self) -> Result<(), AnisetteError>;   // erase provisioning (dsid -2)
}
pub struct AnisetteHeaders {
    pub md: String, pub md_m: String, pub md_rinfo: String, pub md_lu: String,  // X-Apple-I-MD, -MD-M, -MD-RINFO, -MD-LU
    pub device_id: String, pub serial: String,                                  // X-Mme-Device-Id, X-Apple-I-SRL-NO ("0")
    pub client_time: String, pub locale: String, pub time_zone: String,
}
pub struct LocalAdi { /* AdiHelper, state dir, Arc<SecretStore>, GSA lookup URLs */ }
impl LocalAdi {
    pub async fn ensure_libraries(dir: &std::path::Path, abi: Abi, consent: &DownloadConsent) -> Result<LibrarySet, AnisetteError>;
    pub async fn open(libs: LibrarySet, state: AdiState, http: reqwest::Client) -> Result<Self, AnisetteError>;
}
pub struct RemoteV3 { pub url: url::Url }   // opt-in; omnisette-server v3 protocol
pub enum Abi { X86_64, Arm64V8a }
```

**Library acquisition** (`ensure_libraries`):

1. **Consent** (first time only). The user is told what is downloaded, from where, and why.
2. **Fetch only what is needed.**
   - `HEAD` the APK URL [V12] and record `ETag`, `Content-Length` and `x-apple-version-number`.
   - Read the last 64 KiB to find the EOCD, then range-fetch the central directory (about 272 KB).
   - Locate `lib/<abi>/libstoreservicescore.so` and `lib/<abi>/libCoreADI.so`.
   - Range-fetch each local header plus compressed data and inflate them, about 2 MB in total
     [V12]. This uses 0.3 % of the bandwidth and disk the full APK would.
3. **Verify.** Check each library's SHA-256 against `anisette-pins.json`, which Russet ships: hashes
   only, keyed by APK version.
   - An unknown version gets a warning and a Trust-On-First-Use prompt. The pin file is updated in a
     Russet release.
4. **Store.** Libraries go to `$DATA/russet/anisette/libs/<apk-version>/<abi>/` (0700). They are
   **never** included in any Russet artefact, crash report or support bundle. This follows
   NOTICE.md ("fetches it from Apple on your behalf at runtime").

**Provisioning and OTP** (these values are interoperability facts from xtool and omnisette [S]):

- The local user UUID is generated once and stored in `AnisetteState`.
  - `X-Apple-I-MD-LU = hex_upper(SHA256(uuid_string))`.
  - `X-Mme-Device-Id = uuid` (uppercase).
  - Android ID = the first 16 hex characters of the UUID without dashes, lowercase.
- The ADI DSID is `-2`.
- Flow:
  1. `midStartProvisioning` returns `spim`.
  2. `ADIProvisioningStart(-2, spim)` returns `cpim` and a session.
  3. `midFinishProvisioning(cpim)` returns `ptm`, `tk` and `X-Apple-I-MD-RINFO`.
  4. `ADIProvisioningEnd(session, ptm, tk)` writes the `adi.pb` state.
  5. Every request: `ADIOTPRequest(-2)` returns `mid` (`X-Apple-I-MD-M`) and `otp` (`X-Apple-I-MD`).
- `adi.pb` identifies this machine to Apple; "don't leak your ADI data" [S Provision README].
  It is stored as `AnisetteState` (envelope-encrypted, §4).

**`russet-adi-host` (separate process):**

- **Why a separate process:**
  - it isolates Apple's proprietary native code from the daemon;
  - a crash in it cannot take down `russetd`;
  - it lets us sandbox it: no network, and filesystem access limited to its libs and state
    directories.
- **ELF loader (clean-room, from the ELF gABI, the x86-64/AArch64 psABIs and BSD-licensed bionic
  headers).** Its job:
  - map `PT_LOAD` segments with their protections;
  - apply `DT_RELA` and `DT_JMPREL` relocations (`R_X86_64_RELATIVE/GLOB_DAT/JUMP_SLOT/64`,
    `R_AARCH64_RELATIVE/GLOB_DAT/JUMP_SLOT/ABS64`);
  - resolve imports from `libc.so`, `libdl.so`, `libm.so` and `liblog.so` to Rust shims;
  - run `DT_INIT`/`DT_INIT_ARRAY`;
  - look up exports by name (`kq56gsgHG6` LoadLibraryWithPath, `Sph98paBcz` SetAndroidID,
    `nf92ngaK92` SetProvisioningPath, `rsegvyrt87` ProvisioningStart, `uv5t6nhkui` ProvisioningEnd,
    `fy34trz2st` ProvisioningDestroy, `p435tmhbla` ProvisioningErase, `tn46gtiuhw` Synchronize,
    `qi864985u0` OTPRequest, `jk24uiwqrg` Dispose, `aslgmuibau` GetLoginCode;
    names from omnisette `store_services_core.rs` [S]).
- **We do not use or copy `android-loader`**, which has no licence [V13]. Provision and xadi
  (LGPL-2.1) are acceptable references, and also an acceptable fallback on Linux through xadi's
  static libraries.
- **Platform notes:**
  - Linux x86_64 and aarch64: native, with the SysV ABI.
  - Windows x64: calls go through `extern "sysv64"`. Shims translate `stat`, `errno`, time and file
    APIs; Impactor's omnisette has such a Windows shim, which shows it is feasible [S].
    **Risk:** bionic reads TLS and the stack canary through `%fs` on x86_64, and on Windows `%fs` is
    unusable [A]. The loader must patch `fs:`-relative loads or emulate them in a vectored exception
    handler; measure which in SA-22.
  - Windows on ARM: out of scope until requested.
- **Sandbox:**
  - Linux: seccomp-bpf allowlist plus Landlock. No `socket`; read-only libs; read-write state dir.
  - Windows: a Job object with a low-integrity token.
- **IPC.** JSON Lines on stdin/stdout:

```json
{"op":"load","libDir":"/…/x86_64","androidId":"0123456789abcdef","provisioningPath":"/…/adi"}
{"op":"isProvisioned","dsid":-2}                       → {"ok":true,"provisioned":false}
{"op":"startProvisioning","dsid":-2,"spim":"<b64>"}    → {"ok":true,"session":1,"cpim":"<b64>"}
{"op":"endProvisioning","session":1,"ptm":"<b64>","tk":"<b64>"} → {"ok":true}
{"op":"requestOtp","dsid":-2}                          → {"ok":true,"mid":"<b64>","otp":"<b64>"}
{"op":"erase","dsid":-2}                               → {"ok":true}
```

Errors return `{"ok":false,"adiCode":-45061}`. xtool maps `-45061` to "logged out; log in again"
[S xtool `ADIDataProvider.swift`].

**`RemoteV3` (opt-in, for example a self-hosted `anisette-v3-server`):**

- Protocol: `GET /v3/client_info`, websocket `/v3/provisioning_session`
  (`GiveIdentifier → GiveStartProvisioningData → GiveEndProvisioningData → ProvisioningSuccess{adi_pb}`),
  and `POST /v3/get_headers {identifier, adi_pb}` [S xtool `OmnisetteADIProvider.swift`].
- The UI warns that the server operator receives this machine's ADI identity.
- There is no default public server.

### 7.6 Personal-team (free) provisioning policy

- **Limits** [S AltStore `Team.swift` and `InstalledApp.swift`; L sg-10/11]:
  - 10 new App IDs per rolling 7 days;
  - 3 active sideloaded apps per device since iOS 13.3.1 (optionally counting extensions);
  - 7-day profiles;
  - 9 capabilities (§6.5).
- **Tracking.** `russet-account` keeps a local ledger (`accounts/<team>/ledger.json`) of App ID
  creations, so it can show "App IDs: 3/10 this week, next slot frees <date>". Apple does not expose
  remaining quota.
- **Bundle IDs.** Russet first tries the exact ID. If Apple answers that the identifier is not
  available (owned elsewhere), it uses `<app id>.<TEAMID>`. This is believed to be AltStore's
  convention [A: check AltStore's provisioning operation source]. The returned `ALTAppleAPI` error
  for this case is `BundleIdentifierUnavailable` [S AltStore `ALTAppleAPI.m`].
  - Extensions keep the prefix rule: `<app id>.<TEAMID>.<suffix>`.
  - The mapping is stored in `bundle-map.json`, and the signer rewrites `CFBundleIdentifier` in the
    copy it signs.
  - App-group IDs are rewritten the same way (`group.<id>.<TEAMID>`).
- **Certificates.** A free team often refuses a new development certificate while others exist.
  xtool revokes them all after confirmation ("otherwise it doesn't always let us make a new one")
  [S xtool `DeveloperServicesFetchCertificateOperation.swift`]. Russet:
  1. reuses its own certificate if it is still listed;
  2. otherwise offers to import an existing `.p12`, or to revoke with the §6.7 dialog. Revocation
     breaks every app signed by that certificate, including AltStore and SideStore installs.
- **Seven-day refresh.**
  - An opt-in scheduler in `russetd` re-provisions and re-signs 24 hours before expiry, and asks the
    device daemon to reinstall when the device is connected.
  - Unattended runs need the stored password (opt-in), because GS tokens expire.
  - A missed refresh only stops the app from launching; data stays on the device.

---

## 8. `russet-sign`

### 8.1 Inputs and outputs

- **Input:** a built `.app`, or an `.ipa` that is unpacked to a temporary directory. It may be
  unsigned, linker-signed (lld ad-hoc) or previously signed (xtool/zsign).
- **Output:** a signed copy (default) or an in-place re-sign, plus a `SignReport`. IPAs are written
  by `russet-ship` (§9.3).

### 8.2 Bundle model

```rust
pub struct AppBundle { pub root: std::path::PathBuf, pub info: InfoPlist, pub kind: BundleKind, pub nested: Vec<AppBundle>, pub code: Vec<CodeItem> }
pub enum BundleKind { App, AppExtension, AppClip, WatchApp, Framework, Bundle }
pub enum CodeItem { MainExecutable(std::path::PathBuf), Framework(std::path::PathBuf), Dylib(std::path::PathBuf), OtherMachO(std::path::PathBuf) }
impl AppBundle {
    pub fn open(path: &std::path::Path) -> Result<Self, SignError>;  // walks PlugIns/, Extensions/, AppClips/, Watch/, Frameworks/
    pub fn signing_order(&self) -> Vec<std::path::PathBuf>;           // deepest first: dylibs/frameworks → appex → app clip/watch → app
    pub fn bundle_ids(&self) -> Vec<(String /* rel path */, String /* CFBundleIdentifier */)>;
}
```

**Order.** Leaf code is signed first:

1. `Frameworks/*.framework` and `*.dylib` (no entitlements, no profile);
2. each `PlugIns/*.appex`, with its own profile and entitlements;
3. App Clips and Watch apps the same way;
4. the app last.

apple-codesign's `BundleSigner` already signs nested bundles first, and seals the files of each
level in `_CodeSignature/CodeResources` [V9]. Russet configures it with one `SigningSettings` built
like this:

| Setting | Scope | Value |
|---|---|---|
| `set_signing_key(identity, cert)`, `chain_apple_certificates()` | global | identity from `russet-account` |
| `set_team_id_from_signing_certificate()` (fallback `set_team_id(profile.team)`) | global; propagates to nested code [V9] | the team ID |
| `set_entitlements_xml(...)` | `Path("")`/`Main` for the app; `Path("PlugIns/X.appex")` for each extension | merged entitlements (§8.4) |
| `set_binary_identifier` | per path | `CFBundleIdentifier` (after free-team rewriting) |
| `set_designated_requirement_*` | per path | `Auto` (WWDR expression [V9]); explicit override allowed |
| `set_time_stamp_url` | global | **none.** iOS does not need timestamps |
| digest | global | apple-codesign default. A lint warns if no SHA-256 CD is present; xtool emitted SHA-1 + SHA-256 [V11] |

### 8.3 Provisioning profiles

```rust
pub struct ProvisioningProfile {
    pub uuid: String, pub name: String, pub team_id: String, pub team_name: String,
    pub app_id_prefix: String, pub application_identifier: String,     // "<PREFIX>.<bundle>" or "<PREFIX>.*"
    pub kind: ProfileKind,   // derived: get-task-allow + devices → Development; devices → AdHoc; ProvisionsAllDevices → InHouse; else AppStore
    pub platforms: Vec<String>,                                         // e.g. ["iOS","xrOS","visionOS"] [V11]
    pub entitlements: plist::Dictionary,
    pub developer_certificates: Vec<Vec<u8>>, pub provisioned_devices: Option<Vec<String>>,
    pub provisions_all_devices: bool, pub creation: time::OffsetDateTime, pub expiration: time::OffsetDateTime,
    pub ppq_check: Option<bool>, pub has_der_profile: bool,             // DER-Encoded-Profile [V11]
    pub raw: Vec<u8>,
}
impl ProvisioningProfile {
    pub fn parse(cms_der: &[u8]) -> Result<Self, ProfileError>;          // CMS SignedData → eContent plist
    pub fn verify_cms(&self, apple_root_der: &[u8]) -> Result<(), ProfileError>;  // optional chain check
    pub fn covers(&self, bundle_id: &str) -> bool;                       // exact or wildcard match on application-identifier
    pub fn has_certificate(&self, cert_der: &[u8]) -> bool;
    pub fn has_device(&self, udid: &str) -> bool;
    pub fn expires_within(&self, d: std::time::Duration) -> bool;
}
```

**Selection** (manual mode, or a profile supplied with `--profile`). For each bundle, pick the
profile that:

- covers the bundle ID, preferring an exact match over a wildcard;
- contains the identity certificate;
- contains the target device, for development and ad hoc;
- is not expired, and has the kind that matches the requested distribution.

Ties go to the latest expiration.

### 8.4 Entitlements merge (per bundle)

**Inputs:**

- `R`: requested entitlements, from the project's `.entitlements` and capability toggles, via the
  build area.
- `P`: `profile.entitlements`.
- `B`: the final bundle ID.
- `X`: the app ID prefix (`ApplicationIdentifierPrefix[0]`).
- `T`: the team.
- `D`: the distribution.

1. **Expand variables in `R`.**
   - `$(AppIdentifierPrefix)` → `X.`
   - `$(TeamIdentifierPrefix)` → `T.`
   - `$(PRODUCT_BUNDLE_IDENTIFIER)` and `$(CFBundleIdentifier)` → `B`
   - Anything else → `E_ENT_UNRESOLVED_VAR`.
2. **Required keys** (always set):
   - `application-identifier = X.B`. It must match `P`'s value exactly or through a `*` glob.
   - `com.apple.developer.team-identifier = T`, if present in `P`.
   - `get-task-allow = P[get-task-allow]`. For Development it is true, otherwise false. A request
     for true under a distribution profile gives `E_ENT_VALUE_NOT_ALLOWED`.
   - `beta-reports-active = true` only when `D == AppStore` and `P` grants it. This is what Xcode
     does for App Store exports [A, verify in the P0-3 diff].
   - `keychain-access-groups`: if `R` has it, each entry must glob-match some entry in `P`.
     Otherwise the default is `[X.B]`, exactly what the device-accepted xtool signature contains
     [V11].
3. **Each other key `k` in `R`:**
   - `k ∉ P` → `E_ENT_NOT_IN_PROFILE{k}`. The remedy is `EnableCapability` and recreating the profile
     (§6.4), or "not available on free teams".
   - **bool:** `R=true` needs `P=true`.
   - **string:** equality, or a glob when `P` contains `*`.
     - Special case `aps-environment`: the signed value is **`P`'s value** (`development` or
       `production`), with a warning if `R` differs.
   - **array:** every element of `R` must glob-match an element of `P`. This covers associated
     domains (`P=["*"]`), app groups and iCloud containers.
   - **dict:** a recursive subset.
4. Keys that exist only in `P` are **not** copied (least privilege), except the required ones in
   step 2. The option `MergePolicy::ProfileAll`, which copies `P` with wildcards resolved, exists for
   re-signing third-party IPAs and is not the default.
5. Frameworks and dylibs get **no** entitlements.

```rust
pub struct EntitlementsResolver<'a> { pub profile: &'a ProvisioningProfile, pub bundle_id: &'a str, pub distribution: Distribution, pub policy: MergePolicy }
impl EntitlementsResolver<'_> {
    pub fn resolve(&self, requested: &plist::Dictionary) -> Result<ResolvedEntitlements, EntitlementsError>;
}
pub struct ResolvedEntitlements { pub dict: plist::Dictionary, pub notes: Vec<MergeNote> }
pub enum EntitlementsError {
    UnresolvedVariable { key: String, var: String },
    NotInProfile { key: String, capability_hint: Option<String> },
    ValueNotAllowed { key: String, requested: String, allowed: String },
    AppIdMismatch { profile: String, bundle: String },
}
```

On iOS, the device installer rejects signatures whose entitlements are not authorised by the
embedded profile. Typical errors are `0xe8008015` and `0xe8008016` [A for the exact mapping]. The
merge makes this impossible by construction, and the post-sign verifier (§8.7) checks it again.

### 8.5 Fixes before signing (`russet-macho`)

| Fix | Why | Default |
|---|---|---|
| `chmod 0755` on every Mach-O | Unpacked IPAs lose exec bits; Impactor patches apple-codesign for this [V14] | on |
| Pad the LINKEDIT string pool to 8-byte alignment when `LC_BUILD_VERSION sdk ≥ 27.0` | iOS 27 dyld rejects 4-byte-aligned pools with `mis-aligned LINKEDIT string pool, fileOffset=…`. `llvm-strip`/`llvm-install-name-tool` before LLVM 23 produce them [S omarchy receipt 2026-10-08; L sg-08] | on (it is a lint error when the fix is off) |
| Remove stale `_CodeSignature/`, `CodeResources`, `.DS_Store`, `__MACOSX` | Clean sealing | on |
| Rewrite `CFBundleIdentifier` (free-team mapping) | §7.6 | only for free teams |
| Thin to `arm64` | Fat binaries with simulator slices are rejected | off (lint) |

```rust
pub mod russet_macho {
    pub struct MachOFile { /* thin or fat */ }
    impl MachOFile {
        pub fn open(path: &std::path::Path) -> std::io::Result<Self>;
        pub fn build_version(&self) -> Option<BuildVersion>;              // platform, minos, sdk, tools
        pub fn set_sdk_version(&mut self, sdk: Version) -> bool;           // ship stamping fallback (§9.5)
        pub fn string_pool_alignment(&self) -> Option<u64>;                // stroff % 8
        pub fn align_string_pool(&mut self, to: u64) -> Result<bool, MachOError>;
        pub fn code_signature(&self) -> Option<SignatureInfo>;             // CDs, exec seg flags, team, entitlements
        pub fn write(&self, path: &std::path::Path) -> std::io::Result<()>;
    }
}
```

### 8.6 iOS 26/27 rules and lints

Lints run on `.app` and `.ipa`, with profiles `device-install` and `app-store`. Errors block; warnings
do not.

| ID | Rule | Profile | Source |
|---|---|---|---|
| L-SIG-001 | Each `MH_EXECUTE` (app, appex, app clip) has `CS_EXECSEG_MAIN_BINARY`. iOS 26 AMFI kills extensions without it | both | [L sg-07]; apple-codesign sets it [V9] |
| L-SIG-002 | `ALLOW_UNSIGNED` present iff `get-task-allow=true` | both | [V9]; zsign `archo.cpp` |
| L-SIG-003 | CodeDirectory team ID = profile team = certificate OU | both | omarchy validator |
| L-SIG-004 | Signed entitlements ⊆ profile entitlements; signing certificate ∈ `DeveloperCertificates` | both | omarchy validator |
| L-SIG-005 | DER entitlements blob present when entitlements are present (iOS 15+) | both | [V9][V11] |
| L-SIG-006 | Info.plist and CodeResources hashes match sealed slots; every file is sealed | both | omarchy validator |
| L-SIG-007 | Profile valid more than 24 h; device in `ProvisionedDevices` (development/ad hoc) | device-install | |
| L-MACHO-001 | `LC_SYMTAB.stroff % 8 == 0` when `sdk ≥ 27.0` | both | §8.5 |
| L-MACHO-002 | `LC_BUILD_VERSION` platform iOS; `sdk ≥ 26.0` for uploads (ITMS-90725; App Store floor since 2026-04-28) | app-store | [S Upcoming requirements]; omarchy 33 |
| L-MACHO-003 | `minos ≤ MinimumOSVersion`, and `MinimumOSVersion ≥ 13.0` for uploads (since 2026-09-09) | app-store | [S Upcoming requirements] |
| L-MACHO-004 | Thin `arm64`; no `arm64e` (stock lld cannot link it) | both | landscape §2.4 |
| L-BND-001 | `PlugIns/*.appex` have no `Frameworks/` (ITMS-90206) | app-store | omarchy 31 |
| L-BND-002 | No loose `Frameworks/*.dylib` (ITMS-90426) | app-store | omarchy |
| L-BND-003 | appex versions equal the app's (ITMS-90473); appex has `arm64` in `UIRequiredDeviceCapabilities` (ITMS-90502) | app-store | omarchy `asc.py stamp` |
| L-BND-004 | Extension bundle IDs are prefixed by the app bundle ID | both | Apple rule |
| L-BND-005 | No unresolved `$(…)` in any Info.plist | both | omarchy validator |
| L-BND-006 | `UIDeviceFamily` and `DT*` group present. iOS 26+ installs but refuses to register apps without them (single source) | device-install: warn | [L dv-26] |
| L-BND-007 | `CFBundleIcons`/`CFBundleIconName` + `Assets.car` with a 1024 px opaque icon (ITMS-90713/90717) | app-store | landscape §1.3; omarchy 35/38 |
| L-BND-008 | Multi-size `AppIcon` catalogs are known to wedge processing; prefer single-size | app-store: warn | omarchy receipt 2026-10-09 |
| L-BND-009 | iOS 27 SDK apps declare a UIScene manifest | both: warn | [L fx-31] |
| L-BND-010 | `libswift*.dylib` in `Frameworks/` needs a matching `SwiftSupport/` in the IPA | app-store | [A] |

### 8.7 API

```rust
pub struct SignRequest<'a> {
    pub bundle: &'a AppBundle,
    pub identity: &'a LoadedIdentity,
    pub per_bundle: std::collections::BTreeMap<String /* rel path, "" = app */, BundleSigningInput>,
    pub options: SignOptions,
}
pub struct BundleSigningInput { pub profile: ProvisioningProfile, pub requested_entitlements: plist::Dictionary, pub bundle_id_override: Option<String> }
pub struct SignOptions {
    pub output: SignOutput,                // InPlace | CopyTo(PathBuf)
    pub distribution: Distribution,
    pub merge_policy: MergePolicy,         // Least (default) | ProfileAll
    pub fixes: PreSignFixes,               // chmod, align_string_pool, clean, thin
    pub lint: LintProfile,                 // DeviceInstall | AppStore | None
}
pub struct SignReport {
    pub bundles: Vec<SignedBundle>,        // rel path, bundle id, cdhash (SHA-256), team, entitlements, profile uuid
    pub fixes_applied: Vec<FixRecord>,
    pub lints: Vec<LintFinding>,
}
pub fn sign(req: SignRequest<'_>) -> Result<SignReport, SignError>;
pub fn verify(path: &std::path::Path, opts: &VerifyOptions) -> Result<VerifyReport, SignError>;   // runs L-SIG-* on any signed app/ipa
pub fn lint(path: &std::path::Path, profile: LintProfile) -> Result<Vec<LintFinding>, SignError>;
pub fn inspect(path: &std::path::Path) -> Result<Vec<SignatureInfo>, SignError>;                  // JSON-able, redacts team by default
pub enum SignError { Bundle(String), Profile(ProfileError), Entitlements(EntitlementsError), Identity(String), MachO(String), Codesign(String), LintFailed(Vec<LintFinding>), Io(std::io::Error) }
```

**Verification after signing** always runs, because it is cheap. It reparses every signature with
`russet-macho`, recomputes the sealed resource hashes, and checks L-SIG-001 to L-SIG-006.

---

## 9. `russet-ship`

### 9.1 Archive

The archive uses the `.xcarchive` layout, so a Mac user can open it in Xcode Organizer:

```text
<Name> <date>.xcarchive/
  Info.plist                 ApplicationProperties{ApplicationPath, CFBundleIdentifier, CFBundleShortVersionString,
                             CFBundleVersion, SigningIdentity, Team}, ArchiveVersion=2, CreationDate, Name, SchemeName
  Products/Applications/<App>.app
  dSYMs/<App>.app.dSYM, <Ext>.appex.dSYM, <Fw>.framework.dSYM
  russet/StampReport.json    (when stamped; §9.5)
  russet/ValidationReport.json
```

`dsymutil` runs on the unstripped binaries before stripping. The tool is xtool's toolset
`dsymutil`, or the system's LLVM `dsymutil` (`/usr/bin/dsymutil`, 23.1.1, on this host).

- **No dSYM upload API exists** [L sg-38]. Xcode's "upload symbols" uses `.symbols` files, which
  only the macOS `symbols` tool produces [A], so Linux-native IPAs ship **without** `Symbols/`.
- Russet keeps the dSYMs in the archive.
- Russet symbolicates TestFlight crash logs locally (§9.8) with `llvm-symbolizer`/`atos`-style
  lookup through the dSYM UUIDs. It can push dSYMs to third-party services (Sentry, Crashlytics)
  through their own APIs as plug-ins (later).

### 9.2 Export methods

| Method | Profile | Certificate | `get-task-allow` | Notes |
|---|---|---|---|---|
| `development` | IOS_APP_DEVELOPMENT | DEVELOPMENT | true | default for Run/Debug |
| `ad-hoc` | IOS_APP_ADHOC | DISTRIBUTION | false | registered devices only |
| `app-store-connect` | IOS_APP_STORE | DISTRIBUTION | false | `beta-reports-active` when granted; for TestFlight and the App Store |
| `enterprise` | IOS_APP_INHOUSE | enterprise | false | ADEP teams only |

### 9.3 IPA writer

The writer uses the `zip` 9 crate. Russet never shells out to `zip` (verified-facts).

- Layout: `Payload/<App>.app/…`. `SwiftSupport/iphoneos/` is added only when L-BND-010 applies.
  `Symbols/` is never added (§9.1).
- Unix mode goes into the external attributes: `0100755` for Mach-O executables, `0100644` for
  files, `0040755` for directories. **Symlinks are stored as symlinks** (`S_IFLNK`).
- Entries are sorted. Timestamps are fixed from `SOURCE_DATE_EPOCH` or the archive creation date,
  making the output byte-reproducible for identical inputs.
- Compression is deflate level 6; `zip64` is used automatically above 4 GiB. Root-level junk
  (`__MACOSX`, `.DS_Store`) is excluded.

```rust
pub struct IpaOptions { pub method: ExportMethod, pub swift_support: SwiftSupport, pub mtime: Option<std::time::SystemTime>, pub level: u32 }
pub fn package_ipa(app: &std::path::Path, opts: &IpaOptions, out: &std::path::Path) -> Result<IpaInfo, ShipError>;
pub struct IpaInfo { pub path: std::path::PathBuf, pub size: u64, pub md5: String, pub sha256: String, pub bundle_id: String, pub version: String, pub build: String }
```

### 9.4 Offline validation

`validate_ipa(ipa, ValidationProfile::AppStore)` runs every L-* lint from §8.6 plus the omarchy
`asc.py validate` set (about 45 checks), each mapped to its ITMS code where one exists:

- zip layout;
- exactly one `.app`;
- required Info.plist keys;
- version formats;
- `CFBundlePackageType = APPL`;
- `CFBundleSupportedPlatforms = [iPhoneOS]`;
- launch screen declared;
- main storyboard compiled;
- `UIDeviceFamily ⊆ {1,2}`;
- the four iPad orientations unless `UIRequiresFullScreen`;
- icon sizes;
- profile type App Store, with no devices;
- signature seals;
- per-appex checks.

**Output:** a `ValidationReport { checks: Vec<Check{id, itms: Option<String>, ok, message}> }` in
JSON and human-readable form.

### 9.5 Opt-in, visible metadata stamping

**Principle.** Russet never stamps silently, never invents values, and stamps as little as possible.
The landscape lists this as risk R4.

- Values come only from `SdkMetadata`, that is, the user's own imported Xcode and SDK [V17].
- Stamping is off by default: `russet.toml [ship] stamp = "off" | "sdk" | "xcode-equivalent"`.

| Key / field | Tier | Value (this host) | Source |
|---|---|---|---|
| `LC_BUILD_VERSION.sdk` (every Mach-O) | **sdk** | 27.0 | SDK `SDKSettings.json` `Version`. **The right fix is in the link step** (`ld64.lld -platform_version ios <minos> 27.0`; build area). Patching is a fallback; today the field equals minos [V11] |
| `DTPlatformName` | sdk | `iphoneos` | constant |
| `DTPlatformVersion` | sdk | `27.0` | `SDKSettings.json` |
| `DTSDKName` | sdk | `iphoneos27.0` | `SDKSettings.json` `CanonicalName` |
| `DTPlatformBuild`, `DTSDKBuild` | sdk | `24A430` | SDK `SystemVersion.plist` |
| `DTXcode` | **xcode-equivalent** | `2700` | imported `Xcode.app/Contents/version.plist` `CFBundleShortVersionString` |
| `DTXcodeBuild` | xcode-equivalent | `27A266a` | same, `ProductBuildVersion` |
| `DTCompiler` | xcode-equivalent | `com.apple.compilers.llvm.clang.1_0` | constant |
| `BuildMachineOSBuild` | not set | none | Linux has no macOS build; never invented |
| Marker `RussetToolchain` | visible (default **on** when stamping; §21 Q1) | `{tool: "russet", version, host: "linux-x86_64"}` | discloses the real toolchain |

The same keys go into every appex Info.plist, along with the app's versions and `arm64` (as in
omarchy `asc.py stamp`).

**Tier logic.** The `sdk` tier records facts: the app **was** compiled against that SDK. The
`xcode-equivalent` tier asserts an Xcode version. Its consent text says so plainly, and recommends
the Apple-hardware mode for App Store submissions.

**Flow:**

1. `plan_stamp(app, sdk, tier)` returns a `StampPlan`: the per-file Info.plist changes and Mach-O
   changes.
2. The IDE or CLI shows the diff.
3. Consent is stored in the committed `russet.toml`:

   ```toml
   stamp_consent = { tier = "sdk", by = "<git user>", at = 2026-10-11, russet = "0.x" }
   ```

4. `apply_stamp(app, plan, consent)` makes the changes, then the app is re-signed.

**Interaction with L-MACHO-001.** Raising `sdk` to at least 27.0 turns on the iOS 27 dyld
string-pool check. Stamping therefore always runs the alignment fix first.

The minimal key set is still [A]. Task SA-12 bisects it with TestFlight canaries, as omarchy did for
the asset catalog, and with the P0-3 Mac diff when the oracle is reachable [V20].

### 9.6 Upload (ASC build-upload API)

Verified current [V7], with field names from spec 4.5.1 [V6]. The algorithm follows omarchy
`asc.py upload` (MIT) and the asc CLI's commit reconciliation (MIT) [S].

1. **Preconditions.**
   - `GET /v1/apps?filter[bundleId]=B` must return the app record. Otherwise `E_SHIP_NO_APP_RECORD`,
     and the wizard (§13.5) helps create it on the web; the API cannot [V6].
   - The build number must be unique for the version. Russet checks this with
     `GET /v1/builds?filter[app]=…&filter[preReleaseVersion.version]=V&filter[version]=N`.
   - With `[ship] build_number = "auto"`, the build number is the highest existing number + 1, or
     `yyyymmddHHMM`.
2. `POST /v1/buildUploads` with `{cfBundleShortVersionString, cfBundleVersion, platform: IOS}` and
   relationship `app`.
3. `POST /v1/buildUploadFiles` with `{fileName, fileSize, uti: "com.apple.ipa", assetType: "ASSET"}`
   and relationship `buildUpload`. Uploading only `ASSET` is enough: Linux-built IPAs reached
   **VALID** this way [S omarchy receipts 2026-10-04 and 2026-10-09].
4. **Parts.** For each `uploadOperations[i]`
   `{method, url, offset, length, requestHeaders[], expiration, partNumber}`, send bytes
   `[offset, offset+length)`.
   - Parallelism defaults to 4.
   - Each part retries 5 times with exponential backoff.
   - Each completed part is recorded in the journal
     (`$STATE/russet/uploads/<uploadId>.json {ipa sha256, parts done, expiration}`), which allows
     `russet ship upload --resume <id>`.
5. **Commit.** `PATCH /v1/buildUploadFiles/{id}` with
   `{uploaded: true, sourceFileChecksums: {file: {hash: <md5 hex>, algorithm: "MD5"}}}`.
   MD5 is the variant proven by omarchy; `SHA_256` is also in the spec [V6].
6. **Reconciliation.** If the commit times out or returns 5xx or 408, Russet re-reads
   `GET /v1/buildUploads/{id}`. It accepts `PROCESSING` or `COMPLETE`, and otherwise reports
   `E_SHIP_COMMIT_AMBIGUOUS` with the upload ID [S asc CLI `build_uploads.go`].
7. **Poll** `GET /v1/buildUploads/{id}` every 15 seconds.
   - `state.state ∈ {AWAITING_UPLOAD, PROCESSING, FAILED, COMPLETE}`.
   - Russet shows `state.errors[]`, `warnings[]` and `infos[]` (`code`, `description`), for example
     `90725`, `90562`, `90713`.
   - The usual time to `COMPLETE` is about 2 minutes.
   - **Wedge detection:** after 30 minutes in `PROCESSING` with no errors, Russet warns about the
     known asset-catalog wedge (L-BND-008), and keeps polling until a 60-minute default timeout
     [S omarchy receipt 2026-10-09].
8. **Build processing.** Russet follows the `build` relationship and polls
   `GET /v1/builds/{id}` until `processingState` is `VALID` or `INVALID`. It then optionally:
   - patches `usesNonExemptEncryption`, when the Info.plist has no `ITSAppUsesNonExemptEncryption`
     (`PATCH /v1/builds/{id}`) [V6];
   - continues with TestFlight (§9.7).
9. **Webhook.** `BUILD_UPLOAD_STATE_UPDATED` is supported only when the user configures a public
   endpoint; polling is the default.

```rust
pub struct Uploader { /* Arc<AscClient>, journal dir, parallelism */ }
impl Uploader {
    pub async fn upload(&self, ipa: &std::path::Path, opts: &UploadOptions, progress: &dyn ProgressSink) -> Result<UploadOutcome, ShipError>;
    pub async fn resume(&self, upload_id: &str, ipa: &std::path::Path, progress: &dyn ProgressSink) -> Result<UploadOutcome, ShipError>;
    pub async fn wait(&self, upload_id: &str, timeout: std::time::Duration, progress: &dyn ProgressSink) -> Result<ProcessingResult, ShipError>;
}
pub struct UploadOptions { pub app_id: Option<String>, pub wait: WaitMode /* None | UploadComplete | BuildValid */ }
pub struct UploadOutcome { pub upload_id: String, pub build_id: Option<String>, pub state: String, pub diagnostics: Vec<Diagnostic> }
```

**Alternatives (not the default):**

- **Transporter 4.2** (Windows 11 and RHEL) needs an `AppStoreInfo.plist` that only macOS `swinfo`
  produces [L sg-27/28].
- **altool** is macOS-only.
- Russet supports neither in the Linux-native path. In the Apple-hardware path, the runner exports
  the IPA and Russet's uploader still sends it (§9.9).

### 9.7 TestFlight

All of these go through API endpoints that exist in 4.5.1 [V6]:

| Action | API |
|---|---|
| List or create groups (internal: `isInternalGroup`; external: public link options) | `GET/POST /v1/betaGroups` (filter `app`) |
| Add or remove testers (email; internal testers must be App Store Connect users) | `POST /v1/betaTesters` (relationship `betaGroups`), `POST/DELETE /v1/betaGroups/{id}/relationships/betaTesters` |
| Add a build to groups | `POST /v1/builds/{id}/relationships/betaGroups` |
| What to Test | `POST /v1/betaBuildLocalizations {whatsNew, locale}` |
| Export compliance | Info.plist `ITSAppUsesNonExemptEncryption`, or `PATCH /v1/builds/{id} {usesNonExemptEncryption}` |
| External beta review | `POST /v1/betaAppReviewSubmissions {build}`; needs `betaAppReviewDetails` |

Russet methods (ARCHITECTURE §8.4, [D 64]): `ship.testflight.groups`, `.groups.create`, `.testers`,
`.addTesters`, `.removeTesters`, `.distribute` (with export compliance and an optional tester
notification) and `.betaReview.submit`. W23's wiremock suite asserts the exact request sequence of each.
| Notify testers | `POST /v1/buildBetaNotifications` |
| Crash feedback | `GET /v1/apps/{id}/betaFeedbackCrashSubmissions`, `GET …/{id}/crashLog`; also `GET /v1/builds/{id}/diagnosticSignatures` → `/logs` |

The owner's team already has 3 internal groups and 1 external group with a public link [V4]. Russet
lists and reuses them, and never deletes groups.

### 9.8 Metadata, screenshots and submission (M5+)

- **Versions:**
  - `POST /v1/appStoreVersions {platform, versionString, …}` with relationships `app` and `build`;
  - `PATCH /v1/appStoreVersionLocalizations/{id}` (`description`, `keywords`, `whatsNew`,
    `promotionalText`, `supportUrl`, `marketingUrl`).
- **Screenshots and previews:** App Asset Library [V8].
  1. `GET /v1/apps/{id}/assetLibrary`.
  2. `GET /v1/appAssetLibraryRefData` for specifications and placement limits.
  3. `POST /v1/appAssetLibraryImages {fileName, fileSize, category: APP_SCREENSHOTS_AND_PREVIEWS}`.
  4. Run the returned upload operations (same 4-step reservation flow [S *Uploading assets*]).
  5. `POST /v1/appAssetLibraryPlacements {placementType: APP_SCREENSHOT}`, related to the
     `appStoreVersionLocalization`.
  6. Order with `POST /v1/appAssetLibraryPlacementOrderingRequests`.
  The deprecated `appScreenshotSets` path is not implemented.
- **Local format.** A fastlane-`deliver`-like directory, `metadata/<locale>/{description.txt, keywords.txt, …}`
  and `screenshots/<locale>/<displayType>/*.png`, synchronised by
  `russet ship metadata pull|push` (plan → approve → apply).
- **Review submission:** `POST /v1/reviewSubmissions {platform}` (relationship `app`), then
  `POST /v1/reviewSubmissionItems` (`appStoreVersion`), then `PATCH /v1/reviewSubmissions/{id}`
  (submit). This is always a human-approved action and never exposed to agents.
- **Russet methods** (ARCHITECTURE §8.4, [D 64]): `ship.version.plan/apply` (create or reuse the
  `appStoreVersion`, attach the build, export compliance), `ship.review.submit` (typed confirmation
  of `"<app name> <version>"`; CLI and IDE only) and `ship.review.status`. W23's wiremock suite
  asserts the exact POST/PATCH sequence; a live submission needs the owner's approval (ARCHITECTURE
  §21 Q32).

### 9.9 Apple-hardware release mode

**Purpose:** a licence-clean final build. Apple's SDK runs on Apple hardware, so no stamping is
needed (landscape §2.12, R1, R4).

**(A) GitHub Actions macOS runner** (default for this mode). Generated by
`russet ship ci init github` [ARCHITECTURE D 76].

Runner facts (2026-10-11): `xcode-27` is the only GitHub-hosted image with Xcode 27 (public preview:
no SLA, possible queueing). Image 20261006.0244.1 runs macOS 27.0.1 (26A434) with Xcode 27.0
(27A266a) as the default at `/Applications/Xcode_27.app` (symlink `Xcode_27.0.app`), plus Xcode 27.1
at `Xcode_27.1_beta.app` and 27.2 beta 2. `macos-26` and `macos-latest` carry only Xcode
26.0.1–26.6. There is no `macos-27` label, and the GA label is unannounced [S runner-images README,
#14404]. The template below was checked: PyYAML parses it, every `run:` block passes `bash -n`, no
`run:` block contains a `${{ }}` expression (zizmor's template-injection rule), and the empty-array
idiom works on the image's bash 3.2.

```yaml
# .github/workflows/russet-release.yml (generated by `russet ship ci init github`; edit freely)
# Apple-hardware release: Apple's toolchain builds the app on a GitHub macOS runner.
name: russet-release
on:
  workflow_dispatch:
    inputs:
      version: { description: "CFBundleShortVersionString", required: true, type: string }
      build: { description: "CFBundleVersion", required: true, type: string }
permissions: { contents: read }
concurrency: { group: russet-release, cancel-in-progress: false }
jobs:
  ios:
    # Chosen from the imported SDK's Xcode major via pins/stable.toml: 27 -> xcode-27 (public preview), 26 -> macos-26.
    runs-on: ${{ vars.RUSSET_RUNNER || 'xcode-27' }}
    timeout-minutes: 45
    env:
      RUSSET_XCODE: ${{ vars.RUSSET_XCODE || '27.0' }}        # the imported SDK's Xcode version
      RUSSET_SIGNING: ${{ vars.RUSSET_SIGNING || 'russet' }}   # russet | xcode-manual | xcode-cloud
      SCHEME: ${{ vars.RUSSET_SCHEME }}
      WORKSPACE: ${{ vars.RUSSET_WORKSPACE }}                  # SwiftPM: xtool/<Product>.xcworkspace
      PROJECT: ${{ vars.RUSSET_PROJECT }}
      APP_VERSION: ${{ inputs.version }}
      APP_BUILD: ${{ inputs.build }}
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with: { persist-credentials: false }
      - name: Select Xcode (exact version, never a beta)
        run: |
          set -euo pipefail
          app="/Applications/Xcode_${RUSSET_XCODE}.app"
          if [ ! -d "$app" ]; then
            echo "::error::Xcode ${RUSSET_XCODE} is not on this image"; ls -d /Applications/Xcode_*.app; exit 1
          fi
          real="$(cd "$app" && pwd -P)"
          case "$real" in *[Bb]eta*) echo "::error::$app resolves to a beta: $real"; exit 1 ;; esac
          echo "DEVELOPER_DIR=$app/Contents/Developer" >> "$GITHUB_ENV"
          DEVELOPER_DIR="$app/Contents/Developer" xcodebuild -version | tee "$RUNNER_TEMP/xcode-version.txt"
          v="$(head -1 "$RUNNER_TEMP/xcode-version.txt")"
          case "$v" in "Xcode ${RUSSET_XCODE}" | "Xcode ${RUSSET_XCODE}."*) ;; *) echo "::error::got $v"; exit 1 ;; esac
      - name: Generate the Xcode workspace (SwiftPM stack; xtool pinned by hash)
        if: ${{ vars.RUSSET_STACK == 'swiftpm' }}
        run: |
          set -euo pipefail
          curl -fsSL -o "$RUNNER_TEMP/xtool.app.zip" https://github.com/xtool-org/xtool/releases/download/1.21.0/xtool.app.zip
          echo "33bb2ef12de3f2acfb5eb8385fca0de9df3a982862ec067185a59e8ae18b64e8  $RUNNER_TEMP/xtool.app.zip" | shasum -a 256 -c -
          ditto -x -k "$RUNNER_TEMP/xtool.app.zip" "$RUNNER_TEMP/xtool"
          "$RUNNER_TEMP/xtool/xtool.app/Contents/Resources/bin/xtool" dev generate-xcode-project
      - name: Archive, unsigned (no portal calls, no certificates minted)
        run: |
          set -euo pipefail
          c=()
          if [ -n "${WORKSPACE:-}" ]; then c=(-workspace "$WORKSPACE"); elif [ -n "${PROJECT:-}" ]; then c=(-project "$PROJECT"); fi
          a=(-scheme "$SCHEME" -destination 'generic/platform=iOS' -archivePath "$RUNNER_TEMP/App.xcarchive"
             CODE_SIGNING_ALLOWED=NO MARKETING_VERSION="$APP_VERSION" CURRENT_PROJECT_VERSION="$APP_BUILD")
          xcodebuild archive ${c[@]+"${c[@]}"} "${a[@]}"
      - name: Carry entitlements into the export (throwaway identity, every bundle inside-out)
        if: ${{ env.RUSSET_SIGNING != 'russet' }}
        run: .russet/ci/presign.sh "$RUNNER_TEMP/App.xcarchive" .russet/ci/entitlements
      - name: Distribution identity (xcode-manual)
        if: ${{ env.RUSSET_SIGNING == 'xcode-manual' }}
        env:
          P12_B64: ${{ secrets.RUSSET_DIST_P12_B64 }}
          P12_PASSWORD: ${{ secrets.RUSSET_DIST_P12_PASSWORD }}
          PROFILES_B64: ${{ secrets.RUSSET_DIST_PROFILES_B64 }}
        run: .russet/ci/import-identity.sh
      - name: API key (xcode-cloud only; needs an Admin-role team key)
        if: ${{ env.RUSSET_SIGNING == 'xcode-cloud' }}
        env:
          P8: ${{ secrets.RUSSET_ASC_KEY_P8 }}
          KEY_ID: ${{ vars.RUSSET_ASC_KEY_ID }}
        run: |
          umask 077
          mkdir -p "$RUNNER_TEMP/asc"
          printf '%s' "$P8" > "$RUNNER_TEMP/asc/AuthKey_${KEY_ID}.p8"
      - name: Export with Xcode (xcode-manual or xcode-cloud)
        if: ${{ env.RUSSET_SIGNING != 'russet' }}
        env:
          KEY_ID: ${{ vars.RUSSET_ASC_KEY_ID }}
          ISSUER_ID: ${{ vars.RUSSET_ASC_ISSUER_ID }}
        run: |
          set -euo pipefail
          a=(-exportArchive -archivePath "$RUNNER_TEMP/App.xcarchive" -exportPath "$RUNNER_TEMP/export"
             -exportOptionsPlist ".russet/ci/ExportOptions.${RUSSET_SIGNING}.plist")
          if [ "$RUSSET_SIGNING" = xcode-cloud ]; then
            a+=(-allowProvisioningUpdates -authenticationKeyPath "$RUNNER_TEMP/asc/AuthKey_${KEY_ID}.p8"
                -authenticationKeyID "$KEY_ID" -authenticationKeyIssuerID "$ISSUER_ID")
          fi
          if ! xcodebuild "${a[@]}" > "$RUNNER_TEMP/export.log" 2>&1; then
            tail -n 50 "$RUNNER_TEMP/export.log"
            if grep -q 'Cloud signing permission error' "$RUNNER_TEMP/export.log"; then
              echo "::error::Cloud signing needs an Admin-role team key (App Manager keys are refused). Use RUSSET_SIGNING=russet or xcode-manual."
            fi
            exit 1
          fi
          .russet/ci/verify-entitlements.sh "$RUNNER_TEMP"/export/*.ipa .russet/ci/entitlements
      - name: Zip the unsigned archive (russet mode; Russet signs, validates and uploads locally)
        if: ${{ env.RUSSET_SIGNING == 'russet' }}
        run: ditto -c -k --keepParent "$RUNNER_TEMP/App.xcarchive" "$RUNNER_TEMP/App.xcarchive.zip"
      - uses: actions/upload-artifact@cf430e030ddbb5b0abf93d22962f4752f3646cd9 # v7.0.2
        with:
          name: russet-build
          path: |
            ${{ runner.temp }}/export/*.ipa
            ${{ runner.temp }}/App.xcarchive.zip
          if-no-files-found: error
          retention-days: 3          # Russet deletes the artifact after `ship.ci.fetch`; Free plans have 500 MB
      - uses: actions/upload-artifact@cf430e030ddbb5b0abf93d22962f4752f3646cd9 # v7.0.2
        if: ${{ env.RUSSET_SIGNING != 'russet' }}   # russet mode: the dSYMs are inside App.xcarchive.zip
        with: { name: russet-dsyms, path: "${{ runner.temp }}/App.xcarchive/dSYMs", retention-days: 3 }
      - name: Remove signing material
        if: ${{ always() }}
        run: |
          security delete-keychain "$RUNNER_TEMP/russet.keychain-db" 2>/dev/null || true
          rm -rf "$RUNNER_TEMP/asc"
```

**Signing modes.** `ship.ci.init` asks "Who signs?":
- `russet` (default): the runner returns the unsigned `.xcarchive`. Russet signs, validates and
  uploads it locally, with the same Apple Distribution identity as the Linux-native path. No Apple
  credentials go to GitHub, and it works with the App Manager key.
- `xcode-manual`: Xcode on the runner exports with the distribution `.p12` and App Store profiles
  from GitHub secrets. This gives Xcode's own packaging, which is useful for parity checks.
- `xcode-cloud`: cloud-managed signing. It needs an Admin-role team key stored on GitHub (§6.6);
  App Manager keys fail with "Cloud signing permission error" (`ship.ciCloudSigningForbidden`, 7009).

**Generated helpers** in `.russet/ci/` (0BSD):
- `presign.sh`:
  1. creates a temporary keychain at `$RUNNER_TEMP/russet.keychain-db`;
  2. generates a throwaway RSA code-signing certificate with `/usr/bin/openssl` (LibreSSL 3.3.6 on
     macOS 27.0.1; a config file with `extendedKeyUsage=codeSigning`);
  3. imports it with `security import -T /usr/bin/codesign` and runs `set-key-partition-list`;
  4. signs each bundle listed in `entitlements/manifest.tsv`, deepest first (frameworks,
     `PlugIns/*.appex`, then the app), with `codesign --force -s <name> --keychain <kc> [--entitlements <plist>]`.

  Without this step, `-exportArchive` re-signs an unsigned archive with only the profile defaults and
  drops capability entitlements such as `aps-environment` and associated domains; Tauri pre-signs only
  the main binary, so its app extensions still lose theirs (tauri#15663) [S].
- `import-identity.sh`: imports the distribution `.p12` into the temporary keychain and installs the
  App Store profiles [A: whether Xcode 27 reads `~/Library/Developer/Xcode/UserData/Provisioning
  Profiles` or `~/Library/MobileDevice/Provisioning Profiles`; it installs into both].
- `verify-entitlements.sh`: unzips the IPA and compares `codesign -d --entitlements - --xml` of every
  bundle with Russet's expected distribution entitlements. It fails, for example, if `aps-environment`
  is not `production`.
- `ExportOptions.xcode-manual.plist`: `method = app-store-connect` (`app-store` is deprecated),
  `destination = export`, `signingStyle = manual`, `signingCertificate = "Apple Distribution"`,
  `provisioningProfiles` = {bundle id → profile} for the app and every appex, `teamID`,
  `manageAppVersionAndBuildNumber = false` (it defaults to YES).
- `ExportOptions.xcode-cloud.plist`: the same with `signingStyle = automatic` and no
  `provisioningProfiles`. Xcode 27's help says a manually signed archive exported this way gets
  profiles and cloud certificates without device or App ID changes, so App ID capabilities must
  already be applied by Russet's provisioning plan/apply.

Russet does **not** use `destination = upload`: in a 2021 forum report it did not work with API-key
auth, unverified since [A] [S Apple forums 682091]. Upload is always done by Russet (§9.6).

- **Upload:**
  - Default: the IDE (or `russet ship ci fetch`) downloads the artifact through the GitHub API,
    deletes it from GitHub (Free plans have 500 MB of artifact storage), and runs
    `russet ship upload` locally, so the key never has to upload from GitHub.
  - Alternative: a second job on `ubuntu-24.04` (never a moving `-latest` label) runs the Linux
    `russet` binary.
- **Setup wizard (§13.6).**
  - Creates the workflow file and the `.russet/ci/` helpers.
  - Sets the repository variables (`RUSSET_RUNNER`, `RUSSET_XCODE`, `RUSSET_SIGNING`,
    `RUSSET_SCHEME`, `RUSSET_WORKSPACE` or `RUSSET_PROJECT`, `RUSSET_STACK`) and, only for the
    Xcode modes, the secrets, through the GitHub REST API: a libsodium sealed box with the
    repository's public key, using the user's GitHub token from `GithubToken` in secrets.
  - Explains the cost: private repositories consume the included minutes at list price (a macOS
    minute counts as about 10.3 Linux minutes); public repositories run standard macOS runners for
    free; `-large`/`-xlarge` runners are never free (QA §4.8).
- **Stacks:**
  - SwiftPM: the hash-pinned xtool 1.21.0 generates `xtool/<Product>.xcworkspace` (which wraps
    `xtool/.xtool-tmp/<Product>.xcodeproj`); `RUSSET_WORKSPACE` names it.
  - Flutter: `flutter build ipa --no-codesign` [A], with the project's pinned Flutter installed in the
    job (the image has none).
  - Xcode projects and React Native: `pod install` (CocoaPods 1.17.0 is preinstalled), then
    `xcodebuild archive` on the `.xcworkspace`.
- **Alternatives** (none is needed for Russet's own CI once the repository is public, because
  `xcode-27` is then free):
  - a self-hosted macOS runner is free, and users can set `RUSSET_RUNNER` to its label. Not for
    Russet's own CI: the owner's 8 GB M1 is the RE oracle and hosts a VM, and GitHub advises against
    self-hosted runners on public repositories;
  - Codemagic: 500 free M2 minutes a month on the individual plan, then $0.095/min, with Xcode 27.0
    already installed; a possible later `ship.ci.init codemagic` provider, outside W23's scope;
  - Xcode Cloud: (B) below.

**(B) Xcode Cloud through the ASC API** (only for teams that already have a product):

- **Limitation.** `ciProducts` cannot be created through the API [V6]. Apple's setup guide assumes
  Xcode on a Mac [S *Setting up your project to use Xcode Cloud*]. The owner's team has 0 products
  [V4], so this path is optional and mocked in tests.
- **Price:** 25 compute hours a month are included with the Apple Developer Program; add-ons cost
  $49.99, $99.99, $399.99 or $3,999.99 a month for 100, 250, 1,000 or 10,000 hours. Onboarding is
  configured in Xcode on a Mac [S Xcode Cloud].
- **Operations:**
  - list `ciProducts` and `ciWorkflows`;
  - `POST /v1/ciWorkflows`, when the product exists;
  - `POST /v1/ciBuildRuns {workflow, sourceBranchOrTag | pullRequest, clean}`;
  - poll `GET /v1/ciBuildRuns/{id}`;
  - list `ciBuildActions`, `ciIssues` and `ciTestResults`;
  - download `ciArtifacts` (`fileType` ARCHIVE, ARCHIVE_EXPORT, LOG_BUNDLE, …) through
    `downloadUrl`.
- **Tokens.** Monitoring tokens can be GET-scoped with long lifetimes; Xcode Cloud resources allow up
  to 6 months [S].
- **SwiftPM-only repos** need a generated `.xcodeproj` committed, or created in
  `ci_scripts/ci_post_clone.sh` [A: whether Xcode Cloud resolves the container path after
  post-clone].

**Differences report.** `russet ship compare <linux.ipa> <applehw.ipa>` diffs Info.plist keys,
`LC_BUILD_VERSION`, signatures, entitlements, the bundle file list and asset catalogs. This is the
M5 exit criterion and feeds the stamping minimisation.

### 9.10 `russet-ship` top-level API (summary)

```rust
pub fn create_archive(req: &ArchiveRequest, out_dir: &std::path::Path) -> Result<Archive, ShipError>;
pub fn generate_dsyms(binaries: &[std::path::PathBuf], out_dir: &std::path::Path, dsymutil: &std::path::Path) -> Result<Vec<std::path::PathBuf>, ShipError>;
pub fn package_ipa(app: &std::path::Path, opts: &IpaOptions, out: &std::path::Path) -> Result<IpaInfo, ShipError>;
pub fn plan_stamp(app: &std::path::Path, sdk: &SdkMetadata, tier: StampTier) -> Result<StampPlan, ShipError>;
pub fn apply_stamp(app: &std::path::Path, plan: &StampPlan, consent: &StampConsent) -> Result<StampReport, ShipError>;
pub fn validate_ipa(ipa: &std::path::Path, profile: ValidationProfile) -> Result<ValidationReport, ShipError>;
pub struct TestFlight<'a> { /* &AscClient */ }
pub struct Metadata<'a> { /* &AscClient */ }
pub mod apple_hw { /* render_github_workflow, configure_github_repo, dispatch, status, fetch_artifacts, XcodeCloud */ }
pub fn compare_ipas(a: &std::path::Path, b: &std::path::Path) -> Result<IpaDiff, ShipError>;
```

---

## 10. Daemon JSON-RPC API

> **Superseded (ARCHITECTURE §8.4, §9, §20.1):** method names, the long-running model (requests stay
> open unless `background: true`), notifications (`job.updated`/`job.events`), approvals
> (`approval.list/get/grant/deny`, approval ids instead of HMAC tokens) and error codes are defined
> there. This table is kept for its rationale.

This uses the transport and envelope of the daemon design: JSON-RPC 2.0, a Unix socket on Linux and
a named pipe on Windows [A: reconcile with the daemon doc].

- Long-running methods return `{jobId}` immediately and stream `job.progress` and `job.done`
  notifications.
- Errors use `error.data = {code: "E_…", remedy, docs, retryable}` (§16).

| Method | Params | Result |
|---|---|---|
| `account.list` | `{}` | `Account[]` |
| `account.addAscKey` | `{keyId, issuerId?, p8Path? \| p8Pem?, label?}` | `Account` (the PEM is read and moved into secrets; never echoed) |
| `account.verify` | `{accountId}` | `{ok, teamId?, roleProbe: {certificates, profiles, builds}: "ok"\|"forbidden"}` (GET-scoped) |
| `account.appleId.begin` | `{appleId, password, anisette: "local"\|{"remote": url}}` | `{sessionId, step: "done"\|"twoFactor", twoFactor?: {via: "trustedDevice"\|"sms", numbers?: [{id, masked}]}}` |
| `account.appleId.requestCode` | `{sessionId, via, phoneId?}` | `{}` |
| `account.appleId.submitCode` | `{sessionId, code}` | `Account` |
| `account.remove` / `account.setDefault` | `{accountId, wipeSecrets?}` | `{}` |
| `account.anisette.status` | `{}` | `{kind, librariesPresent, apkVersion?, provisioned}` |
| `account.anisette.fetchLibraries` | `{consent: true}` | job → `{apkVersion, bytesDownloaded}` |
| `identity.list` / `identity.importP12` / `identity.exportP12` / `identity.importXtool` | see §6.3 | `SigningIdentity[]` / `{}` |
| `portal.certificates.list` / `.devices.list` / `.bundleIds.list` / `.profiles.list` | `{accountId, filter?}` | records (UDIDs masked unless `reveal: true` from the IDE) |
| `provision.plan` | `ProvisionRequest` + `{accountId, project?}` | `ProvisionPlan` |
| `provision.apply` | `{planId, approvalToken}` | job → `ProvisionOutcome` |
| `cert.revoke` | `{accountId, certId, typedConfirmation}` | `{}` (IDE and CLI only) |
| `device.register` | `{accountId, udid, name}` | plan (auto-approved per policy) |
| `sign.run` | `{appPath, distribution, identityId?, profiles?: {relPath: path}, mergePolicy?, output?}` | job → `SignReport` |
| `sign.verify` / `sign.lint` / `sign.inspect` | `{path, profile?}` | `VerifyReport` / `LintFinding[]` / `SignatureInfo[]` |
| `ship.archive` | `{project, configuration?, exportMethod}` | job → `Archive` |
| `ship.ipa` | `{archivePath, method}` | `IpaInfo` |
| `ship.stamp.plan` / `ship.stamp.apply` | `{archivePath, tier}` / `{planId, consentToken}` | `StampPlan` / `StampReport` |
| `ship.validate` | `{ipaPath}` | `ValidationReport` |
| `ship.upload` | `{ipaPath, appId?, wait}` + approval | job → `UploadOutcome` (progress: bytes, parts, state) |
| `ship.upload.resume` / `ship.upload.status` | `{uploadId}` | job / `UploadOutcome` |
| `ship.builds.list` | `{appId, limit?}` | `Build[]` |
| `ship.testflight.groups` / `.addTesters` / `.distribute` | `{appId}` / `{groupId, emails}` / `{buildId, groupIds, whatToTest?}` + approval | records / job |
| `ship.metadata.pull` / `.plan` / `.apply` | `{appId, version, dir}` | files / plan / job |
| `ship.ci.init` / `.dispatch` / `.status` / `.fetch` | `{provider: "github"\|"xcodeCloud", …}` | config / `{runId}` / status / paths |
| `ship.crashes.list` / `.get` | `{appId, buildId?, limit}` / `{submissionId, symbolicate: true}` | crash summaries / symbolicated log |
| `approval.pending` / `approval.grant` / `approval.deny` | `{}` / `{approvalId}` (**IDE and CLI TTY only**) | requests / `{approvalToken}` |

**Notifications:**

- `job.progress {jobId, stage, fraction?, message}`
- `job.done {jobId, result | error}`
- `account.event {kind: "tokenExpiring"|"profileExpiring"|"freeQuota", accountId, detail}`
- `approval.requested {approvalId, summary, mutations[], requester: "mcp:<client>"|"cli"|"ide"}`

**Approvals.** The approval token is an HMAC computed by the daemon over `(planId, plan digest,
expiry)`. Only the IDE dialog or a CLI TTY prompt can mint one; `approval.grant` refuses
connections that identify as MCP.

---

## 11. MCP tools

These tools are exposed by the built-in MCP server.

- Read tools use **GET-scoped JWTs** [V3]. A compromised agent session cannot mutate even through a
  token it obtains.
- Mutating tools only **request** approval.

```json
[
  {"name": "russet_account_status", "description": "Accounts, teams, token health, free-team quota. Read-only.",
   "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false}},
  {"name": "russet_signing_status", "description": "What identity/profile/entitlements a project would use per bundle, and lint findings. Read-only.",
   "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}, "distribution": {"enum": ["development", "ad-hoc", "app-store-connect"]}}, "required": ["project"]}},
  {"name": "russet_provision_plan", "description": "Dry-run provisioning; returns the mutations that would be made. Read-only.",
   "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}, "distribution": {"enum": ["development", "ad-hoc", "app-store-connect"]}, "connectedDevice": {"type": "boolean", "default": true}}, "required": ["project"]}},
  {"name": "russet_request_approval", "description": "Ask the human (IDE dialog) to approve a plan returned by russet_provision_plan / russet_ship_upload_plan. Returns approvalId; poll russet_approval_status.",
   "inputSchema": {"type": "object", "properties": {"planId": {"type": "string"}, "reason": {"type": "string", "maxLength": 500}}, "required": ["planId", "reason"]}},
  {"name": "russet_approval_status", "inputSchema": {"type": "object", "properties": {"approvalId": {"type": "string"}}, "required": ["approvalId"]}},
  {"name": "russet_apply", "description": "Apply a plan after the human approved it.",
   "inputSchema": {"type": "object", "properties": {"approvalId": {"type": "string"}}, "required": ["approvalId"]}},
  {"name": "russet_sign_verify", "description": "Verify signatures and run lints on an .app/.ipa. Read-only.",
   "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}, "profile": {"enum": ["device-install", "app-store"]}}, "required": ["path"]}},
  {"name": "russet_ship_validate", "inputSchema": {"type": "object", "properties": {"ipa": {"type": "string"}}, "required": ["ipa"]}},
  {"name": "russet_ship_upload_plan", "description": "Prepare an upload (checks app record, build number). Upload itself requires approval.",
   "inputSchema": {"type": "object", "properties": {"ipa": {"type": "string"}, "distributeToGroups": {"type": "array", "items": {"type": "string"}}}, "required": ["ipa"]}},
  {"name": "russet_builds", "description": "Recent builds and processing states for an app. Read-only.",
   "inputSchema": {"type": "object", "properties": {"bundleId": {"type": "string"}, "limit": {"type": "integer", "maximum": 50}}, "required": ["bundleId"]}},
  {"name": "russet_crash_reports", "description": "TestFlight crash feedback, symbolicated with local dSYMs. Read-only.",
   "inputSchema": {"type": "object", "properties": {"bundleId": {"type": "string"}, "limit": {"type": "integer", "maximum": 20}}, "required": ["bundleId"]}},
  {"name": "russet_asc_get", "description": "Allowlisted read-only GET against App Store Connect (paths under /v1/apps, /v1/builds, /v1/betaGroups, /v1/bundleIds, /v1/profiles, /v1/devices, /v1/certificates, /v1/ci*). Token is scoped to exactly this request.",
   "inputSchema": {"type": "object", "properties": {"pathAndQuery": {"type": "string", "pattern": "^/v1/[A-Za-z]"}}, "required": ["pathAndQuery"]}}
]
```

**Never exposed to agents:**

- certificate revocation;
- account add or remove;
- password entry;
- review submission;
- `approval.grant`.

Tool output never contains key material, GS tokens, raw profiles or full UDIDs.

---

## 12. CLI

```text
russet account list | default <id> | remove <id> [--wipe-secrets] | verify [<id>]
russet account add-key --key-id <ID> [--issuer-id <UUID> | --individual] --p8 <path> [--label <text>]
russet account login --apple-id <email> [--anisette local|remote=<url>]        # 2FA prompted interactively
russet account import --from-xtool [--dry-run]
russet account anisette status | fetch-libraries [--yes] | reset
russet account certs list | create --type development|distribution | revoke <id> | import <p12> | export <id> --out <p12>
russet account devices list | register [--connected | --udid <u> --name <n>] | disable <id>
russet account bundle-ids list | register <identifier>
russet account profiles list | download <id> | delete <id>
russet provision [--distribution development|ad-hoc|app-store-connect] [--dry-run] [--yes] [--json]
russet sign <App.app|App.ipa> [--distribution …] [--identity <team/serial>] [--profile <bundle-rel>=<file>]…
            [--entitlements <bundle-rel>=<plist>]… [--merge least|profile-all] [--out <path>] [--no-fix <name>]
russet sign verify|lint|inspect <path> [--profile device-install|app-store] [--json]
russet ship archive [--configuration Release] [--method app-store-connect|ad-hoc|development]
russet ship ipa <archive> [--method …] [--out <file.ipa>]
russet ship stamp <archive> --tier sdk|xcode-equivalent [--yes]
russet ship validate <ipa> [--json]
russet ship upload <ipa> [--app-id <id>] [--wait upload|valid] [--yes] | --resume <uploadId>
russet ship builds [--bundle-id <id>] [--limit N]
russet ship testflight groups [create <name> [--external]] | testers [--group <g>] | add-testers|remove-testers --group <g> <email>…
                     | distribute <buildId> --group <g>… [--what-to-test <file>] [--encryption exempt|non-exempt] [--notify]
                     | beta-review <buildId>
russet ship metadata pull|push --version <v> [--dir fastlane-like/] [--dry-run]
russet ship version plan|apply --version <v> --build <id> [--encryption exempt|non-exempt] [--release manual|after-approval]
russet ship submit <versionId|--version <v>> [--confirm-text "<App> <v>"] [--dry-run] | review status
russet ship ci init github [--signing russet|xcode-manual|xcode-cloud] | init xcode-cloud | run [--ref <ref>] | status <run> | fetch <run>
russet ship compare <a.ipa> <b.ipa>
```

**Conventions:**

- Every mutating command prints the plan and asks for confirmation. Non-interactive runs need
  `--yes`.
- `--json` emits machine-readable results.
- Exit codes: ARCHITECTURE §13.4 is the only table ([D 8]); the list this draft had is superseded.

---

## 13. IDE wizards and panels

All of these run inside the bundled Russet extension, which talks JSON-RPC to the daemon.

### 13.1 Account setup (first run)

**Screen 1: choose an account type.**

- **"App Store Connect API key (recommended)":**
  - steps with a deep link to `https://appstoreconnect.apple.com/access/integrations/api`;
  - role: App Manager (the default; enough for provisioning, uploads and TestFlight) or Admin
    (needed only for the `xcode-cloud` signing mode, §6.6). Team keys carry only a role (Apple:
    "Under Access, select the role for the key"); "access to Certificates, Identifiers & Profiles"
    is a per-user permission, not a key setting;
  - drag and drop the `.p8` (the key ID is parsed from `AuthKey_<ID>.p8`);
  - issuer ID field;
  - **Test**, which calls `account.verify` and shows "Team: <name or ID>, permissions:
    certificates ✓, profiles ✓, builds ✓";
  - a note: "Apple lets you download this key once; keep your backup safe."
- **"Apple ID (free or personal team) (advanced)":**
  1. Risk and consent screen (§7.1).
  2. Anisette libraries consent, with download progress (about 2 MB).
  3. Email and password; the password is never stored unless "Remember for automatic weekly
     refresh" is ticked.
  4. 2FA: "Enter the code shown on your trusted device", or an SMS number picker.
  5. Team picker.
  6. Certificate step (§6.7 if revocation is needed).
  7. Summary showing the free-team limits.

**Screen 2: secrets backend status.** For example, "Stored in KDE Wallet (Secret Service)" [V19],
or "No keyring found; set a Russet passphrase".

### 13.2 Signing & Capabilities editor (per target)

- **Fields:**
  - team picker;
  - signing mode (Automatic or Manual);
  - bundle ID (exact; free teams show the mapped ID);
  - status rows per bundle (app and each extension): identity, profile name, expiry, devices
    covered, lint status.
- **Capabilities list:** toggles write the target's `.entitlements` (owned by the build area) and run
  `provision.plan`. Unsupported-on-free capabilities are greyed out with a tooltip.
- **App Groups and iCloud containers:** an inline notice ("needs a one-time step on
  developer.apple.com") with **Open portal** and **I've done it — re-check** buttons (§6.4 step 6).
- **Fix-it buttons for:**
  - "Register this iPhone";
  - "Recreate profile";
  - "Import distribution certificate (.p12)";
  - "Certificate limit reached → options".

### 13.3 F5 / Run on device (paid)

`provision.plan` runs with the connected device. With the default policy everything is
auto-approved, except a first certificate creation, which asks once. Then `sign.run` and the device
install (device area). Errors land in the Problems panel with fix-its.

### 13.4 Publishing panel

**Stepper:**

1. **Version and build:** auto-increment from ASC.
2. **Archive:** release build plus dSYMs.
3. **Validate:** list of checks with ITMS codes.
4. **Release mode:**
   - *Linux-native*, with a stamping consent card (tier selector, diff, honest wording);
   - or *Apple hardware*, starting the GitHub run.
5. **Upload:** progress bar with parts and resume support.
6. **Processing:** live `buildUpload` and `build` states, Apple's errors verbatim, wedge hint.
7. **TestFlight:** groups (list, create) and testers (add, remove), What to Test, export compliance,
   beta review for external groups (`ship.testflight.*`).
8. **App Store** (M5+): metadata editor, screenshots, version and build attach with export compliance
   (`ship.version.plan/apply`), **Submit for review** (`ship.review.submit`, a typed confirmation of
   the app name and version), review status.

### 13.5 App record helper

The API cannot create apps [V6]. The helper:

- shows the exact values to type: name, primary language, bundle ID (already registered), SKU,
  "Full Access";
- opens `https://appstoreconnect.apple.com/apps`;
- polls `GET /v1/apps?filter[bundleId]=` every 10 seconds, and continues automatically when the
  record appears.

### 13.6 Apple-hardware setup

1. Pick the provider: GitHub (recommended) or Xcode Cloud (shown only when `ciProducts` is not
   empty).
2. Pick the repository, using a GitHub token from `gh auth token` or a device-code login, stored as
   `GithubToken`.
3. The runner comes from the imported SDK through `pins` `[ci.github]` (27 → `xcode-27`, public
   preview [V18]; 26 → `macos-26`). Pick the signing mode (default `russet`).
4. Confirm that Russet will write the workflow file and the secrets.
5. Run a dry run on a branch, and show the result in the Publishing panel.

### 13.7 Crash reports (shared with the profiling/crash panel)

The panel lists TestFlight crash submissions per build, with a device, OS and date filter.

- Opening one fetches the `crashLog` and symbolicates it with the archive's dSYMs.
- Frames link to source.
- For local device crashes it uses the same symbolication; owned by the device area.

---

## 14. File formats

### 14.1 `accounts.toml`

```toml
version = 1
default = "asc:<KEYID>"

[[account]]
id = "asc:<KEYID>"
kind = "asc-team-key"           # asc-team-key | asc-individual-key | apple-id
label = "Team key (App Manager)"
key_id = "<KEYID>"
issuer_id = "<UUID>"
team_id = "<TEAMID>"            # discovered (§6.1)
secret = "keyring:asc-key/<KEYID>"

[[account]]
id = "appleid:<DSID>"
kind = "apple-id"
apple_id = "someone@example.com"
team_id = "<TEAMID>"
team_tier = "free"
anisette = "local"              # or { remote = "https://anisette.example/" }
secret = "keyring:apple-id/<DSID>"
```

### 14.2 Project `russet.toml` (committed)

```toml
[signing]
team = "<TEAMID>"
mode = "automatic"                          # automatic | manual
merge = "least"                             # least | profile-all

[signing.targets.app]
bundle_id = "com.example.hello"
entitlements = "Sources/Hello/Hello.entitlements"

[signing.targets."PlugIns/Widget.appex"]
bundle_id = "com.example.hello.widget"
entitlements = "Widget/Widget.entitlements"

[ship]
mode = "linux-native"                       # linux-native | apple-hardware
stamp = "off"                               # off | sdk | xcode-equivalent
build_number = "auto"
testflight_groups = ["Internal"]
app_id = "<numeric ASC app id>"             # filled on first upload
```

### 14.3 Other files

- **`.russet/signing.lock.json`** (gitignored; per machine):
  `{"team": "...", "identity": "<team>/<serial>", "profiles": {"": "<uuid>", "PlugIns/Widget.appex": "<uuid>"}, "resolvedAt": "..."}`.
- **`policy.toml`:** the auto-approval rules of §6.4. User-level, with a project-level override for
  stricter rules only.
- **`identities/<team>/<serial>.json` + `.cer`:** `SigningIdentity` without the key.
- **`profiles/index.json`:** `[{uuid, name, team, bundle, kind, expires, certSerials[], deviceCount}]`.
- **`uploads/<uploadId>.json`:**
  `{ipaSha256, ipaPath, fileId, parts: [{n, offset, length, done, etag}], expiration, createdAt}`.
- **`anisette/`:**
  - `libs/<apkVersion>/<abi>/*.so` (downloaded; 0700);
  - `state.json` (`{apkVersion, abi, provisioned, rinfo}`; the local user UUID and `adi.pb` live in
    secrets);
  - `anisette-pins.json` (shipped hashes).
- **`gsa-profile.json`:** §7.2.
- **`audit.log`:** JSON lines, one per applied mutation:
  `{at, account, team, mutation, approvalSource, result}`. No secrets.

---

## 15. Platform differences

| Topic | Linux | Windows |
|---|---|---|
| Secret store | Secret Service (KWallet/ksecretd [V19], GNOME Keyring); passphrase file store when headless | Credential Manager (2,560-byte blobs [V16]) + envelope files; optional CNG cert store for signing keys |
| File permissions | 0700 dirs, 0600 files, enforced and checked by `doctor` | user-profile ACLs; `doctor` warns if `%LOCALAPPDATA%` is redirected or roaming |
| Anisette | `russet-adi-host` x86_64/aarch64, seccomp + Landlock; xadi (LGPL) as an emergency fallback | `russet-adi-host` x86_64 with sysv64 shims and `%fs` handling [A]; Job object sandbox; no Windows on ARM |
| GSA TLS | Apple Root CA pinned store | same |
| IPA symlinks | native | input bundles from Windows builds have none; symlinks in inputs produced on Linux are preserved when re-zipping from the zip source |
| Long paths | n/a | `\\?\` prefixes in archive and IPA staging (landscape §5.1) |
| Device registration source | device daemon (usbmuxd) | device daemon (Apple Mobile Device Service) |
| Apple-hardware mode | same (cloud) | same (cloud) |

---

## 16. Error catalog

Each entry has a stable code, a message template and a remedy, and is shown in the CLI, the IDE and
MCP.

| Code | Meaning | Remedy |
|---|---|---|
| `E_SECRETS_NO_BACKEND` | No keyring and no passphrase | Install or enable a Secret Service, or set `RUSSET_SECRETS_PASSPHRASE` |
| `E_ASC_UNAUTHORIZED` | 401: wrong key or issuer, revoked key, clock skew, or lifetime over 20 minutes | Re-check the IDs; sync the clock (NTP); re-add the key |
| `E_ASC_FORBIDDEN` | 403: the role lacks permission | Use Admin, or App Manager with Certificates, Identifiers & Profiles access |
| `E_ASC_SCOPE` | 403 `FORBIDDEN.REQUEST_DOES_NOT_MATCH_SCOPE` [V3] | Internal bug: report it |
| `E_ASC_INDIVIDUAL_KEY_NO_PROVISIONING` | Individual key on a provisioning call | Use a team key |
| `E_ASC_RATE_LIMITED` | 429 | Automatic backoff; 3,600 per hour per key [V2] |
| `E_ASC_AMBIGUOUS` | More than one exact match | Clean up duplicates in the portal |
| `E_PROV_CERT_LIMIT` | Certificate limit reached | Import a `.p12`, revoke one (typed), or use Apple-hardware mode |
| `E_PROV_DEVICE_PENDING` | Device registered but not yet eligible (24–72 h) | Wait; use another registered device |
| `E_PROV_DEVICE_LIMIT` | 100 devices per family per year | Remove devices at renewal |
| `E_PROV_PORTAL_STEP` | App Group or iCloud container needs the portal | Open the portal link, then re-check |
| `E_PROV_FREE_QUOTA` | 10 App IDs per 7 days exhausted | Wait until `<date>`, or reuse an existing ID |
| `E_PROV_FREE_UNSUPPORTED` | Capability not available on free teams | Remove the capability or use a paid team |
| `E_ENT_NOT_IN_PROFILE` / `E_ENT_VALUE_NOT_ALLOWED` / `E_ENT_UNRESOLVED_VAR` | §8.4 | Enable the capability and recreate the profile; fix `.entitlements` |
| `E_SIGN_NO_IDENTITY` | No usable identity for the team and kind | Run provisioning; import a `.p12` |
| `E_SIGN_PROFILE_MISMATCH` | Profile does not cover the bundle, certificate or device | Recreate the profile |
| `E_LINT_*` | §8.6 ID | Per lint |
| `E_GSA_BAD_CREDENTIALS`, `E_GSA_BAD_CODE`, `E_GSA_LOCKED`, `E_GSA_IDENTITY_REJECTED` | §7.3 | Re-enter; unlock at iforgot.apple.com; update Russet or `gsa-profile.json` |
| `E_ANISETTE_LIBS_MISSING` / `E_ANISETTE_PIN_MISMATCH` / `E_ANISETTE_ADI <code>` | §7.5 | Fetch libraries; update Russet; reset provisioning |
| `E_SHIP_NO_APP_RECORD` | No app record | §13.5 helper |
| `E_SHIP_DUPLICATE_BUILD` | Build number already used | Auto-increment |
| `E_SHIP_UPLOAD_FAILED <ITMS>` | Apple processing error | Lint hint per ITMS code |
| `E_SHIP_COMMIT_AMBIGUOUS` | Commit outcome unknown | `russet ship upload --resume`/`status` |
| `E_SHIP_STAMP_NO_CONSENT` / `E_SHIP_STAMP_NO_SDK_METADATA` | §9.5 | Grant consent; import the SDK from your Xcode |
| `ship.ciCloudSigningForbidden` (7009) | "Cloud signing permission error": the team key is not Admin | Use signing mode `russet` or `xcode-manual`, or an Admin-role key |
| `ship.ciRunnerUnavailable` (7010) | The pinned runner label or Xcode version is missing from the image | Update the pins, or set `RUSSET_RUNNER`/`RUSSET_XCODE` |

Numeric codes and dotted names are binding (ARCHITECTURE §13.3); the `E_*` names above map onto them.

---

## 17. Security, privacy and legal notes

- **Secret hygiene.**
  - Secrets live only in `russet-secrets`.
  - A global tracing layer redacts JWTs (`eyJ…`), `X-Apple-GS-Token`, `X-Apple-I-MD*`, PEM blocks
    and `profileContent`.
  - Tests assert the redaction, and the keyring leak test runs in CI.
  - Panics with `SecretBytes` in scope never print values.
- **Agents.** They get read-only scoped tokens and approval-gated plans (§10, §11). An agent can
  never obtain the PEM, the GS token or `adi.pb`.
- **Downloaded Apple code.**
  - It runs in a sandboxed helper.
  - It is hash-pinned.
  - It is never redistributed.
  - It is fetched from Apple's own CDN over TLS, with `ETag` recorded.
- **Apple public certificates.**
  - apple-codesign compiles Apple's public CA certificates (WWDR G2–G8, Apple Root CA and others)
    into the binary, through `apple_certificates.rs`.
  - These are public PKI certificates that Apple distributes for anyone to install, but they are
    arguably "Apple files". §21 Q2 asks the owner to accept this or to patch the crate to load them
    at runtime.
  - Russet's own code fetches Apple Root CA at runtime and pins its hash [V15].
- **Licences of Apple's terms** (not legal advice):
  - The ADPLA forbids running the Apple SDKs on non-Apple computers, and "Provisioning Profiles on
    or in connection with devices other than Apple-branded products". It also forbids using services
    "in any unauthorized way … including … by trespass or burdening network capacity" [S ADPLA
    restrictions section; landscape R1/R3].
  - Russet's mitigations:
    - the paid public API is the default;
    - free mode is opt-in with warnings;
    - the Apple-hardware release mode exists;
    - stamping is opt-in and visible.
- **Upstream licences** (§3.1 table):
  - Code is reused only from MIT, Apache, MPL or LGPL sources, with attribution in `NOTICE.md`.
  - Unlicensed repositories (android-loader, anisette-v3-server) are not used [V13].
  - Interface facts (endpoint names, symbol names, header names) are documented from them, not code.

---

## 18. Test plan

| Layer | Tests | Where |
|---|---|---|
| Unit | JWT claims and ES256 signature verified by a second implementation (`jsonwebtoken` in dev-deps); scope builder; open-enum round-trip including `DRIVERKIT_*` [V5]; prefix-filter equality; pagination with `limit` + `cursor` | `russet-asc` |
| Unit | Entitlements merge: table-driven, at least 40 cases (variables, wildcards, `aps-environment`, keychain default [V11], app groups, free filtering, `get-task-allow` by kind) | `russet-sign` |
| Unit | Profile parse: synthetic CMS profiles from a self-signed test CA (the omarchy "test identity" approach); `DER-Encoded-Profile` present or absent | `russet-sign` |
| Unit | Mach-O: synthetic thin and fat images; string-pool misalignment detect and fix; `LC_BUILD_VERSION` sdk patch; signature parse equals the `csinfo` output on the M1 Hello [V11] | `russet-macho` |
| Unit | IPA writer: modes, symlinks, determinism (two runs give the same SHA-256), zip64 path | `russet-ship` |
| Unit | GSA SRP: a test server implementing the same SRP-6a variant; `s2k` and `s2k_fo`; HAMK/`np` verification; AES-CBC/GCM decode | `russet-gsa` |
| Unit | Secrets: envelope round-trip at 8 KiB; tamper detection; index; Argon2 file store; redaction | `russet-secrets` |
| Integration | `wiremock` ASC server replaying recorded (redacted) fixtures: provisioning plan/apply, upload with 3 parts + resume + ambiguous commit, TestFlight distribute | crates' `tests/` |
| Live, read-only (opt-in env `RUSSET_LIVE_ASC=1`) | Parse every list endpoint on the owner's team with GET-scoped tokens; `account.verify`; plan (dry-run) for `com.evilcomputer12.russet.hello` shows the expected mutations and nothing else | developer machine, never public CI |
| Live, mutating (owner approval per run) | Create a dev profile for the exact bundle ID, reusing the imported xtool certificate (no new certificate); install and launch on the iPhone 12 (device agent); later, a TestFlight upload of the sample app | manual checklist in `docs/runbooks/` |
| Golden signature | Sign M1 `Hello.app` with `russet-sign` using the same identity and profile: CD fields, exec-seg flags `0x11`, entitlements set, DER blob equal in structure to xtool's [V11]; when the Mac is reachable, `codesign --verify --deep --strict` | SA-05 |
| Extension rule | Sample with a WidgetKit appex and a framework: installs and runs on iOS 27 (MAIN_BINARY in the appex) | device agent |
| iOS 27 alignment | Sample dylib post-processed by an old `llvm-strip` (misaligned) → lint fails → fix → loads on device | device agent |
| Free account | Throwaway Apple ID: login + 2FA, anisette provisioning (Linux), certificate, profile, install; record whether `0xe8008015` reproduces [S iloader#570] | owner approval |
| Windows | Secrets (8 KiB secret, Credential Manager), anisette helper on Windows 11 x64, CLI paid flow | physical Windows PC (M6) |
| Apple-hardware | Wave 2 smoke (PLAN W16): unsigned archive of RussetProbe on `xcode-27` plus a throwaway pre-sign, entitlements checked. W23: `russet` mode → Russet signs and uploads → VALID; one owner-approved `xcode-manual` export whose per-bundle entitlements equal the Linux-native IPA's (`ship compare`); `xcode-cloud` only with an Admin key, otherwise its error path (7009) | GitHub (included minutes; free once public) |
| TestFlight canary | Linux-native IPA of the sample: VALID, **installed through TestFlight on the iPhone** (closes the landscape evidence gap) | M5 exit |

---

## 19. Ordered tasks

Sizes: S ≤ 2 days, M ≤ 1 week, L ≤ 2 weeks, XL > 2 weeks (one developer with agents). Each task
lists its acceptance criteria.

| ID | Task | Depends on | Size | Acceptance |
|---|---|---|---|---|
| SA-01 | `russet-secrets`: keyring 4.2 backend, envelope store, passphrase store, env backend, redaction layer, `doctor` probe | none | M | Round-trip of 100 B and 8 KiB secrets on this host (ksecretd); passphrase store works with `DBUS_SESSION_BUS_ADDRESS` unset; leak test passes; no `Debug` output contains secret bytes |
| SA-02 | `russet-asc` core: ES256 JWT (p256), token cache, scoped tokens, `Auth` trait, JSON:API types, `Open<E>`, pagination, rate limiter, error mapping, `xtask asc-gen` with curated YAML, `xtask asc-drift` | SA-01 | M | Live read-only suite passes on the owner's team (certificates, devices, bundleIds+capabilities, profiles, apps, builds, betaGroups, ciProducts); scope test yields `E_ASC_SCOPE`; 60-minute token yields `E_ASC_UNAUTHORIZED` |
| SA-03 | `russet-macho`: parse thin/fat, `LC_BUILD_VERSION`, `LC_SYMTAB` alignment check and fix, signature parse | none | S | Unit fixtures; parses M1 Hello identically to [V11] |
| SA-04 | `russet-sign` profiles + entitlements merge + bundle walker (no signing yet) | SA-03 | M | At least 40 merge cases; parses the real Hello profile [V11]; `russet sign inspect` works |
| SA-05 | `russet-sign` signing via apple-codesign (`default-features=false`), pre-sign fixes, post-sign verify, lints L-SIG/L-MACHO/L-BND | SA-04 | L | `cargo build` of apple-codesign without the notarize feature succeeds [A]; golden comparison with the xtool signature; the device agent installs and launches the Russet-signed Hello (paid, xtool identity) on iOS 27; Mac `codesign --verify` when reachable |
| SA-06 | `russet-account`: accounts, identity store (CSR, p12, **xtool import**), `AscPortal`, provisioning plan/apply, policy, profile cache, audit log | SA-02, SA-04 | L | Dry-run plan for the exact ID `com.evilcomputer12.russet.hello` on the owner's team lists only {RegisterBundleId, CreateProfile} when reusing the xtool certificate; with owner approval, apply produces a profile and the app installs with its **exact** bundle ID |
| SA-07 | CLI: `russet account/provision/sign` | SA-05, SA-06 | S | Every command in §12 for these groups works, with `--json` and `--yes` |
| SA-08 | Daemon JSON-RPC methods, jobs, approvals (HMAC tokens) | SA-06, daemon doc | M | Integration tests through a JSON-RPC client; MCP-origin connections cannot call `approval.grant` |
| SA-09 | IDE: account wizard (paid), Signing & Capabilities editor, F5 provisioning hooks, fix-its | SA-08, IDE doc | L | Manual e2e: add key → F5 → app on the phone, no terminal |
| SA-10 | `russet-ship`: archive layout, dsymutil, IPA writer | SA-05 | M | Deterministic IPA; `pymobiledevice3 apps install` of a Russet IPA succeeds (device agent); Xcode Organizer opens the archive (Mac, when reachable) |
| SA-11 | Offline validation (about 45 checks with ITMS mapping) | SA-10 | M | Each check has a failing fixture; the omarchy demo-equivalent passes |
| SA-12 | Stamping (tiers, consent, plan/apply, alignment interplay) + bisect of the minimal key set | SA-11, SDK area | M | Refuses without consent or SDK metadata; values equal [V17]; bisect report committed (needs SA-13 canaries) |
| SA-13 | Upload: build uploads, parallel parts, journal and resume, commit reconciliation, polling, wedge hint, build processing | SA-02, SA-10 | M | wiremock suite; with owner approval and an app record, a sample upload reaches `COMPLETE` → `VALID` |
| SA-14 | TestFlight: groups, testers, distribute, What to Test, encryption, beta review | SA-13 | M | wiremock asserts the exact request sequence of every TestFlight method (ARCHITECTURE §8.4); with approval, a build is added to an internal group managed through Russet and installed through TestFlight on the iPhone (M5 exit) |
| SA-15 | Metadata and screenshots via the App Asset Library; App Store versions, export compliance and review submission (human-only) | SA-13 | L | Pull/push round-trip on a test version (approval); screenshot placements visible in App Store Connect; `ship.version.apply` and `ship.review.submit` send exactly the expected request sequence against wiremock; a live submission only with the owner's approval (ARCHITECTURE §21 Q32) |
| SA-16 | Apple-hardware mode: the §9.9 template with its three signing modes and `.russet/ci/` helpers, repo setup via API, dispatch, status (runner label and Xcode checks), artifact fetch and deletion, `ship compare` | SA-13 | L | RussetProbe builds on `xcode-27` in `russet` and `xcode-manual` modes; exported per-bundle entitlements are verified; `xcode-cloud` is tested live only with an Admin key, otherwise through its error path (7009); the generated workflow is zizmor-clean; the build is uploaded by Russet and VALID; compare report committed |
| SA-17 | Xcode Cloud integration (list, trigger, poll, artifacts) | SA-02 | M | Mocked tests; live only if a team with a product is available (the owner has 0 [V4]) |
| SA-18 | `russet-gsa`: SRP, 2FA (trusted device and SMS), app token, Apple Root CA runtime pin, client profile file | SA-01 | L | Unit SRP server tests; with a throwaway Apple ID (approval), login + 2FA succeeds |
| SA-19 | `russet-anisette` + `russet-adi-host` (Linux x86_64): range download, pins, ELF loader, sandbox, provisioning, OTP; RemoteV3 opt-in | SA-01, SA-18 | XL | Libraries fetched with less than 3 MB of traffic; helper runs without network (seccomp); GSA accepts headers in a live login |
| SA-20 | DS2: `Ds2Auth` for `russet-asc`, QH65B2 `listTeams`/app groups, `Ds2Portal` | SA-18, SA-19, SA-06 | M | Free team: list certificates, devices, bundle IDs; app-group assign works |
| SA-21 | Free-team policy: quota ledger, bundle-ID mapping, revocation dialog, 7-day refresh scheduler | SA-20 | M | Refresh job re-signs and reinstalls before expiry (device agent); quota shown correctly after 3 registrations |
| SA-22 | Windows: secrets (Credential Manager + envelope), CNG identity option, adi-host x64 shims (`%fs` handling), path handling | SA-01, SA-19 | L | Windows 11 physical PC: paid flow end to end; anisette provisioning succeeds |
| SA-23 | MCP tools of §11 with approval gating and scoped tokens | SA-08 | S | Claude Code can read status, plan and request approval; cannot mutate without the IDE dialog; output contains no secrets |
| SA-24 | Crash feedback: TestFlight crash submissions + local symbolication from archive dSYMs | SA-10, SA-14 | M | A crash from a TestFlight build symbolicates to file:line |
| SA-25 | Docs and runbooks: account setup, free-account risks, stamping policy, Apple-hardware setup, error catalog pages | ongoing | S | Every `E_*` code has a docs URL that resolves |

**Critical path for replacing xtool in M1/M3** (development signing with exact bundle IDs):
SA-01 → SA-02 → SA-03 → SA-04 → SA-05 → SA-06 → SA-07.

---

## 20. Risks

| ID | Risk | Mitigation |
|---|---|---|
| R-1 | GSA client identity churn breaks free login again; it changed three times in one week in September 2026 | Data-driven `gsa-profile.json`, refreshed from Russet releases; paid stays the default |
| R-2 | Free installs fail with `0xe8008015` on iOS 26.5 and 27 across tools (iloader#570 open) | Paid first; reproduce on the iPhone 12 in SA-21; inspect the profile's devices and entitlements in installd logs |
| R-3 | Private-API use (GSA, DS2, ADI) may breach Apple's terms; accounts may be locked | Opt-in, warnings, recommend a secondary Apple ID; never the default |
| R-4 | ELF loader effort and Windows `%fs` TLS issue | Linux first; xadi (LGPL) fallback on Linux; measure in SA-22; RemoteV3 to a self-hosted server as the last resort |
| R-5 | Apple changes or withdraws the Apple Music APK URL or its libraries | Pins keyed by version; TOFU prompt; error tells the user to update |
| R-6 | Stamping judged as misrepresentation; App Store adds fingerprint checks | Tiers; honest marker; Apple-hardware mode recommended for submissions; minimal key set from bisecting |
| R-7 | App Store processing of Linux artefacts: asset-catalog rejections and wedges | Validation lints; TestFlight canaries; Apple-hardware fallback |
| R-8 | ASC API drift: open enums, new required fields, deprecations such as screenshots | `asc-drift` in CI weekly; open enums; read docs before each release |
| R-9 | Rate limit of 3,600 requests per hour per key, shared with other tools (xtool, CI) | Client-side bucket; cache lists for 60 s within a plan |
| R-10 | Certificate limits; the distribution key lives on another machine | p12 import; Apple-hardware cloud signing; never auto-revoke |
| R-11 | Secrets exposure through logs, agents, or world-readable legacy files (xtool [V1]) | Redaction layer; scoped tokens; xtool import and cleanup guidance |
| R-12 | apple-codesign unmaintained (no release since 2024-11) | Pin 0.29.0; minimal wrapper; fork if needed (MPL allows); `apple-codesign-quick` (LGPL) as fallback |
| R-13 | Shipping Apple public CA certificates inside apple-codesign conflicts with the "no Apple files" policy | §21 Q2; patch to load at runtime if the owner says so |
| R-14 | Device registration delay (24–72 h for devices 11–100) and the PPQ online check confuse users | Plan warnings; `doctor` connectivity check |
| R-15 | Mac oracle unreachable, so P0-3 diffs and `xcodebuild` flag checks are unverified [V20] | Tasks SA-05, SA-12 and SA-16 carry the checks; the GitHub runner can serve as the oracle for `xcodebuild` behaviour |
| R-16 | `xcode-27` is a preview image (no SLA, possible queueing, GA label unannounced); macOS minutes cost in private repositories; one report says each portal request takes about 3 s on macOS 27 guests (about 90 s per cloud-signed export; FB25000569; low confidence) | `xcode-27` default from pins and a repository variable; `ship.ci.status` detects a missing label or Xcode (7010); `russet` mode makes no portal calls on the runner; public repositories run it free; Xcode Cloud for teams that have it |

---

## 21. Open questions for the owner

1. **Release and stamping defaults.**
   - Should the App Store submission default be Apple-hardware, and Linux-native the default for
     TestFlight iterations?
   - Stamping off by default, with the `sdk` tier as the first opt-in?
   - Should the visible `RussetToolchain` Info.plist marker be on when stamping?
2. **Apple public CA certificates.** apple-codesign embeds them. Accept that, or patch the crate to
   fetch them at runtime with pinned hashes?
3. **Licence of `russet-asc`** (answered 2026-10-31: AGPL-3.0-only, like all of Russet). GPL-3.0-or-later, or MIT/Apache-2.0 so the wider Rust ecosystem
   can reuse it?
4. **Owner approval for live tests that mutate the account.**
   - (a) Import xtool's existing development identity and reuse its certificate (it expires
     2027-10-10), instead of creating a fifth development certificate?
   - (b) Register the bundle ID `com.evilcomputer12.russet.hello` and create one development
     profile?
   - (c) Later, create an app record and do a TestFlight upload?
5. **Free-account testing.** Is a throwaway Apple ID available? Free mode stays opt-in either way.
6. **Distribution certificate.** The team has 2 Apple Distribution certificates, with keys
   presumably on the Mac.
   - Should we export one as `.p12` when the Mac is back?
   - Or create a new one?
   - Or rely on Apple-hardware cloud signing for releases? That needs an Admin-role key (§6.6;
     ARCHITECTURE §21 Q5, which carries this question now).
7. **App records.** Is the manual web step with the §13.5 helper acceptable, or should we
   investigate the private App Store Connect web API (asc CLI "web" commands) as an opt-in?
8. **GitHub macOS runner budget.** Answered by decisions.md (free private-repo minutes only):
   owner-dispatched Apple-hardware runs within the included minutes until the repository is public
   (ARCHITECTURE §21 Q6).
9. **GSA profile updates.** May Russet fetch a signed `gsa-profile.json` from its own GitHub
   releases at runtime to react quickly to Apple changes, or should updates only come with new
   Russet versions?
10. **xtool credential files.** They were locked down to 0600 on 2026-10-11 (decisions.md).
    After SA-06 lands (Russet-native provisioning), should we remove xtool's stored credentials with
    `xtool auth logout`?

---

## 22. Sources

**Apple (fetched 2026-10-10/11):**

- [Generating tokens for API requests](https://developer.apple.com/documentation/appstoreconnectapi/generating-tokens-for-api-requests)
- [Creating API keys](https://developer.apple.com/documentation/appstoreconnectapi/creating-api-keys-for-app-store-connect-api)
- Build uploads:
  - [Build uploads](https://developer.apple.com/documentation/appstoreconnectapi/build-uploads)
  - [Create a build upload](https://developer.apple.com/documentation/appstoreconnectapi/post-v1-builduploads)
  - [Reserve a build upload file](https://developer.apple.com/documentation/appstoreconnectapi/post-v1-builduploadfiles)
  - [Commit a build upload file](https://developer.apple.com/documentation/appstoreconnectapi/patch-v1-builduploadfiles-_id_)
- API release notes:
  - [4.1 release notes](https://developer.apple.com/documentation/appstoreconnectapi/app-store-connect-api-4-1-release-notes)
  - [4.5 release notes](https://developer.apple.com/documentation/appstoreconnectapi/app-store-connect-api-4-5-release-notes)
- [OpenAPI spec zip (4.5.1)](https://developer.apple.com/sample-code/app-store-connect/app-store-connect-openapi-specification.zip)
- Assets and screenshots:
  - [Uploading assets](https://developer.apple.com/documentation/appstoreconnectapi/uploading-assets-to-app-store-connect)
  - [App Asset Library](https://developer.apple.com/documentation/appstoreconnectapi/app-asset-library)
  - [App Screenshots (deprecated)](https://developer.apple.com/documentation/appstoreconnectapi/app-screenshots)
- [Upload builds (help)](https://developer.apple.com/help/app-store-connect/manage-builds/upload-builds)
- Account help:
  - [Certificates overview](https://developer.apple.com/help/account/certificates/certificates-overview/)
  - [Devices overview](https://developer.apple.com/help/account/devices/devices-overview)
  - [Device registration updates](https://developer.apple.com/help/account/reference/device-registration-updates)
  - [Provisioning profile updates](https://developer.apple.com/help/account/provisioning-profiles/provisioning-profile-updates)
  - [Supported capabilities (iOS)](https://developer.apple.com/help/account/reference/supported-capabilities-ios)
- [Upcoming requirements](https://developer.apple.com/news/upcoming-requirements/)
- [ADPLA](https://developer.apple.com/support/terms/apple-developer-program-license-agreement/); local copy at `~/RE/ios-nomac/research/signing/apple-docs/adpla.txt`
- [Setting up your project to use Xcode Cloud](https://developer.apple.com/documentation/xcode/setting-up-your-project-to-use-xcode-cloud)
- [Apple Root CA](https://www.apple.com/appleca/AppleIncRootCertificate.cer)
- Forum threads:
  - [xcodebuild + API keys, 682091](https://developer.apple.com/forums/thread/682091)
  - [CI automatic signing, 764554](https://developer.apple.com/forums/thread/764554)

**Open source (licences in §3.1 and [V13]):**

- [apple-platform-rs / apple-codesign 0.29.0](https://github.com/indygreg/apple-platform-rs): `apple-codesign/src/{macho_signing,entitlements,signing_settings,bundle_signing,policy,windows}.rs`; [app-store-connect 0.7.0](https://crates.io/crates/app-store-connect)
- [xtool](https://github.com/xtool-org/xtool): `Sources/XKit/GrandSlam/**`, `DeveloperServices/**`, `Anisette/{ADIDataProvider,XADIProvider,OmnisetteADIProvider}.swift`, `DeveloperServices+OpenAPI.swift`, `Sources/DeveloperAPI/patch.js`; commits `893cf4f8f` (#244) and `450223eb0` (#257)
- [xadi](https://github.com/xtool-org/xadi) (LGPL-2.1); [Provision](https://github.com/Dadoum/Provision) (LGPL-2.1); [android-loader](https://github.com/Dadoum/android-loader) (no licence; not used); [anisette-v3-server](https://github.com/Dadoum/anisette-v3-server) (protocol only)
- [SideStore apple-private-apis / omnisette](https://github.com/SideStore/apple-private-apis) (MPL-2.0); [Impactor](https://github.com/claration/impactor), including `3rdparty/omnisette`, `crates/plume_core`, `3rdparty/apple-codesign-0.29.0`
- [isideload](https://github.com/nab138/isideload) (MIT): `auth/apple_account.rs`, `auth/grandslam.rs`, `util/keyring_storage.rs`; [iloader#570](https://github.com/nab138/iloader/issues/570)
- [omarchy-apple-dev](https://github.com/joshuaswarren/omarchy-apple-dev) (MIT): `tools/asc.py`, `tools/sign-dev.sh`, `FINDINGS.md` items 23/31/33/35/38/64, receipts 2026-10-04, 2026-10-08, 2026-10-09; local copies in `~/RE/ios-nomac/research/signing-factcheck/omarchy/`
- [App-Store-Connect-CLI](https://github.com/rorkai/App-Store-Connect-CLI) (MIT): `build_uploads.go`, `API_NOTES.md`
- [zsign](https://github.com/zhlynn/zsign) (MIT): `archo.cpp` exec-seg flags
- [AltStore](https://github.com/altstoreio/AltStore): `Team.swift` (`maximumFreeAppIDs = 10`), `InstalledApp.swift` (3 active apps)
- [keyring-rs 4.2.0](https://github.com/open-source-cooperative/keyring-rs); [Microsoft CREDENTIAL structure](https://learn.microsoft.com/en-us/windows/win32/api/wincred/ns-wincred-credentiala)
- [actions/runner-images](https://github.com/actions/runner-images): `xcode-27` and `macos-26` labels; `images/macos/xcode-27-arm64-Readme.md`, `macos-26-arm64-Readme.md`; issues #14404 (Xcode 27 preview), #14748, #14856
- GitHub docs: [runners](https://docs.github.com/en/actions/reference/runners/github-hosted-runners), [runner pricing](https://docs.github.com/en/billing/reference/actions-runner-pricing), [Actions billing](https://docs.github.com/en/billing/concepts/product-billing/github-actions); changelog [2026-07-16 Xcode 27 runner image in public preview](https://github.blog/changelog/2026-07-16-xcode-27-runner-image-now-in-public-preview/)
- Apple: [roles](https://developer.apple.com/help/account/access/roles/), [cloud-managed certificates](https://developer.apple.com/help/account/certificates/cloud-managed-certificates/), [Xcode Cloud](https://developer.apple.com/xcode-cloud/)
- Cloud-signing failures with App Manager keys: github.com/whyisjacob/metronome/issues/1, github.com/charannanduri/ChicagoLMap/pull/25, github.com/tornidomaroc-web/scan-and-action/pull/288
- Entitlements lost on unsigned-archive export: github.com/parob/homecast/issues/33, github.com/MyAvenCEO/avenOS/issues/97; tauri `crates/tauri-cli/src/mobile/ios/build.rs` and [tauri#15663](https://github.com/tauri-apps/tauri/issues/15663)
- xtool 1.21.0 release asset `xtool.app.zip` (sha256 33bb2ef1…b64e8); Homebrew cask `xtool-org/homebrew-tap` (`version :latest`, `sha256 :no_check`)
- [apple-codesign-quick 0.1.0](https://crates.io/crates/apple-codesign-quick) (LGPL-2.1-or-later): fallback signer

---

## 23. Wave-1 implementation notes (W6)

Where the Wave-1 code (W6: SA-02…SA-07 subsets) differs from the sections above. ARCHITECTURE.md stays binding;
the contract points among these are listed as contract change requests for the lead.

- **russet-macho** (§3.1, §8.5) is MIT (ARCHITECTURE §5.1) and has its own bounds-checked parser instead of
  goblin/object: the edits need raw load-command offsets. The string-pool fix moves the table to the next 8-byte
  boundary and shifts everything after it by 16 bytes, so the trailing code signature keeps its alignment. `Arch`
  keeps ARCHITECTURE §5.3's three variants (arm64, arm64e, x86_64); a slice of any other CPU (an old armv7 slice
  in a vendored framework) is described by `Cpu` and listed by `MachO::cpu_slices`, while `slices()` lists the
  `Arch` slices only.
- **Profiles** (§8.3) are parsed with `cryptographic-message-syntax` (apple-codesign's CMS crate), not `cms`.
- **App Store Connect** (§5.1–§5.3): the typed models are written by hand (`xtask asc-gen`/`asc-drift` are not
  in Wave 1). Every Wave-1 read carries a token scoped to exactly that request (`TokenMode::PerRequest`), not only
  agent reads, so App Store Connect refuses anything else; scoped tokens are GET-only. The rate limiter is a token
  bucket per key (3,600 requests an hour, bursts of 100) that also caps itself at the `X-Rate-Limit` quota left;
  russet-account keeps one per key, so the quota spans calls. `raw_get` (the future `asc.get`) checks its
  allowlist on the URL as it will be sent, after the parser normalized it, and refuses encoded dots, slashes and
  backslashes outright. With `RUSSET_OFFLINE=1` the client reaches loopback hosts only, and `asc.reachable` reads
  the root from `RUSSET_ENDPOINTS_FILE`.
- **Keys** (§6.1): an individual key is checked with `GET /v1/apps` (it cannot use the provisioning endpoints) and
  stored without a team; a team key whose role cannot read certificates (403) is kept without a team instead of
  being refused; a 401 refuses either. `account.verify` is read-only: it reports the team it discovers but writes
  nothing (re-adding the key records one), and its `ok` means "reads certificates" for a team key and "reads
  builds" for an individual key.
- **xtool import** (§4.4): `XTLAuthToken` is the JSON of xtool's Swift `AuthToken` enum
  (`{"appStoreConnect": {"_0": {"id", "issuerID", "pem"}}}`); the identity is the property list named after the
  identity ID (the issuer ID for App Store Connect logins) with `privateKey.data` (PKCS#1 PEM) and `certificate`
  (DER). The team comes from the certificate's OU, so the import needs no network. The `XTL-<prefix>` bundle-ID
  prefix is the issuer ID's first segment, uppercased. An Apple ID session in xtool is not imported (W22). The
  `chmod 600` advice, the import's errors and the `account.xtool-files` doctor check name xtool's directory, never
  its files (the identity file is named after the issuer ID).
- **Identities** (§6.3): `IdentityStore::load` and `import_p12` keep ARCHITECTURE §5.3's synchronous signatures
  (they read the async secret store on a short-lived thread with its own runtime, so they work from a runtime
  worker or a blocking thread); `load_async` and `import_p12_async` are the async forms. The key is stored as PKCS#8
  DER whatever its source format. `.p12` files are read with `p12-keystore` (PBES2/AES as OpenSSL 3 writes them, and
  the legacy 3DES/RC2 forms); apple-codesign's reader is not used (legacy forms only, and it asserts on a SHA-256 MAC
  in debug builds). On the wire an identity is named by its certificate serial (`IdentityId::wire`); the daemon
  also accepts the SHA-1 fingerprint, and keeps `<team>/<serial>` internal. The private key is sealed as
  `signing-key/<certificate SHA-1>`, not `signing-key/<team>/<cert-serial>` (§5): `secrets.list` shows secret
  names, and they must not name the team.
- **Planning** (§6.4): step 5 also accepts profiles that are not Russet-named (Russet-named ones are tried first),
  checked for kind, validity, certificate, devices, App ID and entitlements — this is how the Wave-1 slice reuses
  xtool's `XTL-…` profile with 0 mutations. `DeleteProfile` is only planned for Russet-named profiles. The plan
  digest is a SHA-256 over account, team, distribution, identity, mutations and reused profiles. Plans can be
  applied for 10 minutes; `apply` re-plans and answers `provision.planStale` (5206) on drift or once the plan has
  expired (`details.reason`: `drifted` or `expired`); an id the daemon never planned (or planned over an hour ago)
  is `core.notFound`. Without an approval a plan with mutations answers `perm.approvalRequired`; with one,
  `core.notImplemented` until W12.
- **Signing** (§8.1, §8.7): `sign` takes `.app` directories (an `.ipa` is read by `verify`, `lint` and `inspect`).
  It signs **in place by default** (§8.1 prefers a copy): `russet sign App.app` then `russet sign verify App.app`
  is the flow, and `--out` (`SignOutput::CopyTo`) signs a copy, as the run pipeline does into
  `.russet/build/<profile>-iphoneos/signed/`. `--out` names the signed `.app` itself; an existing path there is
  replaced only when it is an earlier signed copy of the same app (an `.app` with `_CodeSignature/CodeResources`
  and the same `CFBundleIdentifier`), a path inside the input or containing it is refused before anything is
  written, and a run that fails before signing removes the copy it made. `BundleSigningInput` keeps its two F0
  fields; `get-task-allow` and the `CFBundleIdentifier` rewrite are `SignOptions` fields. Post-sign verification
  always runs and fails with `sign.verifyFailed` (5009); lint errors fail with `sign.lintFailed`. A build product
  that is unsigned or only linker-signed skips the L-SIG lint rules with an info finding. The pipeline's step is
  `SignAppRequest::from_outcome(app, distribution, &outcome)` after a zero-mutation `provision.apply`: it takes
  the plan's identity, cached profiles and effective bundle IDs (§16.5), so a product whose `Info.plist` carries
  the canonical ID is rewritten to the provisioned one in the signed copy (the unsigned product keeps its ID).
- **Lints** (§8.6): L-MACHO-002's platform check (not iOS) is an error under `device-install` too, because dyld on
  the device refuses such an image; its SDK floor stays `app-store` only. L-MACHO-003 (minos above
  `MinimumOSVersion`) is a warning under `device-install` (the device installs it) and an error under `app-store`.
  L-BND-003's ITMS-90502 rule is an error when `UIRequiredDeviceCapabilities` lacks `arm64` and a warning when the
  extension has no such key (Xcode's extension templates omit it and upload). L-BND-008 (multi-size icon
  catalogs) is not checked (it needs the CAR reader, W21).
- **Wire** (§10): `sign.lint` answers `{findings}` and `sign.inspect` `{signatures}` (results are objects,
  ARCHITECTURE §7.3). No team ID leaves the daemon unmasked (AGENTS.md §8): team IDs are `*******XYZ` in every DTO
  and in `inspect`; identities are named by certificate serial; entitlement and profile errors render App ID
  prefixes masked (`*******XYZ.com.example.*`); paths through a team's identity directory are masked; and each W6
  handler masks every team ID the account service knows in its errors' message, hint, details and cause chain. No
  certificate subject is reported (it names a person); portal UDIDs, device names and certificate display names are
  masked or omitted unless an `ide` connection asks to reveal them. Bundle IDs are shown as they are: the
  `team-suffix` bundle-ID policy puts the team ID into the bundle ID by design, and an `XTL-…` bundle ID carries
  the first segment of the App Store Connect issuer ID.
- **CLI** (§12): `russet account list` also lists the signing identities (`identity.list`); `account
  certs|devices|bundle-ids|profiles list` are the read-only portal rows; `sign --lint none` skips the post-sign
  lints (`noLint`).
- **Errors** (§16): new codes `sign.profileInvalid` (5007), `sign.bundleInvalid` (5008), `sign.verifyFailed` (5009),
  `account.keyInvalid` (5109) and `account.noAccount` (5110), each with a hint.
- **Doctor**: `account.configured`, `account.identity`, `account.xtool-files` (files only) and `asc.reachable`
  (`--online`; the root from `RUSSET_ENDPOINTS_FILE`, skipped under `RUSSET_OFFLINE=1` for a non-loopback root).

## 24. Wave-2 implementation notes (W12)

Where the Wave-2 code (W12: SA-06…SA-08, SA-10 subsets) differs from the sections above. ARCHITECTURE.md stays
binding.

- **`provision.apply` mutations** (§6.4): after re-planning for staleness, the mutations run in dependency order
  — register devices, register bundle IDs, enable capabilities (additive only), delete stale Russet-named
  profiles, then create the profiles bound to the plan's certificate and (development/ad hoc) every enabled
  mobile device — and each chosen profile, reused or created, is downloaded and cached. **Certificate creation
  (the CSR flow, §6.6) is not wired yet**: a plan whose only path forward is `CreateCertificate` is refused with
  a hint to import or reuse a certificate. So apply covers the acceptance-#1/DC-08 case (an existing certificate,
  only `RegisterBundleId` + `CreateProfile`).
- **Approvals** (§6.4, ARCHITECTURE §11.3): a mutating `provision.apply` needs a human approval. For an `mcp`
  connection the permission engine already requires and validates a tier-2 method approval before the handler
  runs; for `cli`/`ide` the handler creates and validates a plan-bound approval (`ApprovalSubject::Plan`, checked
  against the plan id and digest and the caller), so a forged `approval` string cannot trigger the mutations.
- **`russet-asc` writes** (§5.2, §5.3): the Wave-1 client stays GET-only with per-request scopes; an approved
  plan uses a second **bearer-token** client (App Store Connect refuses non-GET under a scoped token). `post_json`
  and `delete` send JSON:API bodies and are not retried on a transport error or 5xx (mutations are not
  idempotent); a 429 is retried. `AscPortal` holds both clients.
- **`asc.get`** (§11): `AccountService::asc_get` runs the client's allowlisted `raw_get` with a request-scoped
  token and returns `{status, body}`; the allowlist and path normalization are the client's.
- **`russet-ship`** (§9.1, §9.3, §9.10): the crate uses `russet_core::Result` with the `ship.*` error codes
  (`ship.archiveFailed`, `ship.ipaInvalid`) rather than a separate `ShipError`. `Archive` carries a `digest`, a
  SHA-256 over the whole `.xcarchive` tree (sorted), which is the reproducibility fingerprint (the acceptance:
  identical across two runs); the archive is named `<name>.xcarchive`. `IpaInfo` omits `md5` (W23's upload adds
  it when it needs the `Content-MD5`); the IPA is written through the deterministic `russet-ipa` writer. The
  `ship.archive` handler builds the project through the shared `build.start` entry point, then lays out the
  archive under `<project>/.russet/archives` with the `SOURCE_DATE_EPOCH` timestamp.
- **Still open for later cards:** `identity.importP12`/`exportP12` (need the interactive password reverse-prompt),
  `portal.devices.register` (its standalone `ProvisionPlan` return overlaps apply), the `account.event`
  notification emitter, and certificate creation (the CSR flow).

## 25. Wave-3 implementation notes (W22)

Where the W22 code (SA-18, SA-19, SA-20/21 subsets; native Apple ID sign-in, paid personal teams first) differs
from §7. ARCHITECTURE.md stays binding; `docs/windows/W22-STATUS.md` has the RPC shapes and the live-check steps.

- **Errors** (§7.3): `russet-gsa` returns `russet_core::Result` with the `gsa.*`/`anisette.*` codes rather than a
  `GsaError` enum; Apple's `Status.ec` maps to `gsa.badCredentials` (-22406), `gsa.locked` (-20209, -20283,
  -22421, -20751), `gsa.identityRejected` (-36607, -45061, -45054, HTTP 429/503) and `gsa.protocol` otherwise,
  with e-mail-like words masked in Apple's message.
- **SRP** (§7.2): `k`, `u` and `M1` pad `g`, `A` and `B` to the group length (RFC 5054); `K = H(S)` hashes `S`
  unpadded, as the srp-crate clients (isideload, apple-private-apis) do; the client redraws `a` until `A` is
  full-length. `M2` is verified; `np` is read but not verified (no known client verifies it).
- **Sign-in flow** (§13.1): `account.appleId.begin` pushes a trusted-device code at once when Apple offers it, and
  lists the trusted numbers for SMS; pending sessions live 10 minutes in the daemon (`free::AppleIds`) and end
  after 5 wrong codes. After the code, Russet logs in again (as xtool does), takes the Xcode token, picks the paid
  team from `listTeams` (else the first), seals `{adsid, dsid, GsIdmsToken, sk, c, Xcode token}` as
  `secret:apple-id-session/<DSID>` (JSON, not CBOR; never the password) and writes the `apple-id` account. The DS2
  portal refreshes the Xcode token from the sealed session when it is within 5 minutes of expiry.
- **DS2** (§7.4): `Ds2Auth` lives in `russet-asc/src/ds2.rs` behind a `Ds2Credentials` trait (asc sits below
  gsa); the QH65B2 actions live in `russet-gsa/src/ds2.rs`. Apple ID accounts get an `AscPortal` over `Ds2Auth`
  (the `bundleIdCapabilities` DS2 relationship is not applied yet).
- **Anisette** (§7.5): `AnisetteProvider::client_info` returns the GSA profile's identity for local ADI (the
  profile is the single source); the Apple-Root-only HTTP client lives in `russet-anisette::apple_tls` because
  provisioning (`midStart/midFinish`) also talks to `gsa.apple.com`. Unknown APK versions are trusted on first
  use with a `tofu.json` beside the libraries (the shipped `anisette-pins.json` pins APK 4.9.6.1447/x86_64, recorded
  by W22 on 2026-10-31); `consent: true` covers that prompt and the result says `pinned: false`.
- **`russet-adi-host`** (§7.5): x86_64 only so far; DT_RELA/JMPREL/RELR relocations; imports resolve to Rust shims,
  the helper's own `dlopen`/`dlsym`, then the host glibc (same SysV ABI, `struct stat`, errno and `open` flags);
  unknown imports get trap stubs that name them and exit 70. A library's `DT_INIT`/`DT_INIT_ARRAY` run only when
  all its imports resolved: Apple's `libstoreservicescore.so` (APK 4.9.6.1447) imports 360 symbols of
  `libc++_shared`, CoreFoundation and `libmediaplatform`, and one of its 64 initializers builds a mediaplatform work
  queue; the ADI entry points work without them. The helper leaves with `_exit`, so destructors registered through
  `__cxa_atexit` never run. The seccomp filter is a deny-list (network, exec,
  ptrace, namespaces, mounts, BPF, modules, keyrings, io_uring), Landlock is applied where the kernel has it (WSL
  kernels do not).
- **Client identity** (§7.2, profile version 4): GrandSlam refuses (`-80045` at `init`, since 2026-10-12) a
  client info whose platform differs from the anisette's; Russet's local ADI is Apple's Android CoreADI, so the
  built-in profile presents `<PC> <Windows;6.2(0,0);9200> <com.apple.AuthKit/1 (com.apple.akd/1.0)>` with the akd
  user agent `akd/1.0 CFNetwork/808.1.4` (Android identities are refused at two-factor, `-80046`). Every GrandSlam
  request takes the provider's identity (`AnisetteProvider::identity`, a remote server's own), `cpd` carries
  `X-MMe-Client-Info`, and local ADI re-provisions when the identity it provisioned under changes.
- **Free-team policy** (§7.6): `free::policy` has the ledger (`<data>/accounts/<team>/ledger.json`), the bundle map
  and the refresh timing; the provisioner does not call them yet, and there is no remember-password option.
