# russet-build

Build orchestrator: project detection, stack adapters, Info.plist synthesis, bundle assembly, lints, metadata policy.

- Layer **L3** (domain crate; may depend on F and L0–L2 except russet-ipc, and on the L3 edges of ARCHITECTURE §5.2) (ARCHITECTURE §5.1–§5.2). Licence: AGPL-3.0-only.
- **Owner:** W5 (Wave 1), W16 (Wave 2); stack and resource modules per PLAN §3.
- Pre-created module files: `src/stack/{flutter,xcode,rn,maui}/mod.rs` (W17, W28, W29, W29), `src/stack/flutter/aot.rs` (W24), `src/resources/mod.rs` (W21), `src/platform/windows/mod.rs` (W27b).

`Builder::build` runs resolve → prepare → compile → resources → assemble → lint → package and returns a
`BuildOutput` with the `SigningPlan`, writing `<project>/.russet/build/<profile>-iphoneos/{<App>.app, <App>.ipa,
build-report.json}` and the event log `<project>/.russet/logs/<jobId>.jsonl`.

| Module | What |
|---|---|
| `manifest` | `russet.toml` + `russet.local.toml` for `[project]`, `[app]`, `[build]` (strict, `file:line:col`) and `[signing] bundle-id-policy`/`team` |
| `detect` | project root (walking up) and stack detection |
| `bundle_id` | the four bundle-ID policies and the original → effective map |
| `stack::swiftpm` | `swift package describe`/`show-dependencies` planning, the generated builder package, the `swift build --build-system swiftbuild` invocation with an explicit environment, output parsing |
| `infoplist`, `metadata` | Info.plist synthesis with provenance, the scene manifest, the DT* policy of ARCHITECTURE §16.4, nested-bundle normalization |
| `bundle` | `.app` assembly, Swift back-deployment, rpath hygiene through russet-macho, fixed timestamps, the signing plan |
| `report`, `inspect`, `events` | `build-report.json` (schema 1), `build.inspect`, the `JobEvent` kinds (converted through the §9.2 wire form, so they carry W2's F1 fields), the warning codes and the JSONL log |

`Builder::needs_xtool_prefix` tells the daemon when a request needs the signing account's xtool prefix
(`[signing] bundle-id-policy = "xtool-prefix"` and no policy in the request); `Builder::clean` takes the project
lock, so it answers `build.inProgress` while a build runs.

Tests: `tests/build_pipeline.rs` runs the whole pipeline with a scripted `swift` (CI); `tests/build_probe_host.rs`
builds RussetProbe for real when `RUSSET_TEST_SDK_BUNDLE` names a Darwin SDK bundle (debug, release, and a local
package with a `.dynamic` product), with the production `RussetMachO` once W6's russet-macho bodies exist and LLVM's
rewriters until then.
