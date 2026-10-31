# russet-toolchain

Pin manifest, probes, resolver with the env contract, installer, GC, SDK registry probe.

- Layer **L2** (may depend on L0–L1) (ARCHITECTURE §5.1–§5.2). Licence: AGPL-3.0-only.
- **Owner:** W3 (CORE-10, CORE-11 subset); W26 (pins in CI). W1 created the crate as a compiling stub (freeze F0, PLAN §12).
- `reqwest` uses `rustls-no-provider`; binaries install the ring provider (ARCHITECTURE §16.6).

## What it does

- `pins`: the embedded `pins/stable.toml`, verified with minisign (debug builds trust `pins/dev-minisign.pub`;
  release builds trust only W26's release keys, none yet) [D 87].
- `Toolchains::probe_all` lists every candidate of every component in the order of ARCHITECTURE §16.1, with
  version, provenance, env and problems; `resolve` returns the first working one, `resolve_build_env` checks the
  §16.1 compatibility rules (`sdk.incompatible`, `toolchain.versionMismatch`, `toolchain.linkerUnsupported`).
- SDKs: Russet's imports from `data/sdks/registry.json`, then xtool bundles in `~/.swiftpm/swift-sdks/`, read in
  place (`info.json`, `swift-sdk.json`, `SDKSettings.json`, `Xcode.app/Contents/version.plist`,
  `darwin-sdk-version.txt`, `toolset.json`).
- LLDB (RD-05): flavor `swift` (default) or `llvm`; `liblldb`'s `DT_NEEDED` libpython is found through
  `ldconfig`, an existing uv-managed PBS (provenance `uv`) or Russet's `python-shared`, never
  `~/.local/opt/python-3.12`, and becomes the env contract of §16.2 (`LD_LIBRARY_PATH`, `PYTHONHOME`).
- `install`: `python-shared` and `darwin-tools` only (no Swift or LLVM downloads in Wave 1): free-space guard,
  resumable download, size and SHA-256 check, safe extraction, atomic rename, `selected.json`; `uninstall`, `gc`.
- Probe output is cached in `cache/toolchain-probes.json` by program path, mtime, size, arguments and env. Probes
  run through the `Exec` seam: `ProcessExec` (`tokio::process`) by default, or `RunnerExec` over russet-core's
  `CommandRunner` (`Toolchains::with_runner`), which the daemon passes once W2's `ProcessRunner` (F1) is on main.
- `unmet(tool, requirement)` says why a candidate misses a project's pin or flavor (`toolchain.list` with a
  project), and a resolve error that a managed install fixes names it in `details.install` (`darwin-tools`,
  `python-shared`; doctor's automatic fix reads it).

## Tests

`cargo test -p russet-toolchain` is hermetic (fixture toolchains, a local HTTP server, keys made at test time).
Env-gated: `RUSSET_TEST_HOST=1 … --test dev_box` checks the dev box's acceptance facts, and
`RUSSET_TEST_NETWORK=1 … --test install` downloads the pinned PBS 3.12.15 (34 MB) and starts the host's LLDB on it.
