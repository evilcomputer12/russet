// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Actual LLDB compatibility and late IDE handshaking; no device or signing.
#![cfg(unix)]
#![allow(clippy::unwrap_used)]

const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use russet_debug::DapEndpoint;
use russet_debug::dap::{DapHub, NoHooks, encode, read_message, spawn_lldb_dap};
use russet_testkit::TempHome;
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

async fn ide_request(stream: &mut UnixStream, seq: u64, command: &str, arguments: Value) -> Value {
    let request = json!({"seq": seq, "type": "request", "command": command, "arguments": arguments});
    stream.write_all(&encode(&request).unwrap()).await.unwrap();
    loop {
        let message =
            tokio::time::timeout(Duration::from_secs(20), read_message(stream)).await.unwrap().unwrap().unwrap();
        if message["type"] == "response" && message["request_seq"] == seq {
            assert_eq!(message["success"], true, "{message}");
            return message["body"].clone();
        }
    }
}

#[tokio::test]
#[ignore = "requires RUSSET_TEST_LLDB_DAP, its environment JSON and a C compiler"]
async fn real_lldb_bootstrap_and_late_ide_share_one_process() {
    let tool = std::env::var("RUSSET_TEST_LLDB_DAP").unwrap();
    let environment: BTreeMap<String, String> =
        serde_json::from_str(&std::env::var("RUSSET_TEST_LLDB_ENV").unwrap()).unwrap();
    let home = TempHome::new().unwrap();
    let source = home.root().join("probe.c");
    let program = home.root().join("probe");
    std::fs::write(&source, "int main(void) { return 0; }\n").unwrap();
    let mut compile = std::process::Command::new("cc");
    home.apply(&mut compile);
    assert!(compile.arg("-g").arg(&source).arg("-o").arg(&program).status().unwrap().success());
    let hub = DapHub::start(spawn_lldb_dap(tool.as_ref(), &[], &environment).unwrap(), Arc::new(NoHooks));
    let config = json!({
        "initCommands": ["settings set target.disable-aslr false"],
        "attachCommands": [format!("!target create '{}'", program.display()), "!process launch --stop-at-entry"],
        "stopOnEntry": true,
    });
    hub.bootstrap_attach(config.clone()).await.unwrap();
    let agent = hub.agent_client();
    let before = agent.request("threads", json!({})).await.unwrap();
    let first = before["threads"][0]["id"].as_u64().unwrap();
    let endpoint = DapEndpoint::Unix { path: home.root().join("ide.sock") };
    hub.serve_ide(&endpoint).await.unwrap();
    let DapEndpoint::Unix { path } = endpoint else { unreachable!() };
    let mut ide = UnixStream::connect(path).await.unwrap();
    ide_request(
        &mut ide,
        1,
        "initialize",
        json!({
            "adapterID":"russet", "clientID":"late-ide-test", "pathFormat":"path",
            "linesStartAt1":true, "columnsStartAt1":true,
        }),
    )
    .await;
    ide_request(&mut ide, 2, "attach", config).await;
    let after = agent.request("threads", json!({})).await.unwrap();
    assert_eq!(after["threads"][0]["id"].as_u64(), Some(first), "IDE handshake replaced the agent's process");
    ide_request(&mut ide, 3, "disconnect", json!({"terminateDebuggee":true})).await;
}
