# Russet

Build, run, debug and ship iOS apps from **Linux and Windows**, with a real iPhone
plugged in, and **no Mac**.

> Status: private, pre-alpha. The repository goes public once milestone M1 works
> (a SwiftUI app built on Linux, installed on an iPhone and stopped at a breakpoint).

## What Russet is

- **`russet`**: a Rust command-line core and device daemon. It covers:
  - SDK import from your own Xcode download
  - builds: SwiftPM, Flutter, and later Xcode projects and React Native
  - signing with a free Apple ID or a paid App Store Connect API key
  - install, launch and LLDB debugging over USB or Wi-Fi, without root
  - logs, screen mirroring and profiling
  - TestFlight and App Store uploads
- **Russet IDE**: a desktop IDE built on Code-OSS (the open-source core of VS Code).
  - Dedicated buttons for device, run, debug and hot reload.
  - Live iPhone mirror.
  - Editors for Info.plist, signing, capabilities and assets.
  - Publishing and profiling panels.
- **Agent-ready**: a built-in MCP server lets AI agents build, run, debug, read logs,
  take screenshots and tap the screen. Supported agents: Claude Code, Codex, Gemini CLI,
  Copilot, Cline, Continue and local models.

Russet builds on open-source work: xtool, idevice, apple-codesign, LLVM/LLDB,
Swift, Flutter and others. See [docs/research](docs/research/).

## Apple software

Russet contains no Apple software and does not redistribute any. You supply your own
Xcode download (`Xcode.xip`), which you get from Apple with your own Apple ID. See
[NOTICE.md](NOTICE.md).

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
