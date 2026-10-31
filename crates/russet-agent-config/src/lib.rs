// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Writes MCP registrations for the agent clients (plan, apply, detect; IDE §6.6, ARCHITECTURE §11.7).
//!
//! The crate is transport-agnostic: it computes the file changes that register the Russet MCP server with an
//! agent client, applies them idempotently with a backup, and reports what it detects. The CLI and the daemon
//! (`handlers/mcp.rs`) drive it; it never talks to the daemon itself.
//!
//! **Scope (W13, MCP-04 partial).** [`AgentClient::ClaudeCode`], [`AgentClient::Codex`],
//! [`AgentClient::Gemini`] and [`AgentClient::VsCode`] are implemented; the other variants of IDE §6.6 parse
//! and [`detect`] but [`plan`] returns [`PlanError::Unsupported`] for them until their wave (W20, SPK-03).
//!
//! **Mechanism.** Registration is file-based: comment-preserving JSON(C) edits (`jsonc-parser` CST), TOML
//! through `toml_edit`. The client's own `mcp add` CLI is reported in [`Change::via_cli`] for reference, but
//! [`apply`] writes the file so the operation is deterministic and offline. Every write backs the original up
//! to `<file>.russet-bak` first and is atomic (write a temp file in the same directory, then rename).

// Tests use unwrap/expect freely (the repo convention for test code).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::path::PathBuf;

mod clients;
mod edit;

pub use clients::DetectedState;

/// An agent client Russet can register its MCP server with (IDE §6.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentClient {
    ClaudeCode,
    Codex,
    Gemini,
    CopilotCli,
    VsCode,
    Cline,
    Continue,
    OpenHands,
    OpenCode,
}

impl AgentClient {
    /// Every client, in the order of IDE §6.6.
    pub const ALL: &'static [AgentClient] = &[
        AgentClient::ClaudeCode,
        AgentClient::Codex,
        AgentClient::Gemini,
        AgentClient::CopilotCli,
        AgentClient::VsCode,
        AgentClient::Cline,
        AgentClient::Continue,
        AgentClient::OpenHands,
        AgentClient::OpenCode,
    ];

    /// The stable client id written into the registration's `--client` argument (ARCHITECTURE §11.2: the
    /// trusted identity).
    pub fn id(self) -> &'static str {
        match self {
            AgentClient::ClaudeCode => "claude-code",
            AgentClient::Codex => "codex",
            AgentClient::Gemini => "gemini",
            AgentClient::CopilotCli => "copilot-cli",
            AgentClient::VsCode => "vscode",
            AgentClient::Cline => "cline",
            AgentClient::Continue => "continue",
            AgentClient::OpenHands => "openhands",
            AgentClient::OpenCode => "opencode",
        }
    }

    /// The client of this id, if any.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|c| c.id() == id)
    }

    /// Whether W13 implements registration for this client (MCP-04 partial).
    pub fn is_supported(self) -> bool {
        matches!(self, AgentClient::ClaudeCode | AgentClient::Codex | AgentClient::Gemini | AgentClient::VsCode)
    }

    /// A human-readable name.
    pub fn display_name(self) -> &'static str {
        match self {
            AgentClient::ClaudeCode => "Claude Code",
            AgentClient::Codex => "Codex CLI",
            AgentClient::Gemini => "Gemini CLI",
            AgentClient::CopilotCli => "GitHub Copilot CLI",
            AgentClient::VsCode => "VS Code",
            AgentClient::Cline => "Cline",
            AgentClient::Continue => "Continue",
            AgentClient::OpenHands => "OpenHands",
            AgentClient::OpenCode => "OpenCode",
        }
    }
}

/// Where a registration is written (IDE §6.6). `User` is the per-user config; `Project` is a file committed in
/// a repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    User,
    Project(PathBuf),
}

impl Scope {
    /// The wire name (`user` / `project`).
    pub fn as_str(&self) -> &'static str {
        match self {
            Scope::User => "user",
            Scope::Project(_) => "project",
        }
    }
}

/// A planned MCP-server registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registration {
    pub client: AgentClient,
    pub scope: Scope,
    /// The MCP server name clients key the entry by (always `russet`).
    pub server_name: String,
    /// The executable the client should run: the stable shim in [`Scope::User`], `russet` from PATH in
    /// [`Scope::Project`] (committed files, ARCHITECTURE §11.7).
    pub command: PathBuf,
    /// The arguments after the command (`mcp serve --client <id>`).
    pub args: Vec<String>,
}

impl Registration {
    /// A stdio registration of the Russet MCP server: server name `russet`, args `mcp serve --client <id>`.
    pub fn stdio(client: AgentClient, scope: Scope, command: impl Into<PathBuf>) -> Self {
        let args = vec!["mcp".to_owned(), "serve".to_owned(), "--client".to_owned(), client.id().to_owned()];
        Self { client, scope, server_name: "russet".to_owned(), command: command.into(), args }
    }
}

/// One change [`apply`] would make: a file write, described by its path and the before/after text, plus the
/// client's own CLI command for reference ([`Change::via_cli`], IDE §6.6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// The file the change writes, or `None` for a CLI-only mechanism.
    pub path: Option<PathBuf>,
    /// The client's own `mcp add` command, when it has one (informational; [`apply`] edits the file instead).
    pub via_cli: Option<Vec<String>>,
    /// The file's current content, or `None` when it does not exist yet.
    pub before: Option<String>,
    /// The file's content after the change.
    pub after: String,
}

/// Why a registration could not be planned.
#[derive(Debug)]
pub enum PlanError {
    /// This client's registration is not implemented yet (MCP-04 partial, W13).
    Unsupported(AgentClient),
    /// The scope does not apply to this client (for example VS Code user scope, which is the extension API,
    /// IDE §6.9).
    ScopeUnsupported { client: AgentClient, scope: &'static str, hint: &'static str },
    /// The home or config directory could not be resolved.
    NoHome,
    /// Reading or parsing an existing file failed.
    Io(String),
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanError::Unsupported(c) => {
                write!(f, "registering {} is not implemented yet", c.display_name())
            }
            PlanError::ScopeUnsupported { client, scope, hint } => {
                write!(f, "{} does not support {scope} scope: {hint}", client.display_name())
            }
            PlanError::NoHome => f.write_str("cannot resolve the user's home directory"),
            PlanError::Io(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for PlanError {}

/// The changes that would register `reg` (a dry run). An empty vector means the registration is already in
/// place (idempotent). The client's own CLI is noted in each [`Change::via_cli`].
pub fn plan(reg: &Registration) -> Result<Vec<Change>, PlanError> {
    if !reg.client.is_supported() {
        return Err(PlanError::Unsupported(reg.client));
    }
    clients::plan_install(reg)
}

/// The changes that would remove `reg`'s registration (a dry run). Empty when nothing is registered.
pub fn plan_uninstall(reg: &Registration) -> Result<Vec<Change>, PlanError> {
    if !reg.client.is_supported() {
        return Err(PlanError::Unsupported(reg.client));
    }
    clients::plan_uninstall(reg)
}

/// Applies `changes`: for each file change, backs the original up to `<file>.russet-bak` and writes the new
/// content atomically (a temp file in the same directory, then rename).
pub fn apply(changes: &[Change]) -> Result<(), PlanError> {
    for change in changes {
        let Some(path) = &change.path else { continue };
        edit::write_atomic_with_backup(path, change.before.as_deref(), &change.after)
            .map_err(|e| PlanError::Io(format!("writing {}: {e}", path.display())))?;
    }
    Ok(())
}

/// Detects, for every client of IDE §6.6, whether it is installed and whether Russet is registered, and where.
pub fn detect() -> Vec<(AgentClient, DetectedState)> {
    AgentClient::ALL.iter().map(|&c| (c, clients::detect_one(c))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        for &c in AgentClient::ALL {
            assert_eq!(AgentClient::from_id(c.id()), Some(c));
        }
        assert_eq!(AgentClient::from_id("nope"), None);
    }

    #[test]
    fn supported_set_is_mcp04_partial() {
        let supported: Vec<_> = AgentClient::ALL.iter().filter(|c| c.is_supported()).map(|c| c.id()).collect();
        assert_eq!(supported, ["claude-code", "codex", "gemini", "vscode"]);
    }

    #[test]
    fn registration_args_carry_the_client_id() {
        let reg = Registration::stdio(AgentClient::Codex, Scope::User, "/shim/russet");
        assert_eq!(reg.args, ["mcp", "serve", "--client", "codex"]);
        assert_eq!(reg.server_name, "russet");
    }
}
