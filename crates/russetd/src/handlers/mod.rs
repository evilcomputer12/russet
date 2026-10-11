// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The handler seam (ARCHITECTURE §5.3, §6.1): one file per namespace, each owned by its area (PLAN §3).
//!
//! Seam file: lead-only after W1 merges (PLAN §0 rule 2). The list of namespaces is fixed here; every file
//! exports `pub fn register(r: &mut Router<DaemonCtx>)`. W1 registered every method of ARCHITECTURE §8 with
//! [`not_implemented`], so unimplemented methods answer `core.notImplemented` (1005) until their wave.

use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_ipc::{RequestCtx, Router};
use russet_protocol::RpcMethod;

use crate::DaemonCtx;

pub mod account;
pub mod app;
pub mod approval;
pub mod asc;
pub mod assets;
pub mod audit;
pub mod build;
pub mod config;
pub mod crash;
pub mod daemon;
pub mod debug;
pub mod device;
pub mod doctor;
pub mod files;
pub mod flutter_build;
pub mod flutter_run;
pub mod forward;
pub mod ib;
pub mod identity;
pub mod initialize;
pub mod input;
pub mod job;
pub mod lease;
pub mod location;
pub mod logs;
pub mod mcp;
pub mod perf;
pub mod portal;
pub mod process;
pub mod project;
pub mod provision;
pub mod run;
pub mod screen;
pub mod sdk;
pub mod secrets;
pub mod ship;
pub mod sign;
pub mod subscription;
pub mod test;
pub mod toolchain;
pub mod xcode;
pub mod xctest;

macro_rules! register_table {
    ($($module:ident),* $(,)?) => {
        /// Registers every namespace's handlers, in catalog order.
        pub fn register_all(r: &mut Router<DaemonCtx>) {
            $($module::register(r);)*
        }
    };
}

register_table!(
    initialize,
    daemon,
    subscription,
    lease,
    job,
    approval,
    audit,
    config,
    secrets,
    toolchain,
    sdk,
    doctor,
    device,
    app,
    process,
    forward,
    logs,
    screen,
    input,
    files,
    perf,
    location,
    project,
    build,
    assets,
    ib,
    flutter_build,
    account,
    identity,
    portal,
    provision,
    sign,
    asc,
    ship,
    run,
    debug,
    crash,
    flutter_run,
    test,
    xcode,
    xctest,
    mcp,
);

/// The F0 handler: answers `core.notImplemented` (1005) for method `M`.
pub async fn not_implemented<M: RpcMethod>(_cx: DaemonCtx, _rq: RequestCtx, _params: M::Params) -> Result<M::Result> {
    Err(Error::new(codes::core::NOT_IMPLEMENTED, format!("{} is not implemented yet", M::NAME))
        .with_hint("This method lands in a later workstream (docs/PLAN.md); see ARCHITECTURE §8 for its wave."))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn every_registered_method_has_a_handler_and_nothing_else() {
        let mut router = Router::<DaemonCtx>::new();
        register_all(&mut router);
        let handled: BTreeSet<&str> = router.methods().map(|m| m.name).collect();
        let catalog: BTreeSet<&str> = russet_protocol::registry().iter().map(|m| m.name).collect();
        assert_eq!(handled, catalog);
        assert_eq!(router.methods().count(), russet_protocol::registry().len());
    }
}
