// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet forward` (ARCHITECTURE §8.7, §10.4). Owner: W4.
//!
//! `russet forward (--service NAME | --tunnel-port N | --usbmux-port N) [--bind tcp:ADDR|unix:PATH]` prints
//! `{leaseId, local, url}` and holds the forward until interrupted (the lease belongs to this connection). The
//! default bind is a 0600 Unix socket under the daemon's runtime directory on Linux.

use std::process::ExitCode;

use clap::{Arg, ArgGroup, ArgMatches, Command};
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_protocol::forward as p;

use crate::CliCtx;

/// The `forward` command.
pub fn command() -> Command {
    Command::new("forward")
        .about("Forward a device service or port (--service NAME | --tunnel-port N | --usbmux-port N) [--bind …]")
        .arg(
            Arg::new("service")
                .long("service")
                .value_name("NAME")
                .help("An RSD service, e.g. com.apple.internal.dt.remote.debugproxy"),
        )
        .arg(
            Arg::new("tunnel-port")
                .long("tunnel-port")
                .value_name("PORT")
                .value_parser(clap::value_parser!(u16))
                .help("A TCP port on the device's tunnel address"),
        )
        .arg(
            Arg::new("usbmux-port")
                .long("usbmux-port")
                .value_name("PORT")
                .value_parser(clap::value_parser!(u16))
                .help("A device port through usbmux (iproxy-style)"),
        )
        .group(ArgGroup::new("target").args(["service", "tunnel-port", "usbmux-port"]).required(true))
        .arg(Arg::new("bind").long("bind").value_name("tcp:ADDR|unix:PATH").help("Where to listen locally"))
}

/// The `forward.open` params of a command line.
pub fn open_params(m: &ArgMatches, udid: Option<String>) -> Result<p::ForwardOpenParams> {
    let target = p::ForwardTargetParam {
        service: m.get_one::<String>("service").cloned(),
        tunnel_port: m.get_one::<u16>("tunnel-port").copied(),
        usbmux_port: m.get_one::<u16>("usbmux-port").copied(),
    };
    let bind = match m.get_one::<String>("bind") {
        None => None,
        Some(b) => {
            if let Some(addr) = b.strip_prefix("tcp:") {
                Some(p::ForwardBindParam { tcp: Some(addr.to_owned()), unix: None })
            } else if let Some(path) = b.strip_prefix("unix:") {
                Some(p::ForwardBindParam { tcp: None, unix: Some(super::device::absolute(path)) })
            } else {
                return Err(Error::new(
                    codes::core::INVALID_ARGUMENT,
                    format!("--bind {b:?}: expected tcp:ADDR or unix:PATH"),
                ));
            }
        }
    };
    Ok(p::ForwardOpenParams { udid, target, bind })
}

/// Runs this area's command; `m` is the root matches, so `m.subcommand()` names the command.
pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    let Some(("forward", fm)) = m.subcommand() else {
        return super::not_implemented(m);
    };
    let params = open_params(fm, cx.global.device.clone())?;
    let opened = cx.peer.call::<p::ForwardOpen>(params).await?;
    let v = serde_json::to_value(&opened)
        .map_err(|e| Error::new(codes::core::INTERNAL, format!("cannot render the result: {e}")))?;
    cx.reporter.result(&v);
    // The forward lives as long as this connection (ARCHITECTURE §6.3): wait until interrupted.
    cx.peer.closed().await;
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_lines_become_params() {
        let m = crate::root_command()
            .try_get_matches_from([
                "russet",
                "forward",
                "--service",
                "com.apple.internal.dt.remote.debugproxy",
                "--bind",
                "tcp:127.0.0.1:0",
            ])
            .expect("valid");
        let (_, fm) = m.subcommand().expect("forward");
        let params = open_params(fm, None).expect("params");
        assert_eq!(params.target.service.as_deref(), Some("com.apple.internal.dt.remote.debugproxy"));
        assert_eq!(params.bind.and_then(|b| b.tcp).as_deref(), Some("127.0.0.1:0"));
        assert!(crate::root_command().try_get_matches_from(["russet", "forward"]).is_err());
        assert!(
            crate::root_command()
                .try_get_matches_from(["russet", "forward", "--tunnel-port", "1", "--usbmux-port", "2"])
                .is_err()
        );
    }
}
