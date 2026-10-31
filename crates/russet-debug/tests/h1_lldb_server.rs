// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! H1 (RD §23.2): the Rust gdb-remote client does `vRun` and `vAttach` against `lldb-server` from the Swift
//! toolchain, and LLDB's synchronous `process connect` through the broker returns a stopped process at once, over
//! a Unix socket and over TCP. Skipped (with the reason) when the host lacks lldb-server or a C compiler, unless
//! `RUSSET_TEST_REQUIRE_LLDB=1`.

// Unix sockets, /proc and lldb-server: these harnesses run on Linux (and macOS) hosts.
#![cfg(unix)]
#![allow(clippy::unwrap_used)]

mod common;

use std::time::Duration;

use russet_debug::broker::{Broker, BrokerOptions, BrokerState, EndpointSpec};
use russet_debug::{BoxStream, BrokerEndpoint};
use russet_gdbremote::{GdbClient, HandshakeOptions, ResumeAction, StopEvent};
use russet_testkit::debug::programs::{self, build_probe, elf_symbol, proc_load_base, process_alive};
use russet_testkit::debug::{LldbServer, LldbTools};

fn suspended_probe(program: &std::path::Path) -> std::process::Child {
    let child = std::process::Command::new(program)
        .arg("--suspended")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    // Give it a moment to reach its wait-for-a-debugger loop.
    std::thread::sleep(Duration::from_millis(50));
    child
}

#[tokio::test(flavor = "multi_thread")]
async fn the_rust_client_runs_and_attaches_against_lldb_server() {
    let Some(tools) = LldbTools::for_test("H1 client", false) else { return };
    if programs::find_cc().is_none() {
        eprintln!("skipping H1 client: no C compiler");
        return;
    }
    let dir = common::scratch("h1-client");
    let probe = build_probe(dir.path()).unwrap();

    // vRun: lldb-server launches the probe stopped at its entry.
    let server = LldbServer::listen(&tools).unwrap();
    let stream = tokio::net::TcpStream::connect(server.addr()).await.unwrap();
    let (mut c, info) = GdbClient::handshake(stream, HandshakeOptions::default()).await.unwrap();
    assert!(info.no_ack && info.thread_suffix, "{info:?}");
    let stop = c.run_program(&common::path_str(&probe.path), &[]).await.unwrap();
    assert_eq!(stop.signal, 0x13, "Linux SIGSTOP at the entry point: {stop:?}");
    let pid = c.process_info().await.unwrap().pid;
    assert!(process_alive(pid));
    // A breakpoint on probe_step, through the PIE base and the ELF symbol.
    let regs = c.register_infos().await.unwrap();
    let pc = regs.iter().find(|r| r.generic.as_deref() == Some("pc")).unwrap().number;
    let base = proc_load_base(pid, &probe.path).unwrap().unwrap();
    let addr = base + elf_symbol(&probe.path, "probe_step").unwrap().unwrap();
    let kind = if cfg!(target_arch = "aarch64") { 4 } else { 1 };
    c.set_breakpoint(addr, kind).await.unwrap();
    c.resume(ResumeAction::ContinueAll).await.unwrap();
    let hit = loop {
        match c.wait_stop().await.unwrap() {
            StopEvent::Stopped(r) => break r,
            StopEvent::Output(_) => continue,
            other => panic!("{other:?}"),
        }
    };
    assert!(hit.is_breakpoint() || hit.signal == 5, "{hit:?}");
    let tid = hit.thread.unwrap();
    assert_eq!(c.read_register(tid, pc).await.unwrap(), addr);
    c.remove_breakpoint(addr, kind).await.unwrap();
    assert!(c.kill().await.unwrap().is_some());
    drop(server);

    // vAttach: the probe launched "suspended" by us, attached by a process-less lldb-server.
    let mut child = suspended_probe(&probe.path);
    let server = LldbServer::listen(&tools).unwrap();
    let stream = tokio::net::TcpStream::connect(server.addr()).await.unwrap();
    let (mut c, _) = GdbClient::handshake(stream, HandshakeOptions::default()).await.unwrap();
    let stop = c.attach(child.id()).await.unwrap();
    assert!(stop.thread.is_some(), "{stop:?}");
    assert_eq!(c.process_info().await.unwrap().pid, child.id());
    let images_or_unsupported = c.loaded_images().await;
    assert!(images_or_unsupported.is_err(), "lldb-server on Linux has no jGetLoadedDynamicLibrariesInfos");
    c.kill().await.unwrap();
    let _ = child.wait();
}

async fn broker_for(tools: &LldbTools, pid: u32) -> (Broker, LldbServer) {
    let server = LldbServer::listen(tools).unwrap();
    let stream: BoxStream = Box::new(tokio::net::TcpStream::connect(server.addr()).await.unwrap());
    let mut b = Broker::connect(stream, BrokerOptions::default()).await.unwrap();
    b.attach(pid).await.unwrap();
    (b, server)
}

#[tokio::test(flavor = "multi_thread")]
async fn lldb_sync_connect_through_the_broker_returns_a_stopped_process() {
    let Some(tools) = LldbTools::for_test("H1 broker", true) else { return };
    if programs::find_cc().is_none() {
        eprintln!("skipping H1 broker: no C compiler");
        return;
    }
    let dir = common::scratch("h1-broker");
    let probe = build_probe(dir.path()).unwrap();
    for transport in ["unix", "tcp"] {
        let mut child = suspended_probe(&probe.path);
        let pid = child.id();
        let (broker, _server) = broker_for(&tools, pid).await;
        let spec = match transport {
            "unix" => EndpointSpec::Unix { path: dir.join(format!("s-{transport}/gdb.sock")) },
            _ => EndpointSpec::Tcp { bind: "127.0.0.1:0".parse().unwrap() },
        };
        let handle = broker.serve(spec).await.unwrap();
        match (transport, handle.endpoint()) {
            ("unix", BrokerEndpoint::Unix { .. }) | ("tcp", BrokerEndpoint::Tcp { .. }) => {}
            other => panic!("{other:?}"),
        }
        let url = handle.lldb_url();
        // `platform shell date` around the connect times it without LLDB's start-up (and without Python).
        let clock = "platform shell date +%s%N";
        let args: Vec<String> = [
            "-x",
            "-b",
            "-o",
            &format!("target create {}", common::path_str(&probe.path)),
            "-o",
            clock,
            "-o",
            &format!("process connect -p gdb-remote {url}"),
            "-o",
            clock,
            "-o",
            "process status",
            "-o",
            "process kill",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        let run = common::run_lldb(&tools, &args, Duration::from_secs(60)).await;
        assert_eq!(run.status, Some(0), "{transport}: {}\n{}", run.stdout, run.stderr);
        assert!(run.stdout.contains(&format!("Process {pid} stopped")), "{transport}: {}", run.stdout);
        assert!(run.stdout.contains("exited with status = 9"), "{transport}: {}", run.stdout);
        // RD §23.2: the connect returns in < 2 s with a stopped process (RD V4 measured ~260 ms).
        let stamps: Vec<u128> = run
            .stdout
            .lines()
            .filter(|l| l.len() >= 18 && l.bytes().all(|b| b.is_ascii_digit()))
            .map(|l| l.parse().unwrap())
            .collect();
        assert_eq!(stamps.len(), 2, "{transport}: {}", run.stdout);
        let connect = Duration::from_nanos(u64::try_from(stamps[1] - stamps[0]).unwrap());
        eprintln!("H1 {transport}: process connect through the broker took {connect:?}");
        assert!(connect < Duration::from_secs(2), "{transport}: the connect took {connect:?}");
        assert!(run.elapsed < Duration::from_secs(20), "{transport}: {:?}", run.elapsed);
        assert!(matches!(handle.wait_for(|_| false).await, BrokerState::Exited { .. }));
        let _ = child.wait();
        assert!(!process_alive(pid));
    }
}
