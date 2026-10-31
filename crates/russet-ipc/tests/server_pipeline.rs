// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The server pipeline (ARCHITECTURE §7.1–§7.6) over the in-process duplex, a Unix socket and TCP:
//! `initialize` first, `-32601`, `-32602` with `details.path`, progress and cancellation (`-32800`),
//! confirmations answered by the client, the Unix socket's permissions and stale-socket handling, the TCP
//! token, and the 16 MiB frame limit.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use russet_core::error::codes;
use russet_ipc::message::{INVALID_PARAMS, METHOD_NOT_FOUND, REQUEST_CANCELLED, SERVER_NOT_INITIALIZED};
use russet_ipc::{CallOptions, Endpoint, ListenOptions, Server, in_process, serve_peer};
use russet_protocol::common::ProgressKind;
use russet_protocol::ui::{UiConfirm, UiConfirmResult, UiPrompt, UiPromptResult};
use serde_json::json;
use support::{TYPED_TEXT, TestAuth, client, router};
use tokio_util::sync::CancellationToken;

async fn in_process_client(confirm: bool, prompt: bool) -> russet_ipc::Peer {
    let (c, s) = in_process();
    serve_peer(&s, router(), (), Arc::new(TestAuth));
    russet_ipc::initialize(&c, &client(confirm, prompt), None).await.expect("initialize");
    c
}

#[tokio::test]
async fn requests_before_initialize_get_32002_and_initialize_works_once() {
    let (c, s) = in_process();
    serve_peer(&s, router(), (), Arc::new(TestAuth));
    let e = c.call_raw("test.echo", json!({"text": "x"}), CallOptions::default()).await.expect_err("not yet");
    assert_eq!(e.code, SERVER_NOT_INITIALIZED);
    let result = russet_ipc::initialize(&c, &client(false, false), None).await.expect("initialize");
    assert!(result.connection_id.starts_with("c-"));
    assert_eq!(c.connection().id, result.connection_id);
    let again = c
        .call_raw(
            "initialize",
            serde_json::to_value(client(false, false).initialize_params(None)).unwrap(),
            CallOptions::default(),
        )
        .await;
    assert_eq!(again.expect_err("twice").code, russet_ipc::message::INVALID_REQUEST);
}

#[tokio::test]
async fn unknown_methods_and_bad_params_get_their_codes() {
    let c = in_process_client(false, false).await;
    let echo = c.call::<support::TestEcho>(support::EchoParams { text: "hi".into(), count: 2 }).await.expect("echo");
    assert_eq!(echo.text, "hi");
    let e = c.call_raw("test.nope", json!({}), CallOptions::default()).await.expect_err("unknown");
    assert_eq!(e.code, METHOD_NOT_FOUND);
    let e =
        c.call_raw("test.echo", json!({"text": "x", "count": "many"}), CallOptions::default()).await.expect_err("bad");
    assert_eq!(e.code, INVALID_PARAMS);
    assert_eq!(e.data.as_ref().and_then(|d| d.details.clone()), Some(json!({"path": "count"})));
    assert_eq!(e.to_error().code(), codes::core::INVALID_ARGUMENT);
    let denied = c.call::<support::TestDenied>(support::Empty {}).await.expect_err("denied");
    assert_eq!(denied.code(), codes::perm::DENIED);
}

#[tokio::test]
async fn progress_arrives_and_cancellation_answers_32800() {
    let c = in_process_client(false, false).await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let cancel = CancellationToken::new();
    let (seen2, cancel2) = (Arc::clone(&seen), cancel.clone());
    let opts = CallOptions {
        cancel: Some(cancel.clone()),
        on_progress: Some(Box::new(move |v| {
            let done = v.kind == ProgressKind::Report && v.message.as_deref() == Some("step 2");
            seen2.lock().unwrap().push(v);
            if done {
                cancel2.cancel();
            }
        })),
        timeout: Some(Duration::from_secs(10)),
    };
    let e = c.call_raw("test.slow", json!({"steps": 3}), opts).await.expect_err("cancelled");
    assert_eq!(e.code, REQUEST_CANCELLED);
    assert_eq!(e.data.as_ref().map(|d| d.russet_code.as_str()), Some("core.cancelled"));
    let seen = seen.lock().unwrap();
    assert_eq!(seen.first().map(|v| v.kind), Some(ProgressKind::Begin));
    assert_eq!(seen.iter().filter(|v| v.kind == ProgressKind::Report).count(), 3);
}

#[tokio::test]
async fn plain_confirmations_go_to_the_requester() {
    // Confirmed.
    let c = in_process_client(true, false).await;
    c.on_request::<UiConfirm, _, _>(|p| async move {
        assert_eq!(p.requested_by.name, "test-client");
        Ok(UiConfirmResult { confirmed: true })
    });
    c.call::<support::TestConfirm>(support::Empty {}).await.expect("confirmed");
    // Declined.
    let c = in_process_client(true, false).await;
    c.on_request::<UiConfirm, _, _>(|_p| async move { Ok(UiConfirmResult { confirmed: false }) });
    let e = c.call::<support::TestConfirm>(support::Empty {}).await.expect_err("declined");
    assert_eq!(e.code(), codes::perm::CONFIRMATION_DECLINED);
    // No confirm capability (no TTY, no --yes).
    let c = in_process_client(false, false).await;
    let e = c.call::<support::TestConfirm>(support::Empty {}).await.expect_err("unavailable");
    assert_eq!(e.code(), codes::perm::CONFIRMATION_UNAVAILABLE);
    // Declares the capability but does not implement ui.confirm.
    let c = in_process_client(true, false).await;
    let e = c.call::<support::TestConfirm>(support::Empty {}).await.expect_err("unanswerable");
    assert_eq!(e.code(), codes::perm::CONFIRMATION_UNAVAILABLE);
}

#[tokio::test]
async fn typed_confirmations_need_the_exact_text() {
    // `confirmText` satisfies it without a prompt (headless CLI with --confirm-text).
    let c = in_process_client(false, false).await;
    c.call_raw("test.typed", json!({"confirmText": TYPED_TEXT}), CallOptions::default()).await.expect("typed text");
    let e = c.call_raw("test.typed", json!({"confirmText": "nope"}), CallOptions::default()).await.expect_err("wrong");
    assert_eq!(e.to_error().code(), codes::perm::CONFIRMATION_UNAVAILABLE);
    // A prompt-capable client types it.
    let c = in_process_client(true, true).await;
    c.on_request::<UiPrompt, _, _>(
        |p| async move { Ok(UiPromptResult { value: p.expected.clone(), cancelled: false }) },
    );
    c.call::<support::TestTyped>(support::Empty {}).await.expect("typed");
    let c = in_process_client(true, true).await;
    c.on_request::<UiPrompt, _, _>(
        |_p| async move { Ok(UiPromptResult { value: Some("yes".into()), cancelled: false }) },
    );
    let e = c.call::<support::TestTyped>(support::Empty {}).await.expect_err("mistyped");
    assert_eq!(e.code(), codes::perm::CONFIRMATION_DECLINED);
    // `--yes` (a plain confirmation) never satisfies a typed one.
    let c = in_process_client(true, false).await;
    c.on_request::<UiConfirm, _, _>(|_p| async move { Ok(UiConfirmResult { confirmed: true }) });
    let e = c.call::<support::TestTyped>(support::Empty {}).await.expect_err("plain is not typed");
    assert_eq!(e.code(), codes::perm::CONFIRMATION_UNAVAILABLE);
}

#[cfg(unix)]
#[tokio::test]
async fn unix_sockets_are_private_and_stale_sockets_are_replaced() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("runtime/russetd.sock");
    // A stale socket file from a crashed daemon.
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    drop(std::os::unix::net::UnixListener::bind(&path).expect("stale"));
    assert!(path.exists());

    let server = Server::bind(&Endpoint::Unix(path.clone()), ListenOptions::default())
        .await
        .expect("binds over the stale socket");
    let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&path), 0o600);
    assert_eq!(mode(path.parent().unwrap()), 0o700);
    let shutdown = CancellationToken::new();
    let serving = tokio::spawn(server.serve(router(), (), Arc::new(TestAuth), shutdown.clone()));

    // A second server refuses while the first one answers.
    let e = Server::bind(&Endpoint::Unix(path.clone()), ListenOptions::default()).await.expect_err("live");
    assert_eq!(e.code(), codes::ipc::DAEMON_UNAVAILABLE);

    let (peer, result) =
        russet_ipc::connect(&Endpoint::Unix(path.clone()), client(false, false)).await.expect("connects");
    assert_eq!(peer.identity().uid, Some(russet_core::platform::current_uid()));
    assert_eq!(result.server.name, "test-server");
    let echo =
        peer.call::<support::TestEcho>(support::EchoParams { text: "unix".into(), count: 0 }).await.expect("echo");
    assert_eq!(echo.text, "unix");

    shutdown.cancel();
    serving.await.expect("join").expect("serve");
    assert!(!path.exists(), "the socket is removed on shutdown");
    let e = russet_ipc::connect(&Endpoint::Unix(path), client(false, false)).await.expect_err("gone");
    assert_eq!(e.code(), codes::ipc::DAEMON_UNAVAILABLE);
}

#[cfg(unix)]
#[tokio::test]
async fn frames_over_16_mib_close_the_connection() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("r/russetd.sock");
    let server = Server::bind(&Endpoint::Unix(path.clone()), ListenOptions::default()).await.expect("binds");
    let shutdown = CancellationToken::new();
    tokio::spawn(server.serve(router(), (), Arc::new(TestAuth), shutdown.clone()));
    let mut raw = tokio::net::UnixStream::connect(&path).await.expect("connect");
    raw.write_all(format!("Content-Length: {}\r\n\r\n", 17 * 1024 * 1024).as_bytes()).await.expect("write");
    let mut reply = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), raw.read_to_end(&mut reply)).await.expect("closes").expect("read");
    let text = String::from_utf8_lossy(&reply);
    assert!(text.contains("\"code\":1104"), "{text}");
    assert!(text.contains("ipc.frameTooLarge"), "{text}");
    shutdown.cancel();
}

#[tokio::test]
async fn tcp_needs_the_token_within_two_seconds() {
    let token = "0123456789abcdef0123456789abcdef";
    let endpoint = Endpoint::Tcp { addr: "127.0.0.1:0".parse().unwrap(), token: token.to_owned().into() };
    let server = Server::bind(&endpoint, ListenOptions::default()).await.expect("binds");
    let bound: std::net::SocketAddr = server.endpoint().trim_start_matches("tcp://").parse().expect("addr");
    let shutdown = CancellationToken::new();
    tokio::spawn(server.serve(router(), (), Arc::new(TestAuth), shutdown.clone()));

    let good = Endpoint::Tcp { addr: bound, token: token.to_owned().into() };
    let (peer, _) = russet_ipc::connect(&good, client(false, false)).await.expect("right token");
    peer.call::<support::TestEcho>(support::EchoParams::default()).await.expect("works");

    let bad = Endpoint::Tcp { addr: bound, token: "wrong".to_owned().into() };
    let e = russet_ipc::connect(&bad, client(false, false)).await.expect_err("wrong token");
    assert_eq!(e.code(), codes::perm::AUTH_FAILED);

    // A connection that never initializes is closed after 2 s.
    let silent = good.open(Duration::from_secs(1)).await.expect("opens");
    tokio::time::timeout(Duration::from_secs(4), silent.closed()).await.expect("closed by the server");

    let nonlocal = Endpoint::Tcp { addr: "0.0.0.0:0".parse().unwrap(), token: token.to_owned().into() };
    let e = Server::bind(&nonlocal, ListenOptions::default()).await.expect_err("loopback only");
    assert_eq!(e.code(), codes::core::INVALID_ARGUMENT);
    shutdown.cancel();
}
