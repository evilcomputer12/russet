# Russet documentation

Russet is an iOS IDE for Windows. It builds Swift and SwiftUI apps with an iOS SDK made from Apple's Xcode
archive, and runs them on your iPhone over USB or on a virtual iPhone inside your PC. Its engine runs in a
private WSL distribution named `Russet-Dev`, which Russet installs and manages for you.

These pages describe the Windows release. They ship with Russet, so they always match the version you have.
Run **Russet: Open Documentation** to open this page again.

## Contents

- [Getting started](getting-started.md): install Russet and go through the setup page, step by step.
- [Signing](signing.md): sign in with your Apple ID, or add an App Store Connect API key, and choose your
  bundle identifiers.
- [Run and debug](run-and-debug.md): choose where to run, then run, debug and stop your app; read the build
  log, the console and the Problems view.
- [Virtual iPhone](virtual-iphone.md): create, start and use the virtual iPhone, and how its engine updates.
- [Updates](updates.md): how Russet, Swift and the virtual iPhone engine stay up to date.
- [AI agents](ai-agents.md): give coding agents Russet's tools through MCP.
- [Command line](command-line.md): `russet` in a terminal or a script, without Russet's window: build, run,
  debug, the virtual iPhone, setup and MCP.
- [Troubleshooting](troubleshooting.md): an offline engine, WSL memory and disk, slow builds, where the logs
  are, and what to put in a bug report.
- [Settings](settings.md): every Russet setting and its default.
- [Error reference](errors.md): what each Russet error code means and what to do about it.

## Where to start

1. Read [Getting started](getting-started.md) and finish the setup page.
2. Sign in as described in [Signing](signing.md).
3. Create or open a project, then follow [Run and debug](run-and-debug.md).

When an error notification has an **Open Docs** button, it opens the matching section of the
[Error reference](errors.md).
