// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! H3 (RD §23.2, V8): the RX-page demo serviced natively over gdb-remote against `lldb-server`, ported from
//! `rx_servicer_proto.py`. At each stop on `NOTIFY_DEBUGGER_ABOUT_RX_PAGES` the servicer reads the region from the
//! argument registers, pipelines the touch plan (verify fill 0x5a) plus the emulated return in one write, and
//! resumes; there are no step-over packets. The demo checks that every page was touched and prints
//! `RESULT PASS`. The product servicer (arming, statistics, crash capture) is W18's (RD-10).

// Unix sockets, /proc and lldb-server: these harnesses run on Linux (and macOS) hosts.
#![cfg(unix)]
#![allow(clippy::unwrap_used)]

mod common;

use std::time::Duration;

use russet_debug::jit::{touch_plan_with, touch_writes};
use russet_gdbremote::{GdbClient, HandshakeOptions, ResumeAction, StopEvent};
use russet_testkit::debug::programs::{self, build_jitdemo, elf_symbol, proc_load_base};
use russet_testkit::debug::{LldbServer, LldbTools};

const PAGE: u64 = 16384;

#[tokio::test(flavor = "multi_thread")]
async fn the_jit_demo_passes_with_a_native_servicer() {
    let Some(tools) = LldbTools::for_test("H3", false) else { return };
    if programs::find_cc().is_none() {
        eprintln!("skipping H3: no C compiler");
        return;
    }
    let dir = common::scratch("h3");
    let demo = build_jitdemo(dir.path()).unwrap();
    let server = LldbServer::listen(&tools).unwrap();
    let stream = tokio::net::TcpStream::connect(server.addr()).await.unwrap();
    let (mut c, _) = GdbClient::handshake(stream, HandshakeOptions::default()).await.unwrap();
    c.run_program(&common::path_str(&demo.path), &["5".to_owned()]).await.unwrap();
    let pid = c.process_info().await.unwrap().pid;

    let regs = c.register_infos().await.unwrap();
    let num = |names: &[&str]| regs.iter().find(|r| names.contains(&r.name.as_str())).map(|r| r.number).unwrap();
    let arm64 = cfg!(target_arch = "aarch64");
    let (pc, sp, arg0, arg1) = if arm64 {
        (num(&["pc"]), num(&["sp"]), num(&["x0"]), num(&["x1"]))
    } else {
        (num(&["rip"]), num(&["rsp"]), num(&["rdi"]), num(&["rsi"]))
    };
    let lr = if arm64 { Some(num(&["lr", "x30"])) } else { None };
    let notify = proc_load_base(pid, &demo.path).unwrap().unwrap()
        + elf_symbol(&demo.path, "NOTIFY_DEBUGGER_ABOUT_RX_PAGES").unwrap().unwrap();
    let kind = if arm64 { 4 } else { 1 };
    c.set_breakpoint(notify, kind).await.unwrap();

    let mut output = Vec::new();
    let (mut stops, mut pages, mut packets) = (0u64, 0u64, 0u64);
    let started = std::time::Instant::now();
    c.resume(ResumeAction::ContinueAll).await.unwrap();
    loop {
        let event = tokio::time::timeout(Duration::from_secs(60), c.wait_stop()).await.unwrap().unwrap();
        let stop = match event {
            StopEvent::Output(o) => {
                output.extend_from_slice(&o);
                continue;
            }
            StopEvent::Exited { code } => {
                assert_eq!(code, 0, "{}", String::from_utf8_lossy(&output));
                break;
            }
            StopEvent::Terminated { signal } => panic!("killed by {signal}: {}", String::from_utf8_lossy(&output)),
            StopEvent::Stopped(s) => s,
        };
        let tid = stop.thread.unwrap();
        let here = match stop.reg(pc) {
            Some(v) => v,
            None => c.read_register(tid, pc).await.unwrap(),
        };
        if here != notify {
            // Not ours: pass signals through, as LLDB does.
            let action = if stop.signal == 0 || stop.signal == 5 {
                ResumeAction::ContinueAll
            } else {
                ResumeAction::ContinueWithSignal { tid, signal: stop.signal }
            };
            c.resume(action).await.unwrap();
            continue;
        }
        let reg = |n: u32| stop.reg(n);
        let base = match reg(arg0) {
            Some(v) => v,
            None => c.read_register(tid, arg0).await.unwrap(),
        };
        let size = match reg(arg1) {
            Some(v) => v,
            None => c.read_register(tid, arg1).await.unwrap(),
        };
        let mut batch = touch_plan_with(base, size, PAGE, 0x5a);
        // Emulate the empty function's return: arm64 `pc = lr`; x86_64 `pc = [sp], sp += 8`.
        if let Some(lr) = lr {
            let ret = c.read_register(tid, lr).await.unwrap();
            batch.push(c.register_write_packet(tid, pc, ret).into_bytes());
        } else {
            let sp_now = c.read_register(tid, sp).await.unwrap();
            let ret = u64::from_le_bytes(c.read_memory(sp_now, 8).await.unwrap().try_into().unwrap());
            batch.push(c.register_write_packet(tid, pc, ret).into_bytes());
            batch.push(c.register_write_packet(tid, sp, sp_now + 8).into_bytes());
        }
        let writes = c.stats().writes;
        let replies = c.pipeline(&batch).await.unwrap();
        assert_eq!(c.stats().writes - writes, 1, "the whole batch goes out in one write");
        assert!(replies.iter().all(|r| &r[..] == b"OK"), "{replies:?}");
        stops += 1;
        pages += size.div_ceil(PAGE);
        packets += batch.len() as u64;
        assert_eq!(base % PAGE, 0, "the demo announces page-aligned regions, as Dart does");
        assert_eq!(touch_writes(base, size, PAGE).len() as u64, 1 + (size / PAGE - 1).div_ceil(2));
        c.resume(ResumeAction::ContinueAll).await.unwrap();
    }
    let text = String::from_utf8_lossy(&output);
    assert!(text.contains("RESULT PASS"), "{text}");
    assert!(!text.contains("untouched"), "{text}");
    // Regions of 1, 8, 15, 22 and 29 pages.
    assert_eq!((stops, pages), (5, 75), "{text}");
    eprintln!("H3: {stops} stops, {pages} pages, {packets} packets in {:?}", started.elapsed());
    drop(server);
}
