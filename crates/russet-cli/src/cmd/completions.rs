// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet completions <SHELL>` and `russet completions --man-dir <DIR>` (ARCHITECTURE §8.7; core §16.1:
//! completions for bash, zsh and fish, man pages). Owner: W2.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Arg, ArgMatches, Command};
use clap_complete::Shell;
use russet_core::error::{Error, Result};

use crate::CliCtx;

/// The `completions` command.
pub fn command() -> Command {
    Command::new("completions")
        .about("Generate shell completions or man pages")
        .arg(
            Arg::new("shell")
                .value_name("SHELL")
                .value_parser(clap::value_parser!(Shell))
                .help("bash, zsh, fish, elvish or powershell"),
        )
        .arg(
            Arg::new("man-dir")
                .long("man-dir")
                .value_name("DIR")
                .value_parser(clap::value_parser!(PathBuf))
                .conflicts_with("shell")
                .help("Write man pages for every command into DIR"),
        )
}

/// Writes `russet.1` and one page per subcommand (`russet-daemon.1`, `russet-daemon-status.1`, …).
fn write_man_pages(cmd: &Command, prefix: &str, dir: &std::path::Path, written: &mut Vec<String>) -> Result<()> {
    let name = if prefix.is_empty() { cmd.get_name().to_owned() } else { format!("{prefix}-{}", cmd.get_name()) };
    let mut out = Vec::new();
    clap_mangen::Man::new(cmd.clone())
        .title(name.to_uppercase())
        .render(&mut out)
        .map_err(|e| Error::io("rendering a man page", e))?;
    let path = dir.join(format!("{name}.1"));
    std::fs::write(&path, out).map_err(|e| Error::io(format!("writing {}", path.display()), e))?;
    written.push(path.display().to_string());
    for sub in cmd.get_subcommands().filter(|s| !s.is_hide_set()) {
        write_man_pages(sub, &name, dir, written)?;
    }
    Ok(())
}

/// Runs this area's command; `m` is the root matches, so `m.subcommand()` names the command.
pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    let Some((_, a)) = m.subcommand() else { return Err(Error::invalid_argument("no completions command")) };
    let mut root = crate::root_command();
    if let Some(dir) = a.get_one::<PathBuf>("man-dir") {
        std::fs::create_dir_all(dir).map_err(|e| Error::io(format!("creating {}", dir.display()), e))?;
        root.build();
        let mut written = Vec::new();
        write_man_pages(&root, "", dir, &mut written)?;
        cx.output(&serde_json::json!({"pages": written}), |_| {
            format!("wrote {} man pages to {}", written.len(), dir.display())
        })?;
        return Ok(ExitCode::SUCCESS);
    }
    let Some(shell) = a.get_one::<Shell>("shell").copied() else {
        return Err(Error::invalid_argument("name a shell (bash, zsh, fish, elvish, powershell) or pass --man-dir")
            .with_hint("For example: russet completions bash > ~/.local/share/bash-completion/completions/russet"));
    };
    let mut script = Vec::new();
    clap_complete::generate(shell, &mut root, "russet", &mut script);
    cx.reporter.human(String::from_utf8_lossy(&script).trim_end());
    if cx.reporter.is_json() {
        cx.reporter
            .result(&serde_json::json!({"shell": shell.to_string(), "script": String::from_utf8_lossy(&script)}));
    }
    Ok(ExitCode::SUCCESS)
}
