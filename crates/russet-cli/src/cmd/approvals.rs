// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet approvals`, `russet approve`, `russet deny` (ARCHITECTURE §8.7, §11.3). Owner: W2.
//!
//! Approving and denying need a terminal: the CLI declares `interactive` only when stdin is a TTY, and the daemon
//! refuses grants from anything but Russet IDE and interactive CLIs.

use std::process::ExitCode;

use clap::{Arg, ArgMatches, Command};
use russet_core::error::{Error, Result};
use russet_core::ids::ApprovalId;
use russet_protocol::approval::{
    Approval, ApprovalDeny, ApprovalDenyParams, ApprovalGrant, ApprovalGrantParams, ApprovalList, ApprovalListParams,
    ApprovalListResult, ApprovalScope, ApprovalState, ApprovalSubject,
};

use crate::CliCtx;

/// This area's top-level commands, as a [`super::group`].
pub fn command() -> Command {
    let id = || Arg::new("id").value_name("APPROVAL_ID").required(true).help("The approval, apr_…");
    super::group(
        "approvals",
        [
            Command::new("approvals").about("Approvals requested by agents").subcommand(
                Command::new("list").about("List approvals").arg(
                    Arg::new("state")
                        .long("state")
                        .value_name("STATE")
                        .value_parser(["pending", "granted", "denied", "expired", "used"])
                        .help("Only approvals in this state"),
                ),
            ),
            Command::new("approve").about("Grant an approval (in a terminal only)").arg(id()).arg(
                Arg::new("scope")
                    .long("scope")
                    .value_name("SCOPE")
                    .value_parser(["once", "session"])
                    .help("once (tier 3, default) or session (tier 2 default)"),
            ),
            Command::new("deny")
                .about("Deny an approval (in a terminal only)")
                .arg(id())
                .arg(Arg::new("reason").long("reason").value_name("TEXT").help("Why")),
        ],
    )
}

fn state_of(s: &str) -> Option<ApprovalState> {
    match s {
        "pending" => Some(ApprovalState::Pending),
        "granted" => Some(ApprovalState::Granted),
        "denied" => Some(ApprovalState::Denied),
        "expired" => Some(ApprovalState::Expired),
        "used" => Some(ApprovalState::Used),
        _ => None,
    }
}

/// One approval as a line.
pub fn human_approval(a: &Approval) -> String {
    let what = match &a.subject {
        ApprovalSubject::Method { method, .. } => method.clone(),
        ApprovalSubject::Plan { plan_id, .. } => format!("plan {plan_id}"),
    };
    format!(
        "{}  {:<8} tier {}  {}  from {} ({})  expires {}",
        a.approval_id,
        russetd::approvals::state_name(a.state),
        a.tier,
        what,
        a.requested_by.name,
        a.requested_by.connection_id,
        a.expires_at
    )
}

fn parse_id(m: &ArgMatches) -> Result<ApprovalId> {
    let raw = m.get_one::<String>("id").ok_or_else(|| Error::invalid_argument("an approval id is required"))?;
    ApprovalId::parse(raw)
}

/// Runs this area's command; `m` is the root matches, so `m.subcommand()` names the command.
pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    match m.subcommand() {
        Some(("approvals", a)) => {
            let state = match a.subcommand() {
                Some(("list", l)) => l.get_one::<String>("state").and_then(|s| state_of(s)),
                _ => None,
            };
            let r: ApprovalListResult = cx.peer.call::<ApprovalList>(ApprovalListParams { state }).await?;
            cx.output(&r, |r| {
                if r.approvals.is_empty() {
                    "no approvals".to_owned()
                } else {
                    r.approvals.iter().map(human_approval).collect::<Vec<_>>().join("\n")
                }
            })?;
        }
        Some(("approve", a)) => {
            let scope = match a.get_one::<String>("scope").map(String::as_str) {
                Some("once") => Some(ApprovalScope::Once),
                Some("session") => Some(ApprovalScope::Session),
                _ => None,
            };
            let r = cx.peer.call::<ApprovalGrant>(ApprovalGrantParams { approval_id: parse_id(a)?, scope }).await?;
            cx.output(&r, |a| format!("granted: {}", human_approval(a)))?;
        }
        Some(("deny", a)) => {
            let reason = a.get_one::<String>("reason").cloned();
            let r = cx.peer.call::<ApprovalDeny>(ApprovalDenyParams { approval_id: parse_id(a)?, reason }).await?;
            cx.output(&r, |a| format!("denied: {}", human_approval(a)))?;
        }
        _ => return Err(Error::invalid_argument("unknown approvals command")),
    }
    Ok(ExitCode::SUCCESS)
}
