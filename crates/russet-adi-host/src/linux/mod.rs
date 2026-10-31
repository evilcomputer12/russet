// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The Linux helper: sandbox first, then serve the JSON Lines protocol with Apple's libraries loaded by the
//! clean-room loader. The x86_64 loader is complete; aarch64 hosts answer `unsupported` until its relocations and
//! trap stubs land (W22 follow-up).

pub mod adi;
pub mod elf;
pub mod sandbox;
pub mod shims;

use std::io::{BufRead as _, Write as _};
use std::path::{Path, PathBuf};

use base64::Engine as _;

use crate::protocol::{Request, Response};

fn b64(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}

fn unb64(s: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD.decode(s).ok()
}

/// The helper's state between requests.
struct Host {
    libs: PathBuf,
    state: PathBuf,
    adi: Option<adi::Adi>,
}

fn inside(path: &str, root: &Path) -> Option<PathBuf> {
    let p = Path::new(path).canonicalize().ok()?;
    let root = root.canonicalize().ok()?;
    p.starts_with(&root).then_some(p)
}

impl Host {
    fn handle(&mut self, req: Request) -> Response {
        match req {
            Request::Load { lib_dir, android_id, provisioning_path } => {
                if self.adi.is_some() {
                    return Response::ok();
                }
                let Some(lib_dir) = inside(&lib_dir, &self.libs) else {
                    return Response::error("libDir is outside the libraries directory this helper may read");
                };
                let Some(prov) = inside(&provisioning_path, &self.state) else {
                    return Response::error("provisioningPath is outside the state directory this helper may write");
                };
                if android_id.len() != 16 || !android_id.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Response::error("androidId must be 16 hex digits");
                }
                match adi::Adi::load(&lib_dir.to_string_lossy(), &android_id, &prov.to_string_lossy()) {
                    Ok(a) => {
                        self.adi = Some(a);
                        Response::ok()
                    }
                    Err(e) => Response::error(e),
                }
            }
            other => {
                let Some(adi) = &self.adi else { return Response::error("nothing is loaded yet (send load first)") };
                match other {
                    Request::IsProvisioned { dsid } => match adi.is_provisioned(dsid) {
                        Ok(p) => Response { ok: true, provisioned: Some(p), ..Response::default() },
                        Err(e) => Response::adi(e.0),
                    },
                    Request::StartProvisioning { dsid, spim } => {
                        let Some(spim) = unb64(&spim) else { return Response::error("spim is not base64") };
                        match adi.start_provisioning(dsid, &spim) {
                            Ok((session, cpim)) => Response {
                                ok: true,
                                session: Some(session),
                                cpim: Some(b64(&cpim)),
                                ..Response::default()
                            },
                            Err(e) => Response::adi(e.0),
                        }
                    }
                    Request::EndProvisioning { session, ptm, tk } => {
                        let (Some(ptm), Some(tk)) = (unb64(&ptm), unb64(&tk)) else {
                            return Response::error("ptm or tk is not base64");
                        };
                        match adi.end_provisioning(session, &ptm, &tk) {
                            Ok(()) => Response::ok(),
                            Err(e) => Response::adi(e.0),
                        }
                    }
                    Request::Otp { dsid } => match adi.request_otp(dsid) {
                        Ok((mid, otp)) => {
                            Response { ok: true, mid: Some(b64(&mid)), otp: Some(b64(&otp)), ..Response::default() }
                        }
                        Err(e) => Response::adi(e.0),
                    },
                    Request::Erase { dsid } => match adi.erase(dsid) {
                        Ok(()) => Response::ok(),
                        Err(e) => Response::adi(e.0),
                    },
                    Request::Load { .. } => Response::ok(),
                }
            }
        }
    }
}

/// Runs the helper: `--libs <dir> --state <dir> [--no-sandbox]` (the last only for diagnosis).
pub fn run(args: &[String]) -> i32 {
    let arg = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(PathBuf::from);
    let (Some(libs), Some(state)) = (arg("--libs"), arg("--state")) else {
        eprintln!("usage: russet-adi-host --libs <dir> --state <dir>");
        return 2;
    };
    if !cfg!(target_arch = "x86_64") {
        return crate::serve_unsupported("this helper loads x86_64 libraries only for now");
    }
    shims::preload_host();
    if !args.iter().any(|a| a == "--no-sandbox") {
        let landlock = match sandbox::apply_landlock(&libs, &state) {
            Ok(on) => on,
            Err(e) => {
                eprintln!("russet-adi-host: cannot apply Landlock: {e}");
                return 3;
            }
        };
        if let Err(e) = sandbox::apply_seccomp() {
            eprintln!("russet-adi-host: cannot apply seccomp: {e}");
            return 3;
        }
        eprintln!(
            "russet-adi-host: sandbox seccomp=on landlock={}",
            if landlock { "on" } else { "unavailable (kernel without Landlock)" }
        );
    }
    let mut host = Host { libs, state, adi: None };
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let resp = match serde_json::from_str::<Request>(&line) {
            Ok(req) => host.handle(req),
            Err(_) => Response::error("unreadable request"),
        };
        let Ok(text) = serde_json::to_string(&resp) else { break };
        if writeln!(out, "{text}").and_then(|()| out.flush()).is_err() {
            break;
        }
    }
    // Skip the destructors the Android libraries registered with __cxa_atexit.
    shims::exit_now(0)
}
