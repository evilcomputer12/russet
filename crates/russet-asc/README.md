# russet-asc

Async typed App Store Connect client: ES256 JWT, scoped tokens, rate limiter, pluggable auth.

- Layer **L3** (domain crate; may depend on F and L0–L2 except russet-ipc, and on the L3 edges of ARCHITECTURE §5.2) (ARCHITECTURE §5.1–§5.2). Licence: GPL-3.0-or-later.
- **Owner:** W6 (SA-02, GET-only in Wave 1); `src/ds2.rs` is W22's. W1 created the crate as a compiling stub (freeze F0, PLAN §12).
- Key dependencies: reqwest 0.13 with `rustls-no-provider` (the ring provider is installed on first use), p256.

## What it does (Wave 1: GET-only)

- `AscKey::from_p8` parses the EC P-256 PKCS#8 key; `mint` signs ES256 JWTs with Apple's claims (`iss` or
  `sub: "user"`, `iat`, `exp`, `aud: appstoreconnect-v1`, optional `scope`). `Debug` never shows the key.
- `TokenFactory::bearer` caches 19-minute tokens until a minute remains; `scoped` mints GET-only scoped tokens (any
  non-GET entry is refused). `ScopeEntry::get` drops `limit`, `cursor` and `sort`, which Apple ignores when matching.
- `JwtAuth` implements the pluggable `Auth` trait. The Wave-1 default, `TokenMode::PerRequest`, scopes every request
  to exactly itself, so App Store Connect refuses anything else even on a bug [V3].
- `AscClient` sends GETs only: retries for 5xx and transport errors, exponential backoff with jitter (and
  `Retry-After`) on 429, a per-key `RateLimiter` (a token bucket of 3,600 requests an hour, capped by the
  `X-Rate-Limit` quota left, shared by every client of the key through `with_limiter`), pacing below 200 remaining
  requests, error mapping to the `account` family, JSON:API paging that keeps `limit` next to `cursor` and never
  follows a next link to another origin, and an allowlisted `raw_get` for W12's `asc.get` whose allowlist is checked
  on the normalized URL (encoded dots, slashes and backslashes are refused). `RUSSET_OFFLINE=1` limits it to
  loopback hosts.
- Typed accessors: certificates, devices, bundle IDs with capabilities (exact-match post-filter, ambiguity error),
  profiles (content, bundle ID, certificates, devices), apps, builds, beta groups, Xcode Cloud products. Every enum
  is open (`model::Open`) [V5]. The models are written by hand; SIGN §5.1's `xtask asc-gen`/`asc-drift` are not in
  Wave 1.
- Doctor: `asc.reachable` (with `--online`; the root from `RUSSET_ENDPOINTS_FILE`).
