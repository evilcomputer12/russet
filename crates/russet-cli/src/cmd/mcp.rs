// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet mcp` (ARCHITECTURE §8.7, §11; IDE §6.2). Owner: W13 (Wave 2), W20 (http; Wave 3).
//!
//! - `serve` stands up the stdio bridge ([`russet_mcp::serve_stdio`]); it speaks MCP on stdout, so it prints
//!   nothing else there.
//! - `tools` prints the canonical catalog (ARCHITECTURE §11.4); `--json` is the contract snapshot.
//! - `install` / `uninstall` / `status` drive the daemon's `mcp.*` methods.
//! - `http enable|disable` is W20.

use std::process::ExitCode;

use clap::{Arg, ArgAction, ArgMatches, Command};
use russet_core::error::{Error, Result, codes};
use russet_protocol::mcp::{
    McpInstall, McpInstallParams, McpScope, McpStatus, McpStatusParams, McpUninstall, McpUninstallParams,
};

use crate::CliCtx;

/// The clients W13 can register (MCP-04 partial).
const SUPPORTED_CLIENTS: &[&str] = &["claude-code", "codex", "gemini", "vscode"];

/// The `mcp` command.
pub fn command() -> Command {
    let client_arg = || {
        Arg::new("client")
            .long("client")
            .value_name("ID")
            .required(true)
            .value_parser(SUPPORTED_CLIENTS.to_vec())
            .help("The agent client to register")
    };
    let scope_arg = || {
        Arg::new("scope")
            .long("scope")
            .value_name("SCOPE")
            .value_parser(["user", "project"])
            .help("user (default) or project")
    };
    let project_arg = || Arg::new("project").long("project").value_name("DIR").help("Project root for project scope");
    let dry_run = || Arg::new("dry-run").long("dry-run").action(ArgAction::SetTrue).help("Plan only; do not write");

    Command::new("mcp")
        .about("The MCP server: serve, tools, install, uninstall, status, http")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(
            Command::new("serve")
                .about("Run the stdio MCP server (used by agent clients)")
                .arg(Arg::new("client").long("client").value_name("ID").help("The trusted client id"))
                .arg(Arg::new("http").long("http").value_name("ADDR").help("Serve streamable HTTP instead (W20)"))
                .arg(
                    Arg::new("tier-max")
                        .long("tier-max")
                        .value_name("N")
                        .value_parser(clap::value_parser!(u8).range(0..=3))
                        .help("Override the client's tier ceiling"),
                ),
        )
        .subcommand(
            Command::new("tools").about("Print the tool catalog (ARCHITECTURE §11.4)").arg(
                Arg::new("tier-max")
                    .long("tier-max")
                    .value_name("N")
                    .value_parser(clap::value_parser!(u8).range(0..=3))
                    .help("Only tools at or below this tier"),
            ),
        )
        .subcommand(
            Command::new("install")
                .about("Register the Russet MCP server with an agent client")
                .arg(client_arg())
                .arg(scope_arg())
                .arg(project_arg())
                .arg(dry_run()),
        )
        .subcommand(
            Command::new("uninstall")
                .about("Remove the Russet MCP registration from an agent client")
                .arg(client_arg())
                .arg(scope_arg())
                .arg(project_arg())
                .arg(dry_run()),
        )
        .subcommand(Command::new("status").about("Show registrations and the stdio launch command"))
        .subcommand(
            Command::new("http")
                .about("Enable or disable the HTTP endpoint (W20)")
                .subcommand(Command::new("enable"))
                .subcommand(Command::new("disable")),
        )
}

/// Runs this area's command; `m` is the root matches, so `m.subcommand()` names the top-level `mcp` command.
pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    let sub = match m.subcommand() {
        Some(("mcp", s)) => s,
        _ => return Err(Error::invalid_argument("unknown mcp command")),
    };
    match sub.subcommand() {
        Some(("serve", a)) => serve(a).await,
        Some(("tools", a)) => tools(a, cx),
        Some(("install", a)) => install(a, cx).await,
        Some(("uninstall", a)) => uninstall(a, cx).await,
        Some(("status", _)) => status(cx).await,
        Some(("http", _)) => Err(Error::new(codes::core::NOT_IMPLEMENTED, "`russet mcp http` lands in W20")
            .with_hint("The HTTP endpoint and policy arrive in Wave 3.")),
        _ => Err(Error::invalid_argument("unknown mcp command")),
    }
}

async fn serve(a: &ArgMatches) -> Result<ExitCode> {
    if a.get_one::<String>("http").is_some() {
        return Err(Error::new(codes::core::NOT_IMPLEMENTED, "`--http` lands in W20")
            .with_hint("Use stdio for now; the HTTP endpoint arrives in Wave 3."));
    }
    let cfg = russet_mcp::McpConfig {
        client: a.get_one::<String>("client").cloned(),
        http: None,
        tier_max: a.get_one::<u8>("tier-max").copied(),
    };
    russet_mcp::serve_stdio(cfg).await.map_err(|e| Error::internal(e.to_string()))?;
    Ok(ExitCode::SUCCESS)
}

fn tools(a: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    let tier_max = a.get_one::<u8>("tier-max").copied();
    let value = russet_mcp::snapshot::catalog_json(tier_max);
    cx.output(&value, |v| {
        let mut lines = vec![format!("{:<24} tier  rpc", "tool")];
        if let Some(tools) = v["tools"].as_array() {
            for t in tools {
                let name = t["name"].as_str().unwrap_or("");
                let tier = t["tier"].as_u64().unwrap_or(0);
                let rpc = t["rpc"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|m| m.as_str()).collect::<Vec<_>>().join(", "))
                    .unwrap_or_default();
                lines.push(format!("{name:<24} {tier:<5} {rpc}"));
            }
        }
        lines.join("\n")
    })?;
    Ok(ExitCode::SUCCESS)
}

fn scope_of(a: &ArgMatches) -> Option<McpScope> {
    match a.get_one::<String>("scope").map(String::as_str) {
        Some("user") => Some(McpScope::User),
        Some("project") => Some(McpScope::Project),
        _ => None,
    }
}

/// The project directory for project scope: `--project`, else the global `--project`, else the current dir.
fn project_of(a: &ArgMatches, cx: &CliCtx) -> Option<String> {
    a.get_one::<String>("project").cloned().or_else(|| cx.global.project.as_ref().map(|p| p.display().to_string()))
}

async fn install(a: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    let params = McpInstallParams {
        client: a.get_one::<String>("client").cloned().unwrap_or_default(),
        scope: scope_of(a),
        project: project_of(a, cx),
        dry_run: a.get_flag("dry-run"),
        tier_max: None,
    };
    let r = cx.peer.call::<McpInstall>(params).await?;
    cx.output(&r, render_changes)?;
    Ok(ExitCode::SUCCESS)
}

async fn uninstall(a: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    let params = McpUninstallParams {
        client: a.get_one::<String>("client").cloned().unwrap_or_default(),
        scope: scope_of(a),
        project: project_of(a, cx),
        dry_run: a.get_flag("dry-run"),
    };
    let r = cx.peer.call::<McpUninstall>(params).await?;
    cx.output(&r, render_changes)?;
    Ok(ExitCode::SUCCESS)
}

fn render_changes(r: &russet_protocol::mcp::McpInstallResult) -> String {
    if r.changes.is_empty() {
        return "already in the desired state; no changes".to_owned();
    }
    let verb = if r.applied { "wrote" } else { "would change" };
    r.changes
        .iter()
        .map(|c| match &c.path {
            Some(p) => format!("{verb} {p}"),
            None => format!("{verb} (via the client's own CLI)"),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

async fn status(cx: &CliCtx) -> Result<ExitCode> {
    let r = cx.peer.call::<McpStatus>(McpStatusParams {}).await?;
    cx.output(&r, |r| {
        let mut lines = vec![format!("stdio: {} {}", r.stdio.command, r.stdio.args.join(" "))];
        for c in &r.clients {
            lines.push(format!(
                "{:<12} installed={} registered={}{}",
                c.id,
                c.installed,
                c.registered,
                c.path.as_ref().map(|p| format!("  ({p})")).unwrap_or_default()
            ));
        }
        lines.join("\n")
    })?;
    Ok(ExitCode::SUCCESS)
}
