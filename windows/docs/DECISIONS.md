# Windows architecture experiment, 2026-10-31

Status: recommendation presented to owner; reversible implementation permitted by
the current brief. No production UI replacement or release claim.

| Criterion | Code-OSS with existing extension | New WinUI IDE |
|---|---|---|
| Editor and languages | Existing VS Code editor, extension host and language tooling | Must embed an editor, integrate languages, workspace and diagnostics |
| Debugging | Existing debug config/factory, VS Code breakpoint/variable/step UI, daemon DAP hub | Must build/adopt the debugger client and all its UI |
| Embedded phone | Existing mirror webview, frame and pointer/input surfaces to extend; virtual backend still missing | Native/WebView2 display feasible; same backend missing, new input integration |
| Windows integration | Native supervisor can own WSL/setup, process lifetime and local IPC | Direct native APIs; same backend/supervisor work required |
| Effort | Preserve working extension; build/bundle a Windows editor product | Add editor/debugger parity plus backend work |
| Packaging | Electron/editor payload size must be measured; no `ide/` product build in recovered checkout | Smaller shell possible, editor/runtime payload unknown; no size measurement yet |
| Accessibility | Editor has established keyboard/debug workflows; local smoke test pending | WinUI controls provide useful foundations; embedded editor and phone still require tests |
| Ownership | Maintain editor pins and extension compatibility | Maintain a new application and editor/debugger integrations |

Decision: retain Code-OSS and Rust, add a Windows supervisor and managed setup.
Evaluate WinUI for small setup/device surfaces only when it provides a concrete
benefit. Installed VS 2026 has Windows App SDK support; no bootstrap is needed to
establish that fact. No duplicate WinUI setup workflow installed.

Backend: initially a dedicated `Russet-Dev` WSL2 distribution on NTFS-backed ext4
VHDX. Existing user `Ubuntu` stays separate. Pin an Ubuntu 26.04 WSL rootfs:
upstream vphone setup requires Ubuntu 25.04+ and explicitly rejects 24.04.
Upstream WSL display is lavapipe; `/dev/dxg` alone does not prove compatible
Vulkan imported memory or Radeon acceleration. Keep ARM TCG and CPU rendering
performance as an explicit release risk until measured on this machine.

Prototype transport: Windows current-user-only named pipe -> owned `wsl.exe`
stdio -> same-user Linux Unix socket. No LAN listener or token in arguments.
Preserve Content-Length framing for RPC and DAP. Linux helper validates socket
ownership, mode, and SO_PEERCRED. RPC and each DAP session have separate relays.
Session endpoint discovery replaces backend `dap.path` with the host pipe;
never deliver `/home/.../dap.sock` as a Windows-local endpoint. The prototype is
integrated on the W14 branch with opt-in launcher environment. The host maps
filesystem-valued protocol fields between Windows drive paths and WSL `/mnt/`
paths, preserving app arguments and environment. Unicode/spaces round trips
and actual Rust project detection pass. Mirror WebSocket forwarding remains open.

Authoritative editable source: Windows NTFS checkout. Initial build experiments
will access it at a deterministic `/mnt/c/...` mapping. Move build products and
VM disks to managed ext4. A later Linux mirror requires explicit one-way sync,
conflict handling and source mappings; do not silently add bidirectional sync.

Sources: engine `docs/ARCHITECTURE.md` sections 7 and 10, `extension/src/daemon/`
and `extension/src/debug/adapterFactory.ts`, workstream cards W14/W25/W27a-c/W31;
[WSL import](https://learn.microsoft.com/windows/wsl/basic-commands),
[WSL network modes](https://learn.microsoft.com/windows/wsl/networking),
[vphone WSL support](https://github.com/YahiaAngelo/vphone-linux/blob/main/docs/WSL.md),
[Microsoft Windows skills](https://github.com/microsoft/win-dev-skills).
