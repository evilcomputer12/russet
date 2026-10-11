// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `schemas/config/*.schema.json` are generated and committed (ARCHITECTURE §7.7). This test fails when they are
//! stale; `RUSSET_UPDATE_SCHEMAS=1 cargo test -p russet-config --test schemas` regenerates them.

use std::path::PathBuf;

use russet_config::ConfigFile;
use russet_config::schema::{schema, schema_file_name};

fn schemas_dir() -> PathBuf {
    // The run-time CARGO_MANIFEST_DIR names the tree under test (shared target slots, AGENTS.md §6).
    let manifest = std::env::var_os("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    manifest.join("../../schemas/config")
}

fn render(file: ConfigFile) -> String {
    let mut text = serde_json::to_string_pretty(&schema(file)).expect("schema serializes");
    text.push('\n');
    text
}

#[test]
fn committed_config_schemas_are_current() {
    let dir = schemas_dir();
    let update = std::env::var_os("RUSSET_UPDATE_SCHEMAS").is_some_and(|v| v == "1");
    let mut stale = Vec::new();
    for file in ConfigFile::ALL {
        let path = dir.join(schema_file_name(file));
        let want = render(file);
        if update {
            std::fs::create_dir_all(&dir).expect("schemas/config");
            std::fs::write(&path, &want).expect("write schema");
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(have) if have == want => {}
            _ => stale.push(path.display().to_string()),
        }
    }
    assert!(
        stale.is_empty(),
        "stale config schemas {stale:?}; regenerate with `RUSSET_UPDATE_SCHEMAS=1 cargo test -p russet-config --test schemas`"
    );
}

#[test]
fn the_documented_example_files_validate() {
    // The examples of ARCHITECTURE §14.2–§14.6, trimmed to their keys.
    let project = r#"
schema = 1
[project]
name = "Hello"
stack = "swiftpm"
[app]
bundle-id = "com.example.hello"
display-name = "Hello"
version = "1.0.0"
build-number = "1"
deployment-target = "17.0"
devices = ["iphone"]
orientations = { iphone = ["portrait"] }
icon = "AppIcon"
accent-color = "AccentColor"
launch-screen = { color = "LaunchBackground", image = "LaunchLogo", respects-safe-area = true }
info-plist = "Resources/Info.plist"
entitlements = "Resources/Hello.entitlements"
privacy-manifest = "Resources/PrivacyInfo.xcprivacy"
resources = ["Resources/Assets.xcassets", "Resources/Localizable.xcstrings"]
[[app.extensions]]
product = "HelloWidget"
kind = "widgetkit"
bundle-id-suffix = "widget"
info-plist = "Widget/Info.plist"
entitlements = "Widget/Widget.entitlements"
[build]
default-profile = "debug"
sdk = "auto"
backend = "swiftbuild"
jobs = "auto"
metadata = "sdk"
metadata-consent = { by = "", at = "" }
dsym = "release"
strip = "release"
lint = "auto"
launch-screen-fallback = "error"
launch-storyboard = "auto"
overrides = {}
[build.swiftpm]
package = "."
product = "Hello"
swiftc-flags = []
cc-flags = []
ld-flags = []
[build.flutter]
version = "3.47.7"
target = "lib/main.dart"
flavor = ""
dart-defines = []
plugins = "swiftpm"
gen-snapshot = "auto"
[build.xcode]
workspace = "App.xcworkspace"
scheme = "App"
configuration = { debug = "Debug", release = "Release" }
cocoapods = "auto"
[build.profiles.release]
dsym = "always"
package = "ipa-store"
[signing]
account = ""
team = ""
mode = "automatic"
bundle-id-policy = "auto"
merge = "least"
[signing.targets."PlugIns/HelloWidget.appex"]
entitlements = "Widget/Widget.entitlements"
profile = ""
[run]
device = "auto"
args = []
env = {}
console = true
install = "if-changed"
launch = "auto"
[debug]
lldb = "auto"
init-commands = []
symbols = "auto"
jit = "auto"
swift-expressions = "auto"
attach-mode = "broker"
[ship]
mode = "linux-native"
app-id = ""
build-number = "auto"
testflight-groups = ["Internal"]
uses-non-exempt-encryption = false
[ship.ci]
provider = "github"
signing = "russet"
[toolchain]
swift = "6.4.0"
xcode-sdk = "27.0"
linker = "auto"
[agents]
tier-max = 1
deny-tools = []
"#;
    russet_config::load::parse::<russet_config::ProjectConfig>("russet.toml".as_ref(), project).expect("§14.2");
    let custom = "[signing]\nbundle-id-policy = { custom = { \"com.example.hello\" = \"com.me.hello\" } }\n";
    russet_config::load::parse::<russet_config::ProjectConfig>("russet.toml".as_ref(), custom).expect("custom");
    let local =
        "[app]\nbundle-id = \"com.evilcomputer12.orchard.hello\"\n[signing]\nbundle-id-policy = \"xtool-prefix\"\n";
    russet_config::load::parse::<russet_config::ProjectConfig>("russet.local.toml".as_ref(), local).expect("§4.10");

    let global = r#"
schema = 1
[daemon]
idle-timeout = "30m"
max-parallel-jobs = 4
max-build-jobs = 1
build-nice = 10
log-filter = "info"
log-retention-days = 7
listen-tcp = ""
[device]
keep-warm = "10m"
lease-grace = "2s"
mux = "auto"
[device.ddi]
order = ["dir", "imported"]
dir = ""
allow-mirror = false
ticket-cache = false
[storage]
min-free-space = "5GiB"
symbol-quota = "20GiB"
[devices.aliases]
iphone12 = "<udid>"
[web]
listen = "127.0.0.1:0"
[mcp.http]
enabled = false
listen = "127.0.0.1:43117"
expose-docker = false
[fallback]
pymobiledevice3 = "auto"
[update]
check = "notify"
channel = "stable"
[privacy]
telemetry = "off"
"#;
    russet_config::load::parse::<russet_config::GlobalConfig>("config.toml".as_ref(), global).expect("§14.3");

    let accounts = r#"
schema = 1
default = "asc:<KEYID>"
[[account]]
id = "asc:<KEYID>"
kind = "asc-team-key"
label = "Team key (App Manager)"
key-id = "<KEYID>"
issuer-id = "<UUID>"
team-id = "<TEAMID>"
secret = "secret:asc-key/<KEYID>"
imported-from = "xtool"
[[account]]
id = "appleid:<DSID>"
kind = "apple-id"
apple-id = "someone@example.com"
team-id = "<TEAMID>"
team-tier = "free"
anisette = "local"
secret = "secret:apple-id-session/<DSID>"
"#;
    let (a, _) =
        russet_config::load::parse::<russet_config::accounts::AccountsFile>("accounts.toml".as_ref(), accounts)
            .expect("§14.4");
    assert_eq!(a.accounts.len(), 2);

    let provision = r#"
schema = 1
[paid]
register-device = "auto"
register-bundle-id = "auto"
enable-capability = "auto"
russet-profiles = "auto"
create-certificate = "ask-once"
revoke-certificate = "typed"
disable-capability = "typed"
delete-foreign-profile = "typed"
[free]
register-device = "ask"
register-bundle-id = "ask"
enable-capability = "auto"
russet-profiles = "auto"
create-certificate = "ask"
revoke-certificate = "typed"
disable-capability = "typed"
delete-foreign-profile = "typed"
"#;
    let (p, _) =
        russet_config::load::parse::<russet_config::ProvisionPolicy>("provision-policy.toml".as_ref(), provision)
            .expect("§14.5");
    assert_eq!(p, russet_config::ProvisionPolicy { schema: Some(1), ..Default::default() }, "the defaults are §14.5");

    let mcp = r#"
schema = 1
default-tier-max = 1
session-approval-ttl = "30m"
[clients.claude-code]
tier-max = 1
reveal-udid = false
[clients.local-llama]
tier-max = 0
[tools.ship_upload]
approval = "always"
rate-limit = "1/10m"
[tools.plan_apply]
approval = "session"
"#;
    let (m, _) =
        russet_config::load::parse::<russet_config::McpPolicy>("mcp-policy.toml".as_ref(), mcp).expect("§14.6");
    assert!(m.validate().is_ok());
    assert_eq!(m.client_tier_max("local-llama"), 0);
}
