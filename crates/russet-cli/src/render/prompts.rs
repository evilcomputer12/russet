// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The CLI's answers to the daemon's `ui.*` requests (ARCHITECTURE §7.5, §9.3).
//!
//! - `ui.confirm`: yes at once with `--yes`; otherwise a `[y/N]` question on the TTY.
//! - `ui.prompt`: only on a TTY (never pre-answered by `--yes`); secrets are read without echo, a typed
//!   confirmation must match its expected text.
//! - `ui.openUrl`: the URL is printed; the CLI opens no browser.

use std::time::Duration;

use russet_core::platform::{read_line_from_tty, stderr_is_tty, stdin_is_tty};
use russet_ipc::Peer;
use russet_protocol::ui::{
    PromptKind, UiConfirm, UiConfirmResult, UiOpenUrl, UiOpenUrlResult, UiPrompt, UiPromptResult,
};

use crate::global::GlobalArgs;

/// Registers the handlers on a fresh connection.
pub fn install(peer: &Peer, global: &GlobalArgs) {
    let yes = global.yes;
    peer.on_request::<UiConfirm, _, _>(move |p| async move {
        if yes {
            return Ok(UiConfirmResult { confirmed: true });
        }
        if !(stdin_is_tty() && stderr_is_tty()) {
            return Ok(UiConfirmResult { confirmed: false });
        }
        let question = match &p.detail {
            Some(d) => format!("{}\n{}\n{d}\nProceed? [y/N] ", p.title, p.message),
            None => format!("{}\n{}\nProceed? [y/N] ", p.title, p.message),
        };
        let answer = ask(question, false, p.timeout_ms).await;
        let confirmed = answer.is_some_and(|a| matches!(a.trim().to_ascii_lowercase().as_str(), "y" | "yes"));
        Ok(UiConfirmResult { confirmed })
    });
    peer.on_request::<UiPrompt, _, _>(|p| async move {
        if !(stdin_is_tty() && stderr_is_tty()) {
            return Ok(UiPromptResult { value: None, cancelled: true });
        }
        let question = match (&p.kind, &p.expected) {
            (PromptKind::TypedConfirmation, Some(expected)) => {
                format!("{}\n{}\nType `{expected}` to confirm: ", p.title, p.message)
            }
            _ => format!("{}\n{}: ", p.title, p.message),
        };
        let hide = p.secret || matches!(p.kind, PromptKind::Password);
        match ask(question, hide, p.timeout_ms).await {
            Some(value) => Ok(UiPromptResult { value: Some(value), cancelled: false }),
            None => Ok(UiPromptResult { value: None, cancelled: true }),
        }
    });
    peer.on_request::<UiOpenUrl, _, _>(|p| async move {
        crate::render::eprint_line(&format!("Open this URL to continue ({}): {}", p.reason, p.url));
        Ok(UiOpenUrlResult { opened: false })
    });
}

/// Asks on the TTY within the request's timeout; `None` when it timed out or stdin closed.
async fn ask(question: String, hide: bool, timeout_ms: u64) -> Option<String> {
    let read = tokio::task::spawn_blocking(move || read_line_from_tty(&question, hide).ok());
    let timeout = Duration::from_millis(timeout_ms.max(1000));
    tokio::time::timeout(timeout, read).await.ok().and_then(Result::ok).flatten()
}
