// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `input` handlers (§8.2). Owner: W10.
//!
//! F0 stub (W1): every method answers `core.notImplemented` (1005) until its owner replaces the handler.

use russet_ipc::Router;
use russet_protocol::input as p;

use super::not_implemented;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::InputTap, _, _>(not_implemented::<p::InputTap>);
    r.request::<p::InputSwipe, _, _>(not_implemented::<p::InputSwipe>);
    r.request::<p::InputButton, _, _>(not_implemented::<p::InputButton>);
    r.request::<p::InputText, _, _>(not_implemented::<p::InputText>);
    r.request::<p::InputKey, _, _>(not_implemented::<p::InputKey>);
}
