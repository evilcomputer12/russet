// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet sdk list|import|remove` (ARCHITECTURE §8.7). Owner: W3 (`list`), W16b (`import`, `remove`), W33
//! (`device-support`, deferred). `import` and `remove` report `core.notImplemented` until W16b fills them.

use std::process::ExitCode;

use clap::{Arg, ArgMatches, Command};
use russet_core::Result;
use russet_protocol::sdk as p;

use super::config::report_as;
use super::toolchain::sdk_line;
use crate::CliCtx;

/// The `sdk` command.
pub fn command() -> Command {
    let stub_args = || Arg::new("args").num_args(0..).trailing_var_arg(true).allow_hyphen_values(true).hide(true);
    Command::new("sdk")
        .about("Darwin SDKs and DDIs: list, import, remove")
        .subcommand_required(true)
        .subcommand(Command::new("list").about("Russet's imported SDKs, then xtool bundles read in place"))
        .subcommand(
            Command::new("import")
                .about(
                    "Import an SDK and DDI from Xcode.xip, Xcode.app, a bundle, a .pkg, a .dmg or a DDI folder (W16b)",
                )
                .arg(stub_args()),
        )
        .subcommand(Command::new("remove").about("Remove an imported SDK (W16b)").arg(stub_args()))
}

/// Runs this area's command; `m` is the root matches, so `m.subcommand()` names the command.
pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    match m.subcommand().and_then(|(_, a)| a.subcommand_name()) {
        Some("list") => {
            report_as(cx, &cx.peer.call::<p::SdkList>(p::SdkListParams {}).await?, |r| {
                if r.sdks.is_empty() {
                    "no SDK; import one with `russet sdk import <Xcode_27.xip>`".to_owned()
                } else {
                    r.sdks.iter().map(sdk_line).collect::<Vec<_>>().join("\n")
                }
            })?;
            Ok(ExitCode::SUCCESS)
        }
        _ => super::not_implemented(m),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn subcommands_parse() {
        let m = crate::root_command().try_get_matches_from(["russet", "sdk", "list"]).expect("list");
        assert_eq!(m.subcommand().and_then(|(_, a)| a.subcommand_name()), Some("list"));
        assert!(crate::root_command().try_get_matches_from(["russet", "sdk", "import", "--ddi-only", "x.pkg"]).is_ok());
    }
}
