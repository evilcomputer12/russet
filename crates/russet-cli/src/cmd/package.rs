// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet package` (ARCHITECTURE §8.7). Owner: W5.
//!
//! ```text
//! russet package ipa <App.app> -o <out.ipa> [--store]
//! ```
//!
//! Packing an existing bundle needs no daemon state, and no RPC method carries it yet (a contract change adding
//! one is requested), so this runs in-process through russetd's build handler module, with russet-ipa's
//! deterministic writer.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Arg, ArgAction, ArgMatches, Command, value_parser};
use russet_core::Result;

use super::build::absolute;
use crate::CliCtx;

/// The `package` command.
pub fn command() -> Command {
    Command::new("package").about("Package an app").subcommand_required(true).subcommand(
        Command::new("ipa")
            .about("Pack an .app into a deterministic IPA")
            .arg(
                Arg::new("app")
                    .required(true)
                    .value_name("APP")
                    .value_parser(value_parser!(PathBuf))
                    .help("The .app bundle"),
            )
            .arg(
                Arg::new("output")
                    .short('o')
                    .long("output")
                    .required(true)
                    .value_name("IPA")
                    .value_parser(value_parser!(PathBuf))
                    .help("Where to write the IPA"),
            )
            .arg(
                Arg::new("store").long("store").action(ArgAction::SetTrue).help("Deflate entries (App Store uploads)"),
            ),
    )
}

/// `{path, entries, bytes, sha256}`.
pub fn package(app: &str, out: &str, store: bool) -> Result<serde_json::Value> {
    let r = russetd::handlers::build::package_ipa(app, out, store)?;
    Ok(serde_json::json!({"path": r.path.as_str(), "entries": r.entries, "bytes": r.bytes, "sha256": r.sha256}))
}

/// Runs this area's command; `m` is the root matches, so `m.subcommand()` names the command.
pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    let Some(("package", pm)) = m.subcommand() else { return super::not_implemented(m) };
    let Some(("ipa", sub)) = pm.subcommand() else { return super::not_implemented(m) };
    let app = absolute(sub.get_one::<PathBuf>("app").map(PathBuf::as_path))?;
    let out = absolute(sub.get_one::<PathBuf>("output").map(PathBuf::as_path))?;
    let value = package(&app, &out, sub.get_flag("store"))?;
    cx.reporter.result(&value);
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn package_ipa_parses_and_packs() {
        let m = crate::root_command()
            .try_get_matches_from(["russet", "package", "ipa", "A.app", "-o", "A.ipa", "--store"])
            .unwrap();
        let (_, pm) = m.subcommand().unwrap();
        let (_, sub) = pm.subcommand().unwrap();
        assert!(sub.get_flag("store"));
        let dir = std::env::temp_dir().join(format!("russet-cli-package-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let app = dir.join("A.app");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(app.join("Info.plist"), b"<plist/>").unwrap();
        let out = dir.join("A.ipa");
        let v = package(app.to_str().unwrap(), out.to_str().unwrap(), false).unwrap();
        assert_eq!(v["entries"], 3);
        assert!(out.is_file());
        let err = package(dir.join("missing.app").to_str().unwrap(), out.to_str().unwrap(), false).unwrap_err();
        assert_eq!(err.code(), russet_core::error::codes::build::PACKAGE_FAILED);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
