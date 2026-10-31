// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The `mcp.*` administration handlers in-process (ARCHITECTURE §8.6, §11.7; W13): `mcp.install`,
//! `mcp.uninstall` and `mcp.status` over `russet-agent-config`. The file-editing logic is tested in
//! `russet-agent-config`; these exercise the daemon wiring, the DTO mapping and the error paths.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "daemon_support.rs"]
mod support;

use russet_core::error::codes;
use russet_core::fsx::TempDir;
use russet_protocol::mcp::{
    McpInstall, McpInstallParams, McpScope, McpStatus, McpStatusParams, McpUninstall, McpUninstallParams,
};
use support::{Home, start};

#[tokio::test]
async fn install_then_status_then_uninstall_project_scope() {
    let home = Home::new("mcp-install");
    let (_daemon, cli) = start(home.options()).await;
    let proj = TempDir::new("mcp-proj").unwrap();
    let proj_dir = proj.path().display().to_string();

    // Dry run plans but writes nothing.
    let dry = cli
        .call::<McpInstall>(McpInstallParams {
            client: "claude-code".into(),
            scope: Some(McpScope::Project),
            project: Some(proj_dir.clone()),
            dry_run: true,
            tier_max: None,
        })
        .await
        .expect("dry-run install");
    assert!(!dry.applied);
    assert_eq!(dry.changes.len(), 1);
    assert!(!proj.path().join(".mcp.json").exists(), "dry run wrote nothing");

    // Real install writes the file.
    let done = cli
        .call::<McpInstall>(McpInstallParams {
            client: "claude-code".into(),
            scope: Some(McpScope::Project),
            project: Some(proj_dir.clone()),
            dry_run: false,
            tier_max: None,
        })
        .await
        .expect("install");
    assert!(done.applied);
    let text = std::fs::read_to_string(proj.path().join(".mcp.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["mcpServers"]["russet"]["command"], "russet");
    assert_eq!(v["mcpServers"]["russet"]["args"][3], "claude-code");

    // Idempotent: a second install changes nothing.
    let again = cli
        .call::<McpInstall>(McpInstallParams {
            client: "claude-code".into(),
            scope: Some(McpScope::Project),
            project: Some(proj_dir.clone()),
            dry_run: false,
            tier_max: None,
        })
        .await
        .expect("install again");
    assert!(again.changes.is_empty());
    assert!(!again.applied);

    // Status lists the four supported clients and the stdio launch command.
    let status = cli.call::<McpStatus>(McpStatusParams {}).await.expect("status");
    assert!(status.stdio.command.ends_with("russet") || status.stdio.command.ends_with("russet.exe"));
    assert_eq!(status.stdio.args, ["mcp", "serve"]);
    let ids: Vec<_> = status.clients.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["claude-code", "codex", "gemini", "vscode"]);
    assert!(status.http.is_none(), "HTTP is W20");

    // Uninstall removes it, and is idempotent.
    let removed = cli
        .call::<McpUninstall>(McpUninstallParams {
            client: "claude-code".into(),
            scope: Some(McpScope::Project),
            project: Some(proj_dir.clone()),
            dry_run: false,
        })
        .await
        .expect("uninstall");
    assert!(removed.applied);
    let text = std::fs::read_to_string(proj.path().join(".mcp.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(v["mcpServers"].get("russet").is_none());
}

#[tokio::test]
async fn unknown_client_is_rejected() {
    let home = Home::new("mcp-unknown");
    let (_daemon, cli) = start(home.options()).await;
    let err = cli
        .call::<McpInstall>(McpInstallParams { client: "nope".into(), ..Default::default() })
        .await
        .expect_err("unknown client");
    assert_eq!(err.code(), codes::mcp::CLIENT_UNKNOWN);
}

#[tokio::test]
async fn unsupported_client_reports_its_code() {
    let home = Home::new("mcp-unsupported");
    let (_daemon, cli) = start(home.options()).await;
    // `cline` is a known client id but W13 does not implement its registration.
    let err = cli
        .call::<McpInstall>(McpInstallParams { client: "cline".into(), ..Default::default() })
        .await
        .expect_err("unsupported client");
    assert_eq!(err.code(), codes::mcp::CLIENT_UNSUPPORTED);
}

#[tokio::test]
async fn project_scope_without_a_directory_is_an_argument_error() {
    let home = Home::new("mcp-noproj");
    let (_daemon, cli) = start(home.options()).await;
    let err = cli
        .call::<McpInstall>(McpInstallParams {
            client: "gemini".into(),
            scope: Some(McpScope::Project),
            project: None,
            dry_run: true,
            tier_max: None,
        })
        .await
        .expect_err("needs a project dir");
    assert_eq!(err.code(), codes::core::INVALID_ARGUMENT);
}
