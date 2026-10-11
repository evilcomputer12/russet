// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet account` (ARCHITECTURE §8.7). Owner: W6 (Wave 1), W12 (Wave 2), W22 (login, anisette; Wave 3).
//!
//! Wave 1: `list` (accounts and signing identities), `verify`, `add-key`, `import --from-xtool`, and the
//! read-only portal listings `certs|devices|bundle-ids|profiles list`. `import` and `add-key` are local-file
//! imports: from a shell without a TTY they run with `--yes`, and without it the daemon answers
//! `perm.confirmationUnavailable` (exit 11) (ARCHITECTURE §7.5 [D 85]). `default`, `remove`, `login` and
//! `anisette` arrive with W12 and W22.

use std::path::Path;
use std::process::ExitCode;

use clap::{Arg, ArgAction, ArgMatches, Command};
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_protocol::account as pa;
use russet_protocol::identity as pi;
use russet_protocol::portal as pp;

use crate::CliCtx;

/// The `account` command.
pub fn command() -> Command {
    let list_only = |name: &'static str, about: &'static str| {
        Command::new(name).about(about).subcommand_required(true).subcommand(
            Command::new("list")
                .about("List them (read-only; GET-scoped App Store Connect tokens)")
                .arg(
                    Arg::new("account").long("account").value_name("ID").help("The account (default: the default one)"),
                )
                .arg(
                    Arg::new("filter")
                        .long("filter")
                        .value_name("VALUE")
                        .help("Certificate type, bundle-ID prefix or profile type"),
                ),
        )
    };
    Command::new("account")
        .about("Accounts: list, default, remove, verify, add-key, import --from-xtool, login, anisette; certs, devices, bundle-ids, profiles")
        .subcommand_required(true)
        .subcommand(Command::new("list").about("List accounts and signing identities"))
        .subcommand(
            Command::new("verify")
                .about("Check an account against App Store Connect (read-only) and discover its team")
                .arg(Arg::new("account").value_name("ACCOUNT-ID").help("asc:<KEYID>; default: the default account")),
        )
        .subcommand(
            Command::new("add-key")
                .about("Add an App Store Connect API key (the .p8 is sealed in the secret store)")
                .arg(Arg::new("key-id").long("key-id").value_name("ID").required(true))
                .arg(Arg::new("issuer-id").long("issuer-id").value_name("UUID").conflicts_with("individual"))
                .arg(Arg::new("individual").long("individual").action(ArgAction::SetTrue).help("An individual key (no issuer)"))
                .arg(Arg::new("p8").long("p8").value_name("PATH").required(true).help("The AuthKey_<KEYID>.p8 file"))
                .arg(Arg::new("label").long("label").value_name("TEXT")),
        )
        .subcommand(
            Command::new("import")
                .about("Import xtool's App Store Connect key and development identity, once [D 50]")
                .arg(Arg::new("from-xtool").long("from-xtool").action(ArgAction::SetTrue).required(true))
                .arg(Arg::new("dry-run").long("dry-run").action(ArgAction::SetTrue).help("Show what would be imported")),
        )
        .subcommand(list_only("certs", "Certificates of the team"))
        .subcommand(list_only("devices", "Registered devices (UDIDs masked)"))
        .subcommand(list_only("bundle-ids", "Registered bundle IDs"))
        .subcommand(list_only("profiles", "Provisioning profiles"))
        .subcommand(stub("default", "Set the default account (W12)"))
        .subcommand(stub("remove", "Remove an account (W12)"))
        .subcommand(stub("login", "Log in with an Apple ID (W22)"))
        .subcommand(stub("anisette", "Anisette status, fetch-libraries, reset (W22)"))
}

fn stub(name: &'static str, about: &'static str) -> Command {
    Command::new(name)
        .about(about)
        .arg(Arg::new("args").num_args(0..).trailing_var_arg(true).allow_hyphen_values(true).hide(true))
}

/// A path argument made absolute against the working directory (the wire takes absolute paths).
pub(crate) fn absolute_path(p: &str) -> Result<String> {
    let path = Path::new(p);
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| Error::new(codes::core::IO, format!("cannot read the working directory: {e}")))?
            .join(path)
    };
    Ok(abs.display().to_string())
}

/// What one `russet account …` invocation asks the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountCall {
    List,
    Verify(pa::AccountVerifyParams),
    AddKey(pa::AccountAddAscKeyParams),
    ImportXtool(pa::AccountImportXtoolParams),
    Certs(pp::PortalListParams),
    Devices(pp::PortalListParams),
    BundleIds(pp::PortalListParams),
    Profiles(pp::PortalListParams),
    Later(&'static str),
}

/// Maps the `account` matches to a daemon call.
pub fn call(m: &ArgMatches) -> Result<AccountCall> {
    let list_params = |sub: &ArgMatches| -> pp::PortalListParams {
        let l = sub.subcommand_matches("list");
        pp::PortalListParams {
            account_id: l.and_then(|l| l.get_one::<String>("account").cloned()),
            filter: l.and_then(|l| l.get_one::<String>("filter").cloned()),
            reveal: None,
        }
    };
    Ok(match m.subcommand() {
        Some(("list", _)) => AccountCall::List,
        Some(("verify", s)) => {
            AccountCall::Verify(pa::AccountVerifyParams { account_id: s.get_one::<String>("account").cloned() })
        }
        Some(("add-key", s)) => {
            let issuer = s.get_one::<String>("issuer-id").cloned();
            if issuer.is_none() && !s.get_flag("individual") {
                return Err(Error::new(
                    codes::core::INVALID_ARGUMENT,
                    "pass --issuer-id <UUID> for a team key, or --individual",
                )
                .with_hint("The issuer ID is on the Integrations tab of App Store Connect."));
            }
            AccountCall::AddKey(pa::AccountAddAscKeyParams {
                key_id: s.get_one::<String>("key-id").cloned().unwrap_or_default(),
                issuer_id: issuer,
                p8_path: absolute_path(s.get_one::<String>("p8").map(String::as_str).unwrap_or_default())?,
                label: s.get_one::<String>("label").cloned(),
            })
        }
        Some(("import", s)) => {
            AccountCall::ImportXtool(pa::AccountImportXtoolParams { dry_run: s.get_flag("dry-run").then_some(true) })
        }
        Some(("certs", s)) => AccountCall::Certs(list_params(s)),
        Some(("devices", s)) => AccountCall::Devices(list_params(s)),
        Some(("bundle-ids", s)) => AccountCall::BundleIds(list_params(s)),
        Some(("profiles", s)) => AccountCall::Profiles(list_params(s)),
        Some(("default" | "remove", _)) => AccountCall::Later("W12"),
        Some(("login" | "anisette", _)) => AccountCall::Later("W22"),
        _ => {
            return Err(Error::new(codes::core::INVALID_ARGUMENT, "no account subcommand")
                .with_hint("Run `russet account --help`."));
        }
    })
}

/// Why `account verify` failed, as an error with the matching exit code (5).
pub fn verify_failure(r: &pa::AccountVerifyResult) -> Error {
    match r.role_probe.certificates {
        pa::ProbeOutcome::Unauthorized => {
            Error::new(codes::account::ASC_UNAUTHORIZED, "App Store Connect rejected the key")
                .with_hint("Check the key ID and issuer ID, and that the key was not revoked; sync the clock (NTP).")
        }
        pa::ProbeOutcome::Forbidden => Error::new(codes::account::ASC_FORBIDDEN, "the key cannot read certificates")
            .with_hint("Use a key with the App Manager or Admin role."),
        _ => Error::new(codes::account::APPLE_SERVICE_UNAVAILABLE, "App Store Connect could not be reached")
            .with_hint("Check the network; `russet doctor --online` tests the connection.")
            .retryable(true),
    }
}

fn json<T: serde::Serialize>(v: &T) -> Result<serde_json::Value> {
    serde_json::to_value(v).map_err(|e| Error::new(codes::core::INTERNAL, format!("cannot encode the result: {e}")))
}

/// Runs this area's command; `m` is the root matches, so `m.subcommand()` names the command.
pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    let Some(("account", sub)) = m.subcommand() else {
        return Err(Error::new(codes::core::INVALID_ARGUMENT, "not an account command"));
    };
    let value = match call(sub)? {
        AccountCall::List => {
            let accounts = cx.peer.call::<pa::AccountList>(pa::AccountListParams {}).await?;
            let identities = cx.peer.call::<pi::IdentityList>(pi::IdentityListParams::default()).await?;
            let mut v = json(&accounts)?;
            v["identities"] = json(&identities.identities)?;
            v
        }
        AccountCall::Verify(p) => {
            let r = cx.peer.call::<pa::AccountVerify>(p).await?;
            let v = json(&r)?;
            if !r.ok {
                cx.reporter.result(&v);
                return Err(verify_failure(&r));
            }
            v
        }
        AccountCall::AddKey(p) => json(&cx.peer.call::<pa::AccountAddAscKey>(p).await?)?,
        AccountCall::ImportXtool(p) => json(&cx.peer.call::<pa::AccountImportXtool>(p).await?)?,
        AccountCall::Certs(p) => json(&cx.peer.call::<pp::PortalCertificatesList>(p).await?)?,
        AccountCall::Devices(p) => json(&cx.peer.call::<pp::PortalDevicesList>(p).await?)?,
        AccountCall::BundleIds(p) => json(&cx.peer.call::<pp::PortalBundleIdsList>(p).await?)?,
        AccountCall::Profiles(p) => json(&cx.peer.call::<pp::PortalProfilesList>(p).await?)?,
        AccountCall::Later(card) => {
            return Err(Error::new(codes::core::NOT_IMPLEMENTED, format!("this account command arrives with {card}"))
                .with_hint("Wave 1 has list, verify, add-key, import --from-xtool and the read-only listings."));
        }
    };
    cx.reporter.result(&value);
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn parse(args: &[&str]) -> Result<AccountCall> {
        let mut argv = vec!["account"];
        argv.extend_from_slice(args);
        call(&command().try_get_matches_from(argv).unwrap())
    }

    #[test]
    fn the_tree_is_valid() {
        command().debug_assert();
    }

    #[test]
    fn maps_subcommands_to_calls() {
        assert_eq!(parse(&["list"]).unwrap(), AccountCall::List);
        assert_eq!(
            parse(&["import", "--from-xtool", "--dry-run"]).unwrap(),
            AccountCall::ImportXtool(pa::AccountImportXtoolParams { dry_run: Some(true) })
        );
        assert_eq!(
            parse(&["import", "--from-xtool"]).unwrap(),
            AccountCall::ImportXtool(pa::AccountImportXtoolParams { dry_run: None })
        );
        assert!(command().try_get_matches_from(["account", "import"]).is_err(), "--from-xtool is required");
        assert_eq!(
            parse(&["verify", "asc:K"]).unwrap(),
            AccountCall::Verify(pa::AccountVerifyParams { account_id: Some("asc:K".into()) })
        );
        match parse(&["add-key", "--key-id", "K", "--issuer-id", "U", "--p8", "AuthKey.p8"]).unwrap() {
            AccountCall::AddKey(p) => {
                assert_eq!(p.issuer_id.as_deref(), Some("U"));
                assert!(Path::new(&p.p8_path).is_absolute(), "paths are sent absolute");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            parse(&["add-key", "--key-id", "K", "--p8", "x.p8"]).unwrap_err().code(),
            codes::core::INVALID_ARGUMENT
        );
        assert!(
            matches!(parse(&["add-key", "--key-id", "K", "--individual", "--p8", "x.p8"]).unwrap(), AccountCall::AddKey(p) if p.issuer_id.is_none())
        );
        match parse(&["certs", "list", "--filter", "DEVELOPMENT"]).unwrap() {
            AccountCall::Certs(p) => assert_eq!(p.filter.as_deref(), Some("DEVELOPMENT")),
            other => panic!("{other:?}"),
        }
        assert_eq!(parse(&["login", "--apple-id", "x"]).unwrap(), AccountCall::Later("W22"));
        assert_eq!(parse(&["default", "asc:K"]).unwrap(), AccountCall::Later("W12"));
    }

    #[test]
    fn verify_failures_map_to_account_codes() {
        let r = |c| pa::AccountVerifyResult {
            ok: false,
            account_id: "asc:K".into(),
            team_id: None,
            role_probe: pa::RoleProbe { certificates: c, profiles: c, builds: c },
        };
        assert_eq!(verify_failure(&r(pa::ProbeOutcome::Unauthorized)).code(), codes::account::ASC_UNAUTHORIZED);
        assert_eq!(verify_failure(&r(pa::ProbeOutcome::Forbidden)).code(), codes::account::ASC_FORBIDDEN);
        assert_eq!(verify_failure(&r(pa::ProbeOutcome::Error)).code(), codes::account::APPLE_SERVICE_UNAVAILABLE);
        assert_eq!(verify_failure(&r(pa::ProbeOutcome::Error)).code().exit_code(), 5);
    }
}
