// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet toolchain list|resolve|install|uninstall|gc|pins update` (ARCHITECTURE §8.7, §16). Owner: W3
//! (`pins update`: W26).
//!
//! `install` takes a managed component (`python-shared`, `darwin-tools` in Wave 1) and an optional version (the
//! pinned one by default); it is long-running and cancellable with Ctrl-C.

use std::process::ExitCode;

use clap::{Arg, ArgAction, ArgMatches, Command};
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_protocol::toolchain as p;

use super::config::{project_param, report_as};
use crate::CliCtx;

const COMPONENTS: [&str; 10] =
    ["swift", "darwin-sdk", "linker", "lldb", "lldb-dap", "libpython", "xtool", "flutter", "gen-snapshot-ios", "llvm"];
const MANAGED: [&str; 6] = ["python-shared", "darwin-tools", "swift", "llvm", "flutter", "xtool"];

/// The `toolchain` command.
pub fn command() -> Command {
    Command::new("toolchain")
        .about("Toolchains: list, resolve, install, uninstall, gc, pins update")
        .subcommand_required(true)
        .subcommand(
            Command::new("list")
                .about("Every candidate of every component, with versions, provenance, env and problems")
                .arg(Arg::new("refresh").long("refresh").action(ArgAction::SetTrue).help("Re-run every probe")),
        )
        .subcommand(
            Command::new("resolve")
                .about("The tool a build or debug session would use, with its environment")
                .arg(Arg::new("component").value_name("COMPONENT").required(true).value_parser(COMPONENTS)),
        )
        .subcommand(
            Command::new("install")
                .about("Download, verify and install a pinned component (python-shared, darwin-tools)")
                .arg(Arg::new("component").value_name("COMPONENT").required(true).value_parser(MANAGED))
                .arg(Arg::new("version").value_name("VERSION").help("Default: the pinned version")),
        )
        .subcommand(
            Command::new("uninstall")
                .about("Remove an installed version")
                .arg(Arg::new("component").value_name("COMPONENT").required(true).value_parser(MANAGED))
                .arg(Arg::new("version").value_name("VERSION").required(true)),
        )
        .subcommand(
            Command::new("gc")
                .about("Remove installed versions that are not selected")
                .arg(Arg::new("dry-run").long("dry-run").action(ArgAction::SetTrue)),
        )
        .subcommand(
            Command::new("pins").about("The pin manifest (W26)").subcommand_required(true).subcommand(
                Command::new("update")
                    .about("Fetch and verify a newer pin manifest (W26)")
                    .arg(Arg::new("args").num_args(0..).trailing_var_arg(true).allow_hyphen_values(true).hide(true)),
            ),
        )
}

fn parse<T: serde::de::DeserializeOwned>(s: &str) -> Result<T> {
    serde_json::from_value(serde_json::Value::String(s.to_owned())).map_err(|_| {
        Error::new(codes::core::INVALID_ARGUMENT, format!("`{s}` is not a component"))
            .with_hint("Run `russet toolchain --help`.")
    })
}

/// One `russet toolchain` invocation, as RPC params.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ToolchainRequest {
    List(p::ToolchainListParams),
    Resolve(p::ToolchainResolveParams),
    Install(p::ToolchainInstallParams),
    Uninstall(p::ToolchainUninstallParams),
    Gc(p::ToolchainGcParams),
    PinsUpdate,
}

/// Reads the invocation from the root matches.
pub(crate) fn request(m: &ArgMatches, project: Option<String>) -> Result<ToolchainRequest> {
    let Some(("toolchain", m)) = m.subcommand() else {
        return Err(Error::new(codes::core::INVALID_ARGUMENT, "not a toolchain command")
            .with_hint("Run `russet toolchain --help`."));
    };
    let component = |a: &ArgMatches| a.get_one::<String>("component").cloned().unwrap_or_default();
    Ok(match m.subcommand() {
        Some(("list", a)) => {
            ToolchainRequest::List(p::ToolchainListParams { project, refresh: a.get_flag("refresh").then_some(true) })
        }
        Some(("resolve", a)) => {
            ToolchainRequest::Resolve(p::ToolchainResolveParams { component: parse(&component(a))?, project })
        }
        Some(("install", a)) => ToolchainRequest::Install(p::ToolchainInstallParams {
            component: parse(&component(a))?,
            version: a.get_one::<String>("version").cloned(),
        }),
        Some(("uninstall", a)) => ToolchainRequest::Uninstall(p::ToolchainUninstallParams {
            component: parse(&component(a))?,
            version: a.get_one::<String>("version").cloned().unwrap_or_default(),
        }),
        Some(("gc", a)) => {
            ToolchainRequest::Gc(p::ToolchainGcParams { dry_run: a.get_flag("dry-run").then_some(true) })
        }
        Some(("pins", _)) => ToolchainRequest::PinsUpdate,
        _ => {
            return Err(Error::new(codes::core::INVALID_ARGUMENT, "unknown toolchain subcommand")
                .with_hint("Run `russet toolchain --help`."));
        }
    })
}

/// The wire name of a kebab-case enum value (`lldb-dap`).
fn wire_name<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default()
}

fn provenance(p: &p::ToolProvenance) -> String {
    match p {
        p::ToolProvenance::Managed { pin } => format!("managed {pin}"),
        p::ToolProvenance::System => "system".into(),
        p::ToolProvenance::Uv => "uv".into(),
        p::ToolProvenance::XtoolBundle { .. } => "xtool bundle".into(),
        p::ToolProvenance::User => "user".into(),
        p::ToolProvenance::ProjectPin => "project".into(),
    }
}

/// A tool for humans: component, version, `ok` or `no`, provenance and path, then its problems and its env (the
/// variables every launcher applies, ARCHITECTURE §16.2).
pub(crate) fn tool_lines(t: &p::ToolInfo) -> Vec<String> {
    let version = if t.version.is_empty() { "?" } else { t.version.as_str() };
    let mut out = vec![format!(
        "{:<10} {:<8} {:<3} {:<16} {}",
        wire_name(&t.component),
        version,
        if t.ok { "ok" } else { "no" },
        provenance(&t.provenance),
        t.path
    )];
    out.extend(t.problems.iter().map(|p| format!("    {p}")));
    out.extend(t.env.iter().map(|(k, v)| format!("    {k}={v}")));
    out
}

/// An SDK for humans.
pub(crate) fn sdk_line(s: &p::SdkInfo) -> String {
    format!(
        "{} (Xcode {} {}, iOS {}, Swift {}, darwin-tools {}) at {}",
        s.id,
        s.xcode_version,
        s.xcode_build,
        s.ios_sdk,
        if s.compatible_swift.is_empty() { "?" } else { &s.compatible_swift },
        s.darwin_tools.as_deref().unwrap_or("none"),
        s.bundle
    )
}

/// `toolchain list` for humans: every candidate in resolution order (the first `ok` one of a component is the one
/// in use), the SDKs and the pin manifest.
pub(crate) fn human_list(r: &p::ToolchainListResult) -> String {
    let mut out: Vec<String> = r.tools.iter().flat_map(tool_lines).collect();
    out.push(String::new());
    if r.sdks.is_empty() {
        out.push("SDKs: none; import one with `russet sdk import <Xcode_27.xip>`".into());
    } else {
        out.push("SDKs:".into());
        out.extend(r.sdks.iter().map(|s| format!("  {}", sdk_line(s))));
    }
    out.push(format!("Pins: {} ({})", r.pins.id, r.pins.source));
    out.join("\n")
}

fn bytes(n: u64) -> String {
    format!("{:.1} MiB", n as f64 / f64::from(1u32 << 20))
}

/// Runs this area's command; `m` is the root matches, so `m.subcommand()` names the command.
pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    let one = |t: &p::ToolInfo| tool_lines(t).join("\n");
    match request(m, project_param(&cx.global)?)? {
        ToolchainRequest::List(params) => report_as(cx, &cx.peer.call::<p::ToolchainList>(params).await?, human_list)?,
        ToolchainRequest::Resolve(params) => report_as(cx, &cx.peer.call::<p::ToolchainResolve>(params).await?, one)?,
        ToolchainRequest::Install(params) => report_as(cx, &cx.peer.call::<p::ToolchainInstall>(params).await?, one)?,
        ToolchainRequest::Uninstall(params) => {
            report_as(cx, &cx.peer.call::<p::ToolchainUninstall>(params).await?, |r| {
                format!("removed; {} freed", bytes(r.freed_bytes))
            })?;
        }
        ToolchainRequest::Gc(params) => {
            let dry = params.dry_run == Some(true);
            report_as(cx, &cx.peer.call::<p::ToolchainGc>(params).await?, |r| {
                let mut out: Vec<String> =
                    r.removed.iter().map(|p| format!("{} {p}", if dry { "would remove" } else { "removed" })).collect();
                out.push(format!("{} {}", bytes(r.freed_bytes), if dry { "would be freed" } else { "freed" }));
                out.join("\n")
            })?;
        }
        ToolchainRequest::PinsUpdate => return super::not_implemented(m),
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str]) -> ToolchainRequest {
        let m = crate::root_command().try_get_matches_from(args).expect("parses");
        request(&m, None).expect("request")
    }

    #[test]
    fn invocations_become_params() {
        assert_eq!(
            parse_args(&["russet", "--json", "toolchain", "list"]),
            ToolchainRequest::List(p::ToolchainListParams { project: None, refresh: None })
        );
        assert_eq!(
            parse_args(&["russet", "toolchain", "install", "python-shared"]),
            ToolchainRequest::Install(p::ToolchainInstallParams {
                component: p::ManagedComponent::PythonShared,
                version: None
            })
        );
        assert_eq!(
            parse_args(&["russet", "toolchain", "install", "darwin-tools", "1.1.0"]),
            ToolchainRequest::Install(p::ToolchainInstallParams {
                component: p::ManagedComponent::DarwinTools,
                version: Some("1.1.0".into())
            })
        );
        assert_eq!(
            parse_args(&["russet", "toolchain", "resolve", "lldb-dap"]),
            ToolchainRequest::Resolve(p::ToolchainResolveParams {
                component: p::ToolComponent::LldbDap,
                project: None
            })
        );
        assert_eq!(
            parse_args(&["russet", "toolchain", "gc", "--dry-run"]),
            ToolchainRequest::Gc(p::ToolchainGcParams { dry_run: Some(true) })
        );
        assert_eq!(parse_args(&["russet", "toolchain", "pins", "update"]), ToolchainRequest::PinsUpdate);
        assert!(crate::root_command().try_get_matches_from(["russet", "toolchain", "install", "gcc"]).is_err());
    }

    #[test]
    fn the_human_list_shows_every_candidate_its_problems_and_env() {
        let tool = |component, version: &str, ok, provenance, path: &str| p::ToolInfo {
            component,
            version: version.into(),
            path: path.into(),
            provenance,
            env: Default::default(),
            ok,
            problems: if ok { vec![] } else { vec!["system LLD 23.1.1 is too old".into()] },
        };
        let mut lldb = tool(p::ToolComponent::LldbDap, "21.0.0", true, p::ToolProvenance::System, "/s/lldb-dap");
        lldb.env.insert("PYTHONHOME".into(), "/pbs".into());
        let list = p::ToolchainListResult {
            tools: vec![
                tool(
                    p::ToolComponent::Linker,
                    "21.1.6",
                    true,
                    p::ToolProvenance::XtoolBundle { bundle: "/b".into() },
                    "/b/ld",
                ),
                tool(p::ToolComponent::Linker, "23.1.1", false, p::ToolProvenance::System, "/usr/bin/ld64.lld"),
                lldb,
            ],
            sdks: vec![p::SdkInfo {
                id: "xcode-27.0-27A266a".into(),
                xcode_version: "27.0".into(),
                xcode_build: "27A266a".into(),
                ios_sdk: "27.0".into(),
                swift_sdk_name: "darwin".into(),
                bundle: "/b".into(),
                darwin_tools: Some("1.1.0".into()),
                compatible_swift: "6.4".into(),
            }],
            pins: p::PinsInfo { id: "2026.10.11".into(), source: "embedded".into() },
        };
        let text = human_list(&list);
        let lines: Vec<&str> = text.lines().collect();
        assert!(
            lines[0].starts_with("linker") && lines[0].contains(" ok ") && lines[0].contains("xtool bundle"),
            "{text}"
        );
        assert!(lines[1].contains(" no ") && lines[2].trim() == "system LLD 23.1.1 is too old", "{text}");
        assert!(lines[3].starts_with("lldb-dap") && lines[4].trim() == "PYTHONHOME=/pbs", "{text}");
        assert!(
            text.contains("xcode-27.0-27A266a (Xcode 27.0 27A266a, iOS 27.0, Swift 6.4, darwin-tools 1.1.0) at /b")
        );
        assert_eq!(lines.last().copied(), Some("Pins: 2026.10.11 (embedded)"));
    }
}
