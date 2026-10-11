// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet secrets list|status|set|delete` (ARCHITECTURE §8.7, §12.2). Owner: W3.
//!
//! Nothing here prints a secret: the daemon returns metadata only. `secrets set <id> --from-file F` hands the
//! daemon a file path; it reads and seals the file. From a terminal without a TTY (an agent shell, a lab script)
//! it needs `--yes`, like the other local-file imports (ARCHITECTURE §7.5) [D 85]; the CLI's confirmation layer
//! (W2) applies it.

use std::path::Path;
use std::process::ExitCode;

use clap::{Arg, ArgMatches, Command};
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_protocol::secrets as p;

use super::config::report_as;
use crate::CliCtx;

const KINDS: [&str; 8] = [
    "asc-key",
    "signing-key",
    "apple-id-session",
    "apple-id-password",
    "anisette-state",
    "github-token",
    "mcp-http-token",
    "tcp-token",
];

/// The `secrets` command.
pub fn command() -> Command {
    Command::new("secrets")
        .about("The secret store: list, status, set, delete (metadata only; values never leave the daemon)")
        .subcommand_required(true)
        .subcommand(
            Command::new("list")
                .about("Secrets in the store (ids, labels, sizes; never values)")
                .arg(Arg::new("kind").long("kind").value_name("KIND").value_parser(KINDS)),
        )
        .subcommand(
            Command::new("status")
                .about("The backend (keyring, passphrase, env), whether it is unlocked, its keyring entry"),
        )
        .subcommand(
            Command::new("set")
                .about("Seal the content of a local file as a secret (needs --yes without a TTY)")
                .arg(Arg::new("id").value_name("KIND/NAME").required(true).help("For example github-token/me"))
                .arg(Arg::new("from-file").long("from-file").value_name("FILE").required(true))
                .arg(Arg::new("label").long("label").value_name("LABEL")),
        )
        .subcommand(
            Command::new("delete").about("Delete a secret").arg(Arg::new("id").value_name("KIND/NAME").required(true)),
        )
}

/// One `russet secrets` invocation, as RPC params.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SecretsRequest {
    List(p::SecretsListParams),
    Status,
    Set(p::SecretsSetParams),
    Delete(p::SecretsDeleteParams),
}

fn kind(s: &str) -> Option<p::SecretKind> {
    serde_json::from_value(serde_json::Value::String(s.to_owned())).ok()
}

/// Reads the invocation from the root matches; `cwd` makes `--from-file` absolute.
pub(crate) fn request(m: &ArgMatches, cwd: &Path) -> Result<SecretsRequest> {
    let Some(("secrets", m)) = m.subcommand() else {
        return Err(Error::new(codes::core::INVALID_ARGUMENT, "not a secrets command")
            .with_hint("Run `russet secrets --help`."));
    };
    Ok(match m.subcommand() {
        Some(("list", a)) => {
            SecretsRequest::List(p::SecretsListParams { kind: a.get_one::<String>("kind").and_then(|k| kind(k)) })
        }
        Some(("status", _)) => SecretsRequest::Status,
        Some(("set", a)) => {
            let file = a.get_one::<String>("from-file").cloned().unwrap_or_default();
            let path = Path::new(&file);
            let path = if path.is_absolute() { path.to_path_buf() } else { cwd.join(path) };
            SecretsRequest::Set(p::SecretsSetParams {
                id: a.get_one::<String>("id").cloned().unwrap_or_default(),
                label: a.get_one::<String>("label").cloned(),
                from_file: path.to_string_lossy().into_owned(),
            })
        }
        Some(("delete", a)) => SecretsRequest::Delete(p::SecretsDeleteParams {
            id: a.get_one::<String>("id").cloned().unwrap_or_default(),
        }),
        _ => {
            return Err(Error::new(codes::core::INVALID_ARGUMENT, "unknown secrets subcommand")
                .with_hint("Run `russet secrets --help`."));
        }
    })
}

/// Runs this area's command; `m` is the root matches, so `m.subcommand()` names the command.
pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    let cwd = std::env::current_dir().map_err(|e| {
        Error::new(codes::core::IO, format!("cannot read the current directory: {e}")).with_hint("cd somewhere else.")
    })?;
    match request(m, &cwd)? {
        SecretsRequest::List(params) => report_as(cx, &cx.peer.call::<p::SecretsList>(params).await?, |r| {
            if r.items.is_empty() {
                "no secrets".to_owned()
            } else {
                r.items.iter().map(meta_line).collect::<Vec<_>>().join("\n")
            }
        })?,
        SecretsRequest::Status => {
            report_as(cx, &cx.peer.call::<p::SecretsStatus>(p::SecretsStatusParams {}).await?, status_line)?;
        }
        SecretsRequest::Set(params) => {
            report_as(cx, &cx.peer.call::<p::SecretsSet>(params).await?, |m| format!("sealed {}", meta_line(m)))?;
        }
        SecretsRequest::Delete(params) => {
            let id = params.id.clone();
            report_as(cx, &cx.peer.call::<p::SecretsDelete>(params).await?, |r| {
                if r.deleted { format!("deleted {id}") } else { format!("no secret {id}") }
            })?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn backend_name(b: p::SecretBackend) -> String {
    serde_json::to_value(b).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default()
}

/// A secret's metadata for humans (never its value).
fn meta_line(m: &p::SecretMeta) -> String {
    format!("{}  \"{}\"  {} bytes  {}  {}", m.id, m.label, m.size_bytes, backend_name(m.backend), m.created_at)
}

/// `secrets status` for humans.
fn status_line(s: &p::SecretsStatusResult) -> String {
    let state = match (s.available, s.locked) {
        (false, _) => "not available",
        (true, true) => "locked",
        (true, false) => "available",
    };
    let mut out = format!("{} backend: {state}", backend_name(s.backend));
    if let Some(r) = &s.reason {
        out.push_str(&format!(" ({r})"));
    }
    if let Some(e) = &s.keyring_entry {
        out.push_str(&format!("; keyring entry {e} (service dev.russet)"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> SecretsRequest {
        let m = crate::root_command().try_get_matches_from(args).expect("parses");
        request(&m, Path::new("/work")).expect("request")
    }

    #[test]
    fn invocations_become_params() {
        assert_eq!(parse(&["russet", "secrets", "status"]), SecretsRequest::Status);
        assert_eq!(
            parse(&["russet", "secrets", "list", "--kind", "asc-key"]),
            SecretsRequest::List(p::SecretsListParams { kind: Some(p::SecretKind::AscKey) })
        );
        assert_eq!(
            parse(&["russet", "--yes", "secrets", "set", "github-token/me", "--from-file", "token.txt"]),
            SecretsRequest::Set(p::SecretsSetParams {
                id: "github-token/me".into(),
                label: None,
                from_file: "/work/token.txt".into()
            })
        );
        assert!(
            crate::root_command().try_get_matches_from(["russet", "secrets", "set", "x/y"]).is_err(),
            "--from-file is required"
        );
        assert!(crate::root_command().try_get_matches_from(["russet", "secrets", "list", "--kind", "pw"]).is_err());
    }
}
