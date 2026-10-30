// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `mcp` handlers (§8.6). Owner: W13 (Wave 2), W20 (Wave 3).
//!
//! W13 implements `mcp.status`, `mcp.install` and `mcp.uninstall` over [`russet_agent_config`] (file-based
//! registration for Claude Code, Codex, Gemini and VS Code; MCP-04 partial). `mcp.http.set` and
//! `mcp.policy.{get,set}` stay `core.notImplemented` until W20.

use std::path::PathBuf;

use russet_agent_config::{AgentClient, PlanError, Registration, Scope, detect, plan, plan_uninstall};
use russet_core::error::{Error, Result, codes};
use russet_ipc::Router;
use russet_protocol::mcp as p;

use super::not_implemented;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::McpStatus, _, _>(|cx: DaemonCtx, _rq, params| async move { status(&cx, params) });
    r.request::<p::McpInstall, _, _>(|cx: DaemonCtx, _rq, params| async move { install(&cx, params) });
    r.request::<p::McpUninstall, _, _>(|cx: DaemonCtx, _rq, params| async move { uninstall(&cx, params) });
    // W20 (Wave 3): HTTP and policy.
    r.request::<p::McpHttpSet, _, _>(not_implemented::<p::McpHttpSet>);
    r.request::<p::McpPolicyGet, _, _>(not_implemented::<p::McpPolicyGet>);
    r.request::<p::McpPolicySet, _, _>(not_implemented::<p::McpPolicySet>);
}

/// The stable shim path (ARCHITECTURE §11.7): `<data>/bin/russet[.exe]`.
fn stable_shim(cx: &DaemonCtx) -> PathBuf {
    let exe = if cfg!(windows) { "russet.exe" } else { "russet" };
    cx.dirs.data.join("bin").join(exe)
}

/// `mcp.status`: how the stdio bridge is launched, and each supported client's registration state.
fn status(cx: &DaemonCtx, _params: p::McpStatusParams) -> Result<p::McpStatusResult> {
    let stdio = p::McpStdioInfo {
        command: stable_shim(cx).to_string_lossy().into_owned(),
        args: vec!["mcp".to_owned(), "serve".to_owned()],
    };
    let clients = detect()
        .into_iter()
        .filter(|(c, _)| c.is_supported())
        .map(|(c, state)| p::McpClientStatus {
            id: c.id().to_owned(),
            registered: state.registered,
            installed: state.installed,
            scope: state.registered.then_some(p::McpScope::User),
            path: state.path.map(|p| p.to_string_lossy().into_owned()),
        })
        .collect();
    // HTTP is W20; report it as absent for now.
    Ok(p::McpStatusResult { stdio, http: None, clients })
}

/// `mcp.install`: register the Russet MCP server with an agent client.
fn install(cx: &DaemonCtx, params: p::McpInstallParams) -> Result<p::McpInstallResult> {
    let reg = registration(cx, &params.client, params.scope, params.project.as_deref())?;
    let changes = plan(&reg).map_err(plan_error)?;
    apply_if_wanted(changes, params.dry_run)
}

/// `mcp.uninstall`: remove the registration.
fn uninstall(cx: &DaemonCtx, params: p::McpUninstallParams) -> Result<p::McpUninstallResult> {
    let reg = registration(cx, &params.client, params.scope, params.project.as_deref())?;
    let changes = plan_uninstall(&reg).map_err(plan_error)?;
    apply_if_wanted(changes, params.dry_run)
}

/// Builds the [`Registration`] for a client string and scope, choosing the command per ARCHITECTURE §11.7
/// (the stable shim in user scope, `russet` from PATH in project scope).
fn registration(
    cx: &DaemonCtx,
    client: &str,
    scope: Option<p::McpScope>,
    project: Option<&str>,
) -> Result<Registration> {
    let client = AgentClient::from_id(client).ok_or_else(|| {
        Error::new(codes::mcp::CLIENT_UNKNOWN, format!("unknown agent client `{client}`"))
            .with_hint("Clients: claude-code, codex, gemini, vscode (W13).")
    })?;
    if !client.is_supported() {
        return Err(Error::new(
            codes::mcp::CLIENT_UNSUPPORTED,
            format!("registering {} is not implemented yet", client.display_name()),
        )
        .with_hint("W13 supports claude-code, codex, gemini and vscode; the rest land in W20/SPK-03."));
    }
    let scope = match scope.unwrap_or(p::McpScope::User) {
        p::McpScope::User => Scope::User,
        p::McpScope::Project => {
            let dir = project.ok_or_else(|| {
                Error::new(codes::core::INVALID_ARGUMENT, "project scope needs a project directory")
                    .with_hint("Pass `project` (the repository root).")
            })?;
            Scope::Project(PathBuf::from(dir))
        }
    };
    let command: PathBuf = match &scope {
        Scope::User => stable_shim(cx),
        Scope::Project(_) => PathBuf::from("russet"),
    };
    Ok(Registration::stdio(client, scope, command))
}

/// Applies the changes unless `dry_run`, and renders the result.
fn apply_if_wanted(changes: Vec<russet_agent_config::Change>, dry_run: bool) -> Result<p::McpInstallResult> {
    let applied = if dry_run || changes.is_empty() {
        false
    } else {
        russet_agent_config::apply(&changes).map_err(plan_error)?;
        true
    };
    Ok(p::McpInstallResult { changes: changes.iter().map(to_wire).collect(), applied })
}

fn to_wire(c: &russet_agent_config::Change) -> p::McpChange {
    p::McpChange {
        path: c.path.as_ref().map(|p| p.to_string_lossy().into_owned()),
        via_cli: c.via_cli.clone(),
        before: c.before.clone(),
        after: c.after.clone(),
    }
}

/// Maps a [`PlanError`] to a Russet error.
fn plan_error(e: PlanError) -> Error {
    match e {
        PlanError::Unsupported(c) => Error::new(
            codes::mcp::CLIENT_UNSUPPORTED,
            format!("registering {} is not implemented yet", c.display_name()),
        )
        .with_hint("W13 supports claude-code, codex, gemini and vscode; the rest land in W20/SPK-03."),
        PlanError::ScopeUnsupported { client, scope, hint } => {
            Error::new(codes::core::INVALID_ARGUMENT, format!("{} has no {scope} scope", client.display_name()))
                .with_hint(hint)
        }
        PlanError::NoHome => Error::new(codes::mcp::REGISTRATION_FAILED, "cannot resolve the home directory")
            .with_hint("Set HOME (or USERPROFILE on Windows)."),
        PlanError::Io(m) => Error::new(codes::mcp::REGISTRATION_FAILED, m)
            .with_hint("Check the client's config file and directory permissions, then retry."),
    }
}
