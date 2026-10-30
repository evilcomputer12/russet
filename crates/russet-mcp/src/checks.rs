// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The MCP doctor check (ARCHITECTURE §5.3: `russet-mcp::checks()`), surfaced through russetd's doctor seam.
//! It reports which agent clients are installed and whether Russet is registered with them (MCP-04), so the
//! environment-dependent registration feature has a doctor check (PLAN §0.10).

use std::sync::Arc;

use russet_agent_config::{AgentClient, detect};
use russet_doctor::{Category, Check, CheckCx, CheckResult, Requires, async_trait};

/// This crate's checks (just the registration check for now).
pub fn checks() -> Vec<Arc<dyn Check>> {
    vec![Arc::new(Registration)]
}

struct Registration;

#[async_trait]
impl Check for Registration {
    fn id(&self) -> &'static str {
        "mcp.registration"
    }

    fn title(&self) -> &'static str {
        "MCP agent registration"
    }

    fn category(&self) -> Category {
        Category::Config
    }

    fn requires(&self) -> Requires {
        Requires::default()
    }

    async fn run(&self, _cx: &CheckCx) -> CheckResult {
        let detected = detect();
        let supported: Vec<_> = detected.iter().filter(|(c, _)| c.is_supported()).collect();
        let installed: Vec<&AgentClient> = supported.iter().filter(|(_, s)| s.installed).map(|(c, _)| c).collect();
        let registered: Vec<&AgentClient> = supported.iter().filter(|(_, s)| s.registered).map(|(c, _)| c).collect();

        if installed.is_empty() {
            return CheckResult::skip("no supported agent client (Claude Code, Codex, Gemini, VS Code) detected")
                .with_hint("Install one, then run `russet mcp install --client <id>`.");
        }
        let names = |list: &[&AgentClient]| list.iter().map(|c| c.id()).collect::<Vec<_>>().join(", ");
        if registered.is_empty() {
            return CheckResult::warn(format!("installed: {}; none registered with Russet", names(&installed)))
                .with_hint("Run `russet mcp install --client <id>` to register the Russet MCP server.");
        }
        CheckResult::pass(format!("registered with {}", names(&registered)))
            .with_detail(format!("installed clients: {}", names(&installed)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_the_registration_check() {
        let ids: Vec<_> = checks().iter().map(|c| c.id()).collect();
        assert_eq!(ids, ["mcp.registration"]);
    }
}
