// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! A test router and authorizer for russet-ipc's integration tests (the daemon's own pipeline is tested in
//! russetd): `test.echo`, `test.slow` (progress until cancelled), `test.confirm` (a plain confirmation),
//! `test.typed` (a typed confirmation) and `test.denied`.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use russet_core::error::{Error, codes};
use russet_core::progress::ProgressSink;
use russet_core::rpc::{ClientKind, ConnInfo, Ownership, Permission};
use russet_ipc::{Authorizer, ConfirmSpec, Decision, Grant, PeerIdentity, RequestCtx, Router};
use russet_protocol::MethodInfo;
use russet_protocol::initialize::{
    ExecutableId, Initialize, InitializeParams, InitializeResult, ServerCapabilities, ServerEndpoints, ServerInfo,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct EchoParams {
    pub text: String,
    #[serde(default)]
    pub count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema, TS)]
pub struct Empty {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SlowParams {
    /// Progress reports before waiting for cancellation.
    #[serde(default)]
    pub steps: u32,
}

russet_protocol::rpc_method!(TestEcho, "test.echo", EchoParams => EchoParams, Permission::Read);
russet_protocol::rpc_method!(TestSlow, "test.slow", SlowParams => Empty, Permission::Read, long_running);
russet_protocol::rpc_method!(TestConfirm, "test.confirm", Empty => Empty, Permission::Publish);
russet_protocol::rpc_method!(TestTyped, "test.typed", Empty => Empty, Permission::AccountDestructive, confirm = Typed);
russet_protocol::rpc_method!(TestDenied, "test.denied", Empty => Empty, Permission::System);

/// The text a typed confirmation of `test.typed` expects.
pub const TYPED_TEXT: &str = "delete it";

async fn initialize(_cx: (), rq: RequestCtx, p: InitializeParams) -> russet_core::Result<InitializeResult> {
    if p.protocol.major != russet_protocol::PROTOCOL.major {
        return Err(Error::new(codes::ipc::PROTOCOL_MISMATCH, "protocol mismatch"));
    }
    let session = rq.peer.session().expect("the server records the session before the handler runs");
    Ok(InitializeResult {
        protocol: russet_protocol::PROTOCOL,
        server: ServerInfo {
            name: "test-server".into(),
            version: "0".into(),
            build_id: "0+test".into(),
            pid: std::process::id(),
            executable: "/test".into(),
            executable_id: ExecutableId::default(),
            started_at: russet_core::time::now_rfc3339(),
            platform: russet_core::platform::host_os().into(),
            arch: russet_core::platform::host_arch().into(),
            device_backend: "none".into(),
        },
        connection_id: session.conn.id.clone(),
        grant: session.grant.clone(),
        capabilities: ServerCapabilities::default(),
        endpoints: ServerEndpoints::default(),
    })
}

/// The test router (context `()`).
pub fn router() -> Arc<Router<()>> {
    let mut r = Router::<()>::new();
    r.request::<Initialize, _, _>(initialize);
    r.request::<TestEcho, _, _>(|_cx, _rq, p: EchoParams| async move { Ok(p) });
    r.request::<TestSlow, _, _>(|_cx, rq: RequestCtx, p: SlowParams| async move {
        rq.progress.begin("slow", true);
        for i in 0..p.steps {
            rq.progress.report(Some(&format!("step {i}")), Some(u8::try_from(i * 10).unwrap_or(100)));
        }
        rq.cancel.cancelled().await;
        Err(Error::cancelled())
    });
    r.request::<TestConfirm, _, _>(|_cx, _rq, _p: Empty| async move { Ok(Empty {}) });
    r.request::<TestTyped, _, _>(|_cx, _rq, _p: Empty| async move { Ok(Empty {}) });
    r.request::<TestDenied, _, _>(|_cx, _rq, _p: Empty| async move { Ok(Empty {}) });
    Arc::new(r)
}

/// Same-uid connections only; `publish` confirms, `account-destructive` needs a typed confirmation, `system`
/// is denied.
pub struct TestAuth;

impl Authorizer for TestAuth {
    fn on_connect(&self, id: &PeerIdentity) -> russet_core::Result<()> {
        #[cfg(unix)]
        if let Some(uid) = id.uid
            && uid != russet_core::platform::current_uid()
        {
            return Err(Error::new(codes::perm::AUTH_FAILED, "foreign uid"));
        }
        let _ = id;
        Ok(())
    }

    fn on_initialize(&self, _id: &PeerIdentity, p: &InitializeParams) -> russet_core::Result<Grant> {
        Ok(Grant {
            profile: format!("{:?}", p.client.kind).to_lowercase(),
            allow: vec![Permission::Read],
            confirm: vec![Permission::Publish, Permission::AccountDestructive],
            approval: vec![],
            deny: vec![Permission::System],
            tier_max: None,
        })
    }

    fn decide(
        &self,
        _grant: &Grant,
        method: &MethodInfo,
        params: &serde_json::Value,
        conn: &ConnInfo,
        ownership: &dyn Ownership,
    ) -> Decision {
        let class = match (method.permission_of)(params, conn, ownership) {
            Ok(c) => c,
            Err(e) => return Decision::Deny(e),
        };
        match class {
            Permission::None | Permission::Read => Decision::Allow,
            Permission::Publish => Decision::Confirm(ConfirmSpec {
                title: "Publish?".into(),
                message: "really".into(),
                detail: None,
                danger: false,
                typed: None,
            }),
            Permission::AccountDestructive => Decision::Confirm(ConfirmSpec {
                title: "Destroy?".into(),
                message: "really really".into(),
                detail: None,
                danger: true,
                typed: Some(TYPED_TEXT.into()),
            }),
            _ => Decision::Deny(Error::new(codes::perm::DENIED, format!("{} is denied", method.name))),
        }
    }
}

/// A CLI client description with the given capabilities.
pub fn client(confirm: bool, prompt: bool) -> russet_ipc::ClientInfo {
    let mut c = russet_ipc::ClientInfo::new(ClientKind::Cli, "test-client");
    c.capabilities.confirm = confirm;
    c.capabilities.prompt = prompt;
    c
}
