<!--
SPDX-License-Identifier: GPL-3.0-or-later
SPDX-FileCopyrightText: 2026 Russet contributors
-->

# Third-party notices

The Russet extension bundles or depends on the following third-party software.

| Component | Licence | Use |
|---|---|---|
| [vscode-jsonrpc](https://github.com/microsoft/vscode-languageserver-node) | MIT | JSON-RPC transport to `russetd` (bundled). |
| [VS Code codicons](https://github.com/microsoft/vscode-codicons) | CC-BY-4.0 | Icons referenced by `$(id)` in the manifest (shipped by the host). |

Build- and test-only tools (not shipped in the VSIX): esbuild (MIT), Vite (MIT),
vitest (MIT), typescript-eslint (MIT), `@vscode/test-cli` and `@vscode/test-electron`
(MIT), `@vscode/vsce` (MIT).

Russet never redistributes Apple software. Any Xcode or SDK content is supplied by the
user and imported locally.
