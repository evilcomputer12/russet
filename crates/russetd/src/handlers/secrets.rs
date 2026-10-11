// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `secrets` handlers (ARCHITECTURE §8.1, §12.2). Owner: W3.
//!
//! No handler returns a secret value. `secrets.set` reads the file the client names and seals its content; it
//! is accepted from `cli` and `ide` connections only (the permission engine already denies `secrets` to `mcp`
//! and `flutter-device`; this is the second line). A non-interactive CLI reaches it with `--yes` (§7.5) [D 85].

use std::path::PathBuf;

use russet_core::error::{Error, Result, codes};
use russet_core::rpc::{ClientKind, ConnInfo};
use russet_ipc::Router;
use russet_protocol::secrets as p;
use russet_secrets::{BackendKind, SecretId, SecretKind, SecretMeta, SecretStore};
use time::format_description::well_known::Rfc3339;

use super::config::secrets_of;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::SecretsStatus, _, _>(
        |cx: DaemonCtx, _rq, _params| async move { Ok(status(&*secrets_of(&cx)?).await) },
    );
    r.request::<p::SecretsList, _, _>(
        |cx: DaemonCtx, _rq, params| async move { list(&*secrets_of(&cx)?, params).await },
    );
    r.request::<p::SecretsSet, _, _>(|cx: DaemonCtx, rq: russet_ipc::RequestCtx, params| async move {
        let conn = rq.peer.connection().clone();
        set(&*secrets_of(&cx)?, &conn, params).await
    });
    r.request::<p::SecretsDelete, _, _>(|cx: DaemonCtx, _rq, params| async move {
        delete(&*secrets_of(&cx)?, params).await
    });
}

/// The wire backend.
pub fn wire_backend(k: BackendKind) -> p::SecretBackend {
    match k {
        BackendKind::Keyring => p::SecretBackend::Keyring,
        BackendKind::PassphraseFile => p::SecretBackend::Passphrase,
        BackendKind::Env => p::SecretBackend::Env,
        BackendKind::Memory => p::SecretBackend::Memory,
    }
}

/// The wire kind.
pub fn wire_kind(k: SecretKind) -> p::SecretKind {
    match k {
        SecretKind::AscKey => p::SecretKind::AscKey,
        SecretKind::SigningKey => p::SecretKind::SigningKey,
        SecretKind::AppleIdSession => p::SecretKind::AppleIdSession,
        SecretKind::AppleIdPassword => p::SecretKind::AppleIdPassword,
        SecretKind::AnisetteState => p::SecretKind::AnisetteState,
        SecretKind::GithubToken => p::SecretKind::GithubToken,
        SecretKind::McpHttpToken => p::SecretKind::McpHttpToken,
        SecretKind::TcpToken => p::SecretKind::TcpToken,
    }
}

fn kind_from_wire(k: p::SecretKind) -> SecretKind {
    match k {
        p::SecretKind::AscKey => SecretKind::AscKey,
        p::SecretKind::SigningKey => SecretKind::SigningKey,
        p::SecretKind::AppleIdSession => SecretKind::AppleIdSession,
        p::SecretKind::AppleIdPassword => SecretKind::AppleIdPassword,
        p::SecretKind::AnisetteState => SecretKind::AnisetteState,
        p::SecretKind::GithubToken => SecretKind::GithubToken,
        p::SecretKind::McpHttpToken => SecretKind::McpHttpToken,
        p::SecretKind::TcpToken => SecretKind::TcpToken,
    }
}

/// The wire metadata of a secret (never its value).
pub fn wire_meta(m: &SecretMeta) -> p::SecretMeta {
    p::SecretMeta {
        id: m.id.to_string(),
        kind: wire_kind(m.id.kind),
        name: m.id.name.clone(),
        label: m.label.clone(),
        backend: wire_backend(m.backend),
        created_at: m.created_at.format(&Rfc3339).unwrap_or_default(),
        size_bytes: m.size_bytes as u64,
    }
}

/// `secrets.status`.
pub async fn status(store: &dyn SecretStore) -> p::SecretsStatusResult {
    let info = store.status().await;
    p::SecretsStatusResult {
        backend: wire_backend(info.kind),
        available: info.available,
        locked: info.locked,
        reason: info.reason,
        // After `status()`: `auto` has chosen its backend by then.
        keyring_entry: store.keyring_entry(),
    }
}

/// `secrets.list`.
pub async fn list(store: &dyn SecretStore, params: p::SecretsListParams) -> Result<p::SecretsListResult> {
    let items = store.list(params.kind.map(kind_from_wire)).await?;
    Ok(p::SecretsListResult { items: items.iter().map(wire_meta).collect() })
}

/// `secrets.set`: seals the content of `params.from_file`.
pub async fn set(store: &dyn SecretStore, conn: &ConnInfo, params: p::SecretsSetParams) -> Result<p::SecretMeta> {
    if !matches!(conn.kind, ClientKind::Cli | ClientKind::Ide) {
        return Err(Error::new(codes::perm::DENIED, "secrets.set is accepted from the CLI and the IDE only")
            .with_hint("Store secrets with `russet secrets set` in a terminal."));
    }
    let id = SecretId::parse(&params.id)?;
    let path = PathBuf::from(&params.from_file);
    if !path.is_absolute() {
        return Err(Error::new(
            codes::core::INVALID_ARGUMENT,
            format!("`{}` is not an absolute path", params.from_file),
        )
        .with_hint("Pass the file's absolute path."));
    }
    let bytes =
        tokio::task::spawn_blocking(move || russet_secrets::file::read_secret_file(&path)).await.map_err(|e| {
            Error::new(codes::core::INTERNAL, format!("the read task failed: {e}")).with_hint("Report this bug.")
        })??;
    let label = params.label.unwrap_or_else(|| {
        std::path::Path::new(&params.from_file)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    let meta = store.put(&id, &label, bytes).await?;
    Ok(wire_meta(&meta))
}

/// `secrets.delete`.
pub async fn delete(store: &dyn SecretStore, params: p::SecretsDeleteParams) -> Result<p::SecretsDeleteResult> {
    let id = SecretId::parse(&params.id)?;
    Ok(p::SecretsDeleteResult { deleted: store.delete(&id).await? })
}
