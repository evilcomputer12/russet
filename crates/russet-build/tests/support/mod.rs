// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Test doubles for russet-build's integration tests:
//! - [`LocalRunner`]: a plain tokio process runner (W2's `ProcessRunner` replaces it in the daemon), which also
//!   hides `xtool` from `PATH`, so the real builds prove they never need it (PLAN §4.5 acceptance), and honours
//!   `CommandSpec::timeout`;
//! - [`LlvmMachO`]: a `MachOTool` over `llvm-otool` and `llvm-install-name-tool` (LLVM ≥ 23), standing in for
//!   W6's russet-macho bodies while they are F0 stubs ([`russet_macho_is_implemented`]);
//! - [`ScriptedSwift`], [`FakeMachO`] and [`fake_facts`]: a SwiftPM and Mach-O world without Swift or an SDK, for CI.

#![allow(dead_code, clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use camino::{Utf8Path, Utf8PathBuf};
use plist::{Dictionary, Value};
use russet_build::{MachOSummary, MachOTool};
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_core::proc::{ChildProcess, CommandOutput, CommandRunner, CommandSpec};
use russet_core::progress::{EventSink, JobEventKind, LogStream, ProgressSink};
use russet_sdk::{SdkFacts, SdkProvider, SdkSelector, ToolPaths, Version};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio_util::sync::CancellationToken;

/// The RussetProbe fixture, copied into `dest` (without `.russet`).
pub fn copy_probe(dest: &Utf8Path) -> Utf8PathBuf {
    let src = Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/apps/RussetProbe");
    let to = dest.join("RussetProbe");
    copy_dir(&src, &to);
    to
}

fn copy_dir(from: &Utf8Path, to: &Utf8Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in from.read_dir_utf8().unwrap() {
        let e = e.unwrap();
        if e.file_name() == ".russet" || e.file_name() == ".build" || e.file_name() == "russet.local.toml" {
            continue;
        }
        if e.file_type().unwrap().is_dir() {
            copy_dir(e.path(), &to.join(e.file_name()));
        } else {
            std::fs::copy(e.path(), to.join(e.file_name())).unwrap();
        }
    }
}

/// A temporary directory as a UTF-8 path.
pub fn tmp() -> (tempfile::TempDir, Utf8PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = Utf8PathBuf::from_path_buf(dir.path().canonicalize().unwrap()).unwrap();
    (dir, path)
}

/// Records everything a build sends to its sink.
#[derive(Default)]
pub struct RecordingSink {
    pub lines: Mutex<Vec<(LogStream, String)>>,
    pub events: Mutex<Vec<JobEventKind>>,
    pub reports: Mutex<Vec<String>>,
    /// `begin` and `end` calls.
    pub begins: Mutex<usize>,
}

impl ProgressSink for RecordingSink {
    fn begin(&self, _title: &str, _cancellable: bool) {
        *self.begins.lock().unwrap() += 1;
    }
    fn report(&self, msg: Option<&str>, _pct: Option<u8>) {
        if let Some(m) = msg {
            self.reports.lock().unwrap().push(m.to_owned());
        }
    }
    fn end(&self, _msg: Option<&str>) {
        *self.begins.lock().unwrap() += 1;
    }
    fn log(&self, stream: LogStream, line: &str) {
        self.lines.lock().unwrap().push((stream, line.to_owned()));
    }
}

impl EventSink for RecordingSink {
    fn emit(&self, e: JobEventKind) {
        self.events.lock().unwrap().push(e);
    }
}

/// A tokio process runner: line streaming, collected output, process-group kill on cancel.
pub struct LocalRunner;

async fn pump<R: AsyncRead + Unpin>(r: Option<R>, stream: LogStream, sink: &dyn EventSink) -> Vec<u8> {
    let mut all = Vec::new();
    let Some(r) = r else { return all };
    let mut reader = BufReader::new(r);
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                all.extend_from_slice(&line);
                let text = String::from_utf8_lossy(&line);
                sink.log(stream, text.trim_end_matches(['\n', '\r']));
            }
        }
    }
    all
}

/// `PATH` without the directories that hold an `xtool` binary.
fn path_without_xtool(path: &OsString) -> OsString {
    let dirs: Vec<PathBuf> = std::env::split_paths(path).filter(|d| !d.join("xtool").exists()).collect();
    std::env::join_paths(dirs).unwrap_or_default()
}

#[async_trait]
impl CommandRunner for LocalRunner {
    async fn run(&self, spec: &CommandSpec, sink: &dyn EventSink, cancel: &CancellationToken) -> Result<CommandOutput> {
        let mut cmd = tokio::process::Command::new(&spec.program);
        cmd.args(&spec.args);
        if spec.env_clear {
            cmd.env_clear();
        }
        for (k, v) in &spec.env {
            if k == "PATH" {
                cmd.env(k, path_without_xtool(v));
            } else {
                cmd.env(k, v);
            }
        }
        if let Some(cwd) = &spec.cwd {
            cmd.current_dir(cwd);
        }
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        #[cfg(unix)]
        cmd.process_group(0);
        cmd.kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| {
            Error::new(codes::toolchain::MISSING, format!("cannot run {}: {e}", spec.program.display())).caused_by(e)
        })?;
        let pid = child.id();
        let kill_group = || {
            if let Some(pid) = pid {
                let _ = std::process::Command::new("kill").args(["-TERM", &format!("-{pid}")]).status();
            }
        };
        let (out, err) = (child.stdout.take(), child.stderr.take());
        let work = async {
            let (o, e, status) =
                tokio::join!(pump(out, LogStream::Stdout, sink), pump(err, LogStream::Stderr, sink), child.wait());
            (o, e, status)
        };
        let deadline = async {
            match spec.timeout {
                Some(t) => tokio::time::sleep(t).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            (stdout, stderr, status) = work => {
                let status = status.map_err(|e| Error::new(codes::core::IO, format!("wait: {e}")))?;
                #[cfg(unix)]
                let signal = std::os::unix::process::ExitStatusExt::signal(&status);
                #[cfg(not(unix))]
                let signal = None;
                Ok(CommandOutput { code: status.code(), signal, stdout, stderr })
            }
            () = cancel.cancelled() => {
                kill_group();
                Err(Error::new(codes::core::CANCELLED, "cancelled"))
            }
            () = deadline => {
                kill_group();
                Err(Error::new(codes::core::TIMEOUT, format!("{} timed out", spec.program.display())))
            }
        }
    }

    async fn spawn(&self, _spec: &CommandSpec) -> Result<Box<dyn ChildProcess>> {
        Err(Error::new(codes::core::NOT_IMPLEMENTED, "LocalRunner::spawn"))
    }
}

/// `MachOTool` over LLVM's `llvm-otool` and `llvm-install-name-tool`.
pub struct LlvmMachO {
    pub otool: Utf8PathBuf,
    pub install_name_tool: Utf8PathBuf,
}

impl LlvmMachO {
    /// LLVM ≥ 23 tools next to `swift` or on PATH, if any.
    pub async fn find() -> Option<Self> {
        let h = russet_sdk::host::HostToolchain::probe(&LocalRunner).await.ok()?;
        Some(Self { otool: h.llvm_otool?, install_name_tool: h.llvm_install_name_tool? })
    }

    /// Parses `llvm-otool -l` text.
    pub fn parse(text: &str, head: &[u8]) -> MachOSummary {
        let mut s = MachOSummary::default();
        let mut cmd = String::new();
        for line in text.lines().map(str::trim) {
            let (key, value) = line.split_once(' ').map_or((line, ""), |(k, v)| (k, v.trim()));
            match key {
                "cmd" => cmd = value.to_owned(),
                "platform" if cmd == "LC_BUILD_VERSION" => s.platform = value.parse().ok(),
                "sdk" if cmd == "LC_BUILD_VERSION" => s.sdk = Version::parse(value),
                "minos" if cmd == "LC_BUILD_VERSION" => s.minos = Version::parse(value),
                "tool" if cmd == "LC_BUILD_VERSION" => {
                    s.tools.push(russet_build::macho::tool_name(value.parse().unwrap_or(0)))
                }
                "version" if cmd == "LC_BUILD_VERSION" => {
                    if let Some(last) = s.tools.last_mut() {
                        *last = format!("{last} {value}");
                    }
                }
                "uuid" if cmd == "LC_UUID" => s.uuid = uuid::Uuid::parse_str(value).ok(),
                "path" if cmd == "LC_RPATH" => {
                    s.rpaths.push(value.split(" (offset").next().unwrap_or(value).to_owned())
                }
                "name" if cmd.starts_with("LC_LOAD") || cmd == "LC_REEXPORT_DYLIB" => {
                    s.dylibs.push(value.split(" (offset").next().unwrap_or(value).to_owned());
                }
                "cryptid" if cmd.starts_with("LC_ENCRYPTION_INFO") => s.encrypted = Some(value != "0"),
                _ => {}
            }
        }
        if head.len() >= 12 && head[..4] == [0xCF, 0xFA, 0xED, 0xFE] {
            let cpu = u32::from_le_bytes([head[4], head[5], head[6], head[7]]);
            let sub = u32::from_le_bytes([head[8], head[9], head[10], head[11]]) & 0xFF;
            s.archs.push(match (cpu, sub) {
                (0x0100_000C, 2) => "arm64e".into(),
                (0x0100_000C, _) => "arm64".into(),
                (0x0100_0007, _) => "x86_64".into(),
                _ => format!("cpu{cpu:x}"),
            });
        }
        s
    }
}

impl MachOTool for LlvmMachO {
    fn summary(&self, bytes: &[u8]) -> Result<MachOSummary> {
        let dir = tempfile::tempdir().map_err(|e| Error::new(codes::core::IO, e.to_string()))?;
        let file = dir.path().join("image");
        std::fs::write(&file, bytes).map_err(|e| Error::new(codes::core::IO, e.to_string()))?;
        let out = std::process::Command::new(self.otool.as_std_path())
            .arg("-l")
            .arg(&file)
            .output()
            .map_err(|e| Error::new(codes::core::IO, e.to_string()))?;
        Ok(Self::parse(&String::from_utf8_lossy(&out.stdout), bytes))
    }

    fn delete_rpath(&self, path: &Utf8Path, rpath: &str) -> Result<()> {
        let st = std::process::Command::new(self.install_name_tool.as_std_path())
            .args(["-delete_rpath", rpath, path.as_str()])
            .status()
            .map_err(|e| Error::new(codes::core::IO, e.to_string()))?;
        if st.success() {
            Ok(())
        } else {
            Err(Error::new(codes::core::IO, format!("llvm-install-name-tool failed on {path}")))
        }
    }
}

/// Whether russet-macho has its real bodies (W6) rather than F0's `todo!()`: parsing an empty buffer is an error
/// with W6's code and a panic with the stubs. Probed once per process, with the panic message silenced.
pub fn russet_macho_is_implemented() -> bool {
    static IMPLEMENTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *IMPLEMENTED.get_or_init(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let implemented = std::panic::catch_unwind(|| russet_macho::MachO::parse(&[]).is_err()).unwrap_or(false);
        std::panic::set_hook(previous);
        implemented
    })
}

/// A Mach-O stand-in: the arm64 magic, then one `RPATH <path>` or `LOAD <name>` line per load command.
pub fn fake_binary(rpaths: &[&str], loads: &[&str]) -> Vec<u8> {
    let mut b = vec![0xCF, 0xFA, 0xED, 0xFE, 0x0C, 0x00, 0x00, 0x01, 0, 0, 0, 0, 2, 0, 0, 0, b'\n'];
    for r in rpaths {
        b.extend_from_slice(format!("RPATH {r}\n").as_bytes());
    }
    for l in loads {
        b.extend_from_slice(format!("LOAD {l}\n").as_bytes());
    }
    b
}

/// A `MachOTool` over [`fake_binary`] images; records rpath deletions.
#[derive(Default)]
pub struct FakeMachO {
    pub deleted: Mutex<Vec<(Utf8PathBuf, String)>>,
}

impl MachOTool for FakeMachO {
    fn summary(&self, bytes: &[u8]) -> Result<MachOSummary> {
        let text = String::from_utf8_lossy(bytes.get(17..).unwrap_or_default()).into_owned();
        let mut s = MachOSummary {
            archs: vec!["arm64".into()],
            platform: Some(2),
            minos: Some(Version::new(17, 0, 0)),
            sdk: Some(Version::new(27, 0, 0)),
            tools: vec!["lld 21.1.6".into()],
            uuid: Some(uuid::Uuid::from_u128(u128::from(bytes.len() as u64))),
            encrypted: Some(false),
            ..Default::default()
        };
        for line in text.lines() {
            if let Some(r) = line.strip_prefix("RPATH ") {
                s.rpaths.push(r.to_owned());
            } else if let Some(l) = line.strip_prefix("LOAD ") {
                s.dylibs.push(l.to_owned());
            }
        }
        Ok(s)
    }

    fn delete_rpath(&self, path: &Utf8Path, rpath: &str) -> Result<()> {
        let bytes = std::fs::read(path).unwrap();
        let needle = format!("RPATH {rpath}\n").into_bytes();
        let pos = bytes
            .windows(needle.len())
            .position(|w| w == needle.as_slice())
            .ok_or_else(|| Error::new(codes::core::IO, format!("no rpath {rpath}")))?;
        let mut out = bytes[..pos].to_vec();
        out.extend_from_slice(&bytes[pos + needle.len()..]);
        std::fs::write(path, out).unwrap();
        self.deleted.lock().unwrap().push((path.to_owned(), rpath.to_owned()));
        Ok(())
    }
}

/// SDK facts with made-up values (no SDK on disk).
pub fn fake_facts(root: &Utf8Path) -> SdkFacts {
    let mut info = Dictionary::new();
    for (k, v) in [
        ("DTPlatformName", "iphoneos"),
        ("DTPlatformVersion", "27.0"),
        ("DTSDKName", "$(SDK_NAME)"),
        ("DTSDKBuild", "$(SDK_PRODUCT_BUILD_VERSION)"),
        ("DTPlatformBuild", "$(PLATFORM_PRODUCT_BUILD_VERSION)"),
        ("DTXcode", "$(XCODE_VERSION_ACTUAL)"),
        ("DTXcodeBuild", "$(XCODE_PRODUCT_BUILD_VERSION)"),
    ] {
        info.insert(k.into(), Value::from(v));
    }
    SdkFacts {
        id: "xcode-27.0-27A266a".into(),
        sdk_path: root.join("sdk/iPhoneOS27.0.sdk"),
        sdk_version: Version::new(27, 0, 0),
        sdk_canonical_name: "iphoneos27.0".into(),
        sdk_build: Some("24A430".into()),
        platform_build: Some("24A430".into()),
        platform_version: "27.0".into(),
        xcode_version: Version::new(27, 0, 0),
        xcode_build: "27A266a".into(),
        swift_required: Version::new(6, 4, 0),
        valid_deployment_targets: vec![Version::new(15, 0, 0), Version::new(17, 0, 0), Version::new(27, 0, 0)],
        additional_info: info,
        back_deploy_dirs: vec![root.join("sdk/swift-6.2/iphoneos")],
        platforms_view: root.join("data/sdk/view/Developer/Platforms"),
        toolset_swb: root.join("data/sdk/toolset-swb.json"),
        toolset_native: root.join("data/sdk/toolset-native.json"),
        tools: ToolPaths {
            swift: "/fake/bin/swift".into(),
            swiftc: "/fake/bin/swiftc".into(),
            swift_version: Some(Version::new(6, 4, 0)),
            swift_version_text: "Swift version 6.4 (fake)".into(),
            linker_dir: root.join("sdk/toolset/bin"),
            ld64_lld: root.join("sdk/toolset/bin/ld64.lld"),
            librarian: root.join("sdk/toolset/bin/llvm-lib"),
            ..Default::default()
        },
        bundle: root.join("sdk"),
        default_compiler: Some("com.apple.compilers.llvm.clang.1_0".into()),
        back_deploy_not_needed_from: BTreeMap::from([(
            "libswiftCompatibilitySpan.dylib".to_owned(),
            Version::new(26, 0, 0),
        )]),
        darwin_tools: Some("1.1.0".into()),
    }
}

/// An `SdkProvider` that returns fixed facts.
pub struct FixedSdk(pub SdkFacts);

#[async_trait]
impl SdkProvider for FixedSdk {
    async fn resolve(&self, _sel: &SdkSelector) -> Result<SdkFacts> {
        Ok(self.0.clone())
    }
}

/// A scripted `swift`: answers `swift package resolve/show-dependencies/describe` with a fixed package and makes
/// `swift build` write fake executables (or fail with a compiler error).
pub struct ScriptedSwift {
    pub describe: String,
    pub package_identity: String,
    /// Dependencies of the root package: `(identity, name, path, describe JSON)`.
    pub deps: Vec<(String, String, String, String)>,
    pub binary: Vec<u8>,
    pub fail_with: Option<String>,
    /// Extra lines `swift build` prints (for example an environment dump).
    pub extra_lines: Vec<String>,
    pub calls: Mutex<Vec<Vec<String>>>,
    pub envs: Mutex<Vec<BTreeMap<OsString, OsString>>>,
}

impl ScriptedSwift {
    pub fn new(describe: &str, identity: &str, binary: Vec<u8>) -> Arc<Self> {
        Arc::new(Self {
            describe: describe.to_owned(),
            package_identity: identity.to_owned(),
            deps: Vec::new(),
            binary,
            fail_with: None,
            extra_lines: Vec::new(),
            calls: Mutex::new(Vec::new()),
            envs: Mutex::new(Vec::new()),
        })
    }
}

fn arg_after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).map(String::as_str)
}

#[async_trait]
impl CommandRunner for ScriptedSwift {
    async fn run(
        &self,
        spec: &CommandSpec,
        sink: &dyn EventSink,
        _cancel: &CancellationToken,
    ) -> Result<CommandOutput> {
        let args: Vec<String> = spec.args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        self.calls.lock().unwrap().push(args.clone());
        self.envs.lock().unwrap().push(spec.env.clone());
        let ok = |stdout: Vec<u8>| Ok(CommandOutput { code: Some(0), signal: None, stdout, stderr: Vec::new() });
        match args.first().map(String::as_str) {
            Some("package") if args.contains(&"resolve".to_owned()) => ok(Vec::new()),
            Some("package") if args.contains(&"show-dependencies".to_owned()) => {
                let out = arg_after(&args, "-o").unwrap();
                let pkg = arg_after(&args, "--package-path").unwrap();
                let deps: Vec<serde_json::Value> = self
                    .deps
                    .iter()
                    .map(|(id, name, path, _)| {
                        serde_json::json!({"identity": id, "name": name, "path": path, "dependencies": []})
                    })
                    .collect();
                let json = serde_json::json!({"identity": self.package_identity, "name": "Pkg", "path": pkg, "dependencies": deps});
                std::fs::write(out, json.to_string()).unwrap();
                ok(Vec::new())
            }
            Some("package") if args.contains(&"describe".to_owned()) => {
                let pkg = arg_after(&args, "--package-path").unwrap();
                let describe = self.deps.iter().find(|d| d.2 == pkg).map_or(&self.describe, |d| &d.3);
                ok(format!("noise\n{describe}").into_bytes())
            }
            Some("build") => {
                sink.log(LogStream::Stdout, "Building for debugging...");
                sink.log(LogStream::Stdout, "[1 / 2] Compiling");
                for line in &self.extra_lines {
                    sink.log(LogStream::Stderr, line);
                }
                if let Some(msg) = &self.fail_with {
                    sink.log(LogStream::Stderr, msg);
                    return Ok(CommandOutput {
                        code: Some(1),
                        signal: None,
                        stdout: Vec::new(),
                        stderr: msg.clone().into_bytes(),
                    });
                }
                let scratch = Utf8PathBuf::from(arg_after(&args, "--scratch-path").unwrap());
                let release = arg_after(&args, "--configuration") == Some("release");
                let products =
                    scratch.join("out/Products").join(if release { "Release-iphoneos" } else { "Debug-iphoneos" });
                std::fs::create_dir_all(&products).unwrap();
                let builder = Utf8PathBuf::from(arg_after(&args, "--package-path").unwrap());
                for entry in builder.join("Sources").read_dir_utf8().unwrap() {
                    let target = entry.unwrap().file_name().to_owned();
                    std::fs::write(products.join(&target), &self.binary).unwrap();
                }
                sink.log(LogStream::Stdout, "[2 / 2] Linking");
                sink.log(LogStream::Stdout, "Build complete! (0.01 secs)");
                ok(Vec::new())
            }
            _ => {
                Ok(CommandOutput { code: Some(127), signal: None, stdout: Vec::new(), stderr: b"unexpected".to_vec() })
            }
        }
    }

    async fn spawn(&self, _spec: &CommandSpec) -> Result<Box<dyn ChildProcess>> {
        Err(Error::new(codes::core::NOT_IMPLEMENTED, "ScriptedSwift::spawn"))
    }
}
