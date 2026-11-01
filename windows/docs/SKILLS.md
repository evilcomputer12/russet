# Skill migration, 2026-10-31

Codex 0.166.0-alpha.5.1 supports `plugin marketplace add` and `plugin add`.
Installed Microsoft `win-dev-skills` marketplace and `winappcli@win-dev-skills`
0.7.1, using its actual pinned `microsoft/winappCli` plugin. Packaging/setup
instructions loaded; no prerequisite installers, certificate trust changes or
SDK updates run. Existing VS desktop/MSIX/SDK components were inspected first.
WinUI was evaluated but is not the primary frontend recommendation; no second
WinUI bootstrap or WinDbg workflow was installed without a need.

Migrated into `.agents/skills/`:

| Workflow | Disposition | Validation |
|---|---|---|
| `ipsw` | Adapted Windows/WSL scope; references and MIT license preserved | Manifest/reference load; CLI absent, no runtime claim |
| `research-apple-kernel-boot-and-firmware` | Adapted host routing and provenance; original reference and metadata retained | Static load; no executable dependency |
| `radare2` | Excluded for now | Refers to missing shared `field-journal`, tool-index and automatic bootstrap scripts; not self-contained |
| `reverse-engineering`, r0crawl catalog, iOS reversing | Deferred | Broad analysis catalogs with shared resources; migrate only a needed bounded workflow |
| Personal audio/electronics/pinokio skills | Excluded | Unrelated to this Windows IDE/backend task |
| Legacy commands and installed Claude plugins | Inspected locations | No Russet Rust/IDE/graphics/packaging skill identified; no permission configuration copied |

Raw Claude histories remain on the Linux volume. A bounded private context index
is outside repositories; product documentation cites redacted source paths.
Installed skills are discoverable on the next turn.

Location support: [official skills documentation](https://learn.chatgpt.com/docs/build-skills).
Catalog: [Microsoft Windows development skills](https://github.com/microsoft/win-dev-skills).
