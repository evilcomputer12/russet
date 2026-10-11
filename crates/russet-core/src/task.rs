// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Supervised tasks (core §5.1 `task`, §6.2): a panic in one task is logged and never takes the daemon down.

use std::future::Future;

use tokio::task::JoinHandle;

/// Spawns `fut` on the current runtime. If it panics, the panic is logged with the task's `name` (the panic
/// hook has already written the report) and the handle resolves to `None`; otherwise to `Some(output)`.
pub fn spawn_supervised<F>(name: &'static str, fut: F) -> JoinHandle<Option<F::Output>>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let inner = tokio::spawn(fut);
    tokio::spawn(async move {
        match inner.await {
            Ok(out) => Some(out),
            Err(e) if e.is_panic() => {
                tracing::error!(task = name, "supervised task panicked");
                None
            }
            Err(_) => None,
        }
    })
}

/// The message of a panic payload (`&str` or `String`), for logs and panic reports.
pub fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "a panic with a non-string payload".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn panics_are_contained() {
        let ok = spawn_supervised("ok", async { 7 }).await.expect("join");
        assert_eq!(ok, Some(7));
        let boom = spawn_supervised("boom", async {
            #[allow(clippy::panic)]
            if std::hint::black_box(true) {
                panic!("expected test panic");
            }
            1
        })
        .await
        .expect("join");
        assert_eq!(boom, None);
        assert_eq!(panic_message(&"x"), "x");
    }
}
