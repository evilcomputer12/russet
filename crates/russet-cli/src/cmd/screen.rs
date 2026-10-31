// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet screenshot`, `russet mirror` (ARCHITECTURE §8.7). Owner: W10.
//!
//! `screenshot` writes a PNG of the device's screen (relative paths resolve against the working directory, as
//! every client sends absolute paths). `mirror` is still an F0 stub reporting `core.notImplemented`.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{Arg, ArgMatches, Command};
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_protocol::screen::{ScreenScreenshot, ScreenScreenshotParams};

use crate::CliCtx;

/// This area's top-level commands, as a [`super::group`].
pub fn command() -> Command {
    super::group(
        "screen",
        [
            Command::new("screenshot").about("Take a device screenshot [-o FILE]").arg(
                Arg::new("output")
                    .short('o')
                    .long("output")
                    .value_name("FILE")
                    .help("Where to write the PNG (default: screenshot-<unix time>.png here)"),
            ),
            super::stub("mirror", "Mirror the device screen [--stdout | --record FILE]"),
        ],
    )
}

/// Runs this area's command; `m` is the root matches, so `m.subcommand()` names the command.
pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    let Some(("screenshot", s)) = m.subcommand() else {
        return super::not_implemented(m);
    };
    let file = match s.get_one::<String>("output") {
        Some(f) => PathBuf::from(f),
        None => {
            let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or_default();
            PathBuf::from(format!("screenshot-{now}.png"))
        }
    };
    let file = if file.is_absolute() {
        file
    } else {
        std::env::current_dir()
            .map_err(|e| Error::new(codes::core::IO, format!("cannot read the working directory: {e}")))?
            .join(file)
    };
    let params = ScreenScreenshotParams {
        udid: cx.global.device.clone(),
        to_file: Some(file.display().to_string()),
        ..Default::default()
    };
    let r = cx.peer.call::<ScreenScreenshot>(params).await?;
    cx.output(&r, |r| format!("{} ({}×{})", r.path.as_deref().unwrap_or_default(), r.width, r.height))?;
    Ok(ExitCode::SUCCESS)
}
