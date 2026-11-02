# Credits

Russet stands on the work of many people. These are the projects it builds on, and the people and
teams behind them. Thank you.

Projects marked *fetched* are not part of this repository: Russet downloads and builds them on your
own machine, from their own repositories, under their own licences. Contributor lists name each
project's most active contributors on GitHub as of November 2026.

## The virtual iPhone

| Project | What Russet uses it for | People and teams |
|---|---|---|
| [vphone-linux](https://github.com/YahiaAngelo/vphone-linux) (*fetched*) | Creating and running the virtual iPhone on Linux and WSL2 | [@YahiaAngelo](https://github.com/YahiaAngelo) |
| [vphone-cli](https://github.com/Lakr233/vphone-cli) (*fetched*) | The virtual iPhone itself: firmware tooling, the guest daemon (vphoned), the VM bundle | [@Lakr233](https://github.com/Lakr233), [@0xJacky](https://github.com/0xJacky), [@zqxwce](https://github.com/zqxwce), [@mpass99](https://github.com/mpass99), [@insidegui](https://github.com/insidegui), [@TastyHeadphones](https://github.com/TastyHeadphones) and contributors |
| [reims-vgpu](https://github.com/steelbrain/reims-vgpu) (*fetched*, vphone-linux's fork) | The paravirtual GPU the guest renders through | [@steelbrain](https://github.com/steelbrain), [@nkhaooat](https://github.com/nkhaooat) and contributors |
| [metal2vulkan](https://github.com/steelbrain/metal2vulkan) (*fetched*, vphone-linux's fork) | Translating the guest's Metal to Vulkan | [@steelbrain](https://github.com/steelbrain) |
| [Inferno (QEMUAppleSilicon)](https://github.com/ChefKissInc/Inferno) (*fetched*, as vphone-linux's QEMU fork) | The Apple Silicon machine emulation under the virtual iPhone | The [ChefKiss](https://github.com/ChefKissInc) team, on [QEMU](https://www.qemu.org/) and its contributors |
| [linux-apfs-rw](https://github.com/linux-apfs/linux-apfs-rw) (*fetched*) | Writing the virtual iPhone's APFS volumes during setup | [@eafer](https://github.com/eafer) and the [linux-apfs](https://github.com/linux-apfs) contributors |
| [usbmuxd](https://github.com/libimobiledevice/usbmuxd) (*fetched*) | USB multiplexing between the host and the virtual iPhone | [@nikias](https://github.com/nikias), [@FunkyM](https://github.com/FunkyM), [@marcan](https://github.com/marcan) and the [libimobiledevice](https://libimobiledevice.org/) team |
| [Mesa](https://mesa3d.org/) (lavapipe) | Vulkan rendering on the CPU under WSL2 | The Mesa developers |

## Real iPhones, signing and builds

| Project | What Russet uses it for | People and teams |
|---|---|---|
| [idevice](https://github.com/jkcoxson/idevice) | Talking to iPhones: pairing, tunnels, installs, the screen mirror, input | [@jkcoxson](https://github.com/jkcoxson), [@neoarz](https://github.com/neoarz), [@abdubanna](https://github.com/abdubanna), [@claration](https://github.com/claration), [@maxslarsson](https://github.com/maxslarsson) and contributors |
| Anisette server `mac-ani.jkcoxson.com` | Apple ID sign-in (the server the owner chose) | [@jkcoxson](https://github.com/jkcoxson) |
| [xtool](https://github.com/xtool-org/xtool) (*fetched*) | The iOS SDK from Xcode, and Swift builds for iPhone on Linux | [@kabiroberai](https://github.com/kabiroberai) and the [xtool](https://github.com/xtool-org) contributors |
| [apple-platform-rs](https://github.com/indygreg/apple-platform-rs) (`apple-codesign`) | Code signing | [@indygreg](https://github.com/indygreg) and contributors |
| [pymobiledevice3](https://github.com/doronz88/pymobiledevice3) | A reference while developing device features | [@doronz88](https://github.com/doronz88), [@matan1008](https://github.com/matan1008) and contributors |
| [Swift](https://www.swift.org/), [LLVM and LLDB](https://llvm.org/) (*fetched*) | Compiling and debugging iOS apps | The Swift and LLVM communities |
| [Flutter](https://flutter.dev/) (*fetched*) | Flutter apps | The Flutter team |

## The IDE and Windows

| Project | What Russet uses it for | People and teams |
|---|---|---|
| [Code-OSS](https://github.com/microsoft/vscode) | The editor Russet's IDE is built on | The [Visual Studio Code](https://github.com/microsoft/vscode) team and contributors |
| [WSL](https://github.com/microsoft/WSL) and [WSLg](https://github.com/microsoft/wslg) | Russet's Linux backend on Windows, and the virtual iPhone's window | The Windows Subsystem for Linux team |
| [Rust](https://www.rust-lang.org/) and its crates | The engine: [tokio](https://tokio.rs/), [axum](https://github.com/tokio-rs/axum), [rustls](https://github.com/rustls/rustls), [reqwest](https://github.com/seanmonstar/reqwest), [plist](https://github.com/ebarnard/rust-plist), [goblin](https://github.com/m4b/goblin), [zip](https://github.com/zip-rs/zip2) and many more | The Rust project and every crate's authors; `cargo about` (see `about.toml`) lists them all with their licences |

Missing someone? Open an issue or a pull request.
