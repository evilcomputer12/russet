// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! H2 (RD §23.2): `lldb-dap` 21 attaches through the broker with nothing but `attachCommands`
//! (`initialize` → `attach` → `initialized` → `configurationDone`), sees the process, and ends it with
//! `disconnect {terminateDebuggee}`. The generated command lists contain no Python (H7a).

// Unix sockets, /proc and lldb-server: these harnesses run on Linux (and macOS) hosts.
#![cfg(unix)]
#![allow(clippy::unwrap_used)]

mod common;

use std::time::Duration;

use russet_debug::BoxStream;
use russet_debug::broker::{Broker, BrokerOptions, BrokerState, EndpointSpec};
use russet_debug::lldb::{self, AttachPlan, InitSettings, LldbPlatform};
use russet_testkit::debug::programs::{self, build_probe, process_alive};
use russet_testkit::debug::{DapClient, LldbServer, LldbTools};

#[tokio::test(flavor = "multi_thread")]
async fn lldb_dap_attaches_with_attach_commands_only() {
    let Some(tools) = LldbTools::for_test("H2", true) else { return };
    if programs::find_cc().is_none() {
        eprintln!("skipping H2: no C compiler");
        return;
    }
    let dir = common::scratch("h2");
    let probe = build_probe(dir.path()).unwrap();
    let mut child =
        std::process::Command::new(&probe.path).arg("--suspended").stdout(std::process::Stdio::null()).spawn().unwrap();
    let pid = child.id();
    std::thread::sleep(Duration::from_millis(100));

    let server = LldbServer::listen(&tools).unwrap();
    let stream: BoxStream = Box::new(tokio::net::TcpStream::connect(server.addr()).await.unwrap());
    let mut broker = Broker::connect(stream, BrokerOptions::default()).await.unwrap();
    broker.attach(pid).await.unwrap();
    let handle = broker.serve(EndpointSpec::Unix { path: dir.join("s/gdb.sock") }).await.unwrap();

    let plan = AttachPlan {
        platform: LldbPlatform::Host,
        arch: None,
        local_executable: probe.path.clone(),
        device_app_dir: None,
        local_app_dir: None,
        broker_url: handle.lldb_url(),
    };
    let init =
        lldb::init_commands(&InitSettings { memory_module_load_level: "minimal".into(), ..InitSettings::default() });
    let attach = lldb::dap_attach_commands(&plan);
    assert!(!lldb::uses_python(&init) && !lldb::uses_python(&attach));

    let tools2 = tools.clone();
    let (attach_ok, threads, exited) = tokio::task::spawn_blocking(move || {
        let mut dap = DapClient::spawn_lldb_dap(&tools2).unwrap();
        let caps = dap.initialize().unwrap();
        assert_eq!(caps.get("success").as_bool(), Some(true), "{caps}");
        let seq = dap.attach(&init, &attach, false).unwrap();
        dap.wait_event("initialized", Duration::from_secs(30)).unwrap();
        let done = dap.request("configurationDone", None, Duration::from_secs(30)).unwrap();
        assert_eq!(done.get("success").as_bool(), Some(true), "{done}");
        let attached = dap.wait_response(seq, Duration::from_secs(30)).unwrap();
        let process = dap.wait_event("process", Duration::from_secs(30)).unwrap();
        assert_eq!(process.get("body").get("systemProcessId").as_u64(), Some(u64::from(pid)), "{process}");
        let threads = dap.request("threads", None, Duration::from_secs(30)).unwrap();
        let bye = dap.disconnect(true).unwrap();
        assert_eq!(bye.get("success").as_bool(), Some(true), "{bye}");
        let exited = dap.wait_event("exited", Duration::from_secs(10)).is_ok()
            || dap.wait_exit(Duration::from_secs(10)).is_some();
        (attached.get("success").as_bool(), threads, exited)
    })
    .await
    .unwrap();
    assert_eq!(attach_ok, Some(true));
    assert!(!threads.get("body").get("threads").as_array().is_empty(), "{threads}");
    assert!(exited);
    let end = handle.wait_for(|_| false).await;
    assert!(matches!(end, BrokerState::Exited { .. }), "{end:?}");
    let _ = child.wait();
    assert!(!process_alive(pid));
    drop(server);
}
